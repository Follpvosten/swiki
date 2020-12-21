use rocket::routes;
use rocket_contrib::{serve::StaticFiles, templates::Template};

mod database;

mod error;
pub use error::Error;
type Result<T> = std::result::Result<T, Error>;

// Route modules
mod article;

#[rocket::main]
async fn main() -> Result<()> {
    loop {
        let sled_db = sled::open("wiki.db")?;
        let db = database::Db::load_or_create(sled_db)?;
        let res = rocket::ignite()
            .mount("/", routes![article::get, article::edit])
            .mount("/res", StaticFiles::from("static"))
            .manage(db)
            .attach(Template::fairing())
            .launch()
            .await;
        if let Err(e) = res {
            println!("Rocket crash: {:?}", e);
            continue;
        }
        break;
    }
    Ok(())
}
