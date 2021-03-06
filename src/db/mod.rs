use rocket::{
    outcome::try_outcome,
    request::{FromRequest, Outcome},
    Request,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::Result;

pub mod articles;
use articles::{ArticleWithRevision, DisplayRevision};
pub mod users;
use users::UserSession;

pub struct Db {
    pub pool: PgPool,
}
impl std::ops::Deref for Db {
    type Target = PgPool;
    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

/// Settings keys
mod flags {
    pub const REGISTRATION_ENABLED: &str = "global:registration_enabled";
}

#[derive(Debug, Clone, Copy)]
pub struct EnabledRegistration;
#[rocket::async_trait]
impl<'r> FromRequest<'r> for EnabledRegistration {
    type Error = crate::Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        use crate::error::IntoOutcomeHack;
        use rocket::outcome::IntoOutcome;
        let db: &Db = try_outcome!(request.rocket().state().or_forward(()));
        if try_outcome!(db.registration_enabled().await.into_outcome_hack()) {
            Outcome::Success(EnabledRegistration)
        } else {
            Outcome::Forward(())
        }
    }
}

impl Db {
    pub async fn try_connect(uri: &str) -> Result<Self> {
        Ok(Self {
            pool: PgPool::connect(uri).await?,
        })
    }

    pub async fn user_name_exists(&self, username: &str) -> Result<bool> {
        users::name_exists(self, username).await
    }
    pub async fn user_id_by_name(&self, username: &str) -> Result<Option<Uuid>> {
        Ok(
            sqlx::query_scalar!(r#"SELECT id FROM "user" WHERE name = $1"#, username)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    pub async fn register_user(&self, username: &str, password: String) -> Result<()> {
        users::register(self, username, password).await?;
        Ok(())
    }
    pub async fn try_login(&self, username: &str, password: String) -> Result<UserSession> {
        users::try_login(self, username, password).await
    }
    pub async fn get_session_user(&self, session_id: Uuid) -> Result<Option<Uuid>> {
        users::get_session_user(self, session_id).await
    }
    pub async fn destroy_session(&self, session_id: Uuid) -> Result<()> {
        users::destroy_session(self, session_id).await
    }
    pub async fn user_is_admin(&self, user_id: Uuid) -> Result<bool> {
        users::is_admin(self, user_id).await
    }

    pub async fn article_id_by_name(&self, article_name: &str) -> Result<Option<Uuid>> {
        let mut conn = self.acquire().await?;
        articles::id_by_name(&mut conn, article_name).await
    }
    pub async fn list_articles(&self) -> Result<Vec<ArticleWithRevision>> {
        articles::list_articles(self).await
    }
    pub async fn get_current_rev(&self, article_name: &str) -> Result<Option<DisplayRevision>> {
        articles::get_current_rev(self, article_name).await
    }

    async fn set_flag(&self, flag: &str, value: bool) -> Result<()> {
        sqlx::query!(
            "INSERT INTO flags(name, value)
            VALUES($1, $2)
            ON CONFLICT(name) DO UPDATE SET value = $2",
            flag,
            value
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn registration_enabled(&self) -> Result<bool> {
        Ok(sqlx::query_scalar!(
            "SELECT value FROM flags WHERE name = $1",
            flags::REGISTRATION_ENABLED
        )
        .fetch_optional(&**self)
        .await?
        .unwrap_or(true))
    }
    pub async fn set_registration_enabled(&self, value: bool) -> Result<()> {
        self.set_flag(flags::REGISTRATION_ENABLED, value).await
    }
}

// #[cfg(test)]
// mod tests {
//     use articles::{Revision, RevisionMeta};

//     use super::*;

//     fn db() -> Db {
//         let sled_db = sled::Config::default()
//             .temporary(true)
//             .open()
//             .expect("Failed to create sled db");
//         Db::load_or_create(sled_db)
//             .and_then(crate::seed_db)
//             .expect("Failed to open database")
//     }

//     #[test]
//     fn create_database() {
//         db();
//     }

//     #[test]
//     fn register_and_login() -> crate::Result<()> {
//         let db = db();
//         let username = "someone";
//         let password = "hunter2";
//         let user_id = db.users.register(&username, &password)?;
//         let user2_id = db.users.register("username", "password")?;
//         // Make sure the user exists now
//         assert!(db.users.name_exists(username)?);
//         assert_eq!(db.users.id_by_name(username)?, Some(user_id));
//         assert_eq!(db.users.name_by_id(user_id)?.as_str(), username);
//         // Verifying a correct password creates a session
//         let session = db
//             .users
//             .try_login(user_id, password)?
//             .expect("Correct user_id and password should yield a session");
//         // The session's id should be enough to get back the user id
//         assert_eq!(
//             db.users.get_session_user(session.session_id)?,
//             Some(session.user_id)
//         );
//         // Destroy the session again
//         assert!(db.users.destroy_session(session.session_id).is_ok());
//         // Verifying a wrong password returns false
//         assert!(db.users.try_login(user_id, "password123")?.is_none());
//         // Verifying the wrong user returns false
//         // Note that it should not be possible to trigger a PasswordNotFound with
//         // normal code anymore, so the verification will just fail.
//         assert!(db.users.try_login(user2_id, password)?.is_none());
//         Ok(())
//     }

//     #[test]
//     fn first_user_is_admin() -> crate::Result<()> {
//         let db = db();
//         let user_id = db.users.register("username", "password")?;
//         assert!(db.users.is_admin(user_id)?);
//         let user_id = db.users.register("user2", "password123")?;
//         assert!(!db.users.is_admin(user_id)?);
//         Ok(())
//     }

//     #[test]
//     fn settings() {
//         let db = db();
//         assert!(db.registration_enabled().unwrap());
//         db.set_registration_enabled(false).unwrap();
//         assert!(!db.registration_enabled().unwrap());
//     }

//     #[test]
//     fn create_article_and_revision() -> crate::Result<()> {
//         let db = db();
//         let article_name = "MainPage";
//         let author_id = db.users.register("username", "password")?;
//         let content = r#"
// This is a **fun** Article with some minimal *Markdown* in it.
// [Link](Link)"#;

//         // Create our article
//         let article_id = db.articles.create(article_name)?;
//         // Verify it exists now
//         assert!(db.articles.name_exists(article_name)?);
//         assert_eq!(db.articles.id_by_name(article_name)?, Some(article_id));
//         assert_eq!(db.articles.name_by_id(article_id)?.as_str(), article_name);
//         // ...but it doesn't have any revisions yet
//         assert_eq!(db.articles.list_revisions(article_id)?.len(), 0);
//         // meaning trying to get the current content or revision doesn't return anything
//         assert_eq!(db.articles.get_current_content(article_id)?, None);
//         assert_eq!(db.articles.get_current_revision(article_id)?, None);
//         // After checking for all of that, we add our first revision
//         let (rev_id, rev) = db.articles.add_revision(article_id, author_id, content)?;
//         // Verify it's now also the current revision
//         assert_eq!(
//             rev_id,
//             db.articles.get_current_revision(article_id)?.unwrap().0
//         );
//         // Retrieve it manually, just to be sure
//         let rev_from_db = db.articles.get_revision(rev_id)?;
//         let RevisionMeta { author_id, date } = rev;
//         let rev = Revision {
//             author_id,
//             date,
//             content: content.into(),
//         };
//         assert_eq!(rev, rev_from_db);

//         // Add another revision
//         let new_content = r#"
// This is a **fun** Article with some minimal *Markdown* in it.
// Something [Link](Links) to something else. New content. Ha ha ha."#;
//         let (new_rev_id, new_rev) = db
//             .articles
//             .add_revision(article_id, author_id, new_content)?;

//         // Verify it's now also the current revision
//         assert_eq!(
//             new_rev_id,
//             db.articles.get_current_revision(article_id)?.unwrap().0
//         );

//         // Verify the new rev id is different
//         assert_ne!(rev_id, new_rev_id);
//         // Verify the new revision is different
//         let RevisionMeta { author_id, date } = new_rev;
//         let new_rev = Revision {
//             author_id,
//             date,
//             content: new_content.to_string(),
//         };
//         assert_ne!(rev, new_rev);
//         Ok(())
//     }

//     #[test]
//     fn add_and_list_revisions() -> crate::Result<()> {
//         let db = db();
//         let article_name = "MainPage";
//         let article_id = db.articles.create(article_name)?;
//         let user1_id = db.users.register("user1", "password123")?;
//         let user2_id = db.users.register("user2", "password123")?;
//         let user3_id = db.users.register("user3", "password123")?;

//         let (rev1_id, _) = db.articles.add_revision(article_id, user1_id, "abc")?;
//         let (rev2_id, _) = db.articles.add_revision(article_id, user2_id, "123")?;
//         let (rev3_id, _) = db.articles.add_revision(article_id, user3_id, "abc123")?;

//         // Retrieve the revisions from the db again
//         let revisions = db.articles.list_revisions(article_id)?;

//         // First, compare the ids to make sure they're the same
//         let revision_ids = revisions
//             .iter()
//             .map(|(id, _)| id)
//             .copied()
//             .collect::<Vec<_>>();
//         assert_eq!(revision_ids, vec![rev1_id, rev2_id, rev3_id]);

//         // Extract the other available information
//         let mut iter = revisions.into_iter();
//         let rev1 = iter.next().unwrap().1;
//         let rev2 = iter.next().unwrap().1;
//         let rev3 = iter.next().unwrap().1;
//         assert_eq!(iter.next(), None);

//         // And compare the author's names
//         assert_eq!(rev1.author_id, user1_id);
//         assert_eq!(rev2.author_id, user2_id);
//         assert_eq!(rev3.author_id, user3_id);

//         // Retrieve the contents for the verified revision ids
//         let content1 = db.articles.get_rev_content(rev1_id)?;
//         let content2 = db.articles.get_rev_content(rev2_id)?;
//         let content3 = db.articles.get_rev_content(rev3_id)?;

//         // Compare them to what we passed to add_revision
//         assert_eq!(content1.as_str(), "abc");
//         assert_eq!(content2.as_str(), "123");
//         assert_eq!(content3.as_str(), "abc123");

//         // Verify that the latest revision is correct
//         assert_eq!(
//             rev3_id,
//             db.articles.get_current_revision(article_id)?.unwrap().0
//         );

//         Ok(())
//     }

//     #[test]
//     fn query_specific_revisions() -> crate::Result<()> {
//         // Basic setup
//         let db = db();
//         let article_name = "MainPage";
//         let article_id = db.articles.create(article_name)?;
//         let user_id = db.users.register("user1", "password123")?;

//         // Store some revisions
//         let (rev1_id, rev1_meta) = db.articles.add_revision(article_id, user_id, "abc")?;
//         let (rev2_id, _) = db.articles.add_revision(article_id, user_id, "123")?;
//         let (rev3_id, rev3_meta) = db.articles.add_revision(article_id, user_id, "abc123")?;

//         // We now query them and then check if they match with what we know
//         let rev1 = db.articles.get_revision(rev1_id)?;
//         assert_eq!(rev1.content.as_str(), "abc");
//         assert_eq!(rev1.author_id, rev1_meta.author_id);
//         assert_eq!(rev1.author_id, user_id);
//         assert_eq!(rev1.date, rev1_meta.date);

//         // Maybe we don't need the whole info about the revision, possibly we
//         // already know the author_id; query only the missing information.
//         let rev2_content = db.articles.get_rev_content(rev2_id)?;
//         assert_eq!(rev2_content.as_str(), "123");
//         // We can't compare this to anything, but it should be there, right?
//         db.articles
//             .get_rev_date(rev2_id)
//             .expect("Date should be there");

//         // We may also just not care about specific revisions, we may just want the current one.
//         let (curr_rev_id, curr_rev) = db
//             .articles
//             .get_current_revision(article_id)?
//             .expect("article should have revisions");
//         assert_eq!(curr_rev_id, rev3_id);
//         assert_eq!(curr_rev.content.as_str(), "abc123");
//         assert_eq!(curr_rev.author_id, rev3_meta.author_id);
//         assert_eq!(curr_rev.author_id, user_id);
//         assert_eq!(curr_rev.date, rev3_meta.date);
//         // The current content can also be queried separately.
//         // This is currently used on the edit page.
//         assert_eq!(
//             db.articles.get_current_content(article_id)?,
//             Some(curr_rev.content)
//         );

//         Ok(())
//     }

//     #[test]
//     fn rename_article() {
//         let db = db();
//         let article_id = db
//             .articles
//             .create("name1")
//             .expect("failed to create article");
//         assert!(db.articles.name_exists("name1").unwrap());
//         db.articles
//             .change_name(article_id, "name2")
//             .expect("failed to rename article");
//         assert!(!db.articles.name_exists("name1").unwrap());
//         assert_eq!(db.articles.id_by_name("name2").unwrap(), Some(article_id));
//     }

//     #[test]
//     fn verified_rev_id() {
//         let db = db();
//         let author_id = db
//             .users
//             .register("user1", "password123")
//             .expect("failed to register user");
//         let article_id = db
//             .articles
//             .create("article")
//             .expect("failed to create article");
//         let (rev_id, _rev) = db
//             .articles
//             .add_revision(article_id, author_id, "blah blah blah")
//             .expect("failed to create revision");
//         // Verify a valid article id + rev number
//         assert_eq!(
//             db.articles.verified_rev_id(rev_id.0, rev_id.1).unwrap(),
//             rev_id
//         );
//         // Verify an invalid rev number returns the appropriate error
//         assert!(matches!(
//             db.articles.verified_rev_id(article_id, rev_id.1.next()),
//             Err(crate::Error::RevisionUnknown(_, _))
//         ));
//     }
// }
