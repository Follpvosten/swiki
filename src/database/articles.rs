use chrono::{DateTime, Utc};
use serde::Serialize;
use sled::{transaction::Transactional, Tree};
use uuid::Uuid;

use super::Id;
use crate::{Error, Result};

pub struct Articles {
    // articleid = article's name
    pub(super) articleid_revisions: Tree,
    pub(super) articleid_current_revision: Tree,
    // revision key = revision uuid
    pub(super) revisionid_content: Tree,
    pub(super) revisionid_author: Tree,
    pub(super) revisionid_date: Tree,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Revision {
    pub content: String,
    pub author: Id,
    pub date: DateTime<Utc>,
}

#[derive(Debug, PartialEq)]
pub struct RevisionMeta {
    pub author: Id,
    pub date: DateTime<Utc>,
}

impl Articles {
    /// Retrieves the list of revision ids for the given article id.
    /// Returns Ok(None) when the article doesn't exist.
    /// Returns RevisionMeta because loading the revision's content doesn't
    /// make sense for listing the revisions.
    pub fn list_revisions(&self, article_id: &Id) -> Result<Option<Vec<(Uuid, RevisionMeta)>>> {
        let revision_ids: Vec<Uuid> = match self.articleid_revisions.get(article_id)? {
            None => return Ok(None),
            Some(ivec) => bincode::deserialize(&ivec)?,
        };
        let mut result = Vec::with_capacity(revision_ids.len());
        for rev_id in revision_ids {
            let author = self
                .revisionid_author
                .get(rev_id.as_bytes())?
                .ok_or(Error::RevisionUnknown(rev_id))?
                .into();
            let date = self
                .revisionid_date
                .get(rev_id.as_bytes())?
                .ok_or(Error::RevisionUnknown(rev_id))
                .and_then(|ivec| bincode::deserialize(&*ivec).map_err(From::from))?;
            result.push((rev_id, RevisionMeta { author, date }));
        }
        Ok(Some(result))
    }
    /// Get the current revision for the given article id if it exists.
    pub fn get_current_revision_id(&self, article_id: &Id) -> Result<Option<Uuid>> {
        Ok(match self.articleid_current_revision.get(article_id)? {
            None => None,
            Some(ivec) => Some(Uuid::from_slice(&ivec)?),
        })
    }
    pub fn get_revision_content(&self, revision_id: Uuid) -> Result<String> {
        self.revisionid_content
            .get(revision_id.as_bytes())?
            //                  will panic on disk corruption v
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap())
            .ok_or(Error::RevisionUnknown(revision_id))
    }
    /// Get all data for the given revision
    pub fn get_revision(&self, revision_id: Uuid) -> Result<Revision> {
        let content = self.get_revision_content(revision_id)?;
        let author = self
            .revisionid_author
            .get(revision_id.as_bytes())?
            .ok_or(Error::RevisionUnknown(revision_id))?
            .into();
        let date = self
            .revisionid_date
            .get(revision_id.as_bytes())?
            .ok_or(Error::RevisionUnknown(revision_id))
            .and_then(|ivec| bincode::deserialize(&*ivec).map_err(From::from))?;

        Ok(Revision {
            content,
            author,
            date,
        })
    }
    /// Add a new revision. Uses the current date and time as the date.
    /// The core part of this type as it touches *all* of its trees.
    pub fn add_revision(
        &self,
        article_id: &Id,
        author: &Id,
        content: &str,
    ) -> Result<(Uuid, Revision)> {
        if let Some(id) = self.get_current_revision_id(article_id)? {
            let prev_content = self.get_revision_content(id)?;
            if prev_content == content {
                return Err(Error::IdenticalNewRevision);
            }
        }
        let id = Uuid::new_v4();
        let date = Utc::now();
        // Just to get rid of an unnecessary level of indentation.
        let all_the_trees = (
            &self.revisionid_content,
            &self.revisionid_author,
            &self.revisionid_date,
            &self.articleid_revisions,
            &self.articleid_current_revision,
        );
        all_the_trees.transaction(
            |(revid_content, revid_author, revid_date, articleid_revs, articleid_currrev)| {
                use sled::transaction::ConflictableTransactionError::Abort;

                // The easy stuff is what's already bytes - so, Strings.
                revid_content.insert(id.as_bytes(), content.as_bytes())?;
                revid_author.insert(id.as_bytes(), author.as_ref())?;

                // The date needs to be serialized.
                let date_bytes = bincode::serialize(&date)
                    .map_err(Error::from)
                    .map_err(Abort)?;
                revid_date.insert(id.as_bytes(), date_bytes)?;

                articleid_currrev.insert(article_id.as_ref(), id.as_bytes())?;
                let new_revs = match articleid_revs.get(&article_id)? {
                    None => bincode::serialize(&vec![id])
                        .map_err(Error::from)
                        .map_err(Abort)?,
                    Some(revs_raw) => {
                        let mut curr_revs: Vec<Uuid> = bincode::deserialize(&revs_raw)
                            .map_err(Error::from)
                            .map_err(Abort)?;
                        curr_revs.push(id);
                        bincode::serialize(&curr_revs)
                            .map_err(Error::from)
                            .map_err(Abort)?
                    }
                };
                articleid_revs.insert(article_id.as_ref(), new_revs)?;
                Ok(())
            },
        )?;

        let revision = Revision {
            content: content.to_string(),
            author: author.clone(),
            date,
        };
        Ok((id, revision))
    }
}
