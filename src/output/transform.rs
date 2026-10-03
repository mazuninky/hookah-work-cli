//! `--jq` and `--template` post-processing of a command's JSON result.
//!
//! jq runs first, so a template sees the filtered value. jq is the jaq 3 family
//! (`jaq-core` runtime, `jaq-std` definitions, `jaq-json` values); templates are minijinja.

use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, compile, data, load, unwrap_valr};
use jaq_json::Val;
use minijinja::{Environment, Value as MjValue};
use serde_json::Value;

use crate::error::{Error, Result};

type JqFilter = jaq_core::Filter<data::JustLut<Val>>;

const TEMPLATE_NAME: &str = "hw-inline";

/// The compiled `--jq` and `--template` of this invocation. Compiling happens before the
/// command runs, so a syntax error costs no API request.
#[derive(Default)]
pub struct Transforms<'a> {
    jq: Option<JqFilter>,
    template: Option<Environment<'a>>,
}

impl<'a> Transforms<'a> {
    /// No transforms.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Compiles both expressions; a syntax error is [`Error::InvalidInput`].
    pub fn compile(jq: Option<&str>, template: Option<&'a str>) -> Result<Self> {
        Ok(Self {
            jq: jq.map(compile_jq).transpose()?,
            template: template.map(compile_template).transpose()?,
        })
    }

    /// Whether neither `--jq` nor `--template` is set.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.jq.is_none() && self.template.is_none()
    }

    /// Whether `--jq` is set.
    #[must_use]
    pub fn has_jq(&self) -> bool {
        self.jq.is_some()
    }
}

/// The pipeline result.
#[derive(Debug, Clone, PartialEq)]
pub enum Transformed {
    /// A value still to be rendered by the selected reporter.
    Json(Value),
    /// Rendered template text, written verbatim.
    Text(String),
}

/// Runs jq (0 results → `null`, 1 → the value, N → an array), then the template.
/// Runtime failures are [`Error::Transform`].
pub fn apply(value: Value, transforms: &Transforms<'_>) -> Result<Transformed> {
    let current = match &transforms.jq {
        Some(filter) => run_jq(filter, value)?,
        None => value,
    };
    match &transforms.template {
        Some(env) => render_template(env, &current).map(Transformed::Text),
        None => Ok(Transformed::Json(current)),
    }
}

fn compile_jq(expr: &str) -> Result<JqFilter> {
    let arena = Arena::default();
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let modules = Loader::new(defs)
        .load(
            &arena,
            File {
                path: (),
                code: expr,
            },
        )
        .map_err(|errs| {
            jq_syntax_error(expr, errs.iter().map(|(_, err)| load_messages(expr, err)))
        })?;

    // All three native tables are needed, or filters like `length` fail to resolve.
    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());
    Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errs| {
            jq_syntax_error(
                expr,
                errs.iter()
                    .map(|(_, undefined)| undefined_messages(undefined)),
            )
        })
}

/// `--jq: <problem>[; <problem>…] in `<expr>``; positions are 1-based columns.
fn jq_syntax_error(expr: &str, messages: impl Iterator<Item = Vec<String>>) -> Error {
    let messages: Vec<String> = messages.flatten().collect();
    let detail = if messages.is_empty() {
        "invalid expression".to_owned()
    } else {
        messages.join("; ")
    };
    Error::InvalidInput(format!("--jq: {detail} in `{}`", expr.trim()))
}

fn load_messages(expr: &str, err: &load::Error<&str>) -> Vec<String> {
    match err {
        load::Error::Io(items) => items
            .iter()
            .map(|(path, why)| format!("cannot load module {path}: {why}"))
            .collect(),
        load::Error::Lex(items) => items
            .iter()
            .map(|(expected, at)| match (expected, at.chars().next()) {
                (load::lex::Expect::Token, Some(c)) => {
                    format!("unexpected `{c}`{}", position(expr, at))
                }
                _ => format!("expected {}{}", expected.as_str(), position(expr, at)),
            })
            .collect(),
        load::Error::Parse(items) => items
            .iter()
            .map(|(expected, found)| {
                let what = if found.is_empty() {
                    "end of input".to_owned()
                } else {
                    format!("`{found}`")
                };
                let at = position(expr, found);
                format!("expected {}, found {what}{at}", expected.as_str())
            })
            .collect(),
    }
}

/// ` at column N` for a slice of `expr` (jaq reports positions as sub-slices of the input).
fn position(expr: &str, part: &str) -> String {
    let start = (part.as_ptr() as usize).wrapping_sub(expr.as_ptr() as usize);
    match expr.get(..start) {
        Some(before) => format!(" at column {}", before.chars().count() + 1),
        None => String::new(),
    }
}

