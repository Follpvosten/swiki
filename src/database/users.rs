use std::{
    convert::{TryFrom, TryInto},
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
    pub(super) userid_email: Tree,
    pub(super) sessionid_userid: Tree,
}

#[derive(Debug, Clone, Copy)]
pub struct UserSession {
    pub session_id: Uuid,
    pub user_id: Id,
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
            let user_id: Option<Id> = match db.users.get_session_user(session_id) {
                Err(e) => {
                    log::error!("Error getting session user: {}", e);
                    None
                }
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

#[derive(serde::Serialize)]
pub struct LoggedUserName(pub String);
#[rocket::async_trait]
impl<'a, 'r> FromRequest<'a, 'r> for LoggedUserName {
    type Error = Error;

    async fn from_request(request: &'a Request<'r>) -> Outcome<Self, Self::Error> {
        use crate::error::IntoOutcomeHack;
        use rocket::outcome::IntoOutcome;
        // Get the logged user's data
        let session: &UserSession = try_outcome!(request.guard().await);
        // Get a handle on the db
        let db: &Db = try_outcome!(request.managed_state().or_forward(()));
        // Finally, get the user's name
        let user_name: Option<String> =
            try_outcome!(db.users.name_by_id(session.user_id).into_outcome_hack());
        // Wrap it in a LoggedUserName and return it
        user_name.map(LoggedUserName).or_forward(())
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
    pub fn id_by_name(&self, username: &str) -> Result<Option<Id>> {
        Ok(self
            .username_userid
            .get(username.as_bytes())?
            .map(|ivec| ivec.as_ref().try_into())
            .transpose()?)
    }
    pub fn name_by_id(&self, user_id: Id) -> Result<Option<String>> {
        Ok(self
            .userid_username
            .get(user_id.to_bytes())?
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap()))
    }

    // TODO Email
    pub fn register(&self, username: &str, password: &str) -> Result<Id> {
        if self.username_userid.contains_key(username.as_bytes())? {
            return Err(Error::UserAlreadyExists(username.to_string()));
        }
        let id = match self.userid_password.iter().next_back() {
            None => Id::first(),
            Some(res) => {
                let curr_id: Id = res?.0.as_ref().try_into()?;
                curr_id.next()
            }
        };
        (
            &self.username_userid,
            &self.userid_username,
            &self.userid_password,
        )
            .transaction(|(name_id, id_name, id_password)| {
                use sled::transaction::ConflictableTransactionError::Abort;
                name_id.insert(username.as_bytes(), &id.to_bytes())?;
                id_name.insert(&id.to_bytes(), username.as_bytes())?;

                let password_hashed = hash_password(password)
                    .map_err(Error::from)
                    .map_err(Abort)?;
                id_password.insert(&id.to_bytes(), password_hashed.as_bytes())?;
                Ok(())
            })?;
        Ok(id)
    }

    pub fn verify_password(&self, user_id: Id, password: &str) -> Result<bool> {
        let hash = self
            .userid_password
            .get(user_id.to_bytes())?
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap())
            .ok_or(Error::PasswordNotFound(user_id))?;
        Ok(verify_password(&hash, password)?)
    }

    pub fn create_session(&self, user_id: Id) -> Result<Uuid> {
        let id = Uuid::new_v4();
        self.sessionid_userid
            .insert(id.as_bytes(), &user_id.to_bytes())?;
        Ok(id)
    }

    pub fn destroy_session(&self, session_id: Uuid) -> Result<()> {
        self.sessionid_userid.remove(session_id.as_bytes())?;
        Ok(())
    }

    pub fn get_session_user(&self, session_id: Uuid) -> Result<Option<Id>> {
        self.sessionid_userid
            .get(session_id.as_bytes())?
            .map(|ivec| Id::from_bytes(&ivec))
            .transpose()
    }

    pub fn name_exists(&self, username: &str) -> Result<bool> {
        Ok(self.username_userid.contains_key(username.as_bytes())?)
    }
}