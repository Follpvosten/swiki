use chrono::{DateTime, Utc};
use pulldown_cmark::{html, BrokenLink, Options, Parser};
use rocket::{
    form::Form,
    get,
    http::Status,
    post,
    response::{status, Redirect},
    FromForm, Route, State,
};
use rocket_dyn_templates::Template;
use serde_json::json;

use crate::{
    db::{
        self,
        articles::{DisplayRevision, RevId},
        users::{LoggedUser, UserSession},
        Db,
    },
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
    user: &Option<LoggedUser>,
) -> status::Custom<Template> {
    let context = json! {{
        "site_name": cfg.site_name,
        "default_path": cfg.default_path,
        "article_name": article_name,
        "user": user,
    }};
    status::Custom(Status::NotFound, Template::render("article_404", context))
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

/// Context used to render an existing article revision.
#[derive(serde::Serialize)]
struct RevContext<'a> {
    site_name: &'a str,
    default_path: &'a str,
    article_name: String,
    user: Option<LoggedUser>,
    rev_id: i64,
    content: String,
    author: String,
    date: DateTime<Utc>,
    specific_rev: bool,
}

#[get("/search?<q>", rank = 0)]
fn search(
    cfg: &State<Config>,
    index: &State<ArticleIndex>,
    user: Option<LoggedUser>,
    q: String,
) -> Result<Template> {
    let results = index.search_by_text(&q)?;
    let exact_match = results.iter().any(|r| r.title == q);
    let context = json! {{
        "site_name": &cfg.site_name,
        "default_path": &cfg.default_path,
        "exact_match": exact_match,
        "results": index.search_by_text(&q)?,
        "page_name": "Search",
        "user": user,
        "query": q,
    }};
    Ok(Template::render("search", context))
}

#[get("/create", rank = 0)]
fn create(cfg: &State<Config>, user: Option<LoggedUser>) -> Template {
    let context = json! {{
        "site_name": &cfg.site_name,
        "default_path": &cfg.default_path,
        "page_name": "New Article",
        "user": user,
    }};
    Template::render("article_create", context)
}

#[get("/<article_name>", rank = 3)]
async fn get(
    db: &State<Db>,
    cfg: &State<Config>,
    article_name: String,
    user: Option<LoggedUser>,
) -> Result<status::Custom<Template>> {
    if let Some(rev) = db.get_current_rev(&article_name).await? {
        let DisplayRevision {
            rev_id,
            author_name,
            content,
            created,
        } = rev;
        let date = DateTime::from_utc(created, Utc);
        let context = RevContext {
            site_name: &cfg.site_name,
            default_path: &cfg.default_path,
            author: author_name,
            article_name,
            user,
            rev_id,
            content: markdown_to_html(&content),
            date,
            specific_rev: false,
        };
        Ok(status::Custom(
            Status::Ok,
            Template::render("article", context),
        ))
    } else if article_name == cfg.main_page {
        let context = RevContext {
            site_name: &cfg.site_name,
            default_path: &cfg.default_path,
            author: String::default(),
            article_name,
            user,
            rev_id: 0,
            content: markdown_to_html(&format!(
                "Welcome to your new wiki!

There's nothing here yet.

To create your main page, go to [{}/edit].  
Have fun!",
                cfg.main_page
            )),
            date: Utc::now(),
            specific_rev: false,
        };
        Ok(status::Custom(
            Status::Ok,
            Template::render("article", context),
        ))
    } else {
        Ok(render_404(&*cfg, &article_name, &user))
    }
}

