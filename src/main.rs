#![recursion_limit = "512"]

use rocket::{fairing::AdHoc, response::Redirect};
use rocket_contrib::{serve::StaticFiles, templates::Template};

mod cache;
pub use cache::Cache;
mod database;
pub use database::Db;
mod search;
pub use search::ArticleIndex;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub site_name: String,
}

mod error;
pub use error::Error;
type Result<T> = std::result::Result<T, Error>;

// Route modules
mod articles;
mod settings;
mod users;

#[rocket::get("/")]
fn index() -> Redirect {
    Redirect::to("/Main")
}

fn seed_db(db: Db) -> Result<Db> {
    if db.articles.id_by_name("Main")?.is_none() {
        let author_id = match db.users.id_by_name("System")? {
            Some(id) => id,
            None => db.users.register("System", "todo lol")?,
        };
        // Create a first page if we don't have one.
        let article_id = db.articles.create("Main")?;

        db.articles.add_revision(
            article_id,
            author_id,
            r#"Welcome to your new wiki!

To edit this main page, go to [Main/edit].  
You can look at past revisions at [Main/revs].  
Have fun!"#,
        )?;
    }
    Ok(db)
}

fn default_db() -> Result<Db> {
    let sled_db = sled::open("wiki.db")?;
    Db::load_or_create(sled_db).and_then(seed_db)
}

fn rocket(db: Db) -> Result<rocket::Rocket> {
    Ok(rocket::ignite()
        .mount("/", rocket::routes![index])
        .mount("/", articles::routes())
        .mount("/u", users::routes())
        .mount("/settings", settings::routes())
        .mount("/res", StaticFiles::from("static"))
        .manage(ArticleIndex::new(&db)?)
        .manage(Cache::new()?)
        .manage(db)
        .attach(Template::fairing())
        .attach(AdHoc::config::<Config>()))
}

#[rocket::main]
async fn main() -> Result<()> {
    loop {
        let db = default_db()?;
        if let Err(e) = rocket(db)?.launch().await {
            println!("Rocket crashed: {:?}", e);
            continue;
        }
        break;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rocket::{
        http::{ContentType, Status},
        local::blocking::Client,
    };
    use rocket_contrib::uuid::Uuid;
    use scraper::Selector;

    use super::{rocket, seed_db, Result};
    use crate::{
        users::{LoginRequest, RegisterRequest},
        Cache, Db,
    };

    const USERNAME: &str = "User";
    const PASSWORD: &str = "Password123";

    fn db() -> Result<Db> {
        let sled_db = sled::Config::default().temporary(true).open()?;
        Db::load_or_create(sled_db).and_then(seed_db)
    }
    fn client() -> Result<Client> {
        let db = db()?;
        Ok(Client::tracked(rocket(db)?).expect("failed to create rocket client"))
    }

    fn register_and_login(client: &Client) {
        // First, register an account. This is the hard part.
        let register_challenge_response = client.get("/u/register").dispatch();
        let body = register_challenge_response.into_string().unwrap();
        let document = scraper::Html::parse_document(&body);
        let selector = Selector::parse("input[name='captcha_id']").unwrap();
        let input = document.select(&selector).next().unwrap();
        let value = input.value().attr("value").unwrap();
        let captcha_id: Uuid = value.parse().unwrap();
        let captcha_solution = client
            .rocket()
            .state::<Cache>()
            .unwrap()
            .get_solution(captcha_id.into_inner())
            .unwrap();
        let request = RegisterRequest {
            username: USERNAME.into(),
            password: PASSWORD.into(),
            pwd_confirm: PASSWORD.into(),
            captcha_id,
            captcha_solution,
        };
        let request_body = serde_urlencoded::to_string(request).unwrap();
        let response = client
            .post("/u/register")
            .header(ContentType::new("application", "x-www-form-urlencoded"))
            .body(request_body)
            .dispatch();
        assert_eq!(response.status(), Status::Ok);

        // Now, we log in, which should give us the appropriate cookies
        let request = LoginRequest {
            username: USERNAME.into(),
            password: PASSWORD.into(),
        };
        let request_body = serde_urlencoded::to_string(request).unwrap();
        let response = client
            .post("/u/login")
            .header(ContentType::new("application", "x-www-form-urlencoded"))
            .body(request_body)
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
    }
    fn logout(client: &Client) {
        let response = client.get("/u/logout").dispatch();
        assert_eq!(response.status(), Status::Ok);
    }

    #[test]
    fn launch() {
        assert!(client().is_ok());
    }

    #[test]
    fn redirects() {
        let client = client().unwrap();
        let assert_redirect = |uri: &str, location| {
            let response = client.get(uri).dispatch();
            assert_eq!(response.status(), Status::SeeOther);
            assert_eq!(response.headers().get_one("Location"), Some(location));
        };
        let assert_no_redirect = |uri: &str| {
            let response = client.get(uri).dispatch();
            assert_ne!(response.status(), Status::SeeOther);
        };
        // Always redirect / to main
        assert_redirect("/", "/Main");
        // When not logged in, don't allow any edits
        assert_redirect("/Main/edit", "/u/login");
        // Also trying to "log out" while not logged in should redirect
        assert_redirect("/u/logout", "/Main");
        // while the login/register routes should not redirect
        assert_no_redirect("/u/login");
        assert_no_redirect("/u/register");
        // Login first to check the u/login and u/register redirects
        register_and_login(&client);
        assert_redirect("/u/login", "/Main");
        assert_redirect("/u/register", "/Main");
        // Editing an article should be possible now
        assert_no_redirect("/Main/edit");
        // Always redirect / to main
        assert_redirect("/", "/Main");
        // Finally, logout should not redirect now, but that only works once lol
        assert_no_redirect("/u/logout");
    }

    #[test]
    fn register_login_logout() {
        let client = client().unwrap();
        // There should be no cookies before logging in
        assert_eq!(client.cookies().iter().count(), 0);
        // There's one cookie, the session id, when you're logged in
        register_and_login(&client);
        assert_eq!(client.cookies().iter().count(), 1);
        assert!(client.cookies().get("session_id").is_some());
        // After logging out, no more cookies should be present
        logout(&client);
        assert_eq!(client.cookies().iter().count(), 0);
    }

    #[test]
    fn basic_article_routes() {
        let client = client().unwrap();
        let assert_status = |uri: &str, status: Status| {
            let response = client.get(uri).dispatch();
            assert_eq!(response.status(), status, "{}", uri);
        };
        // At the start, we only know one article that exists
        let ok = Status::Ok;
        let notfound = Status::NotFound;
        assert_status("/Main", ok);
        assert_status("/Main/revs", ok);
        // There's only a rev 0 as well
        assert_status("/Main/rev/0", ok);
        // Search should always succeed
        assert_status("/search?q=blah", ok);
        // Same for the "create article" helper
        assert_status("/create", ok);
        // An unknown article should return 404
        assert_status("/Blahblub", notfound);
        // Same for unknown revs
        assert_status("/Main/revs/1", notfound);
        // And a combination of those
        assert_status("/Blahblub/revs/1", notfound);
        // Login so we can see the edit page
        register_and_login(&client);
        assert_status("/Main/edit", ok);
    }
}
