use std::collections::HashMap;

use rocket::{get, State};
use rocket_contrib::templates::Template;

use crate::database::{articles::Revision, Db, Id};

#[get("/<article_id>")]
pub fn get(db: State<Db>, article_id: Id) -> crate::Result<Template> {
    if let Some(id) = db.articles.get_current_revision_id(&article_id)? {
        let Revision {
            content,
            author,
            date,
        } = db.articles.get_revision(id)?;
        #[derive(serde::Serialize)]
        struct RevContext {
            pub article_id: Id,
            pub content: String,
            pub author: Id,
            pub date: chrono::DateTime<chrono::Utc>,
        }
        let context = RevContext {
            article_id,
            content,
            author,
            date,
        };
        Ok(Template::render("article", context))
    } else {
        let context: HashMap<_, _> = std::iter::once(("article_id", article_id)).collect();
        Ok(Template::render("article_404", context))
    }
}

#[get("/<_article_id>/edit")]
pub fn edit(_db: State<Db>, _article_id: Id) -> crate::Result<Template> {
    todo!()
}
