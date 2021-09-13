use rocket::{
    form::Form,
    get,
    http::{Cookie, CookieJar},
    post,
    response::{Redirect, Responder},
    FromForm, State,
};
use rocket_dyn_templates::Template;
use serde_json::json;
use uuid::Uuid;

use crate::{
    db::{
        users::{LoggedUser, UserSession},
        EnabledRegistration,
    },
    Cache, Config, Db, Error, Result,
};

pub fn routes() -> Vec<rocket::Route> {
    rocket::routes![
        profile,
        register_page,
        register_form,
        login_redirect,
        login_page,
        login_form,
        logout,
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
    cache.register_captcha(id, &solution);
    Ok((id, base64))
}

#[derive(Debug, serde::Serialize)]
struct RegisterPageContext<'a> {
    site_name: &'a str,
    default_path: &'a str,
    page_name: &'static str,
    username: Option<String>,
    captcha_base64: String,
    captcha_uuid: String,
    pwds_dont_match: bool,
    username_taken: bool,
    no_username: bool,
    failed_captcha: bool,
}
impl<'a> Default for RegisterPageContext<'a> {
    fn default() -> Self {
        Self {
            site_name: "",
            default_path: "",
            page_name: "Register",
            username: None,
            captcha_base64: Default::default(),
            captcha_uuid: Default::default(),
            pwds_dont_match: false,
            username_taken: false,
            no_username: false,
            failed_captcha: false,
        }
    }
}
impl<'a> From<&'a Config> for RegisterPageContext<'a> {
    fn from(cfg: &'a Config) -> Self {
        Self {
            site_name: &cfg.site_name,
            default_path: &cfg.default_path,
            ..Default::default()
        }
    }
}

#[derive(Responder)]
#[allow(clippy::large_enum_variant)]
enum TemplateResult {
    Template(Template),
    #[response(status = 400)]
    Error(Template),
    Redirect(Redirect),
}

#[get("/register")]
async fn register_page(
    cfg: &State<Config>,
    cache: &State<Cache>,
    er: Option<EnabledRegistration>,
    session: Option<&UserSession>,
) -> Result<TemplateResult> {
    // If er is None, registration is disabled.
    // If session is Some, we're already logged in.
    if er.is_none() || session.is_some() {
        return Ok(TemplateResult::Redirect(Redirect::to(
            cfg.default_path.clone(),
        )));
    }
    // Generate a captcha to include in the login form
    let (id, base64) = gen_captcha_and_id(&*cache).await?;
    let context = RegisterPageContext {
        captcha_base64: base64,
        captcha_uuid: id.to_string(),
        ..From::from(&**cfg)
    };
    Ok(TemplateResult::Template(Template::render(
        "register", context,
    )))
}

#[derive(Debug, FromForm)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(crate) struct RegisterRequest {
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) pwd_confirm: String,
    pub(crate) captcha_id: Uuid,
    pub(crate) captcha_solution: String,
}

#[post("/register", data = "<form>")]
async fn register_form(
    cfg: &State<Config>,
    db: &State<Db>,
    cache: &State<Cache>,
    form: Form<RegisterRequest>,
    er: Option<EnabledRegistration>,
    session: Option<&UserSession>,
) -> Result<TemplateResult> {
    // If er is None, registration is disabled.
    // If session is Some, we're already logged in.
    if er.is_none() || session.is_some() {
        return Ok(TemplateResult::Redirect(Redirect::to(
            cfg.default_path.clone(),
        )));
    }
    let RegisterRequest {
        username,
        password,
        pwd_confirm,
        captcha_id,
        captcha_solution,
    } = form.into_inner();

    let (pwds_dont_match, username_taken, no_username, failed_captcha) = (
        password != pwd_confirm || password.is_empty(),
        username == "register" || username == "login" || db.user_name_exists(&username).await?,
        username.is_empty(),
        !cache.validate_captcha(captcha_id, &captcha_solution),
    );

    if pwds_dont_match || username_taken || no_username || failed_captcha {
        let (id, base64) = gen_captcha_and_id(&*cache).await?;
        let context = RegisterPageContext {
            username: Some(username),
            captcha_base64: base64,
            captcha_uuid: id.to_string(),
            pwds_dont_match,
            username_taken,
            no_username,
            failed_captcha,
            ..From::from(&**cfg)
        };
        return Ok(TemplateResult::Error(Template::render("register", context)));
    }
    // If we're here, registration is successful
    // Register the user
    db.register_user(&username, password).await?;
    // Return some success messag
    Ok(TemplateResult::Template(Template::render(
        "register_success",
        &**cfg,
    )))
}

