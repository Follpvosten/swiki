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
mod users;

#[rocket::get("/")]
fn index() -> Redirect {
    Redirect::to("/Main")
}

#[rocket::main]
async fn main() -> Result<()> {
    loop {
        let sled_db = sled::open("wiki.db")?;
        let db = database::Db::load_or_create(sled_db)?;
        if db.articles.id_by_name("Main")?.is_none() {
            let author_id = match db.get_userid_by_name("System")? {
                Some(id) => id,
                None => db.register_user("System", "todo lol")?,
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
        let res = rocket::ignite()
            .mount("/", rocket::routes![index])
            .mount("/", articles::routes())
            .mount("/u", users::routes())
            .mount("/res", StaticFiles::from("static"))
            .manage(ArticleIndex::new(&db)?)
            .manage(Cache::new()?)
            .manage(db)
            .attach(Template::fairing())
            .attach(AdHoc::config::<Config>())
            .launch()
            .await;
        if let Err(e) = res {
            println!("Rocket crashed: {:?}", e);
            continue;
        }
        break;
    }
    Ok(())
}
