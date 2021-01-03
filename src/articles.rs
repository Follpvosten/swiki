use pulldown_cmark::{html, BrokenLink, Options, Parser};
use rocket::{get, post, request::Form, response::Redirect, FromForm, Route, State};
use rocket_contrib::templates::Template;

use crate::{
    database::{
        articles::{Revision, RevisionMeta},
        Db, Id, UserSession,
    },
    Config, Error, Result,
};

pub fn routes() -> Vec<Route> {
    rocket::routes![
        get,
        edit_page,
        edit_form,
        redirect_to_login_get,
        redirect_to_login_post,
        revs,
        rev
    ]
}

fn render_404(cfg: &Config, article_name: &str) -> Result<Template> {
    use rocket_contrib::templates::tera::Context;
    let mut context = Context::from_serialize(cfg)?;
    context.insert("article_name", article_name);
    Ok(Template::render("article_404", context.into_json()))
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
struct RevContext<'a> {
    site_name: &'a str,
    article_name: String,
    rev_id: Id,
    content: String,
    author: String,
    date: chrono::DateTime<chrono::Utc>,
    specific_rev: bool,
}

#[get("/<article_name>")]
fn get(db: State<Db>, cfg: State<Config>, article_name: String) -> Result<Template> {
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
        let context = RevContext {
            site_name: &cfg.site_name,
            author: db.get_user_name(author_id)?.unwrap_or_default(),
            article_name,
            rev_id: rev_id.rev_id(),
            content: markdown_to_html(&content),
            date,
            specific_rev: false,
        };
        Ok(Template::render("article", context))
    } else {
        render_404(&*cfg, &article_name)
    }
}

#[get("/<article_name>/edit")]
fn edit_page(
    db: State<Db>,
    cfg: State<Config>,
    article_name: String,
    // This route will only be called when a user is logged in.
    _session: UserSession,
) -> Result<Template> {
    #[derive(serde::Serialize)]
    struct NewRevContext<'a> {
        site_name: &'a str,
        article_name: String,
        old_content: String,
    }
    // For a new article, the only difference is the content being empty string.
    let old_content = match db.articles.id_by_name(&article_name)? {
        Some(id) => db
            .articles
            .get_current_content(id)?
            .ok_or(Error::ArticleDataInconsistent(id))?,
        // New article
        None => String::default(),
    };
    let context = NewRevContext {
        site_name: &cfg.site_name,
        article_name,
        old_content,
    };
    Ok(Template::render("article_edit", context))
}
#[derive(FromForm)]
struct AddRevRequest {
    content: String,
}
#[post("/<article_name>/edit", data = "<form>")]
fn edit_form(
    db: State<Db>,
    cfg: State<Config>,
    article_name: String,
    form: Form<AddRevRequest>,
    session: UserSession,
) -> Result<Template> {
    // If it's an existing article, use its id; otherwise, create a new article.
    let article_id = match db.articles.id_by_name(&article_name)? {
        Some(id) => id,
        None => db.articles.create(&article_name)?,
    };
    let new_content = form.into_inner().content;

    let res = db
        .articles
        .add_revision(article_id, session.user_id, &new_content);

    #[derive(serde::Serialize)]
    struct EditSuccessContext<'a> {
        site_name: &'a str,
        article_name: String,
        rev_id: Option<Id>,
    }
    let mut context = EditSuccessContext {
        site_name: &cfg.site_name,
        article_name,
        rev_id: None,
    };

    if matches!(res, Err(Error::IdenticalNewRevision)) {
        // This is the case where we early return a success. Huh.
        // This is because we technically succeeded since the article
        // looks like the user wants it to.
        Ok(Template::render("article_edit_success", context))
    } else {
        context.rev_id = Some(res?.0.rev_id());
        Ok(Template::render("article_edit_success", context))
    }
}

#[get("/<_article_name>/edit", rank = 2)]
fn redirect_to_login_get(_article_name: String) -> Redirect {
    Redirect::to("/u/login")
}
#[post("/<_article_name>/edit", rank = 2)]
fn redirect_to_login_post(_article_name: String) -> Redirect {
    Redirect::to("/u/login")
}

#[get("/<article_name>/revs")]
fn revs(db: State<Db>, cfg: State<Config>, article_name: String) -> Result<Template> {
    if let Some(id) = db.articles.id_by_name(&article_name)? {
        let revs = db.articles.list_revisions(id)?;
        let mut revs_with_author = Vec::with_capacity(revs.len());
        for (id, rev) in revs.into_iter() {
            let author = db.get_user_name(rev.author_id)?.unwrap_or_default();
            revs_with_author.push((id, rev, author));
        }
        #[derive(serde::Serialize)]
        struct RevsContext<'a> {
            site_name: &'a str,
            article_name: String,
            revs: Vec<(Id, RevisionMeta, String)>,
        }
        let context = RevsContext {
            site_name: &cfg.site_name,
            article_name,
            revs: revs_with_author,
        };
        Ok(Template::render("article_revs", context))
    } else {
        render_404(&*cfg, &article_name)
    }
}

// TODO: You can manually put in a rev_id from a different article and you'll
// get that article instead of the current one, but with the wrong title. lol.
#[get("/<article_name>/rev/<rev_id>")]
fn rev(db: State<Db>, cfg: State<Config>, article_name: String, rev_id: Id) -> Result<Template> {
    if let Some(article_id) = db.articles.id_by_name(&article_name)? {
        let rev_id = (article_id, rev_id).into();
        let Revision {
            content,
            author_id,
            date,
        } = db.articles.get_revision(rev_id)?;
        let context = RevContext {
            site_name: &cfg.site_name,
            author: db.get_user_name(author_id)?.unwrap_or_default(),
            article_name,
            rev_id: rev_id.rev_id(),
            content: markdown_to_html(&content),
            date,
            specific_rev: true,
        };
        Ok(Template::render("article", context))
    } else {
        render_404(&*cfg, &article_name)
    }
}
