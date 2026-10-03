//! Pager process management: `$HW_PAGER` > `$PAGER` > `less -FRX` (`more` on Windows).

use std::io::Write;
use std::process::{Child, Command, Stdio};

use tracing::debug;

/// Environment variable that turns the pager off when set to a truthy value.
pub const NO_PAGER_ENV: &str = "HW_NO_PAGER";

/// A running pager whose stdin is a pipe.
pub(crate) struct PagerProcess {
    child: Child,
}

impl PagerProcess {
    /// Takes the pipe to the pager's stdin.
    pub(crate) fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.child
            .stdin
            .take()
            .map(|stdin| Box::new(stdin) as Box<dyn Write + Send>)
    }

    /// Waits for the pager to exit; its exit status is not `hw`'s concern.
    pub(crate) fn wait(mut self) {
        let _ = self.child.wait();
    }
}

/// Spawns the configured pager, or returns `None` when it is disabled or cannot start.
pub(crate) fn spawn_pager() -> Option<PagerProcess> {
    if std::env::var(NO_PAGER_ENV).is_ok_and(|v| is_truthy(&v)) {
        return None;
    }
    let hw_pager = std::env::var("HW_PAGER").ok();
    let pager = std::env::var("PAGER").ok();
    let command_line = resolve_pager_command(hw_pager.as_deref(), pager.as_deref(), cfg!(windows));
    let parts = shlex::split(&command_line)?;
    let (program, args) = parts.split_first()?;

    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::piped());
    if program == "less" && std::env::var_os("LESS").is_none() {
        command.env("LESS", "FRX");
    }
    match command.spawn() {
        Ok(child) => Some(PagerProcess { child }),
        Err(err) => {
            debug!("pager disabled: cannot spawn `{command_line}`: {err}");
            None
        }
    }
}

/// Pure pager choice: first non-blank of `hw_pager`, `pager`, then the platform default.
#[must_use]
pub fn resolve_pager_command(
    hw_pager: Option<&str>,
    pager: Option<&str>,
    is_windows: bool,
) -> String {
    [hw_pager, pager]
        .into_iter()
        .flatten()
        .find(|v| !v.trim().is_empty())
        .map_or_else(
            || if is_windows { "more" } else { "less -FRX" }.to_owned(),
            str::to_owned,
        )
}

/// Flag truthiness: non-empty and neither `0` nor (case-insensitive) `false`.
#[must_use]
pub fn is_truthy(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hw_pager_beats_pager_beats_default() {
        assert_eq!(
            resolve_pager_command(Some("bat -p"), Some("less"), false),
            "bat -p"
        );
        assert_eq!(
            resolve_pager_command(Some("  "), Some("less -R"), false),
            "less -R"
        );
        assert_eq!(resolve_pager_command(None, None, false), "less -FRX");
        assert_eq!(resolve_pager_command(None, Some(""), true), "more");
    }

    #[test]
    fn truthiness() {
        for yes in ["1", "true", "yes", " on "] {
            assert!(is_truthy(yes), "{yes}");
        }
        for no in ["", " ", "0", "false", "FALSE"] {
            assert!(!is_truthy(no), "{no}");
        }
    }
}
