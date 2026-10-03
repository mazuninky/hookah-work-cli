//! `hw menu` — the public electronic menu (no token).

use super::{Outcome, fetch};
use crate::client::{Api, Query};

/// Runs `hw menu`; the caller connects without a token.
pub fn run(api: &dyn Api) -> anyhow::Result<Outcome> {
    fetch(api, "/api/menu", &Query::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::FakeApi;
    use serde_json::json;

    #[test]
    fn hits_menu() {
        let menu = json!([{"id": 1, "name": "Классический чай", "items": []}]);
        let api = FakeApi::new().with_json("/api/menu", menu.clone());
        assert_eq!(run(&api).unwrap().json_value(), Some(&menu));
        assert_eq!(api.requests(), vec![("/api/menu".to_owned(), vec![])]);
    }
}
