use std::collections::HashMap;

use rocket::{get, State};
use rocket_contrib::{templates::Template, uuid::Uuid};

use crate::{
    database::{
        articles::{Revision, RevisionMeta},
        Db, Id,
    },
    Result,
};

fn render_404(article_id: &Id) -> Template {
    let context: HashMap<_, _> = std::iter::once(("article_id", article_id)).collect();
    Template::render("article_404", context)
}

#[derive(serde::Serialize)]
struct RevContext {
    article_id: Id,
    rev_id: uuid::Uuid,
    content: String,
    author: Id,
    date: chrono::DateTime<chrono::Utc>,
    specific_rev: bool,
}

#[get("/<article_id>")]
pub fn get(db: State<Db>, article_id: Id) -> Result<Template> {
    if let Some(rev_id) = db.articles.get_current_revision_id(&article_id)? {
        let Revision {
            content,
            author,
            date,
        } = db.articles.get_revision(rev_id)?;
        let context = RevContext {
            article_id,
            rev_id,
            content,
            author,
            date,
            specific_rev: false,
        };
        Ok(Template::render("article", context))
    } else {
        Ok(render_404(&article_id))
    }
}

#[get("/<_article_id>/edit")]
pub fn edit(_db: State<Db>, _article_id: Id) -> Result<Template> {
    todo!()
}

#[get("/<article_id>/revs")]
pub fn revs(db: State<Db>, article_id: Id) -> Result<Template> {
    if let Some(revs) = db.articles.list_revisions(&article_id)? {
        #[derive(serde::Serialize)]
        struct RevsContext {
            article_id: Id,
            revs: Vec<(uuid::Uuid, RevisionMeta)>,
        }
        let context = RevsContext { article_id, revs };
        Ok(Template::render("article_revs", context))
    } else {
        Ok(render_404(&article_id))
    }
}

#[get("/<article_id>/rev/<rev_id>")]
pub fn rev(db: State<Db>, article_id: Id, rev_id: Uuid) -> Result<Template> {
    let rev_id = rev_id.into_inner();
    let Revision {
        content,
        author,
        date,
    } = db.articles.get_revision(rev_id)?;
    let context = RevContext {
        article_id,
        rev_id,
        content,
        author,
        date,
        specific_rev: true,
    };
    Ok(Template::render("article", context))
}
