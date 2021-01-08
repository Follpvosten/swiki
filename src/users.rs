use rocket::{
    get,
    http::{Cookie, CookieJar},
    post,
    request::Form,
    response::Redirect,
    FromForm, State,
};
use rocket_contrib::{
    templates::{tera::Context, Template},
    uuid::Uuid as RocketUuid,
};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    database::users::{LoggedUserName, UserSession},
    Cache, Config, Db, Error, Result,
};

pub fn routes() -> Vec<rocket::Route> {
    rocket::routes![
        profile,
        register_redirect,
        register_page,
        register_form,
        login_redirect,
        login_page,
        login_form,
        logout,
        logout_redirect,
    ]
}

/// Generate a captcha.
/// Returns the captcha as base64 and the characters it contains.
fn generate_captcha() -> Result<(String, String)> {
    use captcha::{
        filters::{Dots, Noise, Wave},
        Captcha,
    };
    use rand::Rng;

    let mut captcha = Captcha::new();
    let mut rng = rand::thread_rng();
    captcha
        .add_chars(5)
        .apply_filter(Noise::new(0.4))
        .apply_filter(Wave::new(rng.gen_range(1.0..3.0), rng.gen_range(10.0..30.0)).horizontal())
        .apply_filter(Wave::new(rng.gen_range(1.0..3.0), rng.gen_range(10.0..30.0)).vertical())
        .view(220, 120)
        .apply_filter(Dots::new(rng.gen_range(3..6)));
    let result = (
        captcha.chars_as_string(),
        captcha.as_base64().ok_or(Error::CaptchaPngError)?,
    );
    Ok(result)
}

/// Generates a captcha on tokio's threadpool and stores it in the cache database.
async fn gen_captcha_and_id(cache: &Cache) -> Result<(Uuid, String)> {
    let (solution, base64) = rocket::tokio::task::spawn_blocking(generate_captcha).await??;
    let id = Uuid::new_v4();
    cache.register_captcha(id, &solution)?;
    Ok((id, base64))
}

#[derive(serde::Serialize)]
struct RegisterPageContext<'a> {
    site_name: &'a str,
    username: Option<String>,
    captcha_base64: String,
    captcha_uuid: String,
    pwds_dont_match: bool,
    username_taken: bool,
    failed_captcha: bool,
}
impl<'a> Default for RegisterPageContext<'a> {
    fn default() -> Self {
        Self {
            site_name: "",
            username: None,
            captcha_base64: Default::default(),
            captcha_uuid: Default::default(),
            pwds_dont_match: false,
            username_taken: false,
            failed_captcha: false,
        }
    }
}
#[get("/register")]
fn register_redirect(_session: &UserSession) -> Redirect {
    Redirect::to("/Main")
}
#[get("/register", rank = 2)]
async fn register_page(cfg: State<'_, Config>, cache: State<'_, Cache>) -> Result<Template> {
    // TODO handle already logged in state
    // Generate a captcha to include in the login form
    let (id, base64) = gen_captcha_and_id(&*cache).await?;
    let context = RegisterPageContext {
        site_name: &cfg.site_name,
        captcha_base64: base64,
        captcha_uuid: id.to_string(),
        ..Default::default()
    };
    Ok(Template::render("register", context))
}
#[derive(FromForm)]
struct RegisterRequest {
    username: String,
    password: String,
    pwd_confirm: String,
    captcha_id: RocketUuid,
    captcha_solution: String,
}
#[post("/register", data = "<form>")]
async fn register_form(
    cfg: State<'_, Config>,
    db: State<'_, Db>,
    cache: State<'_, Cache>,
    form: Form<RegisterRequest>,
) -> Result<Template> {
    let RegisterRequest {
        username,
        mut password,
        pwd_confirm,
        captcha_id,
        captcha_solution,
    } = form.into_inner();
    let captcha_id = captcha_id.into_inner();

    let (pwds_dont_match, username_taken, failed_captcha) = (
        password != pwd_confirm || password.is_empty(),
        db.users.name_exists(&username)? || username == "register" || username == "login",
        !cache.validate_captcha(captcha_id, &captcha_solution)?,
    );

    // Remove/invalidate the used captcha in *any* case
    cache.remove_captcha(captcha_id)?;

    if pwds_dont_match || username_taken || failed_captcha {
        let (id, base64) = gen_captcha_and_id(&*cache).await?;
        let context = RegisterPageContext {
            site_name: &cfg.site_name,
            username: Some(username),
            captcha_base64: base64,
            captcha_uuid: id.to_string(),
            pwds_dont_match,
            username_taken,
            failed_captcha,
        };
        return Ok(Template::render("register", context));
    }
    // If we're here, registration is successful
    // Register the user
    db.users.register(&username, &password)?;
    // Remove the password from RAM
    password.zeroize();
    // Make sure everything is stored on disk
    db.flush().await?;
    // Return some success messag
    Ok(Template::render("register_success", &*cfg))
}

