//! Acceptance tests for adding an unrelated structural rule through the registry.

use evenflow::analyzer::{AnalysisSettings, analyze_source};
use evenflow::model::{ByteSpan, DiagnosticKind, Level, Report, Severity};
use evenflow::registry::{Finding, OptionKind, OptionSpec, Registry, Rule, RuleDefinition};
use evenflow::source::{SourceContext, node_span};

/// A fixture-only rule requiring explicit function return-type syntax.
struct ExplicitReturnType;

impl Rule for ExplicitReturnType {
    fn definition(&self) -> RuleDefinition {
        RuleDefinition {
            id: "XX001",
            name: "explicit-return-type",
            description: "Fixture rule that detects absent return-type syntax",
            default_level: Level::Warning,
            options: vec![OptionSpec {
                name: "enforce",
                kind: OptionKind::Boolean,
                description: "Whether the fixture policy requires a return type",
            }],
        }
    }

    fn check(
        &self,
        context: &SourceContext,
        options: &toml::Table,
    ) -> Result<Vec<Finding>, String> {
        if options.get("enforce").and_then(toml::Value::as_bool) == Some(false) {
            return Ok(Vec::new());
        }
        let mut nodes = vec![context.tree.root_node()];
        let mut findings = Vec::new();
        // Select function syntax without altering shared parsing or the runner,
        // and report the owning construct when its return type is absent.
        while let Some(node) = nodes.pop() {
            if node.kind() == "function_item" && node.child_by_field_name("return_type").is_none() {
                findings.push(Finding {
                    message: "function needs an explicit return type".into(),
                    span: node_span(node),
                    target: node_span(node),
                    annotations: context
                        .leading_comments(node.start_byte())
                        .iter()
                        .map(|comment| comment.span)
                        .collect(),
                });
            }
            let mut cursor = node.walk();
            nodes.extend(node.named_children(&mut cursor));
        }
        Ok(findings)
    }
}

/// A fixture-only implementation that reports a recoverable rule failure.
struct FailedRule;

impl Rule for FailedRule {
    fn definition(&self) -> RuleDefinition {
        RuleDefinition {
            id: "XX002",
            name: "failure-fixture",
            description: "Fixture rule that reports an internal analysis failure",
            default_level: Level::Warning,
            options: Vec::new(),
        }
    }

    fn check(
        &self,
        _context: &SourceContext,
        _options: &toml::Table,
    ) -> Result<Vec<Finding>, String> {
        Err("fixture failure".into())
    }
}

/// Register the unrelated rule through the same interface as production rules.
fn fixture_registry() -> Registry {
    Registry::new(vec![Box::new(ExplicitReturnType)]).expect("fixture registry is valid")
}

/// Verify absence detection, source locations, and configured enforcement.
#[test]
fn unrelated_rule_uses_shared_analysis_and_policy() {
    let registry = fixture_registry();
    let source = b"// ordinary preamble\nfn example() {}\nfn explicit() -> () {}\n";
    let mut settings = AnalysisSettings::default();
    let advisory = analyze_source(source, "standalone.rs", &settings, &registry);
    assert!(advisory.completed);
    assert_eq!(advisory.diagnostics.len(), 1);
    assert_eq!(advisory.diagnostics[0].rule, "XX001");
    assert_eq!(advisory.diagnostics[0].severity, Severity::Warning);
    assert_eq!(
        advisory.diagnostics[0]
            .range
            .expect("located finding")
            .start
            .line,
        2
    );
    settings.strict = true;
    let strict = analyze_source(source, "standalone.rs", &settings, &registry);
    assert_eq!(strict.diagnostics[0].severity, Severity::Error);
    settings.levels.insert("XX001".into(), Level::Off);
    assert!(
        analyze_source(source, "standalone.rs", &settings, &registry)
            .diagnostics
            .is_empty()
    );
}

/// Verify options are typed centrally and remain validated for disabled rules.
#[test]
fn unrelated_rule_options_use_registry_validation() {
    let registry = fixture_registry();
    let mut settings = AnalysisSettings::default();
    settings.options.insert(
        "XX001".into(),
        toml::Table::from_iter([("enforce".into(), toml::Value::Boolean(false))]),
    );
    assert!(
        analyze_source(b"fn example() {}", "standalone.rs", &settings, &registry)
            .diagnostics
            .is_empty()
    );
    settings.levels.insert("XX001".into(), Level::Off);
    settings
        .options
        .get_mut("XX001")
        .expect("fixture options exist")
        .insert("enforce".into(), toml::Value::Integer(7));
    let invalid = analyze_source(b"fn example() {}", "standalone.rs", &settings, &registry);
    assert!(!invalid.completed);
    assert_eq!(
        invalid.diagnostics[0].category.as_deref(),
        Some("configuration")
    );
}