fn undefined_messages(errs: &[compile::Error<&str>]) -> Vec<String> {
    errs.iter()
        .map(|(name, undefined)| match undefined {
            compile::Undefined::Filter(arity) => format!("undefined filter `{name}/{arity}`"),
            other => format!("undefined {} `{name}`", other.as_str()),
        })
        .collect()
}

fn run_jq(filter: &JqFilter, input: Value) -> Result<Value> {
    let ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));
    let input: Val = serde_json::from_value(input)
        .map_err(|e| Error::Transform(format!("jq: cannot convert input: {e}")))?;

    let mut results = Vec::new();
    for item in filter.id.run((ctx, input)).map(unwrap_valr) {
        let value = item.map_err(|e| Error::Transform(format!("jq: {e}")))?;
        let json: Value = serde_json::from_str(&value.to_string())
            .map_err(|e| Error::Transform(format!("jq: cannot convert output: {e}")))?;
        results.push(json);
    }
    Ok(match results.len() {
        0 => Value::Null,
        1 => results.pop().unwrap_or(Value::Null),
        _ => Value::Array(results),
    })
}

fn compile_template(template: &str) -> Result<Environment<'_>> {
    let mut env = Environment::new();
    env.add_template(TEMPLATE_NAME, template)
        .map_err(|e| Error::InvalidInput(format!("--template: {e}")))?;
    Ok(env)
}

fn render_template(env: &Environment<'_>, value: &Value) -> Result<String> {
    env.get_template(TEMPLATE_NAME)
        .and_then(|template| template.render(template_context(value)))
        .map_err(|e| Error::Transform(format!("template: {e}")))
}

/// Object keys become root variables; the whole value is always `this`, so arrays and
/// scalars stay reachable. `this` is spread first because the first spread wins on collisions.
fn template_context(value: &Value) -> MjValue {
    let this = minijinja::context! { this => MjValue::from_serialize(value) };
    match value {
        Value::Object(_) => {
            let flattened = MjValue::from_serialize(value);
            minijinja::context! { ..this, ..flattened }
        }
        _ => this,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jq(expr: &str) -> Transforms<'static> {
        Transforms::compile(Some(expr), None).unwrap()
    }

    fn template(t: &str) -> Transforms<'_> {
        Transforms::compile(None, Some(t)).unwrap()
    }

    fn compile_error(jq: Option<&str>, template: Option<&str>) -> String {
        match Transforms::compile(jq, template) {
            Err(Error::InvalidInput(message)) => message,
            Err(other) => panic!("expected InvalidInput, got {other:?}"),
            Ok(_) => panic!("{jq:?} / {template:?} compiled"),
        }
    }

    #[test]
    fn noop_passes_value_through() {
        let value = json!({"a": 1});
        assert!(Transforms::none().is_noop());
        assert_eq!(
            apply(value.clone(), &Transforms::none()).unwrap(),
            Transformed::Json(value)
        );
    }

    #[test]
    fn jq_streams_collapse() {
        assert_eq!(
            apply(json!({"a": 42}), &jq(".a")).unwrap(),
            Transformed::Json(json!(42))
        );
        assert_eq!(
            apply(json!([{"n": "a"}, {"n": "b"}]), &jq(".[].n")).unwrap(),
            Transformed::Json(json!(["a", "b"]))
        );
        assert_eq!(
            apply(json!(1), &jq("empty")).unwrap(),
            Transformed::Json(Value::Null)
        );
        assert_eq!(
            apply(json!([1, 2, 3]), &jq("map(. + 1) | length")).unwrap(),
            Transformed::Json(json!(3))
        );
    }

    #[test]
    fn jq_errors_are_invalid_input_for_humans() {
        assert_eq!(
            compile_error(Some("{a:}"), None),
            "--jq: expected term, found `}` at column 4 in `{a:}`"
        );
        assert_eq!(
            compile_error(Some(".a | ]"), None),
            "--jq: unexpected `]` at column 6 in `.a | ]`"
        );
        assert_eq!(
            compile_error(Some(".a | nope"), None),
            "--jq: undefined filter `nope/0` in `.a | nope`"
        );
        assert_eq!(
            compile_error(Some("map(.a"), None),
            "--jq: expected closing parenthesis at column 7 in `map(.a`"
        );
    }

    #[test]
    fn template_renders_object_keys_and_this() {
        assert_eq!(
            apply(json!({"name": "Стол 1"}), &template("{{ name }}")).unwrap(),
            Transformed::Text("Стол 1".into())
        );
        assert_eq!(
            apply(
                json!([1, 2]),
                &template("{% for i in this %}{{ i }}{% endfor %}")
            )
            .unwrap(),
            Transformed::Text("12".into())
        );
    }

    #[test]
    fn template_syntax_error_is_invalid_input() {
        assert!(compile_error(None, Some("{{ ")).starts_with("--template: "));
    }
}
