//! [`Prompter`]: the terminal questions of an interactive `hw auth login`, behind a trait so
//! the flow is tested with a scripted `MockPrompter` (`test-util`) instead of a real TTY.

use inquire::ui::RenderConfig;

use super::Secret;

/// Checks one answer; `Err` holds the message shown before the question is asked again.
pub type Validator = fn(&str) -> Result<(), String>;

/// Why a prompt produced no answer.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    /// The user pressed Esc or Ctrl-C.
    #[error("login cancelled")]
    Cancelled,
    /// The input turned out not to be a terminal.
    #[error("the input is not a terminal")]
    NotTerminal,
    /// The terminal could not be driven.
    #[error("prompt failed: {0}")]
    Failed(String),
}

/// Questions asked on the terminal (rendered on stderr, never stdout). An invalid answer is
/// re-asked, so a returned value always passed its validator.
pub trait Prompter {
    /// One line of text; an empty answer takes `default`, then `validate` runs on it.
    fn text(
        &self,
        message: &str,
        default: Option<&str>,
        validate: Validator,
    ) -> Result<String, PromptError>;

    /// A secret whose characters are masked as typed; blank answers are re-asked.
    fn secret(&self, message: &str) -> Result<Secret, PromptError>;

    /// The index of the chosen option; the cursor starts on `options[default]`.
    fn select(&self, message: &str, options: &[&str], default: usize)
    -> Result<usize, PromptError>;
}

/// Accepts anything but an empty or whitespace-only answer.
pub fn non_blank(input: &str) -> Result<(), String> {
    if input.trim().is_empty() {
        Err("the answer cannot be empty".into())
    } else {
        Ok(())
    }
}

/// Production [`Prompter`] on `inquire`'s crossterm backend, which draws on stderr.
#[derive(Debug, Clone, Copy)]
pub struct InquirePrompter {
    /// Whether the prompts are drawn in colour.
    pub color: bool,
}

impl InquirePrompter {
    /// Colour unless `--no-color` or `NO_COLOR` (any value) is set. An explicit render config
    /// turns off inquire's own `NO_COLOR` check, so both are decided here.
    #[must_use]
    pub fn new(no_color_flag: bool, no_color_env: bool) -> Self {
        Self {
            color: !no_color_flag && !no_color_env,
        }
    }

    fn render_config(self) -> RenderConfig<'static> {
        if self.color {
            RenderConfig::default_colored()
        } else {
            RenderConfig::empty()
        }
    }
}

impl From<inquire::InquireError> for PromptError {
    fn from(err: inquire::InquireError) -> Self {
        use inquire::InquireError;
        match err {
            InquireError::OperationCanceled | InquireError::OperationInterrupted => {
                PromptError::Cancelled
            }
            InquireError::NotTTY => PromptError::NotTerminal,
            other => PromptError::Failed(other.to_string()),
        }
    }
}

fn inquire_validator(
    validate: Validator,
) -> impl Fn(&str) -> Result<inquire::validator::Validation, inquire::CustomUserError> + Clone {
    use inquire::validator::Validation;
    move |input: &str| {
        Ok(match validate(input) {
            Ok(()) => Validation::Valid,
            Err(why) => Validation::Invalid(why.into()),
        })
    }
}

impl Prompter for InquirePrompter {
    fn text(
        &self,
        message: &str,
        default: Option<&str>,
        validate: Validator,
    ) -> Result<String, PromptError> {
        let mut prompt = inquire::Text::new(message)
            .with_render_config(self.render_config())
            .with_validator(inquire_validator(validate));
        if let Some(default) = default {
            prompt = prompt.with_default(default);
        }
        Ok(prompt.prompt()?)
    }

    fn secret(&self, message: &str) -> Result<Secret, PromptError> {
        let value = inquire::Password::new(message)
            .with_render_config(self.render_config())
            .without_confirmation()
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .with_validator(inquire_validator(non_blank))
            .prompt()?;
        Ok(Secret::new(value))
    }

