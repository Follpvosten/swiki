use rocket::{
    get,
    http::{Cookie, CookieJar, Status},
    post,
    request::Form,
    response::{status, Redirect},
    FromForm, State,
};
use rocket_contrib::{templates::Template, uuid::Uuid as RocketUuid};
use serde_json::json;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    database::{
        users::{LoggedUser, UserSession},
        EnabledRegistration,
    },
    Cache, Config, Db, Error, Result,
};

pub fn routes() -> Vec<rocket::Route> {
    rocket::routes![
        profile,
        register_redirect,
        register_page,
        register_redirect_always,
        register_post_redirect,
        register_form,
        register_post_redirect_always,
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
    cache.register_captcha(id, &solution);
    Ok((id, base64))
}

#[derive(Debug, serde::Serialize)]
struct RegisterPageContext<'a> {
    site_name: &'a str,
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
#[get("/register")]
fn register_redirect(_session: &UserSession) -> Redirect {
    Redirect::to("/Main")
}
#[get("/register", rank = 2)]
async fn register_page(
    cfg: State<'_, Config>,
    cache: State<'_, Cache>,
    _er: EnabledRegistration,
) -> Result<Template> {
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
// Redirect in all other cases (= registration disabled)
#[get("/register", rank = 3)]
fn register_redirect_always() -> Redirect {
    Redirect::to("/Main")
}

#[cfg(test)]
fn serialize_uuid<S: serde::Serializer>(
    value: &RocketUuid,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&value.to_string())
}
#[derive(Debug, FromForm)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(crate) struct RegisterRequest {
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) pwd_confirm: String,
    #[cfg_attr(test, serde(serialize_with = "serialize_uuid"))]
    pub(crate) captcha_id: RocketUuid,
    pub(crate) captcha_solution: String,
}
#[post("/register")]
fn register_post_redirect(_session: &UserSession) -> Redirect {
    Redirect::to("/Main")
}
#[post("/register", data = "<form>", rank = 2)]
async fn register_form(
    cfg: State<'_, Config>,
    db: State<'_, Db>,
    cache: State<'_, Cache>,
    form: Form<RegisterRequest>,
    _er: EnabledRegistration,
) -> Result<status::Custom<Template>> {
    let RegisterRequest {
        username,
        mut password,
        pwd_confirm,
        captcha_id,
        captcha_solution,
    } = form.into_inner();
    let captcha_id = captcha_id.into_inner();

    let (pwds_dont_match, username_taken, no_username, failed_captcha) = (
        password != pwd_confirm || password.is_empty(),
        db.users.name_exists(&username)? || username == "register" || username == "login",
        username.is_empty(),
        !cache.validate_captcha(captcha_id, &captcha_solution),
    );

    if pwds_dont_match || username_taken || no_username || failed_captcha {
        let (id, base64) = gen_captcha_and_id(&*cache).await?;
        let context = RegisterPageContext {
            site_name: &cfg.site_name,
            username: Some(username),
            captcha_base64: base64,
            captcha_uuid: id.to_string(),
            pwds_dont_match,
            username_taken,
            no_username,
            failed_captcha,
            ..Default::default()
        };
        return Ok(status::Custom(
            Status::BadRequest,
            Template::render("register", context),
        ));
    }
    // If we're here, registration is successful
    // Register the user
    db.users.register(&username, &password)?;
    // Remove the password from RAM
    password.zeroize();
    // Make sure everything is stored on disk
    db.flush().await?;
    // Return some success messag
    Ok(status::Custom(
        Status::Ok,
        Template::render("register_success", &*cfg),
    ))
}
#[post("/register", rank = 3)]
fn register_post_redirect_always() -> Redirect {
    Redirect::to("/Main")
}

#[get("/login")]
fn login_redirect(_session: &UserSession) -> Redirect {
    Redirect::to("/Main")
}
#[get("/login", rank = 2)]
fn login_page(cfg: State<Config>) -> Template {
    let context = json! {{
        "site_name": &cfg.site_name,
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
    cfg: State<'_, Config>,
    db: State<'_, Db>,
    form: Form<LoginRequest>,
    cookies: &CookieJar<'_>,
) -> Result<Template> {
    #[derive(serde::Serialize)]
    struct LoginPageContext<'a> {
        site_name: &'a str,
        page_name: &'static str,
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
            page_name: "Login",
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
        let is_admin = db.users.is_admin(user_id)?;
        let context = json! {{
            "site_name": &cfg.site_name,
            "user": {
                "name": &username,
                "is_admin": is_admin,
            },
        }};
        Ok(Template::render("login_success", context))
    } else {
        password.zeroize();
        let context = LoginPageContext {
            site_name: &cfg.site_name,
            page_name: "Login",
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

#[get("/<_username>", rank = 4)]
fn profile(_db: State<Db>, _username: String, _user: Option<LoggedUser>) -> Result<Template> {
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