#[derive(serde::Serialize)]
struct NewRevContext<'a> {
    site_name: &'a str,
    default_path: &'a str,
    article_name: String,
    user: LoggedUser,
    old_content: String,
    new_article: bool,
    invalid_name_change: bool,
}
#[get("/<article_name>/edit")]
async fn edit_page(
    db: &State<Db>,
    cfg: &State<Config>,
    article_name: String,
    // This route will only be called when a user is logged in.
    user: LoggedUser,
) -> Result<Template> {
    // For a new article, the only difference is the content being empty string.
    let (old_content, new_article) = sqlx::query_scalar!(
        "SELECT content FROM revision r
        INNER JOIN article a ON a.id = r.article_id
        WHERE a.name = $1
        AND num = (SELECT MAX(num) FROM revision WHERE article_id = a.id)",
        article_name
    )
    .fetch_optional(&db.pool)
    .await?
    .map(|content| (content, false))
    .unwrap_or_else(|| (String::default(), true));
    let context = NewRevContext {
        site_name: &cfg.site_name,
        default_path: &cfg.default_path,
        article_name,
        user,
        old_content,
        new_article,
        invalid_name_change: false,
    };
    Ok(Template::render("article_edit", context))
}
#[derive(FromForm)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct AddRevRequest {
    pub title: Option<String>,
    pub content: String,
}
#[post("/<article_name>/edit", data = "<form>")]
async fn edit_form(
    db: &State<Db>,
    cfg: &State<Config>,
    search_index: &State<ArticleIndex>,
    article_name: String,
    form: Form<AddRevRequest>,
    session: &UserSession,
    user: LoggedUser,
) -> Result<status::Custom<Template>> {
    // Get the article's id if it already exists.
    let article_id = db.article_id_by_name(&article_name).await?;

    let AddRevRequest {
        title: new_title,
        content: new_content,
    } = form.into_inner();

    let mut txn = db.begin().await?;

    // Here we check if the "new_name" is valid and also change it in case
    // the article already exists. If it doesn't, we check if there is an
    // article with new_name as the name and also prevent that.
    let new_name = if let Some(new_name) = new_title.as_deref() {
        if new_name != article_name {
            let invalid_request = || {
                let context = NewRevContext {
                    site_name: &cfg.site_name,
                    default_path: &cfg.default_path,
                    article_name: article_name.clone(),
                    user: user.clone(),
                    old_content: new_content.clone(),
                    new_article: article_id.is_none(),
                    invalid_name_change: true,
                };
                status::Custom(
                    Status::BadRequest,
                    Template::render("article_edit", context),
                )
            };
            if let Some(article_id) = article_id {
                // Change the article's title
                let res = db::articles::change_name(&mut txn, article_id, new_name).await;
                // This will trigger the constraint if the user tries to replace an existing article.
                if let Err(Error::SqlxError(sqlx::Error::Database(err))) = &res {
                    if err.constraint() == Some("article_name_unique") {
                        return Ok(invalid_request());
                    }
                }
                res?;
                true
            } else {
                // We catch the same error as above (which would happen further down on article
                // creation) a bit earlier here.
                if db::articles::id_by_name(&mut txn, new_name)
                    .await?
                    .is_some()
                {
                    return Ok(invalid_request());
                }
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    let article_name = new_title.as_deref().unwrap_or(&article_name);
    let (RevId(article_id, rev_id), rev) = if let Some(article_id) = article_id {
        db::articles::add_revision(&mut txn, article_id, session.user_id, &new_content).await?
    } else {
        db::articles::create(&mut txn, article_name, &new_content, session.user_id).await?
    };

    txn.commit().await?;

    let context = json! {{
        "site_name": &cfg.site_name,
        "default_path": &cfg.default_path,
        "article_name": article_name,
        "user": user,
        "rev_id": rev_id,
        "new_name": new_name,
    }};

    // TODO do we really want to return on error here?
    search_index.add_or_update_article(article_id, article_name, &new_content, rev.date)?;

    Ok(status::Custom(
        Status::Ok,
        Template::render("article_edit_success", context),
    ))
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
async fn revs(
    db: &State<Db>,
    cfg: &State<Config>,
    article_name: String,
    user: Option<LoggedUser>,
) -> Result<status::Custom<Template>> {
    let revisions = db::articles::list_revisions(db, &article_name).await?;
    if revisions.is_empty() {
        return Ok(render_404(&*cfg, &article_name, &user));
    }
    let context = json! {{
        "site_name": &cfg.site_name,
        "default_path": &cfg.default_path,
        "article_name": article_name,
        "user": user,
        "revs": revisions,
    }};
    Ok(status::Custom(
        Status::Ok,
        Template::render("article_revs", context),
    ))
}

// TODO: You can manually put in a rev_id from a different article and you'll
// get that article instead of the current one, but with the wrong title. lol.
#[get("/<article_name>/rev/<rev_id>")]
async fn rev(
    db: &State<Db>,
    cfg: &State<Config>,
    article_name: String,
    rev_id: i64,
    user: Option<LoggedUser>,
) -> Result<status::Custom<Template>> {
    if let Some(rev) = db::articles::get_revision(db, &article_name, rev_id).await? {
        let DisplayRevision {
            rev_id,
            author_name,
            content,
            created,
        } = rev;
        let date = DateTime::from_utc(created, Utc);
        let context = RevContext {
            site_name: &cfg.site_name,
            default_path: &cfg.default_path,
            author: author_name,
            article_name,
            user,
            rev_id,
            content: markdown_to_html(&content),
            date,
            specific_rev: true,
        };
        Ok(status::Custom(
            Status::Ok,
            Template::render("article", context),
        ))
    } else {
        Ok(render_404(&*cfg, &article_name, &user))
    }
}
