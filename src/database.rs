use std::convert::TryInto;

use sled::{transaction::Transactional, Tree};
use uuid::Uuid;

use crate::Error;

pub mod id;
pub use id::Id;

pub mod articles;
use articles::Articles;

pub struct Db {
    username_userid: Tree,
    userid_username: Tree,
    userid_password: Tree,
    userid_email: Tree,
    sessionid_userid: Tree,
    pub articles: Articles,
    inner: sled::Db,
}

fn hash_password(password: &str) -> Result<String, argon2::Error> {
    fn gen_salt() -> Vec<u8> {
        use rand::Rng;
        rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .collect()
    }
    let config = argon2::Config {
        variant: argon2::Variant::Argon2i,
        ..Default::default()
    };
    let salt = gen_salt();
    argon2::hash_encoded(password.as_bytes(), &salt, &config)
}

fn verify_password(hash: &str, password: &str) -> Result<bool, argon2::Error> {
    argon2::verify_encoded(hash, password.as_bytes())
}

impl Db {
    pub fn load_or_create(db: sled::Db) -> crate::Result<Self> {
        Ok(Self {
            username_userid: db.open_tree("username_userid")?,
            userid_username: db.open_tree("userid_username")?,
            userid_password: db.open_tree("userid_password")?,
            userid_email: db.open_tree("userid_email")?,
            sessionid_userid: db.open_tree("sessionid_userid")?,
            articles: Articles {
                articleid_name: db.open_tree("articleid_name")?,
                articlename_id: db.open_tree("articlename_id")?,
                revid_content: db.open_tree("revid_content")?,
                revid_author: db.open_tree("revid_author")?,
                revid_date: db.open_tree("revid_date")?,
            },
            inner: db,
        })
    }

    // TODO Email
    pub fn register_user(&self, username: &str, password: &str) -> crate::Result<Id> {
        if self.username_userid.contains_key(username.as_bytes())? {
            return Err(Error::UserAlreadyExists(username.to_string()));
        }
        let id = match self.userid_password.iter().next_back() {
            None => Id::first(),
            Some(res) => {
                let curr_id: Id = res?.0.as_ref().try_into()?;
                curr_id.next()
            }
        };
        (
            &self.username_userid,
            &self.userid_username,
            &self.userid_password,
        )
            .transaction(|(name_id, id_name, id_password)| {
                use sled::transaction::ConflictableTransactionError::Abort;
                name_id.insert(username.as_bytes(), &id.to_bytes())?;
                id_name.insert(&id.to_bytes(), username.as_bytes())?;

                let password_hashed = hash_password(password)
                    .map_err(Error::from)
                    .map_err(Abort)?;
                id_password.insert(&id.to_bytes(), password_hashed.as_bytes())?;
                Ok(())
            })?;
        Ok(id)
    }

    pub fn get_userid_by_name(&self, username: &str) -> crate::Result<Option<Id>> {
        Ok(self
            .username_userid
            .get(username.as_bytes())?
            .map(|ivec| ivec.as_ref().try_into())
            .transpose()?)
    }

