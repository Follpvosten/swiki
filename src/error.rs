use std::{array::TryFromSliceError, io::Cursor};

use rocket::{
    http::Status,
    response::{self, Responder},
    Request, Response,
};
use sled::transaction::TransactionError;

use crate::database::articles::rev_id::RevId;

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
            | Argon2Error(_)
            | BincodeError(_)
            | TransactionError(_)
            | InvalidIdData(_)
            | UserDataInconsistent(_)
            | RevisionDataInconsistent(_) => Status::InternalServerError,
            UserAlreadyExists(_) | IdenticalNewRevision => Status::BadRequest,
            UserNotFound(_) | RevisionUnknown(_) => Status::NotFound,
        };
        let body = self.to_string();
        Ok(Response::build()
            .status(status)
            .sized_body(body.len(), Cursor::new(body))
            .finalize())
    }
}
