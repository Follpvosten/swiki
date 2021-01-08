use std::convert::TryInto;

use chrono::{DateTime, Utc};
use itertools::Itertools;
use serde::Serialize;
use sled::{
    transaction::{ConflictableTransactionResult, Transactional},
    Tree,
};

use super::{users::UserId, Id};
use crate::{Error, Result};

pub mod rev_id;
use rev_id::RevId;

pub struct Articles {
    // key = Id
    pub(super) articleid_name: Tree,
    pub(super) articlename_id: Tree,
    // key = RevId
    pub(super) revid_content: Tree,
    pub(super) revid_author: Tree,
    pub(super) revid_date: Tree,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Revision {
    pub content: String,
    pub author_id: UserId,
    pub date: DateTime<Utc>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct RevisionMeta {
    pub author_id: UserId,
    pub date: DateTime<Utc>,
}

impl Articles {
    pub fn id_by_name(&self, name: &str) -> Result<Option<Id>> {
        self.articlename_id
            .get(name.as_bytes())?
            .map(|ivec| ivec.as_ref().try_into())
            .transpose()
    }
    pub fn name_by_id(&self, id: Id) -> Result<Option<String>> {
        Ok(self
            .articleid_name
            .get(&id.to_bytes())?
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap()))
    }
    pub fn name_exists(&self, name: &str) -> Result<bool> {
        Ok(self.articlename_id.contains_key(name.as_bytes())?)
    }
    pub fn list_articles(&self) -> Result<Vec<Id>> {
        self.articleid_name
            .iter()
            .map_ok(|(id_ivec, _)| id_ivec)
            .map(|res| {
                res.map_err(Error::from)
                    .and_then(|ivec| ivec.as_ref().try_into())
            })
            .collect()
    }
    /// Retrieves the list of revision ids for the given article id.
    /// Returns Ok(None) when the article doesn't exist.
    /// Returns RevisionMeta because loading the revision's content doesn't
    /// make sense for listing the revisions.
    /// TODO or to figure out: We currently return an empty vec when the
    /// article id is unknown. Should it be an Option?
    pub fn list_revisions(&self, article_id: Id) -> Result<Vec<(Id, RevisionMeta)>> {
        let authors = self
            .revid_author
            .scan_prefix(article_id.to_bytes())
            .map(|result| {
                result
                    .map_err(Error::from)
                    .and_then(|(revid_ivec, authorid_ivec)| {
                        let rev_id = RevId::from_bytes(&revid_ivec)?.rev_id();
                        let authorid = UserId(Id::from_bytes(authorid_ivec.as_ref())?);
                        Ok((rev_id, authorid))
                    })
            });
        let dates = self
            .revid_date
            .scan_prefix(article_id.to_bytes())
            .map(|result| {
                result.map_err(Error::from).and_then(|(_revid, date_ivec)| {
                    Ok(bincode::deserialize::<DateTime<Utc>>(&*date_ivec)?)
                })
            });

        authors
            .zip(dates)
            .map(|(res1, res2)| {
                res1.and_then(move |(rev_id, author_id)| {
                    res2.map(move |date| (rev_id, RevisionMeta { author_id, date }))
                })
            })
            .collect()
    }
    pub fn get_current_content(&self, article_id: Id) -> Result<Option<String>> {
        Ok(self
            .revid_content
            .scan_prefix(article_id.to_bytes())
            .last()
            .transpose()?
            .map(|(_, content)| String::from_utf8(content.to_vec()).unwrap()))
    }
    /// Get the current revision for the given article id if it exists.
    pub fn get_current_revision(&self, article_id: Id) -> Result<Option<(RevId, Revision)>> {
        let (rev_id, author_id) = match self
            .revid_author
            .scan_prefix(article_id.to_bytes())
            .last()
            .transpose()?
        {
            None => return Ok(None),
            Some((revid_ivec, authorid_ivec)) => {
                let revid = RevId::from_bytes(&revid_ivec)?;
                let authorid = UserId(authorid_ivec.as_ref().try_into()?);
                (revid, authorid)
            }
        };
        // Since we now have a full revision id...
        let date = self.get_revision_date(rev_id).map_err(|err| match err {
            Error::RevisionUnknown(id) => Error::RevisionDataInconsistent(id),
            _ => err,
        })?;
        let content = self.get_revision_content(rev_id).map_err(|err| match err {
            Error::RevisionUnknown(id) => Error::RevisionDataInconsistent(id),
            _ => err,
        })?;

        Ok(Some((
            rev_id,
            Revision {
                author_id,
                date,
                content,
            },
        )))
    }
    pub fn get_revision_content(&self, rev_id: RevId) -> Result<String> {
        self.revid_content
            .get(rev_id.to_bytes())?
            //                  will panic on disk corruption v
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap())
            .ok_or(Error::RevisionUnknown(rev_id))
    }
    pub fn get_revision_date(&self, rev_id: RevId) -> Result<DateTime<Utc>> {
        let date = self
            .revid_date
            .get(rev_id.to_bytes())?
            .ok_or(Error::RevisionUnknown(rev_id))?;
        Ok(bincode::deserialize(&*date)?)
    }
    /// Get all data for the given revision
    pub fn get_revision(&self, rev_id: RevId) -> Result<Revision> {
        let author = UserId(
            self.revid_author
                .get(rev_id.to_bytes())?
                .ok_or(Error::RevisionUnknown(rev_id))?
                .as_ref()
                .try_into()?,
        );
        let date = self.get_revision_date(rev_id)?;
        let content = self.get_revision_content(rev_id)?;

        Ok(Revision {
            content,
            author_id: author,
            date,
        })
    }
    /// Create an empty article with no revisions.
    pub fn create(&self, name: &str) -> Result<Id> {
        let id = match self.articleid_name.iter().next_back() {
            None => Id::first(),
            Some(res) => {
                let curr_id: Id = res?.0.as_ref().try_into()?;
                curr_id.next()
            }
        };
        (&self.articleid_name, &self.articlename_id).transaction(|(id_name, name_id)| {
            id_name.insert(&id.to_bytes(), name.as_bytes())?;
            name_id.insert(name.as_bytes(), &id.to_bytes())?;
            ConflictableTransactionResult::<_, ()>::Ok(())
        })?;
        Ok(id)
    }
    pub fn change_name(&self, article_id: Id, new_name: &str) -> Result<()> {
        // Article names must be unique
        if self.articlename_id.contains_key(new_name.as_bytes())? {
            return Err(Error::DuplicateArticleName(new_name.into()));
        }
        let old_name = self
            .name_by_id(article_id)?
            .ok_or(Error::ArticleDataInconsistent(article_id))?;
        (&self.articleid_name, &self.articlename_id).transaction(|(id_name, name_id)| {
            id_name.insert(&article_id.to_bytes(), new_name.as_bytes())?;
            name_id.remove(old_name.as_bytes())?;
            name_id.insert(new_name.as_bytes(), &article_id.to_bytes())?;
            ConflictableTransactionResult::<_, ()>::Ok(())
        })?;
        Ok(())
    }
    /// Add a new revision. Uses the current date and time as the date.
    /// The core part of this type as it touches *all* of its trees.
    pub fn add_revision(
        &self,
        article_id: Id,
        author_id: UserId,
        content: &str,
    ) -> Result<(RevId, RevisionMeta)> {
        let id = match self.get_current_revision(article_id)? {
            Some((rev_id, rev)) => {
                if rev.content == content {
                    return Err(Error::IdenticalNewRevision);
                }
                rev_id.next()
            }
            None => RevId::first(article_id),
        };

        let date = Utc::now();
        // Just to get rid of an unnecessary level of indentation.
        (&self.revid_content, &self.revid_author, &self.revid_date).transaction(
            |(revid_content, revid_author, revid_date)| {
                use sled::transaction::ConflictableTransactionError::Abort;

                // The easy stuff is what's already bytes - so, Strings.
                revid_content.insert(&id.to_bytes(), content.as_bytes())?;
                revid_author.insert(&id.to_bytes(), &author_id.to_bytes())?;

                // The date needs to be serialized.
                let date_bytes = bincode::serialize(&date)
                    .map_err(Error::from)
                    .map_err(Abort)?;
                revid_date.insert(&id.to_bytes(), date_bytes)?;
                Ok(())
            },
        )?;

        let revision = RevisionMeta { author_id, date };
        Ok((id, revision))
    }
}
