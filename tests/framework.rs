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
    assert_eq!(bundled.definitions().len(), 1);
    assert_eq!(bundled.definitions()[0].id, "FC001");
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
