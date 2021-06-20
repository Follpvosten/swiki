use std::sync::Arc;

use dashmap::DashMap;
use uuid::Uuid;

/// In-memory sled database used for caching various things
#[derive(Default)]
pub struct Cache {
    captcha_solution: Arc<DashMap<Uuid, String>>,
}

impl Cache {
    /// Registers a captcha in the cache to be validated later.
    pub fn register_captcha(&self, id: Uuid, solution: &str) {
        self.captcha_solution.insert(id, solution.to_string());
    }
    /// Drop the captcha for the given id out of the cache and validate the given solution.
    /// You can only ever validate a single captcha once.
    pub fn validate_captcha(&self, id: Uuid, given_solution: &str) -> bool {
        let stored_solution = self.captcha_solution.remove(&id);
        // If we don't know the captcha, just return false
        stored_solution
            .map(|(_, stored_solution)| given_solution == stored_solution)
            .unwrap_or(false)
    }
    // Used for testing the register routes
    #[cfg(test)]
    pub fn get_solution(&self, id: Uuid) -> Option<String> {
        self.captcha_solution
            .get(&id)
            .map(|entry| entry.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Cache;
    use uuid::Uuid;

    #[test]
    fn store_validate_remove_captcha() {
        let cache = Cache::default();
        let id1 = Uuid::new_v4();
        let solution1 = "12356";
        // Store a first captcha
        cache.register_captcha(id1, solution1);
        // Verify that it works
        assert!(cache.validate_captcha(id1, solution1));
        // But only once
        assert!(!cache.validate_captcha(id1, solution1));
        // And another, just to be sure
        let id2 = Uuid::new_v4();
        let solution2 = "98761";
        cache.register_captcha(id2, solution2);
        assert!(cache.validate_captcha(id2, solution2));
        // Still only once
        assert!(!cache.validate_captcha(id2, solution2));
        // Also verify that using a different known solution doesn't work
        cache.register_captcha(id2, solution2);
        assert!(!cache.validate_captcha(id2, solution1));
        // And this also drops the captcha out
        assert!(!cache.validate_captcha(id2, solution2));
        // An unknown id should always yield a false
        assert!(!cache.validate_captcha(Uuid::new_v4(), "123"));
        // Verify that we can delete again
        cache.register_captcha(id1, solution1);
        cache.register_captcha(id2, solution2);
    }
}
