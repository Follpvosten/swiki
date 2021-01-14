use std::{
    convert::{TryFrom, TryInto},
    ops::Deref,
    result::Result as StdResult,
};

use rocket::{
    request::{FromRequest, Outcome},
    try_outcome, Request,
};
use sled::{Transactional, Tree};
use uuid::Uuid;

use super::Id;
use crate::{Db, Error, Result};

pub struct Users {
    pub(super) username_userid: Tree,
    pub(super) userid_username: Tree,
    pub(super) userid_password: Tree,
    pub(super) userid_flag_value: Tree,
    // TODO implement
    #[allow(dead_code)]
    pub(super) userid_email: Tree,
    pub(super) sessionid_userid: Tree,
}

mod flags {
    pub const ADMIN: &[u8] = b"admin";
}

/// Strongly typed user id. The inner type is pub(super) because you should
/// only ever be able to acquire one from the database, which means it can
/// be assumed to actually exist.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct UserId(pub(super) Id);
impl Deref for UserId {
    type Target = Id;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UserSession {
    pub session_id: Uuid,
    pub user_id: UserId,
}
#[rocket::async_trait]
impl<'a, 'r> FromRequest<'a, 'r> for &'a UserSession {
    type Error = Error;

    async fn from_request(request: &'a Request<'r>) -> Outcome<Self, Self::Error> {
        use rocket::outcome::IntoOutcome;
        let result = request.local_cache(|| {
            // Early return if we can't get a valid session id for whatever reason...
            let session_id = request
                .cookies()
                .get("session_id")
                .and_then(|cookie| base64::decode(cookie.value()).ok())
                .and_then(|vec| uuid::Bytes::try_from(vec.as_slice()).ok())
                .map(Uuid::from_bytes)?;
            // ...and also early return if we can't get a db handle...
            let db: &Db = request.managed_state()?;
            // ...of course, also if querying the session returns an error...
            let user_id: Option<UserId> = match db.users.get_session_user(session_id) {
                Err(e) => {
                    log::error!("Error getting session user: {}", e);
                    None
                }
                // TODO: wtf? Optionception, we're returning an Option<Option<UserId>>
                Ok(user_id) => Some(user_id),
            }?;
            // ...and finally, if the session doesn't exist (returns None), also forward.
            user_id.map(|user_id| UserSession {
                user_id,
                session_id,
            })
        });

        result.as_ref().or_forward(())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LoggedUser {
    id: UserId,
    name: String,
    is_admin: bool,
}
impl LoggedUser {
    pub fn is_admin(&self) -> bool {
        self.is_admin
    }
}
#[rocket::async_trait]
impl<'a, 'r> FromRequest<'a, 'r> for LoggedUser {
    type Error = Error;

    async fn from_request(request: &'a Request<'r>) -> Outcome<Self, Self::Error> {
        use crate::error::IntoOutcomeHack;
        use rocket::outcome::IntoOutcome;
        // Get the logged user's data
        let session: &UserSession = try_outcome!(request.guard().await);
        // Get a handle on the db
        let db: &Db = try_outcome!(request.managed_state().or_forward(()));
        // Finally, get the user's name
        let name: String = try_outcome!(db.users.name_by_id(session.user_id).into_outcome_hack());
        let is_admin: bool = try_outcome!(db.users.is_admin(session.user_id).into_outcome_hack());
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
impl<'a, 'r> FromRequest<'a, 'r> for LoggedAdmin {
    type Error = Error;

    async fn from_request(request: &'a Request<'r>) -> Outcome<Self, Self::Error> {
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

impl Users {
    /// Simply checks if the given username is known to the database.
    pub fn name_exists(&self, username: &str) -> Result<bool> {
        Ok(self.username_userid.contains_key(username.as_bytes())?)
    }
    /// Get the UserId for the given username if it is known.
    pub fn id_by_name(&self, username: &str) -> Result<Option<UserId>> {
        Ok(self
            .username_userid
            .get(username.as_bytes())?
            .map(|ivec| ivec.as_ref().try_into().map(UserId))
            .transpose()?)
    }
    /// Get the username for the given UserId. Will return an error
    /// if the id is unknown because UserIds can be assumed to be valid.
    pub fn name_by_id(&self, user_id: UserId) -> Result<String> {
        self.userid_username
            .get(user_id.to_bytes())?
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap())
            .ok_or(Error::UserDataInconsistent(user_id))
    }

    // TODO Email
    /// Attempts to register a new user with the given password.
    /// This is a heavy operation due to the password being hashed.
    /// It also affects four different sled trees, so this is most likely
    /// the most complicated transaction in the codebase at this moment.
    pub fn register(&self, username: &str, password: &str) -> Result<UserId> {
        if self.username_userid.contains_key(username.as_bytes())? {
            return Err(Error::UserAlreadyExists(username.to_string()));
        }
        let id = UserId(match self.userid_password.iter().next_back() {
            None => Id::first(),
            Some(res) => {
                let curr_id: Id = res?.0.as_ref().try_into()?;
                curr_id.next()
            }
        });
        (
            &self.username_userid,
            &self.userid_username,
            &self.userid_password,
            &self.userid_flag_value,
        )
            .transaction(|(name_id, id_name, id_password, id_flag_val)| {
                use sled::transaction::ConflictableTransactionError::Abort;
                name_id.insert(username.as_bytes(), &id.to_bytes())?;
                id_name.insert(&id.to_bytes(), username.as_bytes())?;

                // If this is the first user, make them an admin
                let admin_key = [&id.to_bytes(), flags::ADMIN].concat();
                id_flag_val.insert(admin_key, &[(id.0 == Id::first()) as u8])?;

                let password_hashed = hash_password(password)
                    .map_err(Error::from)
                    .map_err(Abort)?;
                id_password.insert(&id.to_bytes(), password_hashed.as_bytes())?;
                Ok(())
            })?;
        Ok(id)
    }

    /// Attempts to create a new session for the given user.
    /// Will return Ok(None) when password verification fails.
    /// This is a heavy operation due to the password hash being verified.
    pub fn try_login(&self, user_id: UserId, password: &str) -> Result<Option<UserSession>> {
        let hash = self
            .userid_password
            .get(user_id.to_bytes())?
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap())
            .ok_or(Error::PasswordNotFound(user_id))?;
        if verify_password(&hash, password)? {
            let session_id = self.create_session(user_id)?;
            Ok(Some(UserSession {
                user_id,
                session_id,
            }))
        } else {
            Ok(None)
        }
    }
    fn create_session(&self, user_id: UserId) -> Result<Uuid> {
        let session_id = Uuid::new_v4();
        self.sessionid_userid
            .insert(session_id.as_bytes(), &user_id.to_bytes())?;
        Ok(session_id)
    }

    /// Logs out a user by deleting the given session id.
    pub fn destroy_session(&self, session_id: Uuid) -> Result<()> {
        self.sessionid_userid.remove(session_id.as_bytes())?;
        Ok(())
    }

    /// Returns the user logged in with the given session id, if any.
    pub fn get_session_user(&self, session_id: Uuid) -> Result<Option<UserId>> {
        self.sessionid_userid
            .get(session_id.as_bytes())?
            .map(|ivec| Id::from_bytes(&ivec).map(UserId))
            .transpose()
    }

    /// Checks if the given user has admin privileges.
    pub fn is_admin(&self, user_id: UserId) -> Result<bool> {
        let key = [&user_id.to_bytes(), flags::ADMIN].concat();
        Ok(self
            .userid_flag_value
            .get(key)?
            .map(|ivec| ivec[0] != 0)
            .unwrap_or(false))
    }
    /// Updates whether the given user has admin privileges.
    pub fn set_is_admin(&self, user_id: UserId, is_admin: bool) -> Result<()> {
        let key = [&user_id.to_bytes(), flags::ADMIN].concat();
        self.userid_flag_value.insert(key, &[is_admin as u8])?;
        Ok(())
    }
}