/// Verify generic suppressions attach to declarations rather than only bodies.
#[test]
fn unrelated_rule_suppressions_are_exact_and_reasoned() {
    let registry = fixture_registry();
    let source = b"// evenflow: allow(XX001): This local signature is intentionally implicit.\nfn first() {}\nfn second() {}";
    let result = analyze_source(
        source,
        "standalone.rs",
        &AnalysisSettings::default(),
        &registry,
    );
    assert!(result.completed);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        result.diagnostics[0]
            .range
            .expect("located second function")
            .start
            .line,
        3
    );
    let inactive = AnalysisSettings {
        levels: std::collections::BTreeMap::from([("XX001".into(), Level::Off)]),
        ..AnalysisSettings::default()
    };
    assert!(
        analyze_source(source, "standalone.rs", &inactive, &registry)
            .diagnostics
            .is_empty()
    );
    let unused = analyze_source(
        b"// evenflow: allow(XX001): Already explicit.\nfn first() -> () {}",
        "standalone.rs",
        &AnalysisSettings::default(),
        &registry,
    );
    assert!(!unused.completed);
    assert_eq!(
        unused.diagnostics[0].category.as_deref(),
        Some("suppression")
    );
}

/// Verify failed rules are named and do not expose misleading unused directives.
#[test]
fn rule_failures_are_structured_and_incomplete() {
    let registry = Registry::new(vec![Box::new(FailedRule)]).expect("fixture registry is valid");
    let result = analyze_source(
        b"// evenflow: allow(XX002): Intentional fixture.\nfn example() {}",
        "standalone.rs",
        &AnalysisSettings::default(),
        &registry,
    );
    assert!(!result.completed);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        result.diagnostics[0].category.as_deref(),
        Some("internal-rule")
    );
    assert!(result.diagnostics[0].message.contains("XX002"));
    let registry = Registry::new(vec![Box::new(FailedRule), Box::new(ExplicitReturnType)])
        .expect("independent fixture rules register");
    let incomplete = analyze_source(
        b"// evenflow: allow(XX001): Do not validate unused directives in an incomplete file.\nfn example() -> () {}",
        "standalone.rs",
        &AnalysisSettings::default(),
        &registry,
    );
    assert_eq!(incomplete.diagnostics.len(), 1);
    assert_eq!(
        incomplete.diagnostics[0].category.as_deref(),
        Some("internal-rule")
    );
}

/// Verify malformed suppression syntax and unknown IDs cannot pass silently.
#[test]
fn invalid_directives_are_failures() {
    let registry = fixture_registry();
    for source in [
        "// evenflow: allow(XX001):\nfn example() {}",
        "// evenflow: ignore(XX001): reason\nfn example() {}",
        "// evenflow: allow(XX999): reason\nfn example() {}",
        "fn example() {} // evenflow: allow(XX001): reason",
    ] {
        let result = analyze_source(
            source.as_bytes(),
            "standalone.rs",
            &AnalysisSettings::default(),
            &registry,
        );
        assert!(!result.completed, "{source}");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|item| item.category.as_deref() == Some("suppression"))
        );
    }
}

/// Verify prose mentions and API documentation do not activate suppressions.
#[test]
fn ordinary_prose_and_doc_comments_are_inert() {
    let registry = fixture_registry();
    let source = b"/// evenflow: allow(XX999): API prose.\n// Mention evenflow: allow(XX999): ordinary prose.\nfn example() {}";
    let result = analyze_source(
        source,
        "standalone.rs",
        &AnalysisSettings::default(),
        &registry,
    );
    assert!(result.completed);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].rule, "XX001");
}

/// Verify registry collisions fail before rules can overwrite one another.
#[test]
fn duplicate_rule_ids_are_rejected() {
    assert!(
        Registry::new(vec![
            Box::new(ExplicitReturnType),
            Box::new(ExplicitReturnType)
        ])
        .is_err()
    );
    let bundled = Registry::bundled().expect("bundled rule metadata is valid");
    let definitions = bundled.definitions();
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].id, "FC001");
    assert_eq!(definitions[0].default_level, Level::Warning);
    assert_eq!(definitions[1].id, "LG001");
    assert_eq!(definitions[1].default_level, Level::Off);
    assert_eq!(definitions[1].options[0].name, "allowed");
    assert_eq!(definitions[1].options[0].kind, OptionKind::StringList);
}

