use sled::Tree;
use uuid::Uuid;

use crate::Result;

/// In-memory sled database used for caching various things
pub struct Cache {
    _db: sled::Db,
    captcha_solution: Tree,
}

impl Cache {
    pub fn new() -> Result<Self> {
        let db = sled::Config::default().temporary(true).open()?;
        let captcha_solution = db.open_tree("captcha_solution")?;
        Ok(Self {
            _db: db,
            captcha_solution,
        })
    }

    pub fn register_captcha(&self, id: Uuid, solution: &str) -> Result<()> {
        self.captcha_solution
            .insert(id.as_bytes(), solution.as_bytes())?;
        Ok(())
    }
    pub fn validate_captcha(&self, id: Uuid, given_solution: &str) -> Result<bool> {
        let stored_solution = self
            .captcha_solution
            .get(id.as_bytes())?
            .map(|ivec| String::from_utf8(ivec.to_vec()).unwrap());
        // If we don't know the captcha, just return false
        Ok(stored_solution
            .map(|stored_solution| given_solution == stored_solution)
            .unwrap_or(false))
    }
    pub fn remove_captcha(&self, id: Uuid) -> Result<()> {
        self.captcha_solution.remove(id.as_bytes())?;
        Ok(())
    }
}
