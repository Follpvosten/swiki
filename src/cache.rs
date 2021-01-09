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
    // TODO: This should probably directly delete the captcha since it's really
    // only ever valid for a single request.
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

#[cfg(test)]
mod tests {
    use super::Cache;
    use uuid::Uuid;

    #[test]
    fn store_validate_remove_captcha() -> crate::Result<()> {
        let cache = Cache::new()?;
        let id1 = Uuid::new_v4();
        let solution1 = "12356";
        // Store a first captcha
        cache.register_captcha(id1, solution1)?;
        // Verify that it works
        assert!(cache.validate_captcha(id1, solution1)?);
        assert!(!cache.validate_captcha(id1, "12345")?);
        // And another, just to be sure
        let id2 = Uuid::new_v4();
        let solution2 = "98761";
        cache.register_captcha(id2, solution2)?;
        assert!(cache.validate_captcha(id2, solution2)?);
        // Also verify that using a different known solution doesn't work
        assert!(!cache.validate_captcha(id2, solution1)?);
        // An unknown id should always yield a false
        assert!(!cache.validate_captcha(Uuid::new_v4(), "123")?);
        // Verify that we can delete again
        cache.register_captcha(id1, solution1)?;
        cache.register_captcha(id2, solution2)?;
        Ok(())
    }
}
