//! LG001: opt-in checks for an allowed call at the start of substantial blocks.

use crate::model::Level;
use crate::registry::{Finding, OptionKind, OptionSpec, Rule, RuleDefinition};
use crate::resolution::{EntryKind, ResolvedCall, resolve_call};
use crate::source::SourceContext;

/// Qualified logging entries used when an enabled rule has no custom allowlist.
const DEFAULT_ALLOWED: &[&str] = &[
    "log::trace!",
    "log::debug!",
    "log::info!",
    "tracing::trace!",
    "tracing::debug!",
    "tracing::info!",
];

/// An optional entry-call policy using the shared block and suppression model.
pub struct EntryLoggingRule;

impl Rule for EntryLoggingRule {
    fn definition(&self) -> RuleDefinition {
        RuleDefinition {
            id: "LG001",
            name: "entry-logging",
            description: "A substantial nested body must begin with an allowed macro or function call",
            default_level: Level::Off,
            options: vec![OptionSpec {
                name: "allowed",
                kind: OptionKind::StringList,
                description: "Qualified macro paths ending in ! or function paths ending in ()",
            }],
        }
    }

    fn validate_options(&self, options: &toml::Table) -> Result<(), String> {
        if options.contains_key("allowed") {
            allowed_entries(options).map(|_| ())
        } else {
            Ok(())
        }
    }

    fn check(
        &self,
        context: &SourceContext,
        options: &toml::Table,
    ) -> Result<Vec<Finding>, String> {
        let allowed = allowed_entries(options)?;
        let mut findings = Vec::new();
        for body in &context.bodies {
            // Each substantial body has its own entry and suppression target.
            if !context.is_substantial_body(body) {
                continue;
            }
            let first = context
                .first_non_comment_item(body)
                .ok_or("a substantial block has no first syntax item")?;
            let detail = match resolve_call(context, first) {
                Ok(call) if allowed.iter().any(|entry| same_call(entry, &call)) => continue,
                Ok(call) => format!("found {}", display_call(&call)),
                Err(reason) => format!("could not resolve the first item: {reason}"),
            };
            findings.push(Finding {
                message: format!(
                    "{} body must start with an allowed macro or function call; {detail}",
                    body.kind.description(),
                ),
                span: body.span,
                target: body.span,
                annotations: body.annotations.clone(),
            });
        }
        Ok(findings)
    }
}

/// Compare the resolved namespace and complete path, without suffix matching.
fn same_call(left: &ResolvedCall, right: &ResolvedCall) -> bool {
    left.kind == right.kind && left.path == right.path
}

/// Render a configured or resolved entry with its invocation kind visible.
fn display_call(call: &ResolvedCall) -> String {
    let suffix = match call.kind {
        EntryKind::Macro => "!",
        EntryKind::Function => "()",
    };
    format!("{}{suffix}", call.path)
}

/// Parse the default or configured list without accepting empty policies.
fn allowed_entries(options: &toml::Table) -> Result<Vec<ResolvedCall>, String> {
    let entries: Vec<&str> = match options.get("allowed") {
        None => DEFAULT_ALLOWED.to_vec(),
        Some(toml::Value::Array(values)) => values
            .iter()
            .map(|value| value.as_str().ok_or("allowed must be an array of strings"))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("allowed must be an array of strings".into()),
    };
    if entries.is_empty() {
        return Err("allowed must contain at least one qualified macro or function path".into());
    }
    entries.into_iter().map(parse_allowed).collect()
}

