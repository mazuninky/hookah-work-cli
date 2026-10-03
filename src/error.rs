//! Domain error type and the error → exit-code mapping (the exit codes are a public contract).

use thiserror::Error;

/// Every failure `hw` reports; the variant decides the process exit code.
#[derive(Debug, Error)]
pub enum Error {
    /// Missing, unreadable or inconsistent configuration (exit 3).
    #[error("configuration error: {0}")]
    Config(String),

    /// Missing, rejected or insufficient credentials — HTTP 401/403 included (exit 4).
    #[error("authentication error: {0}")]
    Auth(String),

    /// The requested resource does not exist — HTTP 404 (exit 2).
    #[error("not found: {0}")]
    NotFound(String),

    /// Bad user input caught locally or rejected by the API with 400/422 (exit 5).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Any other non-2xx API response, including 429 after the retries ran out (exit 1).
    #[error("API error {status}: {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Server-provided message or a truncated response body.
        message: String,
    },

    /// Network or protocol failure before a usable response arrived (exit 1).
    // Http, Io and Json embed the cause in the message and are not `#[from]`/`source`:
    // a source would be printed a second time by `{:#}` in `main`.
    #[error("HTTP error: {0}")]
    Http(ureq::Error),

    /// A 2xx response whose body does not have the expected shape (exit 1).
    #[error("invalid API response: {0}")]
    InvalidResponse(String),

    /// The OS credential store failed (exit 1).
    #[error("keyring error: {0}")]
    Keyring(String),

    /// `hw self` could not check for or install a release: GitHub refused the request, the
    /// checksum or the archive is wrong, or the platform or install location is unsupported (exit 1).
    #[error("self-update error: {0}")]
    SelfUpdate(String),

    /// `--jq` or `--template` failed while running (syntax errors are [`Error::InvalidInput`]) (exit 1).
    #[error("output transform error: {0}")]
    Transform(String),

    /// The selected output format cannot represent the result (exit 1).
    #[error("output error: {0}")]
    Render(String),

    /// Local I/O failure (exit 1).
    #[error("I/O error: {0}")]
    Io(std::io::Error),

    /// JSON (de)serialization failure (exit 1).
    #[error("JSON error: {0}")]
    Json(serde_json::Error),
}

impl From<ureq::Error> for Error {
    fn from(err: ureq::Error) -> Self {
        Error::Http(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Json(err)
    }
}

/// Crate-internal result alias; command handlers use `anyhow::Result` instead.
pub type Result<T> = std::result::Result<T, Error>;

/// Process exit codes.
pub mod exit_code {
    /// Success.
    pub const SUCCESS: i32 = 0;
    /// Runtime, API or network failure.
    pub const RUNTIME_ERROR: i32 = 1;
    /// The resource was not found (HTTP 404).
    pub const NOT_FOUND: i32 = 2;
    /// Configuration problem.
    pub const CONFIG_ERROR: i32 = 3;
    /// Missing, invalid or insufficient credentials.
    pub const AUTH_ERROR: i32 = 4;
    /// Invalid input: usage errors, HTTP 400/422, local validation.
    pub const INPUT_ERROR: i32 = 5;
}

impl Error {
    /// The exit code this error maps to.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Config(_) => exit_code::CONFIG_ERROR,
            Error::Auth(_) => exit_code::AUTH_ERROR,
            Error::NotFound(_) => exit_code::NOT_FOUND,
            Error::InvalidInput(_) => exit_code::INPUT_ERROR,
            Error::Api { .. }
            | Error::Http(_)
            | Error::InvalidResponse(_)
            | Error::Keyring(_)
            | Error::SelfUpdate(_)
            | Error::Transform(_)
            | Error::Render(_)
            | Error::Io(_)
            | Error::Json(_) => exit_code::RUNTIME_ERROR,
        }
    }
}

