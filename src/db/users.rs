use std::{convert::TryFrom, result::Result as StdResult};

use rocket::{
    outcome::try_outcome,
    request::{FromRequest, Outcome},
    tokio::task::spawn_blocking,
    Request,
};
use sqlx::PgPool;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{Db, Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct UserSession {
    pub session_id: Uuid,
    pub user_id: Uuid,
}
#[rocket::async_trait]
impl<'r> FromRequest<'r> for &'r UserSession {
    type Error = Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        use rocket::outcome::IntoOutcome;
        let result = request
            .local_cache_async(async {
                // Early return if we can't get a valid session id for whatever reason...
                let session_id = request
                    .cookies()
                    .get("session_id")
                    .and_then(|cookie| base64::decode(cookie.value()).ok())
                    .and_then(|vec| uuid::Bytes::try_from(vec.as_slice()).ok())
                    .map(Uuid::from_bytes)?;
                // ...and also early return if we can't get a db handle...
                let db: &Db = request.rocket().state()?;
                // ...of course, also if querying the session returns an error...
                let user_id = match db.get_session_user(session_id).await {
                    Err(e) => {
                        log::error!("Error getting session user: {}", e);
                        None
                    }
                    // TODO: wtf? Optionception, we're returning an Option<Option<Uuid>>
                    Ok(user_id) => Some(user_id),
                }?;
                // ...and finally, if the session doesn't exist (returns None), also forward.
                user_id.map(|user_id| UserSession {
                    session_id,
                    user_id,
                })
            })
            .await;

        result.as_ref().or_forward(())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LoggedUser {
    id: Uuid,
    name: String,
    is_admin: bool,
}
impl LoggedUser {
    pub fn is_admin(&self) -> bool {
        self.is_admin
    }
}
#[rocket::async_trait]
impl<'r> FromRequest<'r> for LoggedUser {
    type Error = Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        use crate::error::IntoOutcomeHack;
        use rocket::outcome::IntoOutcome;
        // Get the logged user's data
        let session: &UserSession = try_outcome!(request.guard().await);
        // Get a handle on the db
        let db: &Db = try_outcome!(request.rocket().state().or_forward(()));
        // Finally, get the user's info
        async fn get_user_info(pool: &PgPool, id: Uuid) -> Result<(bool, String)> {
            Ok(
                sqlx::query!(r#"SELECT name, is_admin FROM "user" WHERE id = $1"#, id)
                    .fetch_one(pool)
                    .await
                    .map(|r| (r.is_admin, r.name))?,
            )
        }
        let (is_admin, name) =
            try_outcome!(get_user_info(db, session.user_id).await.into_outcome_hack());
        // Wrap it in a LoggedUserName and return it
        Outcome::Success(LoggedUser {
            id: session.user_id,
            name,
            is_admin,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LoggedAdmin(LoggedUser);
#[rocket::async_trait]
impl<'r> FromRequest<'r> for LoggedAdmin {
    type Error = Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let logged_user: LoggedUser = try_outcome!(request.guard().await);
        if logged_user.is_admin {
            Outcome::Success(LoggedAdmin(logged_user))
        } else {
            Outcome::Forward(())
        }
    }
}

fn hash_password(password: &str) -> StdResult<String, argon2::Error> {
    fn gen_salt() -> Vec<u8> {
        use rand::Rng;
        rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .collect()
    }
    let config = argon2::Config {
        variant: argon2::Variant::Argon2i,
        ..Default::default()
    };
    let salt = gen_salt();
    argon2::hash_encoded(password.as_bytes(), &salt, &config)
}

fn verify_password(hash: &str, password: &str) -> StdResult<bool, argon2::Error> {
    argon2::verify_encoded(hash, password.as_bytes())
}

/// Simply checks if the given username is known to the database.
pub async fn name_exists(pool: &PgPool, username: &str) -> Result<bool> {
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM "user" WHERE name = $1) AS "a!""#,
        username
    )
    .fetch_one(pool)
    .await?)
}

// TODO Email
/// Attempts to register a new user with the given password.
/// This is a heavy operation due to the password being hashed,
/// which will be done on a threadpool.
pub async fn register(pool: &PgPool, username: &str, mut password: String) -> Result<Uuid> {
    if name_exists(pool, username).await? {
        return Err(Error::UserAlreadyExists(username.to_string()));
    }
    let id = Uuid::new_v4();
    let pw_hash = spawn_blocking(move || {
        let res = hash_password(&password);
        // Remove the password from RAM
        password.zeroize();
        res
    })
    .await??;
    sqlx::query!(
        r#"INSERT INTO "user"(id, name, pw_hash, is_admin)
        VALUES($1, $2, $3, (SELECT COUNT(*) FROM "user") = 0)"#,
        id,
        username,
        pw_hash
    )
    .execute(pool)
    .await?;
    Ok(id)
}

/// Attempts to create a new session for the given user.
/// Will return Ok(None) when password verification fails.
/// This is a heavy operation due to the password hash being verified.
pub async fn try_login(pool: &PgPool, username: &str, mut password: String) -> Result<UserSession> {
    let (user_id, hash) = sqlx::query!(
        r#"SELECT id, pw_hash FROM "user" WHERE name = $1"#,
        username
    )
    .fetch_optional(pool)
    .await?
    .map(|r| (r.id, r.pw_hash))
    .ok_or_else(|| Error::UserNotFound(username.to_string()))?;
    let pw_valid = spawn_blocking(move || {
        let res = verify_password(&hash, &password);
        password.zeroize();
        res
    })
    .await??;
    if pw_valid {
        let session_id = create_session(pool, user_id).await?;
        Ok(UserSession {
            session_id,
            user_id,
        })
    } else {
        Err(Error::WrongPassword)
    }
}
async fn create_session(pool: &PgPool, user_id: Uuid) -> Result<Uuid> {
    let session_id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO session(session_id, user_id) VALUES($1, $2)",
        session_id,
        user_id
    )
    .execute(pool)
    .await?;
    Ok(session_id)
}

/// Logs out a user by deleting the given session id.
pub async fn destroy_session(pool: &PgPool, session_id: Uuid) -> Result<()> {
    sqlx::query!("DELETE FROM session WHERE session_id = $1", session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Returns the user logged in with the given session id, if any.
pub async fn get_session_user(pool: &PgPool, session_id: Uuid) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar!(
        "SELECT user_id FROM session WHERE session_id = $1",
        session_id
    )
    .fetch_optional(pool)
    .await?)
}

/// Checks if the given user has admin privileges.
pub async fn is_admin(pool: &PgPool, user_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar!(r#"SELECT is_admin FROM "user" WHERE id = $1"#, user_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(false),
    )
}