/// Validate a configured path without requiring the referenced project to exist.
fn parse_allowed(value: &str) -> Result<ResolvedCall, String> {
    let value = value.trim();
    let (kind, path) = if let Some(path) = value.strip_suffix('!') {
        (EntryKind::Macro, path)
    } else if let Some(path) = value.strip_suffix("()") {
        (EntryKind::Function, path)
    } else {
        return Err(format!(
            "invalid allowed entry {value:?}; use a qualified macro! or function() path"
        ));
    };
    let path = path.trim();
    let prefix = "fn configured_entry() { ";
    let source = format!("{prefix}{path}!(); }}");
    let context = SourceContext::parse("allowed.rs", source.as_bytes(), false)
        .map_err(|_| format!("invalid Rust path in allowed entry {value:?}"))?;
    let body = context
        .bodies
        .first()
        .ok_or("missing configured entry body")?;
    if body.statements.len() != 1 {
        return Err(format!(
            "allowed entry {value:?} must contain exactly one call path"
        ));
    }
    let item = context
        .first_non_comment_item(body)
        .ok_or("missing configured entry path")?;
    let invocation = if item.kind() == "expression_statement" {
        item.named_child(0)
            .ok_or("missing configured entry invocation")?
    } else {
        item
    };
    let target = invocation
        .child_by_field_name("macro")
        .filter(|_| invocation.kind() == "macro_invocation")
        .ok_or_else(|| format!("invalid Rust path in allowed entry {value:?}"))?;
    if target.start_byte() != prefix.len() || target.end_byte() != prefix.len() + path.len() {
        return Err(format!(
            "allowed entry {value:?} must contain only a qualified path"
        ));
    }
    let parsed_path = body.statements[0]
        .macro_path
        .as_deref()
        .ok_or_else(|| format!("invalid Rust path in allowed entry {value:?}"))?;
    let parts: Vec<_> = parsed_path
        .split("::")
        .map(|part| part.strip_prefix("r#").unwrap_or(part))
        .collect();
    if parts.len() < 2 || matches!(parts[0], "self" | "super" | "Self") {
        return Err(format!("allowed entry {value:?} must use a qualified path"));
    }
    Ok(ResolvedCall {
        kind,
        path: parts.join("::"),
    })
}

#[cfg(test)]
mod tests {
    //! Rule-level checks for entry ordering, shared scope, and allowlist syntax.

    use super::*;

    /// Configure a custom list without involving file discovery.
    fn options(entries: &[&str]) -> toml::Table {
        toml::Table::from_iter([(
            "allowed".into(),
            toml::Value::Array(
                entries
                    .iter()
                    .map(|entry| toml::Value::from(*entry))
                    .collect(),
            ),
        )])
    }

    /// Run the entry rule directly while keeping flow-comment enforcement independent.
    fn lint(source: &str, options: &toml::Table) -> Vec<Finding> {
        let context = SourceContext::parse("src/example.rs", source.as_bytes(), false)
            .expect("fixture must parse");
        EntryLoggingRule
            .check(&context, options)
            .expect("rule completes")
    }

    /// Merely registering this rule never enables enforcement.
    #[test]
    fn default_is_off_and_defaults_are_qualified_macros() {
        assert_eq!(EntryLoggingRule.definition().default_level, Level::Off);
        let entries = allowed_entries(&toml::Table::new()).expect("valid defaults");
        assert_eq!(entries.len(), 6);
        assert!(entries.iter().all(|entry| entry.kind == EntryKind::Macro));
        assert!(entries.iter().any(|entry| entry.path == "tracing::info"));
    }

    /// A custom list replaces the defaults and preserves macro/function identity.
    #[test]
    fn custom_policy_matches_complete_paths_and_invocation_kinds() {
        let policy = options(&["telemetry::enter()", "telemetry::event!"]);
        assert!(
            lint(
                "fn run() { if ready { telemetry::enter(); a(); b(); } }",
                &policy
            )
            .is_empty()
        );
        assert!(
            lint(
                "fn run() { if ready { telemetry::event!(); a(); b(); } }",
                &policy
            )
            .is_empty()
        );
        for first in ["other::enter()", "telemetry::enter!()", "tracing::info!()"] {
            assert_eq!(
                lint(
                    &format!("fn run() {{ if ready {{ {first}; a(); b(); }} }}"),
                    &policy
                )
                .len(),
                1
            );
        }
    }

    /// Only comments may precede the call; declarations and decorations remain visible.
    #[test]
    fn first_non_comment_item_is_checked() {
        let policy = toml::Table::new();
        for prefix in [
            "// Explain this work.\n",
            "/* Explain this work. */\n",
            "/// Documentation.\n",
        ] {
            assert!(
                lint(
                    &format!("fn run() {{ if ready {{\n{prefix}tracing::info!(); a(); b(); }} }}"),
                    &policy
                )
                .is_empty()
            );
        }
        for prefix in [
            "fn local() {}",
            "use tracing::info;",
            "#[cfg(feature = \"x\")]",
            ";",
            "a();",
        ] {
            assert_eq!(
                lint(
                    &format!("fn run() {{ if ready {{ {prefix}\ntracing::info!(); a(); b(); }} }}"),
                    &policy
                )
                .len(),
                1
            );
        }
    }