#[get("/login")]
fn login_redirect(cfg: &State<Config>, _session: &UserSession) -> Redirect {
    Redirect::to(cfg.default_path.clone())
}
#[get("/login", rank = 2)]
fn login_page(cfg: &State<Config>) -> Template {
    let context = json! {{
        "site_name": &cfg.site_name,
        "default_path": &cfg.default_path,
        "page_name": "Login",
    }};
    Template::render("login", context)
}
#[derive(Debug, FromForm)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(crate) struct LoginRequest {
    pub(crate) username: String,
    pub(crate) password: String,
}
#[post("/login", data = "<form>")]
async fn login_form(
    cfg: &State<Config>,
    db: &State<Db>,
    form: Form<LoginRequest>,
    cookies: &CookieJar<'_>,
    session: Option<&UserSession>,
) -> Result<TemplateResult> {
    if session.is_some() {
        // No double logins
        return Ok(TemplateResult::Redirect(Redirect::to(
            cfg.default_path.clone(),
        )));
    }
    #[derive(serde::Serialize)]
    struct LoginPageContext<'a> {
        site_name: &'a str,
        default_path: &'a str,
        page_name: &'static str,
        username: Option<String>,
        username_unknown: bool,
        wrong_password: bool,
    }
    let LoginRequest { username, password } = form.into_inner();

    match db.try_login(&username, password).await {
        Ok(session) => {
            cookies.add(Cookie::new(
                "session_id",
                base64::encode(session.session_id.as_bytes()),
            ));
            // TODO: Somehow optimize this. Ideally we somehow return is_admin
            // from try_login, or we find out if we actually need it here lol.
            let is_admin = db.user_is_admin(session.user_id).await?;
            let context = json! {{
                "site_name": &cfg.site_name,
                "default_path": &cfg.default_path,
                "user": {
                    "name": &username,
                    "is_admin": is_admin,
                },
            }};
            Ok(TemplateResult::Template(Template::render(
                "login_success",
                context,
            )))
        }
        Err(Error::UserNotFound(_)) => {
            let context = LoginPageContext {
                site_name: &cfg.site_name,
                default_path: &cfg.default_path,
                page_name: "Login",
                username: Some(username),
                username_unknown: true,
                wrong_password: false,
            };
            Ok(TemplateResult::Error(Template::render("login", context)))
        }
        Err(Error::WrongPassword) => {
            let context = LoginPageContext {
                site_name: &cfg.site_name,
                default_path: &cfg.default_path,
                page_name: "Login",
                username: Some(username),
                username_unknown: false,
                wrong_password: true,
            };
            Ok(TemplateResult::Error(Template::render("login", context)))
        }
        Err(e) => Err(e),
    }
}

#[get("/logout")]
async fn logout(
    cfg: &State<Config>,
    db: &State<Db>,
    cookies: &CookieJar<'_>,
    session: Option<&UserSession>,
) -> Result<TemplateResult> {
    // Remove the session from the user's cookies in any case
    cookies.remove(Cookie::named("session_id"));
    if let Some(session) = session {
        // And if it's still in the database, remove it from there as well
        db.destroy_session(session.session_id).await?;
        Ok(TemplateResult::Template(Template::render(
            "logout_success",
            &**cfg,
        )))
    } else {
        // Otherwise, just redirect to main
        Ok(TemplateResult::Redirect(Redirect::to(
            cfg.default_path.clone(),
        )))
    }
}

#[get("/<_username>", rank = 4)]
fn profile(_db: &State<Db>, _username: String, _user: Option<LoggedUser>) -> Result<Template> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::generate_captcha;

    #[test]
    fn captcha_generation() {
        // Do it 5 times to be sure.
        for _ in 0..5 {
            let (solution, base64) = generate_captcha().expect("captcha generation failed");
            // Check if it's valid base64
            assert!(base64::decode(&base64).is_ok());
            // We always call add_chars(5)
            assert_eq!(solution.len(), 5);
            // And I'm pretty sure it should only do alphanumerical characters
            assert!(solution.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }
}
