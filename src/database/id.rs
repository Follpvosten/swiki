use rocket::{http::RawStr, request::FromParam};
use serde::{Deserialize, Serialize};

/// A wrapper type for string that is used as a unique identifier in the
/// context of this crate. Mostly used as the key for users and articles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Id(String);

impl AsRef<[u8]> for Id {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}
impl From<&str> for Id {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}
impl From<sled::IVec> for Id {
    fn from(s: sled::IVec) -> Self {
        let inner = String::from_utf8(s.to_vec()).unwrap();
        Self(inner)
    }
}
impl From<String> for Id {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl<'r> FromParam<'r> for Id {
    type Error = &'r RawStr;

    fn from_param(param: &'r RawStr) -> Result<Self, Self::Error> {
        Ok(Id(String::from_param(param)?))
    }
}

use std::fmt;
impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
