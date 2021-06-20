use std::convert::{TryFrom, TryInto};

use rocket::{http::RawStr, request::FromParam};

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Id(pub u32);

impl Id {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::Error> {
        Ok(Id(u32::from_be_bytes(bytes.try_into()?)))
    }
    pub fn to_bytes(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    pub fn first() -> Self {
        Id(0)
    }
    pub fn next(self) -> Self {
        Id(self.0 + 1)
    }
}

impl From<u32> for Id {
    fn from(n: u32) -> Self {
        Self(n)
    }
}

impl TryFrom<&[u8]> for Id {
    type Error = crate::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Id::from_bytes(value)
    }
}

impl<'r> FromParam<'r> for Id {
    type Error = &'r RawStr;

    fn from_param(param: &'r str) -> Result<Self, Self::Error> {
        Ok(Id(u32::from_param(param)?))
    }
}
