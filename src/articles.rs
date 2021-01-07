use chrono::Utc;
use pulldown_cmark::{html, BrokenLink, Options, Parser};
use rocket::{get, post, request::Form, response::Redirect, FromForm, Route, State};
use rocket_contrib::templates::{tera::Context, Template};

use crate::{
    database::{
        articles::{Revision, RevisionMeta},
        users::{LoggedUserName, UserSession},
        Db, Id,
    },
    search::SearchResult,
    ArticleIndex, Config, Error, Result,
};

pub fn routes() -> Vec<Route> {
    rocket::routes![
        search,
        create,
        get,
        edit_page,
        edit_form,
        redirect_to_login_get,
        redirect_to_login_post,
        revs,
        rev
    ]
}

fn render_404(
    cfg: &Config,
    article_name: &str,
    user_name: &Option<LoggedUserName>,
) -> Result<Template> {
    let mut context = Context::from_serialize(cfg)?;
    context.insert("article_name", article_name);
    if let Some(user_name) = user_name.as_ref() {
        context.insert("user_name", user_name.0.as_str());
    }
    Ok(Template::render("article_404", context.into_json()))
}

fn markdown_to_html(input: &str) -> String {
    let callback = &mut |broken_link: BrokenLink| {
        Some((
            ("/".to_string() + broken_link.reference).into(),
            broken_link.reference.to_owned().into(),
        ))
    };
    let parser =
        Parser::new_with_broken_link_callback(input, Options::all(), Some(callback)).map(|ev| {
            match ev {
                pulldown_cmark::Event::SoftBreak => pulldown_cmark::Event::HardBreak,
                _ => ev,
            }
        });
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

#[derive(serde::Serialize)]
struct RevContext<'a> {
    site_name: &'a str,
    article_name: String,
    user_name: Option<LoggedUserName>,
    rev_id: Id,
    content: String,
    author: String,
    date: chrono::DateTime<chrono::Utc>,
    specific_rev: bool,
    main_page: bool,
}

#[get("/search?<q>", rank = -20)]
fn search(
    db: State<Db>,
    cfg: State<Config>,
    index: State<ArticleIndex>,
    user_name: Option<LoggedUserName>,
    q: String,
) -> Result<Template> {
    #[derive(serde::Serialize)]
    struct SearchResultsContext<'a> {
        site_name: &'a str,
        user_name: Option<LoggedUserName>,
        results: Vec<SearchResult>,
        query: String,
        exact_match: bool,
    }
    let results = index.search_by_text(&q)?;
    let exact_match = db.articles.name_exists(&q)?;
    let context = SearchResultsContext {
        site_name: &cfg.site_name,
        user_name,
        results,
        query: q,
        exact_match,
    };
    Ok(Template::render("search", context))
}

#[get("/create", rank = -20)]
fn create(cfg: State<Config>, user_name: Option<LoggedUserName>) -> Template {
    let mut context = Context::new();
    context.insert("site_name", &cfg.site_name);
    if let Some(user_name) = user_name {
        context.insert("user_name", &user_name.0);
    }
    Template::render("article_create", context.into_json())
}

#[get("/<article_name>")]
fn get(
    db: State<Db>,
    cfg: State<Config>,
    article_name: String,
    user_name: Option<LoggedUserName>,
) -> Result<Template> {
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
        // We could probably skip the "query database for id" step as well
        // because the Main page is always Id(0).
        // That's probably not worth it right now tho.
        let context = RevContext {
            site_name: &cfg.site_name,
            author: db.users.name_by_id(author_id)?.unwrap_or_default(),
            main_page: article_name == "Main",
            article_name,
            user_name,
            rev_id: rev_id.rev_id(),
            content: markdown_to_html(&content),
            date,
            specific_rev: false,
        };
        Ok(Template::render("article", context))
    } else {
        render_404(&*cfg, &article_name, &user_name)
    }
}