    /// The entry and flow rules deliberately share the same qualifying-body policy.
    #[test]
    fn scope_size_and_trivial_body_exemptions_match_flow_comments() {
        let policy = toml::Table::new();
        for source in [
            "fn run() { a(); b(); c(); }",
            "fn run() { if ready { a(); b(); } }",
            "const fn run() { if ready { a(); b(); c(); } }",
            "#[test] fn run() { if ready { a(); b(); c(); } }",
            "fn run() { if ready { warn!(); error!(); info!(); } }",
            "fn run() { if ready { self.a = other.a; self.b = other.b; self.c = 1; } }",
        ] {
            assert!(lint(source, &policy).is_empty(), "{source}");
        }
        assert_eq!(
            lint("fn run() { if ready { a(); b(); c(); } }", &policy).len(),
            1
        );
    }

    /// Entry inspection retains the same first item across every supported block construct.
    #[test]
    fn nested_block_constructs_use_their_own_entry() {
        for source in [
            "fn run() { for item in items { tracing::info!(); a(); b(); } }",
            "fn run() { while ready { tracing::info!(); a(); b(); } }",
            "fn run() { loop { tracing::info!(); a(); b(); } }",
            "fn run() { match value { Some(x) => { tracing::info!(); a(); b(); } _ => {} } }",
            "fn run() { let task = || { tracing::info!(); a(); b(); }; }",
            "fn run() { let task = async { tracing::info!(); a(); b(); }; }",
            "fn run() { unsafe { tracing::info!(); a(); b(); } }",
            "fn run() { let Some(x) = value else { tracing::info!(); a(); panic!(); }; }",
            "fn run() { { tracing::info!(); a(); b(); } }",
        ] {
            assert!(lint(source, &toml::Table::new()).is_empty(), "{source}");
            let missing = source.replace("tracing::info!()", "prepare()");
            assert_eq!(lint(&missing, &toml::Table::new()).len(), 1, "{missing}");
        }
    }

    /// Allowed invocations need not be bare expressions, but calls in arguments do not qualify.
    #[test]
    fn transparent_wrappers_preserve_the_entry_call() {
        let policy = options(&["telemetry::enter()"]);
        for first in [
            "(telemetry::enter())",
            "telemetry::enter()?",
            "telemetry::enter().await",
        ] {
            assert!(
                lint(
                    &format!("async fn run() {{ if ready {{ {first}; a(); b(); }} }}"),
                    &policy
                )
                .is_empty(),
                "{first}"
            );
        }
        for first in [
            "consume(telemetry::enter())",
            "let _ = telemetry::enter()",
            "return telemetry::enter()",
        ] {
            assert_eq!(
                lint(
                    &format!("fn run() {{ if ready {{ {first}; a(); b(); }} }}"),
                    &policy
                )
                .len(),
                1,
                "{first}"
            );
        }
    }

    /// Configuration is syntax-checked rather than treated as arbitrary string suffixes.
    #[test]
    fn invalid_allowlists_are_rejected_and_valid_paths_are_normalized() {
        for entry in [
            "",
            "info!",
            "enter()",
            "tracing::info",
            "tracing::info!(x)",
            "self::enter()",
            "super::enter()",
            "Self::enter()",
            "log::info!(); other::thing!",
            "log::*!",
            "fn helper() {} tracing::info!",
            "; tracing::info!",
            "tracing::info!(); } fn injected() { tracing::info!",
            "use tracing::debug; tracing::info!",
            "#[cfg(feature = \"x\")] tracing::info!",
        ] {
            assert!(
                EntryLoggingRule
                    .validate_options(&options(&[entry]))
                    .is_err(),
                "{entry}"
            );
        }
        assert!(EntryLoggingRule.validate_options(&options(&[])).is_err());
        assert_eq!(
            parse_allowed("::tracing::info!").unwrap().path,
            "tracing::info"
        );
        assert_eq!(
            parse_allowed("crate::r#type::r#match()").unwrap().path,
            "crate::type::match"
        );
        assert_eq!(
            parse_allowed("crate::log_entry()").unwrap().kind,
            EntryKind::Function
        );
    }
}
