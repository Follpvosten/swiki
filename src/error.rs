use std::{array::TryFromSliceError, io::Cursor};

use rocket::{
    http::Status,
    response::{self, Responder},
    Request, Response,
};
use rocket_contrib::templates::tera;
use sled::transaction::TransactionError;

use crate::database::{articles::rev_id::RevId, Id};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Database error: {0}")]
    SledError(#[from] sled::Error),
    #[error("Transaction error: {0}")]
    TransactionError(#[from] TransactionError),
    #[error("Error hashing password: {0}")]
    Argon2Error(#[from] argon2::Error),
    #[error("Data could not be (de)serialized: {0}")]
    BincodeError(#[from] bincode::Error),
    #[error("Username already taken: {0}")]
    UserAlreadyExists(String),
    #[error("Unknown user: {0}")]
    UserNotFound(String),
    #[error("Revision '{0:?}' does not exist")]
    RevisionUnknown(RevId),
    #[error("New content is identical to the previous revision")]
    IdenticalNewRevision,
    #[error("Tried to read a byte slice with the wrong length")]
    InvalidIdData(#[from] TryFromSliceError),
    #[error("Database is inconsistent: Revision {0:?} is missing fields")]
    RevisionDataInconsistent(RevId),
    #[error("User data inconsistent: user {0} exists, but has no password")]
    UserDataInconsistent(String),
    #[error("User id {0:?} does not exist or doesn't have a password")]
    PasswordNotFound(Id),
    #[error("Error rendering template: {0}")]
    TemplateError(#[from] tera::Error),
    #[error("Captcha error; please retry!")]
    CaptchaNotFound,
    #[error("An unexpected error occured when trying to generate a captcha")]
    CaptchaPngError,
    #[error("Error trying to join a blocking task: {0}")]
    TokioJoinError(#[from] rocket::tokio::task::JoinError),
}

// Unwrap more specific errors from transactions.
impl From<TransactionError<Error>> for Error {
    fn from(s: TransactionError<Error>) -> Self {
        match s {
            TransactionError::Abort(e) => e,
            TransactionError::Storage(e) => Error::SledError(e),
        }
    }
}
impl From<TransactionError<()>> for Error {
    fn from(s: TransactionError<()>) -> Self {
        match s {
            TransactionError::Storage(e) => Error::SledError(e),
            TransactionError::Abort(_) => unreachable!(),
        }
    }
}

impl<'r> Responder<'r, 'static> for Error {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        use Error::*;
        let status = match &self {
            SledError(_)
            | CaptchaPngError
            | Argon2Error(_)
            | BincodeError(_)
            | TransactionError(_)
            | InvalidIdData(_)
            | UserDataInconsistent(_)
            | RevisionDataInconsistent(_)
            | TemplateError(_)
            | TokioJoinError(_)
            | PasswordNotFound(_) => Status::InternalServerError,
            UserAlreadyExists(_) | IdenticalNewRevision => Status::BadRequest,
            UserNotFound(_) | RevisionUnknown(_) | CaptchaNotFound => Status::NotFound,
        };
        let body = self.to_string();
        Ok(Response::build()
            .status(status)
            .sized_body(body.len(), Cursor::new(body))
            .finalize())
    }
}