#[get("/login")]
fn login_redirect(_session: &UserSession) -> Redirect {
    Redirect::to("/Main")
}
#[get("/login", rank = 2)]
fn login_page(cfg: State<Config>) -> Template {
    // TODO handle already logged in state
    Template::render("login", &*cfg)
}
#[derive(FromForm)]
struct LoginRequest {
    username: String,
    password: String,
}
#[post("/login", data = "<form>")]
async fn login_form(
    cfg: State<'_, Config>,
    db: State<'_, Db>,
    form: Form<LoginRequest>,
    cookies: &CookieJar<'_>,
) -> Result<Template> {
    #[derive(serde::Serialize)]
    struct LoginPageContext<'a> {
        site_name: &'a str,
        username: Option<String>,
        username_unknown: bool,
        wrong_password: bool,
    }
    let LoginRequest {
        username,
        mut password,
    } = form.into_inner();

    let user_id = if let Some(id) = db.users.id_by_name(&username)? {
        id
    } else {
        password.zeroize();
        let context = LoginPageContext {
            site_name: &cfg.site_name,
            username: Some(username),
            username_unknown: true,
            wrong_password: false,
        };
        return Ok(Template::render("login", context));
    };

    if let Some(session) = db.users.try_login(user_id, &password)? {
        password.zeroize();
        // Everything went well?? Wuuuuut
        // try_login creates a session, we'll want to save that
        db.flush().await?;
        // And save it on the client-side in the user's cookies
        cookies.add(Cookie::new(
            "session_id",
            base64::encode(session.session_id.as_bytes()),
        ));
        // TODO: Do we also auto-login on registrations?
        let mut context = Context::new();
        context.insert("site_name", &cfg.site_name);
        // Just realized that this is a hack: A field "username" in the
        // context is only used by the "login" template, while user_name
        // would cause the top bar to wrongly show a logged-in user.
        context.insert("user_name", &username);
        Ok(Template::render("login_success", context.into_json()))
    } else {
        password.zeroize();
        let context = LoginPageContext {
            site_name: &cfg.site_name,
            username: Some(username),
            username_unknown: false,
            wrong_password: true,
        };
        Ok(Template::render("login", context))
    }
}

#[get("/logout")]
async fn logout(
    cfg: State<'_, Config>,
    db: State<'_, Db>,
    cookies: &CookieJar<'_>,
    session: &UserSession,
) -> Result<Template> {
    // Remove the session, both from the db and from the client's cookies
    cookies.remove(Cookie::named("session_id"));
    db.users.destroy_session(session.session_id)?;
    db.flush().await?;
    Ok(Template::render("logout_success", &*cfg))
}

#[get("/logout", rank = 2)]
fn logout_redirect(cookies: &CookieJar<'_>) -> Redirect {
    // Just make sure there's no session without caring about anything else
    cookies.remove(Cookie::named("session_id"));
    Redirect::to("/Main")
}

#[get("/<_username>", rank = 3)]
fn profile(
    _db: State<Db>,
    _username: String,
    _user_name: Option<LoggedUserName>,
) -> Result<Template> {
    todo!()
}