/// Maps a handler error to an exit code; the first domain [`Error`] in the context chain wins.
#[must_use]
pub fn exit_code_for_error(err: &anyhow::Error) -> i32 {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<Error>())
        .map_or(exit_code::RUNTIME_ERROR, Error::exit_code)
}

/// Maps a clap parse result to an exit code: `--help`/`--version` succeed, usage errors are input errors.
#[must_use]
pub fn exit_code_for_clap_error(err: &clap::Error) -> i32 {
    if err.use_stderr() {
        exit_code::INPUT_ERROR
    } else {
        exit_code::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    fn code(err: Error) -> i32 {
        exit_code_for_error(&anyhow::Error::from(err))
    }

    #[test]
    fn config_maps_to_3() {
        assert_eq!(code(Error::Config("x".into())), exit_code::CONFIG_ERROR);
    }

    #[test]
    fn auth_maps_to_4() {
        assert_eq!(code(Error::Auth("x".into())), exit_code::AUTH_ERROR);
    }

    #[test]
    fn not_found_maps_to_2() {
        assert_eq!(code(Error::NotFound("x".into())), exit_code::NOT_FOUND);
    }

    #[test]
    fn invalid_input_maps_to_5() {
        assert_eq!(
            code(Error::InvalidInput("x".into())),
            exit_code::INPUT_ERROR
        );
    }

    #[test]
    fn runtime_variants_map_to_1() {
        let runtime = [
            Error::Api {
                status: 500,
                message: "boom".into(),
            },
            Error::Http(ureq::Error::ConnectionFailed),
            Error::InvalidResponse("x".into()),
            Error::Keyring("x".into()),
            Error::SelfUpdate("x".into()),
            Error::Transform("x".into()),
            Error::Render("x".into()),
            Error::Io(std::io::Error::other("x")),
            Error::Json(serde_json::from_str::<serde_json::Value>("{").unwrap_err()),
        ];
        for err in runtime {
            let label = format!("{err:?}");
            assert_eq!(code(err), exit_code::RUNTIME_ERROR, "{label}");
        }
    }

    #[test]
    fn foreign_error_maps_to_1() {
        assert_eq!(
            exit_code_for_error(&anyhow::anyhow!("unexpected")),
            exit_code::RUNTIME_ERROR
        );
    }

    #[test]
    fn domain_error_is_found_under_context() {
        let err = Err::<(), _>(Error::NotFound("booking".into()))
            .context("fetching booking 7")
            .unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::NOT_FOUND);
    }

    #[test]
    fn http_error_is_printed_once() {
        let err = anyhow::Error::from(Error::from(ureq::Error::ConnectionFailed));
        assert_eq!(format!("{err:#}"), "HTTP error: connection failed");
    }

    #[test]
    fn io_error_is_printed_once() {
        let err = anyhow::Error::from(Error::from(std::io::Error::other("disk full")));
        assert_eq!(format!("{err:#}"), "I/O error: disk full");
    }

    #[test]
    fn json_error_is_printed_once() {
        let source = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let expected = format!("JSON error: {source}");
        let err = anyhow::Error::from(Error::from(source));
        assert_eq!(format!("{err:#}"), expected);
    }

    #[test]
    fn api_error_message_names_status() {
        let err = Error::Api {
            status: 429,
            message: "Too Many Requests".into(),
        };
        assert_eq!(err.to_string(), "API error 429: Too Many Requests");
    }

    #[test]
    fn clap_usage_error_is_input_error() {
        use clap::Parser;
        #[derive(Debug, Parser)]
        struct Probe {
            #[arg(long)]
            _n: u32,
        }
        let err = Probe::try_parse_from(["probe", "--n", "x"]).unwrap_err();
        assert_eq!(exit_code_for_clap_error(&err), exit_code::INPUT_ERROR);
        let help = Probe::try_parse_from(["probe", "--help"]).unwrap_err();
        assert_eq!(exit_code_for_clap_error(&help), exit_code::SUCCESS);
    }
}
