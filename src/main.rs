#![recursion_limit = "512"]

use rocket::{fairing::AdHoc, fs::FileServer, response::Redirect, Build, Rocket, State};
use rocket_dyn_templates::Template;
use serde::Deserialize;

mod cache;
pub use cache::Cache;
mod db;
pub use db::Db;
mod search;
pub use search::ArticleIndex;

#[derive(serde::Serialize, Deserialize)]
pub struct Config {
    pub site_name: String,
    pub main_page: String,
    #[serde(default)]
    pub default_path: String,
}

mod error;
pub use error::Error;
type Result<T> = std::result::Result<T, Error>;

// Route modules
mod articles;
mod settings;
mod users;

#[rocket::get("/")]
fn index(cfg: &State<Config>) -> Redirect {
    Redirect::to(cfg.default_path.clone())
}

fn rocket() -> Rocket<Build> {
    rocket::build()
        .mount("/", rocket::routes![index])
        .mount("/", articles::routes())
        .mount("/u", users::routes())
        .mount("/settings", settings::routes())
        .mount("/res", FileServer::from("static"))
        .manage(Cache::default())
        .attach(AdHoc::try_on_ignite("Read config", |rocket| async {
            let mut config: Config = match rocket.figment().extract() {
                Ok(c) => c,
                Err(e) => {
                    log::error!("Failed to parse config: {}", e);
                    return Err(rocket);
                }
            };
            if config.default_path.is_empty() {
                config.default_path = "/".to_string() + &config.main_page;
            }
            Ok(rocket.manage(config))
        }))
        .attach(AdHoc::try_on_ignite("Connect to db", |rocket| async {
            #[derive(Deserialize)]
            struct DbConfig {
                database_url: String,
            }
            let config: DbConfig = match rocket.figment().extract() {
                Ok(c) => c,
                Err(e) => {
                    log::error!("Failed to read database url: {}", e);
                    return Err(rocket);
                }
            };
            let db = match Db::try_connect(&config.database_url).await {
                Ok(db) => db,
                Err(e) => {
                    log::error!("Failed to connect to database: {}", e);
                    return Err(rocket);
                }
            };
            Ok(rocket.manage(db))
        }))
        .attach(AdHoc::try_on_ignite(
            "Create search index",
            |rocket| async {
                // I think I can unwrap this because this fairing will only run if the first one succeeds.
                let db = rocket.state::<Db>().unwrap();
                let index = match ArticleIndex::new(db).await {
                    Ok(index) => index,
                    Err(e) => {
                        log::error!("Failed to create article index: {}", e);
                        return Err(rocket);
                    }
                };
                Ok(rocket.manage(index))
            },
        ))
        .attach(Template::fairing())
}

#[rocket::main]
async fn main() -> Result<()> {
    loop {
        if let Err(e) = rocket().launch().await {
            println!("Rocket crashed: {:?}", e);
            continue;
        }
        break;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
