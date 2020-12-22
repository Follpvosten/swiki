use rocket_contrib::{serve::StaticFiles, templates::Template};

mod database;

mod error;
pub use error::Error;
type Result<T> = std::result::Result<T, Error>;

// Route modules
mod articles;

#[rocket::main]
async fn main() -> Result<()> {
    loop {
        let sled_db = sled::open("wiki.db")?;
        let db = database::Db::load_or_create(sled_db)?;
        if !db.articles.exists(&"Main".into())? {
            // Create a first page if we don't have one.
            db.articles.add_revision(
                &"Main".into(),
                &"system".into(),
                r#"Welcome to your new wiki!

To edit this main page, go to [Main/edit].  
You can look at past revisions at [Main/revs].  
Have fun!"#,
            )?;
        }
        let res = rocket::ignite()
            .mount("/", articles::routes())
            .mount("/res", StaticFiles::from("static"))
            .manage(db)
            .attach(Template::fairing())
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
