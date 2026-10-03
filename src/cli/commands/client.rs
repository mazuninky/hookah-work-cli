//! `hw client` — clients, groups, visit history and lookup.

use super::{Outcome, fetch, fetch_paged};
use crate::cli::args::{
    ClientCommand, ClientFindArgs, ClientHistoryArgs, ClientListArgs, join_api_names,
};
use crate::client::{Api, Query};

/// Runs `hw client <command>`.
pub fn run(command: &ClientCommand, api: &dyn Api) -> anyhow::Result<Outcome> {
    match command {
        ClientCommand::List(args) => list(args, api),
        ClientCommand::Groups => fetch(api, "/api/client/groups", &Query::new()),
        ClientCommand::History(args) => history(args, api),
        ClientCommand::Find(args) => find(args, api),
    }
}

fn list(args: &ClientListArgs, api: &dyn Api) -> anyhow::Result<Outcome> {
    let mut query = Query::new();
    query
        .push_opt("search", args.search.as_deref())
        .push_opt("group_id", args.group);
    fetch_paged(api, "/api/clients", query, &args.page)
}

fn history(args: &ClientHistoryArgs, api: &dyn Api) -> anyhow::Result<Outcome> {
    fetch_paged(
        api,
        &format!("/api/client/history/{}", args.id),
        Query::new(),
        &args.page,
    )
}

fn find(args: &ClientFindArgs, api: &dyn Api) -> anyhow::Result<Outcome> {
    let mut form = Query::new();
    form.push_opt("id", args.id)
        .push_opt("card", args.card)
        .push_opt("phone", args.phone.as_deref());
    let mut query = Query::new();
    query.push_opt("expand", join_api_names(&args.expand, ","));
    let response = api.find_client(form.pairs(), query.pairs())?;
    Ok(Outcome::json(response.into_json()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{ClientFindExpand, PageArgs};
    use crate::cli::commands::pairs;
    use crate::client::Pagination;
    use crate::error::Error;
    use crate::test_util::{CallKind, FakeApi};
    use serde_json::json;

    #[test]
    fn list_maps_filters_and_paging() {
        let api = FakeApi::new().with_json("/api/clients", json!([]));
        let args = ClientListArgs {
            search: Some("Яшин".into()),
            group: Some(1),
            page: PageArgs {
                page: Some(1),
                per_page: Some(3),
                all: false,
            },
        };
        run(&ClientCommand::List(args), &api).unwrap();
        assert_eq!(
            api.requests(),
            vec![(
                "/api/clients".to_owned(),
                pairs(&[
                    ("search", "Яшин"),
                    ("group_id", "1"),
                    ("page", "1"),
                    ("per-page", "3")
                ])
            )]
        );
    }

    #[test]
    fn list_all_walks_pages_with_per_page_100() {
        let page = |current| Pagination {
            total_count: 2,
            page_count: 2,
            current_page: current,
            per_page: 100,
        };
        let api = FakeApi::new()
            .with_page("/api/clients", json!([{"id": 1001}]), page(1))
            .with_page("/api/clients", json!([{"id": 1002}]), page(2));
        let args = ClientListArgs {
            search: None,
            group: None,
            page: PageArgs {
                page: None,
                per_page: None,
                all: true,
            },
        };
        let outcome = run(&ClientCommand::List(args), &api).unwrap();
        assert_eq!(
            outcome.json_value(),
            Some(&json!([{"id": 1001}, {"id": 1002}]))
        );
        assert_eq!(
            api.requests()[1].1,
            pairs(&[("page", "2"), ("per-page", "100")])
        );
    }

    #[test]
    fn groups_and_history_paths() {
        let api = FakeApi::new()
            .with_json("/api/client/groups", json!([{"id": 1, "name": "Свои"}]))
            .with_json("/api/client/history/1001", json!([]));
        run(&ClientCommand::Groups, &api).unwrap();
        let history = ClientHistoryArgs {
            id: 1001,
            page: PageArgs::default(),
        };
        run(&ClientCommand::History(history), &api).unwrap();
        assert_eq!(
            api.requests(),
            vec![
                ("/api/client/groups".to_owned(), vec![]),
                ("/api/client/history/1001".to_owned(), vec![]),
            ]
        );
    }

    #[test]
    fn find_posts_form_and_expands_in_query() {
        let api = FakeApi::new().with_json("/api/client/find", json!({"id": 1001}));
        let args = ClientFindArgs {
            id: Some(1001),
            card: Some(1),
            phone: Some("9175555111".into()),
            expand: vec![ClientFindExpand::BonusLevel, ClientFindExpand::NewBookings],
        };
        run(&ClientCommand::Find(args), &api).unwrap();
        let call = &api.calls()[0];
        assert_eq!(call.kind, CallKind::FindClient);
        assert_eq!(
            call.form,
            pairs(&[("id", "1001"), ("card", "1"), ("phone", "9175555111")])
        );
        assert_eq!(call.query, pairs(&[("expand", "bonusLevel,newBookings")]));
    }

    #[test]
    fn find_not_found_is_not_found_error() {
        let api = FakeApi::new().with_status(
            "/api/client/find",
            404,
            r#"{"message":"Клиент не найден"}"#,
        );
        let args = ClientFindArgs {
            id: None,
            card: None,
            phone: Some("70000000000".into()),
            expand: vec![],
        };
        let err = run(&ClientCommand::Find(args), &api).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::NotFound(_))
        ));
    }
}