/// Flow comments and entry calls remain independently enabled and independently satisfied.
#[test]
fn bundled_logging_and_flow_policies_are_independent() {
    let registry = Registry::bundled().expect("bundled rules register");
    let missing_both = "fn work() { if ready() { prepare(); save(); finish(); } }";
    let comment_only = "fn work() { if ready() {\n// Preserve the transaction order.\nprepare(); save(); finish(); } }";
    let entry_only = "fn work() { if ready() { tracing::info!(\"entry\"); save(); finish(); } }";
    for (source, flow, logging, expected) in [
        (missing_both, Level::Warning, Level::Off, vec!["FC001"]),
        (missing_both, Level::Off, Level::Warning, vec!["LG001"]),
        (
            missing_both,
            Level::Warning,
            Level::Warning,
            vec!["FC001", "LG001"],
        ),
        (comment_only, Level::Warning, Level::Warning, vec!["LG001"]),
        (entry_only, Level::Warning, Level::Warning, vec!["FC001"]),
    ] {
        let settings = AnalysisSettings {
            strict: true,
            levels: std::collections::BTreeMap::from([
                ("FC001".into(), flow),
                ("LG001".into(), logging),
            ]),
            ..AnalysisSettings::default()
        };
        let result = analyze_source(source.as_bytes(), "standalone.rs", &settings, &registry);
        assert!(result.completed, "{source}");
        assert_eq!(
            result
                .diagnostics
                .iter()
                .map(|item| item.rule.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{source}"
        );
        assert!(
            result
                .diagnostics
                .iter()
                .all(|item| item.severity == Severity::Error)
        );
    }
}

/// Suppressing one bundled rule does not suppress the other finding on that same body.
#[test]
fn logging_and_flow_suppressions_are_rule_specific() {
    let registry = Registry::bundled().expect("bundled rules register");
    let settings = AnalysisSettings {
        levels: std::collections::BTreeMap::from([("LG001".into(), Level::Warning)]),
        ..AnalysisSettings::default()
    };
    for (suppressed, remaining) in [("LG001", "FC001"), ("FC001", "LG001")] {
        let source = format!(
            "fn work() {{\n// evenflow: allow({suppressed}): This body intentionally omits the policy.\nif ready() {{ prepare(); save(); finish(); }} }}"
        );
        let result = analyze_source(source.as_bytes(), "standalone.rs", &settings, &registry);
        assert!(result.completed, "{suppressed}");
        assert_eq!(result.diagnostics.len(), 1, "{suppressed}");
        assert_eq!(result.diagnostics[0].rule, remaining);
    }
    let result = analyze_source(
        b"fn work() { if ready() {\n// evenflow: allow(LG001): Avoid duplicate logging for this transaction.\n// Preserve the transaction order.\nprepare(); save(); finish(); } }",
        "standalone.rs",
        &settings,
        &registry,
    );
    assert!(result.completed);
    assert!(result.diagnostics.is_empty());
}

/// A logging suppression belongs to one body and cannot hide descendant or sibling findings.
#[test]
fn logging_suppressions_do_not_propagate_to_other_bodies() {
    let registry = Registry::bundled().expect("bundled rules register");
    let settings = AnalysisSettings {
        levels: std::collections::BTreeMap::from([("LG001".into(), Level::Warning)]),
        ..AnalysisSettings::default()
    };
    for (source, remaining_line) in [
        (
            "fn work() {\n// evenflow: allow(LG001): The outer transaction is logged by its caller.\nif ready() {\n// Keep the transaction order stable.\nif child() {\n// Keep this child transaction together.\nprepare(); save(); finish();\n}\nafter(); finish();\n} }",
            5,
        ),
        (
            "fn work() {\n// evenflow: allow(LG001): The first transaction is logged by its caller.\nif ready() {\n// Keep the transaction order stable.\nprepare(); save(); finish();\n}\nif other() {\n// Keep this second transaction together.\nprepare(); save(); finish();\n} }",
            7,
        ),
    ] {
        let result = analyze_source(source.as_bytes(), "standalone.rs", &settings, &registry);
        assert!(result.completed, "{source}");
        assert_eq!(result.diagnostics.len(), 1, "{source}");
        assert_eq!(result.diagnostics[0].rule, "LG001");
        assert_eq!(
            result.diagnostics[0]
                .range
                .expect("located body")
                .start
                .line,
            remaining_line
        );
    }
}

/// Known disabled-rule directives stay inactive; enabled unused directives remain errors.
#[test]
fn logging_suppression_validation_respects_the_disabled_default() {
    let registry = Registry::bundled().expect("bundled rules register");
    let source = b"fn work() {\n// evenflow: allow(LG001): Caller already owns this trace.\nif ready() {\n// Preserve the transaction order.\ntracing::info!(\"entry\"); save(); finish(); } }";
    let inactive = analyze_source(
        source,
        "standalone.rs",
        &AnalysisSettings::default(),
        &registry,
    );
    assert!(inactive.completed);
    assert!(inactive.diagnostics.is_empty());
    let enabled = analyze_source(
        source,
        "standalone.rs",
        &AnalysisSettings {
            levels: std::collections::BTreeMap::from([("LG001".into(), Level::Warning)]),
            ..AnalysisSettings::default()
        },
        &registry,
    );
    assert!(!enabled.completed);
    assert_eq!(enabled.diagnostics.len(), 1);
    assert_eq!(enabled.diagnostics[0].rule, "EF000");
    assert_eq!(
        enabled.diagnostics[0].category.as_deref(),
        Some("suppression")
    );
}

/// The library validates the logging allowlist before skipping disabled enforcement.
#[test]
fn disabled_logging_options_still_use_semantic_validation_in_the_library() {
    let registry = Registry::bundled().expect("bundled rules register");
    for allowed in [
        toml::Value::Boolean(false),
        toml::Value::Array(Vec::new()),
        toml::Value::Array(vec![toml::Value::String("info!".into())]),
        toml::Value::Array(vec![toml::Value::String("tracing::info".into())]),
    ] {
        let settings = AnalysisSettings {
            levels: std::collections::BTreeMap::from([("LG001".into(), Level::Off)]),
            options: std::collections::BTreeMap::from([(
                "LG001".into(),
                toml::Table::from_iter([("allowed".into(), allowed)]),
            )]),
            ..AnalysisSettings::default()
        };
        let result = analyze_source(b"fn work() {}", "standalone.rs", &settings, &registry);
        assert!(!result.completed);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].category.as_deref(),
            Some("configuration")
        );
    }
}

