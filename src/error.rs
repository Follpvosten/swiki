use std::array::TryFromSliceError;

use rocket::{
    http::Status,
    outcome::Outcome,
    response::{self, Responder},
    Request,
};
use rocket_contrib::templates::{tera, Template};
use sled::transaction::TransactionError;
use tantivy::{query::QueryParserError, TantivyError};

use crate::database::{articles::rev_id::RevId, users::UserId, Id};

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
    #[error("Error changing article name: Article {0} already exists")]
    DuplicateArticleName(String),
    #[error("Tried to read a byte slice with the wrong length")]
    InvalidIdData(#[from] TryFromSliceError),
    #[error("Database is inconsistent: Revision {0:?} is missing fields")]
    RevisionDataInconsistent(RevId),
    #[error("User data inconsistent: user {0} exists, but has no password")]
    UserDataInconsistent(String),
    #[error("Database returned inconsistent data: article id {0:?} not found")]
    ArticleDataInconsistent(Id),
    #[error("User id {0:?} does not exist or doesn't have a password")]
    PasswordNotFound(UserId),
    #[error("Error rendering template: {0}")]
    TemplateError(#[from] tera::Error),
    #[error("Captcha error; please retry!")]
    CaptchaNotFound,
    #[error("An unexpected error occured when trying to generate a captcha")]
    CaptchaPngError,
    #[error("Error trying to join a blocking task: {0}")]
    TokioJoinError(#[from] rocket::tokio::task::JoinError),
    #[error("Internal rocket error: failed to get database")]
    DatabaseRequestGuardFailed,
    #[error("Error updating search index: {0}")]
    TantivyError(TantivyError),
    #[error("Error parsing search query: {0}")]
    QueryParserError(QueryParserError),
}

impl From<TantivyError> for Error {
    fn from(err: TantivyError) -> Self {
        Error::TantivyError(err)
    }
}
impl From<QueryParserError> for Error {
    fn from(err: QueryParserError) -> Self {
        Error::QueryParserError(err)
    }
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

impl Error {
    pub fn status(&self) -> Status {
        use Error::*;
        match self {
            SledError(_)
            | CaptchaPngError
            | DatabaseRequestGuardFailed
            | Argon2Error(_)
            | BincodeError(_)
            | TransactionError(_)
            | InvalidIdData(_)
            | UserDataInconsistent(_)
            | RevisionDataInconsistent(_)
            | ArticleDataInconsistent(_)
            | TemplateError(_)
            | TokioJoinError(_)
            | PasswordNotFound(_)
            | TantivyError(_)
            | QueryParserError(_) => Status::InternalServerError,
            UserAlreadyExists(_) | IdenticalNewRevision | DuplicateArticleName(_) => {
                Status::BadRequest
            }
            UserNotFound(_) | RevisionUnknown(_) | CaptchaNotFound => Status::NotFound,
        }
    }
}

// Ouch: I can't implement IntoOutcome for crate::Result<S>.
// I also can't just impl crate::Result<S> and add such a method.
// So I'll have to use a helper trait...
pub trait IntoOutcomeHack<S> {
    fn into_outcome_hack(self) -> Outcome<S, (Status, Error), ()>;
}
impl<S> IntoOutcomeHack<S> for crate::Result<S> {
    fn into_outcome_hack(self) -> Outcome<S, (Status, Error), ()> {
        match self {
            Ok(val) => Outcome::Success(val),
            Err(e) => Outcome::Failure((e.status(), e)),
        }
    }
}

impl<'r> Responder<'r, 'static> for Error {
    fn respond_to(self, request: &'r Request<'_>) -> response::Result<'static> {
        #[derive(serde::Serialize)]
        struct ErrorContext<'a> {
            site_name: &'a str,
            status: String,
            error: String,
        }
        let cfg: &crate::Config = request.managed_state().unwrap();
        let status = self.status().to_string();
        let error = self.to_string();
        let context = ErrorContext {
            site_name: &cfg.site_name,
            status,
            error,
        };
        Template::render("error", context).respond_to(request)
    }
}