#[derive(serde::Serialize)]
struct NewRevContext<'a> {
    site_name: &'a str,
    article_name: String,
    user_name: LoggedUserName,
    old_content: String,
    main_page: bool,
    new_article: bool,
    invalid_name_change: bool,
}
#[get("/<article_name>/edit")]
fn edit_page(
    db: State<Db>,
    cfg: State<Config>,
    article_name: String,
    // This route will only be called when a user is logged in.
    user_name: LoggedUserName,
) -> Result<Template> {
    // For a new article, the only difference is the content being empty string.
    let (old_content, new_article) = match db.articles.id_by_name(&article_name)? {
        Some(id) => (
            db.articles
                .get_current_content(id)?
                .ok_or(Error::ArticleDataInconsistent(id))?,
            false,
        ),
        // New article
        None => (String::default(), true),
    };
    let context = NewRevContext {
        site_name: &cfg.site_name,
        main_page: article_name == "Main",
        article_name,
        user_name,
        old_content,
        new_article,
        invalid_name_change: false,
    };
    Ok(Template::render("article_edit", context))
}
#[derive(FromForm)]
struct AddRevRequest {
    title: Option<String>,
    content: String,
}
#[post("/<article_name>/edit", data = "<form>")]
async fn edit_form(
    db: State<'_, Db>,
    cfg: State<'_, Config>,
    search_index: State<'_, ArticleIndex>,
    article_name: String,
    form: Form<AddRevRequest>,
    session: &UserSession,
    user_name: LoggedUserName,
) -> Result<Template> {
    // If it's an existing article, use its id; otherwise, create a new article.
    let (article_id, new_article) = match db.articles.id_by_name(&article_name)? {
        Some(id) => (id, false),
        None => (db.articles.create(&article_name)?, true),
    };
    let AddRevRequest {
        title: new_title,
        content: new_content,
    } = form.into_inner();

    let new_name = if let Some(new_name) = new_title.as_deref() {
        if new_article || article_name == "Main" {
            // This is not allowed. Re-render editing page.
            let context = NewRevContext {
                site_name: &cfg.site_name,
                main_page: article_name == "Main",
                article_name,
                user_name,
                old_content: new_content,
                new_article,
                invalid_name_change: true,
            };
            return Ok(Template::render("article_edit", context));
        } else if new_name != article_name {
            // Change the article's title
            // This would error if we tried to call it with the same name
            db.articles.change_name(article_id, new_name)?;
            true
        } else {
            false
        }
    } else {
        false
    };

    let res = db
        .articles
        .add_revision(article_id, session.user_id, &new_content);

    #[derive(serde::Serialize)]
    struct EditSuccessContext<'a> {
        site_name: &'a str,
        article_name: String,
        user_name: LoggedUserName,
        rev_id: Option<Id>,
        main_page: bool,
        new_name: bool,
    }
    let mut context = EditSuccessContext {
        site_name: &cfg.site_name,
        main_page: article_name == "Main",
        article_name: new_title.unwrap_or_else(|| article_name.clone()),
        user_name,
        rev_id: None,
        new_name,
    };

    if matches!(res, Err(Error::IdenticalNewRevision)) {
        // This is the case where we early return a success. Huh.
        // This is because we technically succeeded since the article
        // looks like the user wants it to.

        if new_name {
            // We still need to update the article's name with the content.
            // TODO do we really want to return on error here?
            search_index.add_or_update_article(
                article_id,
                &context.article_name,
                &new_content,
                Utc::now(),
            )?;
        }
        Ok(Template::render("article_edit_success", context))
    } else {
        let (rev_id, rev) = res?;
        context.rev_id = Some(rev_id.rev_id());
        db.flush().await?;
        // Update the article's content and possibly its name, we don't care here.
        // TODO do we really want to return on error here?
        search_index.add_or_update_article(article_id, &article_name, &new_content, rev.date)?;
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
fn revs(
    db: State<Db>,
    cfg: State<Config>,
    article_name: String,
    user_name: Option<LoggedUserName>,
) -> Result<Template> {
    if let Some(id) = db.articles.id_by_name(&article_name)? {
        let revs = db.articles.list_revisions(id)?;
        let mut revs_with_author = Vec::with_capacity(revs.len());
        for (id, rev) in revs.into_iter() {
            let author = db.users.name_by_id(rev.author_id)?.unwrap_or_default();
            revs_with_author.push((id, rev, author));
        }
        #[derive(serde::Serialize)]
        struct RevsContext<'a> {
            site_name: &'a str,
            article_name: String,
            user_name: Option<LoggedUserName>,
            revs: Vec<(Id, RevisionMeta, String)>,
            main_page: bool,
        }
        let context = RevsContext {
            site_name: &cfg.site_name,
            main_page: article_name == "Main",
            article_name,
            user_name,
            revs: revs_with_author,
        };
        Ok(Template::render("article_revs", context))
    } else {
        render_404(&*cfg, &article_name, &user_name)
    }
}

// TODO: You can manually put in a rev_id from a different article and you'll
// get that article instead of the current one, but with the wrong title. lol.
#[get("/<article_name>/rev/<rev_id>")]
fn rev(
    db: State<Db>,
    cfg: State<Config>,
    article_name: String,
    rev_id: Id,
    user_name: Option<LoggedUserName>,
) -> Result<Template> {
    if let Some(article_id) = db.articles.id_by_name(&article_name)? {
        let rev_id = (article_id, rev_id).into();
        let Revision {
            content,
            author_id,
            date,
        } = db.articles.get_revision(rev_id)?;
        let context = RevContext {
            site_name: &cfg.site_name,
            author: db.users.name_by_id(author_id)?.unwrap_or_default(),
            main_page: article_name == "Main",
            article_name,
            user_name,
            rev_id: rev_id.rev_id(),
            content: markdown_to_html(&content),
            date,
            specific_rev: true,
        };
        Ok(Template::render("article", context))
    } else {
        render_404(&*cfg, &article_name, &user_name)
    }
}