    fn select(
        &self,
        message: &str,
        options: &[&str],
        default: usize,
    ) -> Result<usize, PromptError> {
        if default >= options.len() {
            return Err(PromptError::Failed(format!(
                "'{message}' has no option {default} to preselect"
            )));
        }
        Ok(inquire::Select::new(message, options.to_vec())
            .with_render_config(self.render_config())
            .with_starting_cursor(default)
            .raw_prompt()?
            .index)
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use mock::{MockPrompter, MockResponse};

#[cfg(any(test, feature = "test-util"))]
mod mock {
    use std::collections::VecDeque;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use super::{PromptError, Prompter, Secret, Validator, non_blank};

    /// One scripted answer for [`MockPrompter`].
    #[derive(Debug, Clone)]
    pub enum MockResponse {
        /// Typed into a `text` prompt; `""` takes the default.
        Text(String),
        /// Typed into a `secret` prompt.
        Secret(String),
        /// Chosen in a `select` prompt.
        Select(usize),
        /// Esc / Ctrl-C at the next prompt of any kind.
        Cancel,
        /// The next prompt of any kind finds no terminal.
        NotTerminal,
    }

    /// Test [`Prompter`] answering from a FIFO script. Like inquire, it applies the default
    /// and the validator, recording each rejection and taking the next answer instead.
    #[derive(Debug, Default)]
    pub struct MockPrompter {
        script: Mutex<VecDeque<MockResponse>>,
        asked: Mutex<Vec<(String, Option<String>)>>,
        rejections: Mutex<Vec<String>>,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    impl MockPrompter {
        /// A prompter that gives `script` in order.
        #[must_use]
        pub fn new(script: Vec<MockResponse>) -> Self {
            Self {
                script: Mutex::new(script.into()),
                ..Self::default()
            }
        }

        /// Each prompt shown, as `(message, default)` (for `select`, the preselected option),
        /// re-asks not repeated.
        #[must_use]
        pub fn asked(&self) -> Vec<(String, Option<String>)> {
            lock(&self.asked).clone()
        }

        /// Validator messages of the rejected answers.
        #[must_use]
        pub fn rejections(&self) -> Vec<String> {
            lock(&self.rejections).clone()
        }

        /// Scripted answers not consumed yet.
        #[must_use]
        pub fn remaining(&self) -> usize {
            lock(&self.script).len()
        }

        fn ask(&self, message: &str, default: Option<&str>) {
            lock(&self.asked).push((message.to_owned(), default.map(str::to_owned)));
        }

        fn next(&self, message: &str) -> Result<MockResponse, PromptError> {
            match lock(&self.script).pop_front() {
                Some(MockResponse::Cancel) => Err(PromptError::Cancelled),
                Some(MockResponse::NotTerminal) => Err(PromptError::NotTerminal),
                Some(response) => Ok(response),
                None => Err(PromptError::Failed(format!(
                    "mock prompter: no answer scripted for '{message}'"
                ))),
            }
        }

        /// Pops answers until one passes `validate`.
        fn answer(
            &self,
            message: &str,
            default: Option<&str>,
            validate: Validator,
            take: fn(MockResponse) -> Option<String>,
        ) -> Result<String, PromptError> {
            loop {
                let response = self.next(message)?;
                let Some(mut answer) = take(response.clone()) else {
                    return Err(PromptError::Failed(format!(
                        "mock prompter: '{message}' got {response:?}"
                    )));
                };
                if let (true, Some(default)) = (answer.is_empty(), default) {
                    default.clone_into(&mut answer);
                }
                match validate(&answer) {
                    Ok(()) => return Ok(answer),
                    Err(why) => lock(&self.rejections).push(why),
                }
            }
        }
    }

    impl Prompter for MockPrompter {
        fn text(
            &self,
            message: &str,
            default: Option<&str>,
            validate: Validator,
        ) -> Result<String, PromptError> {
            self.ask(message, default);
            self.answer(message, default, validate, |r| match r {
                MockResponse::Text(text) => Some(text),
                _ => None,
            })
        }

        fn secret(&self, message: &str) -> Result<Secret, PromptError> {
            self.ask(message, None);
            self.answer(message, None, non_blank, |r| match r {
                MockResponse::Secret(secret) => Some(secret),
                _ => None,
            })
            .map(Secret::new)
        }

        fn select(
            &self,
            message: &str,
            options: &[&str],
            default: usize,
        ) -> Result<usize, PromptError> {
            let Some(preselected) = options.get(default) else {
                return Err(PromptError::Failed(format!(
                    "mock prompter: '{message}' has no option {default} to preselect"
                )));
            };
            self.ask(message, Some(preselected));
            match self.next(message)? {
                MockResponse::Select(index) if index < options.len() => Ok(index),
                other => Err(PromptError::Failed(format!(
                    "mock prompter: '{message}' with {} options got {other:?}",
                    options.len()
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_spaces(input: &str) -> Result<(), String> {
        if input.contains(' ') {
            Err(format!("'{input}' has a space"))
        } else {
            Ok(())
        }
    }

    #[test]
    fn mock_answers_in_order_and_records_prompts() {
        let prompter = MockPrompter::new(vec![
            MockResponse::Select(1),
            MockResponse::Text("ab".into()),
            MockResponse::Secret("s3cret".into()),
        ]);
        assert_eq!(prompter.select("Method?", &["a", "b"], 1).unwrap(), 1);
        assert_eq!(prompter.text("Name:", None, no_spaces).unwrap(), "ab");
        assert_eq!(prompter.secret("Key:").unwrap().expose(), "s3cret");
        assert_eq!(prompter.remaining(), 0);
        assert_eq!(
            prompter.asked(),
            vec![
                ("Method?".to_owned(), Some("b".to_owned())),
                ("Name:".to_owned(), None),
                ("Key:".to_owned(), None)
            ]
        );
    }

    #[test]
    fn mock_text_takes_the_default_and_reasks_invalid_answers() {
        let prompter = MockPrompter::new(vec![
            MockResponse::Text("a b".into()),
            MockResponse::Text(String::new()),
        ]);
        let answer = prompter.text("Name:", Some("dflt"), no_spaces).unwrap();
        assert_eq!(answer, "dflt");
        assert_eq!(prompter.rejections(), vec!["'a b' has a space"]);
        assert_eq!(
            prompter.asked(),
            vec![("Name:".to_owned(), Some("dflt".to_owned()))]
        );
    }

    #[test]
    fn mock_secret_reasks_blank_answers() {
        let prompter = MockPrompter::new(vec![
            MockResponse::Secret("  ".into()),
            MockResponse::Secret("k".into()),
        ]);
        assert_eq!(prompter.secret("Key:").unwrap().expose(), "k");
        assert_eq!(prompter.rejections().len(), 1);
    }

    #[test]
    fn mock_cancel_is_cancelled() {
        let prompter = MockPrompter::new(vec![MockResponse::Cancel]);
        let err = prompter.secret("Key:").unwrap_err();
        assert!(matches!(err, PromptError::Cancelled));
        assert_eq!(err.to_string(), "login cancelled");
    }

    #[test]
    fn mock_rejects_wrong_kind_out_of_range_and_exhausted_script() {
        let prompter = MockPrompter::new(vec![
            MockResponse::Secret("x".into()),
            MockResponse::Select(2),
        ]);
        assert!(matches!(
            prompter.text("Name:", None, non_blank),
            Err(PromptError::Failed(_))
        ));
        assert!(matches!(
            prompter.select("Method?", &["a", "b"], 0),
            Err(PromptError::Failed(_))
        ));
        assert!(matches!(
            prompter.secret("Key:"),
            Err(PromptError::Failed(m)) if m.contains("no answer scripted for 'Key:'")
        ));
    }

    #[test]
    fn inquire_errors_map_to_prompt_errors() {
        use inquire::InquireError;
        for cancel in [
            InquireError::OperationCanceled,
            InquireError::OperationInterrupted,
        ] {
            assert!(matches!(PromptError::from(cancel), PromptError::Cancelled));
        }
        assert!(matches!(
            PromptError::from(InquireError::NotTTY),
            PromptError::NotTerminal
        ));
    }

    #[test]
    fn mock_not_terminal_is_not_terminal() {
        let prompter = MockPrompter::new(vec![MockResponse::NotTerminal]);
        let err = prompter.select("Method?", &["a"], 0).unwrap_err();
        assert!(matches!(err, PromptError::NotTerminal));
    }

    #[test]
    fn prompts_are_colourless_under_no_color_flag_or_env() {
        for (flag, env, color) in [
            (false, false, true),
            (true, false, false),
            (false, true, false),
            (true, true, false),
        ] {
            let prompter = InquirePrompter::new(flag, env);
            assert_eq!(prompter.color, color, "flag {flag}, env {env}");
            let prefix = prompter.render_config().prompt_prefix.style.fg;
            assert_eq!(prefix.is_some(), color, "flag {flag}, env {env}");
        }
    }

    #[test]
    fn select_without_the_preselected_option_fails_before_drawing() {
        let prompter = InquirePrompter::new(true, false);
        for (options, default) in [(&[][..], 0), (&["a", "b"][..], 2)] {
            let err = prompter.select("Method?", options, default).unwrap_err();
            assert!(matches!(err, PromptError::Failed(_)), "{default}");
        }
        let mock = MockPrompter::new(vec![MockResponse::Select(0)]);
        let err = mock.select("Method?", &["a"], 1).unwrap_err();
        assert!(matches!(err, PromptError::Failed(_)));
        assert!(mock.asked().is_empty(), "nothing is shown");
        assert_eq!(mock.remaining(), 1, "no answer is consumed");
    }

    #[test]
    fn non_blank_rejects_whitespace() {
        assert!(non_blank(" \t").is_err());
        assert!(non_blank("x").is_ok());
    }
}
