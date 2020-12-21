use sled::Tree;

use crate::Error;

pub mod articles;
use articles::Articles;

pub mod id;
pub use id::Id;

pub struct Db {
    userid_password: Tree,
    userid_email: Tree,
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
            userid_password: db.open_tree("userid_password")?,
            userid_email: db.open_tree("userid_email")?,
            articles: Articles {
                articleid_revisions: db.open_tree("articleid_revisions")?,
                articleid_current_revision: db.open_tree("articleid_current_revision")?,
                revisionid_content: db.open_tree("revisionid_content")?,
                revisionid_author: db.open_tree("revisionid_author")?,
                revisionid_date: db.open_tree("revisionid_date")?,
            },
            inner: db,
        })
    }

    // TODO: Email?
    pub fn register_user(&self, id: &Id, password: &str) -> crate::Result<()> {
        if self.userid_password.contains_key(id)? {
            return Err(Error::UserAlreadyExists(id.clone()));
        }
        let password_hashed = hash_password(password)?;
        self.userid_password
            .insert(&id, password_hashed.as_bytes())?;
        Ok(())
    }

    pub fn verify_password(&self, user_id: &Id, password: &str) -> crate::Result<bool> {
        if let Some(raw_hash) = self.userid_password.get(user_id)? {
            let hash = String::from_utf8(raw_hash.to_vec()).unwrap();
            Ok(verify_password(&hash, password)?)
        } else {
            Err(Error::UserNotFound(user_id.clone()))
        }
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
    use super::*;
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
        let id = Id::from("someone");
        let password = "hunter2".to_string();
        db.register_user(&id, &password)?;
        // Verifying a correct password returns true
        assert!(db.verify_password(&id, &password)?);
        // Verifying a wrong password returns false
        assert!(!db.verify_password(&id, "password123")?);
        // Verifying an unknown user returns an error
        assert!(matches!(
            db.verify_password(&Id::from("someone else"), &password),
            Err(Error::UserNotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn create_article_or_revision() -> crate::Result<()> {
        let db = db()?;
        let article_id = Id::from("MainPage");
        let author = Id::from("someone");
        let content = r#"
This is a **fun** Article with some minimal *Markdown* in it.
[Link](Link)"#;

        // Store it first
        let (rev_id, rev) = db.articles.add_revision(&article_id, &author, content)?;
        // Verify it's now also the current revision
        assert_eq!(
            rev_id,
            db.articles.get_current_revision_id(&article_id)?.unwrap()
        );
        // Retrieve it manually, just to be sure
        let rev_from_db = db.articles.get_revision(rev_id)?;
        assert_eq!(rev, rev_from_db);

        // Add another revision
        let new_content = r#"
This is a **fun** Article with some minimal *Markdown* in it.
Something [Link](Links) to something else. New content. Ha ha ha."#;
        let (new_rev_id, new_rev) = db
            .articles
            .add_revision(&article_id, &author, new_content)?;

        // Verify it's now also the current revision
        assert_eq!(
            new_rev_id,
            db.articles.get_current_revision_id(&article_id)?.unwrap()
        );

        // Verify the new rev id is different
        assert_ne!(rev_id, new_rev_id);
        // Verify the new revision is different
        assert_ne!(rev, new_rev);
        Ok(())
    }

    #[test]
    fn add_and_list_revisions() -> crate::Result<()> {
        let db = db()?;
        let article_id = Id::from("MainPage");
        let (rev1_id, _) = db
            .articles
            .add_revision(&article_id, &"someone".into(), "abc")?;
        let (rev2_id, _) = db
            .articles
            .add_revision(&article_id, &"someone else".into(), "123")?;
        let (rev3_id, _) =
            db.articles
                .add_revision(&article_id, &"someone yet again".into(), "abc123")?;

        // Retrieve the revisions from the db again
        let revisions = db.articles.list_revisions(&article_id)?.unwrap();

        // First, compare the ids to make sure they're the same
        let revision_ids = revisions
            .iter()
            .map(|(id, _)| id)
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(revision_ids, vec![rev1_id, rev2_id, rev3_id]);

        // Extract the other available information
        let mut iter = revisions.into_iter();
        let rev1 = iter.next().unwrap().1;
        let rev2 = iter.next().unwrap().1;
        let rev3 = iter.next().unwrap().1;
        assert_eq!(iter.next(), None);

        // And compare the author's names
        assert_eq!(rev1.author, "someone".into());
        assert_eq!(rev2.author, "someone else".into());
        assert_eq!(rev3.author, "someone yet again".into());

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
            db.articles.get_current_revision_id(&article_id)?.unwrap()
        );

        Ok(())
    }
}