/// Verify decoding and complete-file syntax errors remain visible with rules off.
#[test]
fn invalid_sources_cannot_produce_clean_results() {
    let registry = fixture_registry();
    let settings = AnalysisSettings {
        levels: std::collections::BTreeMap::from([("XX001".into(), Level::Off)]),
        ..AnalysisSettings::default()
    };
    let encoding = analyze_source(&[0xff], "standalone.rs", &settings, &registry);
    assert!(!encoding.completed);
    assert_eq!(
        encoding.diagnostics[0].category.as_deref(),
        Some("encoding")
    );
    let parsing = analyze_source(
        b"#[cfg(test)] mod tests { fn bad( }",
        "standalone.rs",
        &settings,
        &registry,
    );
    assert!(!parsing.completed);
    assert_eq!(parsing.diagnostics[0].category.as_deref(), Some("parsing"));
}

/// Verify tool errors outrank lint failures and report counts remain idempotent.
#[test]
fn report_aggregation_and_exit_precedence() {
    let analysis = analyze_source(
        b"fn example() {}",
        "standalone.rs",
        &AnalysisSettings {
            strict: true,
            ..AnalysisSettings::default()
        },
        &fixture_registry(),
    );
    let mut report = Report::new(1);
    report.summary.analyzed_files = 1;
    report.diagnostics = analysis.diagnostics;
    report.finish();
    assert_eq!(report.exit_code(), 1);
    assert!(report.summary.analysis_completed);
    report.diagnostics.push(evenflow::model::Diagnostic::error(
        Some("z.rs".into()),
        "input",
        "fixture failure",
        None,
    ));
    report.finish();
    report.finish();
    assert_eq!(report.exit_code(), 2);
    assert!(!report.summary.analysis_completed);
    assert_eq!(report.summary.counts_by_id["XX001"], 1);
    assert_eq!(report.summary.counts_by_id["EF000"], 1);
    assert_eq!(report.diagnostics[0].kind, DiagnosticKind::Error);
}

/// Verify original byte spans become Unicode columns with CRLF kept in place.
#[test]
fn unicode_locations_use_original_source() {
    let text = "// π\r\nfn example() { let π = 1; }";
    let offset = text.find("let").expect("fixture includes a statement");
    let range = evenflow::model::SourceRange::from_span(text, ByteSpan::new(offset, offset + 3));
    assert_eq!((range.start.line, range.start.column), (2, 16));
}
