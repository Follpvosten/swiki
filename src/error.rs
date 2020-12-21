use std::io::Cursor;

use rocket::{
    http::Status,
    response::{self, Responder},
    Request, Response,
};
use sled::transaction::TransactionError;
use uuid::Uuid;

use crate::database::Id;

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
    UserAlreadyExists(Id),
    #[error("Unknown user: {0}")]
    UserNotFound(Id),
    #[error("Revision '{0}' does not exist")]
    RevisionUnknown(Uuid),
    #[error("Failed to parse revision id: {0}")]
    RevisionIdParseFailed(#[from] uuid::Error),
    #[error("New content is identical to the previous revision")]
    IdenticalNewRevision,
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

impl<'r> Responder<'r, 'static> for Error {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        use Error::*;
        let status = match &self {
            SledError(_)
            | Argon2Error(_)
            | BincodeError(_)
            | RevisionIdParseFailed(_)
            | TransactionError(_) => Status::InternalServerError,
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
