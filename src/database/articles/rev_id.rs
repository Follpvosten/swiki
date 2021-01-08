use std::convert::TryInto;

use super::ArticleId;
use crate::database::Id;

/// A revision id.
/// This type wraps an article id and a revision number (both u32).
/// It is used to store an article's revision so it's easier to query
/// e.g. the latest revision of an article.
/// Values of this type can only ever be obtained from the database.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevId(
    pub(in crate::database) ArticleId,
    pub(in crate::database) Id,
);

impl RevId {
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let (article_id, rev_number) = bytes.split_at(4);
        Ok(Self(
            ArticleId(article_id.try_into()?),
            rev_number.try_into()?,
        ))
    }
    pub fn to_bytes(&self) -> [u8; 8] {
        // TODO: This is stupid ugly, rust-lang pls fix this.
        let arr1 = self.0.to_bytes();
        let arr2 = self.1.to_bytes();
        [
            arr1[0], arr1[1], arr1[2], arr1[3], arr2[0], arr2[1], arr2[2], arr2[3],
        ]
    }

    pub fn first(article_id: ArticleId) -> RevId {
        Self(article_id, Id::first())
    }

    pub fn article_id(&self) -> ArticleId {
        self.0
    }
    pub fn rev_number(&self) -> Id {
        self.1
    }

    pub fn next(self) -> Self {
        RevId(self.0, self.1.next())
    }
}
