use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Serialize;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::Result;

/// A revision id.
/// This type wraps an article id and a revision number (both u32).
/// It is used to store an article's revision so it's easier to query
/// e.g. the latest revision of an article.
/// Values of this type can only ever be obtained from the database.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevId(pub Uuid, pub i64);

#[derive(Debug, PartialEq, Serialize)]
pub struct Revision {
    pub content: String,
    pub author_id: Uuid,
    pub date: DateTime<Utc>,
}

pub struct DisplayRevision {
    pub rev_id: i64,
    pub author_name: String,
    pub content: String,
    pub created: NaiveDateTime,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct RevisionMeta {
    pub author_id: Uuid,
    pub date: DateTime<Utc>,
}

pub struct ArticleWithRevision {
    pub id: Uuid,
    pub name: String,
    pub content: String,
    pub rev_created: NaiveDateTime,
}

/// Get the id for the given article name if it exists.
pub async fn id_by_name(conn: &mut PgConnection, name: &str) -> Result<Option<Uuid>> {
    Ok(
        sqlx::query_scalar!("SELECT id FROM article WHERE name = $1", name)
            .fetch_optional(&mut *conn)
            .await?,
    )
}
/// Lists the articles from the database, returning the article name, id and
/// the latest revision.
pub async fn list_articles(pool: &PgPool) -> Result<Vec<ArticleWithRevision>> {
    Ok(sqlx::query_as!(
        ArticleWithRevision,
        r#"SELECT a.id AS "id!", a.name AS "name!", r.content AS "content!",
        r.created AS "rev_created!"
        FROM article a
        INNER JOIN revision r ON (a.id = r.article_id)
        WHERE r.num = (SELECT MAX(num) FROM revision WHERE article_id = a.id)"#
    )
    .fetch_all(pool)
    .await?)
}

#[derive(Serialize)]
pub struct ListRevision {
    pub num: i64,
    pub author_name: String,
    pub date: NaiveDateTime,
}
/// Retrieves the list of revision ids for the given article id.
/// Returns Ok(empty Vec) when the article doesn't exist.
/// Returns RevisionMeta because loading the revision's content doesn't
/// make sense for listing the revisions.
pub async fn list_revisions(pool: &PgPool, article_name: &str) -> Result<Vec<ListRevision>> {
    Ok(sqlx::query_as!(
        ListRevision,
        r#"SELECT r.num, u.name AS author_name, r.created AS date
        FROM revision r
        INNER JOIN "user" u ON u.id = r.author_id
        WHERE article_id = (SELECT id FROM article WHERE name = $1)
        ORDER BY r.num ASC"#,
        article_name
    )
    .fetch_all(pool)
    .await?)
}

/// Get the current revision for the given article id if it exists.
/// Will return None if the article doesn't exist.
pub async fn get_current_rev(pool: &PgPool, article_name: &str) -> Result<Option<DisplayRevision>> {
    Ok(sqlx::query_as!(
        DisplayRevision,
        r#"SELECT r.num AS rev_id, u.name AS author_name, r.content, r.created
        FROM article a
        INNER JOIN revision r ON (a.id = r.article_id)
        INNER JOIN "user" u ON (u.id = r.author_id)
        WHERE a.name = $1
        AND r.num = (SELECT MAX(num) FROM revision WHERE article_id = a.id)"#,
        article_name,
    )
    .fetch_optional(pool)
    .await?)
}
/// Get all data for the given verified revision id
pub async fn get_revision(
    pool: &PgPool,
    article_name: &str,
    num: i64,
) -> Result<Option<DisplayRevision>> {
    Ok(sqlx::query_as!(
        DisplayRevision,
        r#"SELECT r.num AS rev_id, r.content, u.name AS author_name, r.created
        FROM revision r
        INNER JOIN "user" u ON u.id = r.author_id
        WHERE r.article_id = (SELECT id FROM article WHERE name = $1)
        AND r.num = $2"#,
        article_name,
        num,
    )
    .fetch_optional(pool)
    .await?)
}
/// Create an empty article with no revisions.
pub async fn create(
    txn: &mut Transaction<'_, Postgres>,
    name: &str,
    content: &str,
    author_id: Uuid,
) -> Result<(RevId, RevisionMeta)> {
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO article(id, name, creator_id)
        VALUES($1, $2, $3)",
        id,
        name,
        author_id,
    )
    .execute(&mut *txn)
    .await?;
    let rev_num = 1;
    let date = sqlx::query_scalar!(
        "INSERT INTO revision(article_id, num, content, author_id)
        VALUES($1, $2, $3, $4)
        RETURNING created",
        id,
        rev_num,
        content,
        author_id
    )
    .fetch_one(&mut *txn)
    .await?;
    Ok((
        RevId(id, rev_num),
        RevisionMeta {
            author_id,
            date: DateTime::from_utc(date, Utc),
        },
    ))
}
/// Updates the name for the given article.
/// This internally changes two sled trees, removing the old article name and
/// adding the new one in the name_id tree, and updating it in the id_name tree.
pub async fn change_name(conn: &mut PgConnection, article_id: Uuid, new_name: &str) -> Result<()> {
    sqlx::query!(
        "UPDATE article SET name = $1 WHERE id = $2",
        new_name,
        article_id,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}
/// Add a new revision. Uses the current date and time as the date.
/// The core part of this type as it touches *all* of its trees.
pub async fn add_revision(
    conn: &mut PgConnection,
    article_id: Uuid,
    author_id: Uuid,
    content: &str,
) -> Result<(RevId, RevisionMeta)> {
    let (rev_num, date) = sqlx::query!(
        "INSERT INTO revision(article_id, num, content, author_id)
        VALUES ($1, (SELECT MAX(num) + 1 FROM revision WHERE article_id = $1), $2, $3)
        RETURNING num, created",
        article_id,
        content,
        author_id,
    )
    .fetch_one(&mut *conn)
    .await
    .map(|r| (r.num, DateTime::from_utc(r.created, Utc)))?;

    let id = RevId(article_id, rev_num);
    let revision = RevisionMeta { author_id, date };
    Ok((id, revision))
}
