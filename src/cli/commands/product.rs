//! `hw product` — products (paginated) and categories.

use super::{Outcome, fetch, fetch_paged};
use crate::cli::args::{ApiName, ProductCommand, ProductListArgs, join_api_names};
use crate::client::{Api, Query};

/// Runs `hw product <command>`.
pub fn run(command: &ProductCommand, api: &dyn Api) -> anyhow::Result<Outcome> {
    match command {
        ProductCommand::List(args) => list(args, api),
        ProductCommand::Categories => fetch(api, "/api/product/categories", &Query::new()),
    }
}

fn list(args: &ProductListArgs, api: &dyn Api) -> anyhow::Result<Outcome> {
    let mut query = Query::new();
    query
        .push_opt("category_id", args.category)
        .push_opt("type", args.kind.map(|kind| kind.api_name()))
        .push_opt("search", args.search.as_deref())
        .push_opt("expand", join_api_names(&args.expand, ","));
    fetch_paged(api, "/api/products", query, &args.page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{PageArgs, ProductExpand, ProductType};
    use crate::cli::commands::pairs;
    use crate::test_util::FakeApi;
    use serde_json::json;

    #[test]
    fn list_maps_every_filter() {
        let api = FakeApi::new().with_json("/api/products", json!([]));
        let args = ProductListArgs {
            category: Some(24),
            kind: Some(ProductType::Dish),
            search: Some("чай".into()),
            expand: vec![ProductExpand::Components],
            page: PageArgs {
                page: Some(2),
                per_page: Some(3),
                all: false,
            },
        };
        run(&ProductCommand::List(args), &api).unwrap();
        assert_eq!(
            api.requests(),
            vec![(
                "/api/products".to_owned(),
                pairs(&[
                    ("category_id", "24"),
                    ("type", "dish"),
                    ("search", "чай"),
                    ("expand", "components"),
                    ("page", "2"),
                    ("per-page", "3"),
                ])
            )]
        );
    }

    #[test]
    fn categories_has_no_query() {
        let api = FakeApi::new().with_json(
            "/api/product/categories",
            json!([{"id": 24, "name": "Чай"}]),
        );
        run(&ProductCommand::Categories, &api).unwrap();
        assert_eq!(
            api.requests(),
            vec![("/api/product/categories".to_owned(), vec![])]
        );
    }
}
