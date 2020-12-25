use std::collections::HashMap;

use pulldown_cmark::{html, BrokenLink, Options, Parser};
use rocket::{get, Route, State};
use rocket_contrib::templates::Template;

use crate::{
    database::{
        articles::{Revision, RevisionMeta},
        Db, Id,
    },
    Result,
};

pub fn routes() -> Vec<Route> {
    rocket::routes![get, edit, revs, rev]
}

fn render_404(article_name: &str) -> Template {
    let context: HashMap<_, _> = std::iter::once(("article_name", article_name)).collect();
    Template::render("article_404", context)
}

fn markdown_to_html(input: &str) -> String {
    let callback = &mut |broken_link: BrokenLink| {
        Some((
            ("/".to_string() + broken_link.reference).into(),
            broken_link.reference.to_owned().into(),
        ))
    };
    let parser = Parser::new_with_broken_link_callback(input, Options::all(), Some(callback));
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

#[derive(serde::Serialize)]
struct RevContext {
    article_name: String,
    rev_id: Id,
    content: String,
    author: String,
    date: chrono::DateTime<chrono::Utc>,
    specific_rev: bool,
}

#[get("/<article_name>")]
pub fn get(db: State<Db>, article_name: String) -> Result<Template> {
    // TODO is this correct? Technically the inner option being none means we
    // got an unknown id from the database, which would be inconsistent data on
    // the server side, not a 404.
    // Alternatively: Should get_current_revision even return an Option? It could
    // also just error out with UnknownArticle or something.
    if let Some(Some((rev_id, rev))) = db
        .articles
        .id_by_name(&article_name)?
        .map(|id| db.articles.get_current_revision(id))
        .transpose()?
    {
        let Revision {
            content,
            author_id,
            date,
        } = rev;
        let author = db.get_user_name(author_id)?.unwrap_or_default();
        let context = RevContext {
            article_name,
            rev_id: rev_id.rev_id(),
            content: markdown_to_html(&content),
            author,
            date,
            specific_rev: false,
        };
        Ok(Template::render("article", context))
    } else {
        Ok(render_404(&article_name))
    }
}

#[get("/<_article_name>/edit")]
pub fn edit(_db: State<Db>, _article_name: String) -> Result<Template> {
    todo!()
}

#[get("/<article_name>/revs")]
pub fn revs(db: State<Db>, article_name: String) -> Result<Template> {
    if let Some(id) = db.articles.id_by_name(&article_name)? {
        let revs = db.articles.list_revisions(id)?;
        let mut revs_with_author = Vec::with_capacity(revs.len());
        for (id, rev) in revs.into_iter() {
            let author = db.get_user_name(rev.author_id)?.unwrap_or_default();
            revs_with_author.push((id, rev, author));
        }
        #[derive(serde::Serialize)]
        struct RevsContext {
            article_name: String,
            revs: Vec<(Id, RevisionMeta, String)>,
        }
        let context = RevsContext {
            article_name,
            revs: revs_with_author,
        };
        Ok(Template::render("article_revs", context))
    } else {
        Ok(render_404(&article_name))
    }
}

// TODO: You can manually put in a rev_id from a different article and you'll
// get that article instead of the current one, but with the wrong title. lol.
#[get("/<article_name>/rev/<rev_id>")]
pub fn rev(db: State<Db>, article_name: String, rev_id: Id) -> Result<Template> {
    if let Some(article_id) = db.articles.id_by_name(&article_name)? {
        let rev_id = (article_id, rev_id).into();
        let Revision {
            content,
            author_id,
            date,
        } = db.articles.get_revision(rev_id)?;
        let author = db.get_user_name(author_id)?.unwrap_or_default();
        let context = RevContext {
            article_name,
            rev_id: rev_id.rev_id(),
            content: markdown_to_html(&content),
            author,
            date,
            specific_rev: true,
        };
        Ok(Template::render("article", context))
    } else {
        Ok(render_404(&article_name))
    }
}
