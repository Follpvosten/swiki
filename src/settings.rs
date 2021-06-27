use rocket::{form::Form, get, post, response::Redirect, FromForm, State};
use rocket_dyn_templates::Template;
use serde_json::json;

use crate::{
    db::users::{LoggedAdmin, LoggedUser},
    Config, Db, Result,
};

pub fn routes() -> Vec<rocket::Route> {
    rocket::routes![panel_page, panel_redirect, admin_settings, admin_redirect]
}

#[get("/")]
async fn panel_page(db: &State<Db>, cfg: &State<Config>, user: LoggedUser) -> Result<Template> {
    let mut context = json! {{
        "site_name": &cfg.site_name,
        "default_path": &cfg.default_path,
        "user": user,
    }};
    if user.is_admin() {
        let registration_enabled = db.registration_enabled().await?;
        context.as_object_mut().unwrap().extend(vec![(
            "registration_enabled".into(),
            registration_enabled.into(),
        )]);
    }
    Ok(Template::render("settings_panel", dbg!(context)))
}

#[get("/", rank = 2)]
fn panel_redirect() -> Redirect {
    Redirect::to("/u/login")
}

#[derive(FromForm)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct AdminSettings {
    pub registration_enabled: bool,
}

#[post("/admin", data = "<form>")]
async fn admin_settings(
    db: &State<Db>,
    cfg: &State<Config>,
    form: Form<AdminSettings>,
    // Only admins can call this
    // TODO: Mark down the admin's userid somewhere
    admin: LoggedAdmin,
) -> Result<Template> {
    let AdminSettings {
        registration_enabled,
    } = form.into_inner();
    if db.registration_enabled().await? != registration_enabled {
        db.set_registration_enabled(registration_enabled).await?;
        let context = json! {{
            "site_name": &cfg.site_name,
            "default_path": &cfg.default_path,
            "user": admin,
            "changed": true,
        }};
        Ok(Template::render("settings_success", context))
    } else {
        let context = json! {{
            "site_name": &cfg.site_name,
            "default_path": &cfg.default_path,
            "user": admin,
            "changed": false,
        }};
        Ok(Template::render("settings_success", context))
    }
}

#[post("/admin", rank = 2)]
fn admin_redirect() -> Redirect {
    Redirect::to("/settings")
}