    pub fn get_user_name(&self, user_id: Id) -> crate::Result<Option<String>> {
        Ok(self
            .userid_username
            .get(user_id.to_bytes())?
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap()))
    }

    pub fn verify_password(&self, user_id: Id, password: &str) -> crate::Result<bool> {
        let hash = self
            .userid_password
            .get(user_id.to_bytes())?
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap())
            .ok_or(Error::PasswordNotFound(user_id))?;
        Ok(verify_password(&hash, password)?)
    }

    pub fn create_session(&self, user_id: Id) -> crate::Result<Uuid> {
        let id = Uuid::new_v4();
        self.sessionid_userid
            .insert(id.as_bytes(), &user_id.to_bytes())?;
        Ok(id)
    }

    pub fn username_exists(&self, username: &str) -> crate::Result<bool> {
        Ok(self.username_userid.contains_key(username.as_bytes())?)
    }

    /// Call flush_async().await on the internal database to sync
    /// any pending data to disk.
    pub async fn flush(&self) -> sled::Result<()> {
        self.inner.flush_async().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use articles::{rev_id::RevId, Revision, RevisionMeta};

    use super::*;
    use std::ops::Not;
    /// Returns a memory-backed sled database.
    fn sled_db() -> sled::Db {
        use sled::Config;
        Config::default().temporary(true).open().unwrap()
    }
    fn db() -> crate::Result<Db> {
        Db::load_or_create(sled_db())
    }

    #[test]
    fn create_database() {
        db().unwrap();
    }

    #[test]
    fn register_user_verify_password() -> crate::Result<()> {
        let db = db()?;
        let username = "someone";
        let password = "hunter2";
        let user_id = db.register_user(&username, &password)?;
        // Make sure the user exists now
        assert!(db.get_user_name(user_id)?.is_some());
        // Verifying a correct password returns true
        assert!(db.verify_password(user_id, password)?);
        // Verifying a wrong password returns false
        assert!(db.verify_password(user_id, "password123")?.not());
        // Verifying an unknown user returns an error
        assert!(matches!(
            db.verify_password(Id(255), password),
            Err(Error::PasswordNotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn create_article_and_revisions() -> crate::Result<()> {
        let db = db()?;
        let article_name = "MainPage";
        let author_id = Id(1);
        let content = r#"
This is a **fun** Article with some minimal *Markdown* in it.
[Link](Link)"#;

        // Create our article
        let article_id = db.articles.create(article_name)?;
        // Store it first
        let (rev_id, rev) = db.articles.add_revision(article_id, author_id, content)?;
        // Verify it's now also the current revision
        assert_eq!(
            rev_id,
            db.articles.get_current_revision(article_id)?.unwrap().0
        );
        // Retrieve it manually, just to be sure
        let rev_from_db = db.articles.get_revision(rev_id)?;
        let RevisionMeta { author_id, date } = rev;
        let rev = Revision {
            author_id,
            date,
            content: content.into(),
        };
        assert_eq!(rev, rev_from_db);

        // Add another revision
        let new_content = r#"
This is a **fun** Article with some minimal *Markdown* in it.
Something [Link](Links) to something else. New content. Ha ha ha."#;
        let (new_rev_id, new_rev) = db
            .articles
            .add_revision(article_id, author_id, new_content)?;

        // Verify it's now also the current revision
        assert_eq!(
            new_rev_id,
            db.articles.get_current_revision(article_id)?.unwrap().0
        );

        // Verify the new rev id is different
        assert_ne!(rev_id, new_rev_id);
        // Verify the new revision is different
        let RevisionMeta { author_id, date } = new_rev;
        let new_rev = Revision {
            author_id,
            date,
            content: new_content.to_string(),
        };
        assert_ne!(rev, new_rev);
        Ok(())
    }

    #[test]
    fn add_and_list_revisions() -> crate::Result<()> {
        let db = db()?;
        let article_name = "MainPage";
        let article_id = db.articles.create(article_name)?;
        let (rev1_id, _) = db.articles.add_revision(article_id, Id(1), "abc")?;
        let (rev2_id, _) = db.articles.add_revision(article_id, Id(2), "123")?;
        let (rev3_id, _) = db.articles.add_revision(article_id, Id(3), "abc123")?;

        // Retrieve the revisions from the db again
        let revisions = db.articles.list_revisions(article_id)?;

        // First, compare the ids to make sure they're the same
        let revision_ids = revisions
            .iter()
            .map(|(id, _)| id)
            .copied()
            .map(|rev_id| RevId::from((article_id, rev_id)))
            .collect::<Vec<_>>();
        assert_eq!(revision_ids, vec![rev1_id, rev2_id, rev3_id]);

        // Extract the other available information
        let mut iter = revisions.into_iter();
        let rev1 = iter.next().unwrap().1;
        let rev2 = iter.next().unwrap().1;
        let rev3 = iter.next().unwrap().1;
        assert_eq!(iter.next(), None);

        // And compare the author's names
        assert_eq!(rev1.author_id, Id(1));
        assert_eq!(rev2.author_id, Id(2));
        assert_eq!(rev3.author_id, Id(3));

        // Retrieve the contents for the verified revision ids
        let content1 = db.articles.get_revision_content(rev1_id)?;
        let content2 = db.articles.get_revision_content(rev2_id)?;
        let content3 = db.articles.get_revision_content(rev3_id)?;

        // Compare them to what we passed to add_revision
        assert_eq!(content1.as_str(), "abc");
        assert_eq!(content2.as_str(), "123");
        assert_eq!(content3.as_str(), "abc123");

        // Verify that the latest revision is correct
        assert_eq!(
            rev3_id,
            db.articles.get_current_revision(article_id)?.unwrap().0
        );

        Ok(())
    }
}
