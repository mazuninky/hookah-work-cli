//! Response examples copied verbatim from the HookahWork API documentation
//! (https://hookah.work/api/), stored as `tests/fixtures/*.json`.

/// `POST /api/login` — 200.
pub const LOGIN: &str = include_str!("../fixtures/login.json");
/// Any endpoint — 401 Yii error.
pub const UNAUTHORIZED: &str = include_str!("../fixtures/unauthorized.json");
/// `POST /api/client/find` — 403 Yii error.
pub const FORBIDDEN: &str = include_str!("../fixtures/forbidden.json");
/// `GET /api/settings` — 200.
pub const SETTINGS: &str = include_str!("../fixtures/settings.json");
/// `GET /api/tables` — 200.
pub const TABLES: &str = include_str!("../fixtures/tables.json");
/// `GET /api/products?page=1&per-page=3` — 200.
pub const PRODUCTS: &str = include_str!("../fixtures/products.json");
/// `POST /api/client/find?expand=bonusLevel` — 200.
pub const CLIENT_FIND: &str = include_str!("../fixtures/client_find.json");
/// `POST /api/client/find` — 404 Yii error.
pub const CLIENT_NOT_FOUND: &str = include_str!("../fixtures/client_not_found.json");
/// `GET /api/expenses?date=2021-10-01&expand=category,paymentMethod` — 200.
pub const EXPENSES: &str = include_str!("../fixtures/expenses.json");
/// `GET /api/expenses` without `date` — 400 Yii error.
pub const EXPENSES_BAD_REQUEST: &str = include_str!("../fixtures/expenses_bad_request.json");
/// `POST /api/product` — 422 field list.
pub const VALIDATION_FAILED: &str = include_str!("../fixtures/validation_failed.json");

/// Parses a fixture.
pub fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("fixtures are valid JSON")
}
