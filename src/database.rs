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
    use articles::{Revision, RevisionMeta};

    use super::*;

    fn db() -> crate::Result<Db> {
        let sled_db = sled::Config::default().temporary(true).open()?;
        Db::load_or_create(sled_db)
    }

    #[test]
    fn create_database() {
        db().unwrap();
    }

    #[test]
    fn register_and_login() -> crate::Result<()> {
        let db = db()?;
        let username = "someone";
        let password = "hunter2";
        let user_id = db.users.register(&username, &password)?;
        let user2_id = db.users.register("username", "password")?;
        // Make sure the user exists now
        assert!(db.users.name_exists(username)?);
        assert_eq!(db.users.id_by_name(username)?, Some(user_id));
        assert_eq!(db.users.name_by_id(user_id)?.as_deref(), Some(username));
        // Verifying a correct password creates a session
        let session = db
            .users
            .try_login(user_id, password)?
            .expect("Correct user_id and password should yield a session");
        // The session's id should be enough to get back the user id
        assert_eq!(
            db.users.get_session_user(session.session_id)?,
            Some(session.user_id)
        );
        // Destroy the session again
        assert!(db.users.destroy_session(session.session_id).is_ok());
        // Verifying a wrong password returns false
        assert!(db.users.try_login(user_id, "password123")?.is_none());
        // Verifying the wrong user returns false
        // Note that it should not be possible to trigger a PasswordNotFound with
        // normal code anymore, so the verification will just fail.
        assert!(db.users.try_login(user2_id, password)?.is_none());
        Ok(())
    }

    #[test]
    fn create_article_and_revision() -> crate::Result<()> {
        let db = db()?;
        let article_name = "MainPage";
        let author_id = db.users.register("username", "password")?;
        let content = r#"
This is a **fun** Article with some minimal *Markdown* in it.
[Link](Link)"#;

        // Create our article
        let article_id = db.articles.create(article_name)?;
        // Verify it exists now
        assert!(db.articles.name_exists(article_name)?);
        assert_eq!(db.articles.id_by_name(article_name)?, Some(article_id));
        assert_eq!(
            db.articles.name_by_id(article_id)?.as_deref(),
            Some(article_name)
        );
        // ...but it doesn't have any revisions yet
        assert_eq!(db.articles.list_revisions(article_id)?.len(), 0);
        // meaning trying to get the current content or revision doesn't return anything
        assert_eq!(db.articles.get_current_content(article_id)?, None);
        assert_eq!(db.articles.get_current_revision(article_id)?, None);
        // After checking for all of that, we add our first revision
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
        let content1 = db.articles.get_rev_content(rev1_id)?;
        let content2 = db.articles.get_rev_content(rev2_id)?;
        let content3 = db.articles.get_rev_content(rev3_id)?;

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

    #[test]
    fn query_specific_revisions() -> crate::Result<()> {
        // Basic setup
        let db = db()?;
        let article_name = "MainPage";
        let article_id = db.articles.create(article_name)?;
        let user_id = db.users.register("user1", "password123")?;

        // Store some revisions
        let (rev1_id, rev1_meta) = db.articles.add_revision(article_id, user_id, "abc")?;
        let (rev2_id, _) = db.articles.add_revision(article_id, user_id, "123")?;
        let (rev3_id, rev3_meta) = db.articles.add_revision(article_id, user_id, "abc123")?;

        // We now query them and then check if they match with what we know
        let rev1 = db.articles.get_revision(rev1_id)?;
        assert_eq!(rev1.content.as_str(), "abc");
        assert_eq!(rev1.author_id, rev1_meta.author_id);
        assert_eq!(rev1.author_id, user_id);
        assert_eq!(rev1.date, rev1_meta.date);

        // Maybe we don't need the whole info about the revision, possibly we
        // already know the author_id; query only the missing information.
        let rev2_content = db.articles.get_rev_content(rev2_id)?;
        assert_eq!(rev2_content.as_str(), "123");
        // We can't compare this to anything, but it should be there, right?
        db.articles
            .get_rev_date(rev2_id)
            .expect("Date should be there");

        // We may also just not care about specific revisions, we may just want the current one.
        let (curr_rev_id, curr_rev) = db
            .articles
            .get_current_revision(article_id)?
            .expect("article should have revisions");
        assert_eq!(curr_rev_id, rev3_id);
        assert_eq!(curr_rev.content.as_str(), "abc123");
        assert_eq!(curr_rev.author_id, rev3_meta.author_id);
        assert_eq!(curr_rev.author_id, user_id);
        assert_eq!(curr_rev.date, rev3_meta.date);
        // The current content can also be queried separately.
        // This is currently used on the edit page.
        assert_eq!(
            db.articles.get_current_content(article_id)?,
            Some(curr_rev.content)
        );

        Ok(())
    }
}
