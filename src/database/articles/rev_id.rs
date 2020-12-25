use crate::database::Id;
use std::convert::TryInto;

/// A revision id.
/// This type wraps an article id and a revision number (both u32).
/// It is used to store an article's revision so it's easier to query
/// e.g. the latest revision of an article.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevId(Id, Id);

impl From<(u32, u32)> for RevId {
    fn from((article_id, rev_number): (u32, u32)) -> Self {
        Self(Id(article_id), Id(rev_number))
    }
}
impl From<(Id, Id)> for RevId {
    fn from((article_id, rev_number): (Id, Id)) -> Self {
        Self(article_id, rev_number)
    }
}

impl RevId {
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let (article_id, rev_number) = bytes.split_at(4);
        Ok(Self(article_id.try_into()?, rev_number.try_into()?))
    }
    pub fn to_bytes(&self) -> [u8; 8] {
        // TODO: This is stupid ugly, rust-lang pls fix this.
        let arr1 = self.0.to_bytes();
        let arr2 = self.1.to_bytes();
        [
            arr1[0], arr1[1], arr1[2], arr1[3], arr2[0], arr2[1], arr2[2], arr2[3],
        ]
    }

    pub fn first(article_id: Id) -> RevId {
        Self(article_id, Id::first())
    }

    pub fn article_id(&self) -> Id {
        self.0
    }
    pub fn rev_id(&self) -> Id {
        self.1
    }

    pub fn next(mut self) -> Self {
        self.1.increment();
        self
    }
}
