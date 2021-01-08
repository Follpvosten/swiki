use crate::Result;

pub mod id;
pub use id::Id;

pub mod articles;
use articles::Articles;
pub mod users;
use users::Users;

pub struct Db {
    pub users: Users,
    pub articles: Articles,
    inner: sled::Db,
}

impl Db {
    pub fn load_or_create(db: sled::Db) -> Result<Self> {
        Ok(Self {
            users: Users {
                username_userid: db.open_tree("username_userid")?,
                userid_username: db.open_tree("userid_username")?,
                userid_password: db.open_tree("userid_password")?,
                userid_email: db.open_tree("userid_email")?,
                sessionid_userid: db.open_tree("sessionid_userid")?,
            },
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

    /// Invoke flush_async().await on the internal database to sync
    /// any pending data to disk.
    pub async fn flush(&self) -> Result<()> {
        self.inner.flush_async().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Not;

    use articles::{rev_id::RevId, Revision, RevisionMeta};

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
        let username = "someone";
        let password = "hunter2";
        let user_id = db.users.register(&username, &password)?;
        let user2_id = db.users.register("username", "password")?;
        // Make sure the user exists now
        assert!(db.users.name_by_id(user_id)?.is_some());
        // Verifying a correct password returns true
        assert!(db.users.verify_password(user_id, password)?);
        // Verifying a wrong password returns false
        assert!(db.users.verify_password(user_id, "password123")?.not());
        // Verifying the wrong user returns false
        // Note that it should not be possible to trigger a PasswordNotFound with
        // normal code anymore, so the verification will just fail.
        assert!(matches!(
            db.users.verify_password(user2_id, password),
            Ok(false)
        ));
        Ok(())
    }

    #[test]
    fn create_article_and_revisions() -> crate::Result<()> {
        let db = db()?;
        let article_name = "MainPage";
        let author_id = db.users.register("username", "password")?;
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
        let user1_id = db.users.register("user1", "password123")?;
        let user2_id = db.users.register("user2", "password123")?;
        let user3_id = db.users.register("user3", "password123")?;

        let (rev1_id, _) = db.articles.add_revision(article_id, user1_id, "abc")?;
        let (rev2_id, _) = db.articles.add_revision(article_id, user2_id, "123")?;
        let (rev3_id, _) = db.articles.add_revision(article_id, user3_id, "abc123")?;

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
        assert_eq!(rev1.author_id, user1_id);
        assert_eq!(rev2.author_id, user2_id);
        assert_eq!(rev3.author_id, user3_id);

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
