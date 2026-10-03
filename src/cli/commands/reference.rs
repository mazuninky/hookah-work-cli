//! `hw ref` — reference data endpoints without parameters.

use super::{Outcome, fetch};
use crate::cli::args::RefCommand;
use crate::client::{Api, Query};

/// The endpoint behind each reference command.
#[must_use]
pub fn path(command: RefCommand) -> &'static str {
    match command {
        RefCommand::Settings => "/api/settings",
        RefCommand::Tables => "/api/tables",
        RefCommand::PaymentMethods => "/api/payment-methods",
        RefCommand::Employees => "/api/employees",
        RefCommand::Services => "/api/services",
        RefCommand::BonusLevels => "/api/bonus-levels",
    }
}

/// Runs `hw ref <command>`.
pub fn run(command: RefCommand, api: &dyn Api) -> anyhow::Result<Outcome> {
    fetch(api, path(command), &Query::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::FakeApi;
    use serde_json::json;

    #[test]
    fn each_command_hits_its_endpoint_without_query() {
        let cases = [
            (RefCommand::Settings, "/api/settings"),
            (RefCommand::Tables, "/api/tables"),
            (RefCommand::PaymentMethods, "/api/payment-methods"),
            (RefCommand::Employees, "/api/employees"),
            (RefCommand::Services, "/api/services"),
            (RefCommand::BonusLevels, "/api/bonus-levels"),
        ];
        for (command, expected) in cases {
            let api = FakeApi::new().with_json(expected, json!([{"id": 1}]));
            let outcome = run(command, &api).unwrap();
            assert_eq!(api.requests(), vec![(expected.to_owned(), vec![])]);
            assert_eq!(outcome.json_value(), Some(&json!([{"id": 1}])));
        }
    }

    #[test]
    fn settings_fixture_passes_through() {
        let settings = json!({
            "interval": "30", "midnight": "6", "open": "9", "close": "26",
            "timezone": "Asia/Yekaterinburg", "countdown": "10"
        });
        let api = FakeApi::new().with_json("/api/settings", settings.clone());
        assert_eq!(
            run(RefCommand::Settings, &api).unwrap().json_value(),
            Some(&settings)
        );
    }
}
