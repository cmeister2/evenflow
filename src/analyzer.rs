//! Pure analysis orchestration, local suppressions, and enforcement policy.

use crate::model::{
    Analysis, ByteSpan, Diagnostic, DiagnosticKind, Level, Position, Severity, SourceRange,
};
use crate::registry::{Finding, Registry};
use crate::source::{Comment, SourceContext};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Effective source-analysis settings independent of filesystem configuration.
#[derive(Clone, Debug, Default)]
pub struct AnalysisSettings {
    /// Include otherwise excluded test items in source analysis.
    pub include_tests: bool,
    /// Promote enabled ordinary warnings to error-level findings.
    pub strict: bool,
    /// Per-rule enforcement overrides.
    pub levels: BTreeMap<String, Level>,
    /// Validated rule-local option tables.
    pub options: BTreeMap<String, toml::Table>,
}

/// A valid reasoned suppression attached through its original comment span.
struct Directive {
    /// The only rule this directive can suppress.
    rule: String,
    /// Exact original comment range used for target association.
    span: ByteSpan,
}

/// Analyze one UTF-8 source buffer without filesystem discovery or output.
///
/// Syntax and rule failures are returned as structured diagnostics rather than
/// silently producing an empty successful scan.
pub fn analyze_source(
    source: &[u8],
    path: &str,
    settings: &AnalysisSettings,
    registry: &Registry,
) -> Analysis {
    let normalized_path = path.replace('\\', "/");
    let path = normalized_path.as_str();
    if let Err(message) = registry.validate_settings(&settings.levels, &settings.options) {
        return Analysis {
            diagnostics: vec![Diagnostic::error(
                Some(path.into()),
                "configuration",
                message,
                None,
            )],
            completed: false,
        };
    }
    let context = match SourceContext::parse(path, source, settings.include_tests) {
        Ok(context) => context,
        Err(issue) => {
            let range = std::str::from_utf8(source)
                .ok()
                .and_then(|text| issue.span.map(|span| SourceRange::from_span(text, span)));
            return Analysis {
                diagnostics: vec![Diagnostic::error(
                    Some(path.into()),
                    &issue.category,
                    issue.message,
                    range,
                )],
                completed: false,
            };
        }
    };
    let mut diagnostics = Vec::new();
    let directives = collect_directives(&context, registry, &mut diagnostics);
    let mut used = BTreeSet::new();

    for rule in registry.rules() {
        // Run each enabled rule through one contract. Severity and suppression are
        // applied afterward so rule code remains independent of rollout policy.
        let definition = rule.definition();
        let level = settings
            .levels
            .get(definition.id)
            .copied()
            .unwrap_or(definition.default_level);
        if level == Level::Off {
            continue;
        }
        let empty_options = toml::Table::new();
        let options = settings
            .options
            .get(definition.id)
            .unwrap_or(&empty_options);
        let checked = catch_unwind(AssertUnwindSafe(|| rule.check(&context, options)));
        let findings = match checked {
            Ok(Ok(findings)) => findings,
            Ok(Err(message)) => {
                // Preserve the rule identity and continue independent checks
                // while marking this source file's coverage incomplete.
                diagnostics.push(Diagnostic::error(
                    Some(path.into()),
                    "internal-rule",
                    format!("rule {} failed: {message}", definition.id),
                    None,
                ));
                continue;
            }
            Err(_) => {
                // Convert an unexpected rule panic into the same structured
                // failure contract without pretending this scan completed.
                diagnostics.push(Diagnostic::error(
                    Some(path.into()),
                    "internal-rule",
                    format!("rule {} panicked", definition.id),
                    None,
                ));
                continue;
            }
        };
        let severity = if settings.strict || level == Level::Error {
            Severity::Error
        } else {
            Severity::Warning
        };
        apply_findings(
            &context,
            definition.id,
            severity,
            findings,
            &directives,
            &mut used,
            &mut diagnostics,
        );
    }

    // Only a successfully analyzed file can establish that a directive is
    // unused. Existing failures must not cause speculative secondary errors.
    if !diagnostics
        .iter()
        .any(|item| item.kind == DiagnosticKind::Error)
    {
        for (index, directive) in directives.iter().enumerate() {
            // Keep known disabled rules inactive while checking every enabled
            // directive against the findings from this completed source scan.
            let Some(rule) = registry.get(&directive.rule) else {
                continue;
            };
            let definition = rule.definition();
            let level = settings
                .levels
                .get(definition.id)
                .copied()
                .unwrap_or(definition.default_level);
            if level != Level::Off && !used.contains(&index) {
                diagnostics.push(Diagnostic::error(
                    Some(path.into()),
                    "suppression",
                    "unused or misplaced suppression directive",
                    Some(source_range(&context, directive.span)),
                ));
            }
        }
    }
    // Coverage remains explicit even in strict mode and even when lint rules
    // are disabled; completing a scan does not imply macro expansion coverage.
    for span in context.coverage_gaps() {
        diagnostics.push(Diagnostic {
            path: Some(path.into()),
            rule: "EF100".into(),
            kind: DiagnosticKind::Coverage,
            severity: Severity::Note,
            message: "macro may contain executable bodies outside source analysis coverage".into(),
            range: Some(source_range(&context, span)),
            category: None,
        });
    }
    let completed = !diagnostics
        .iter()
        .any(|item| item.kind == DiagnosticKind::Error);
    Analysis {
        diagnostics,
        completed,
    }
}

/// Parse active ordinary comments into directives and structured errors.
fn collect_directives(
    context: &SourceContext,
    registry: &Registry,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Directive> {
    let mut directives = Vec::new();
    for comment in &context.comments {
        // Only recognized comment tokens in analyzed source can activate parsing;
        // prose mentions, doc comments, and opaque macro arguments stay ordinary.
        if comment.is_doc || !comment.in_scope {
            continue;
        }
        let content = comment_content(&comment.text);
        if !content.starts_with("evenflow:") {
            continue;
        }
        match parse_directive(comment, content, registry) {
            Ok(directive) => directives.push(directive),
            Err(message) => diagnostics.push(Diagnostic::error(
                Some(context.path.clone()),
                "suppression",
                message,
                Some(source_range(context, comment.span)),
            )),
        }
    }
    directives
}

/// Strip ordinary comment delimiters while retaining the directive's content.
fn comment_content(text: &str) -> &str {
    let text = text.trim();
    if let Some(content) = text.strip_prefix("//") {
        return content.trim();
    }
    text.strip_prefix("/*")
        .and_then(|content| content.strip_suffix("*/"))
        .unwrap_or(text)
        .trim()
}

/// Validate the single supported directive grammar and its registered rule ID.
///
/// # Errors
/// Returns an explanation for trailing, malformed, unknown, or reasonless input.
fn parse_directive(
    comment: &Comment,
    content: &str,
    registry: &Registry,
) -> Result<Directive, String> {
    if !comment.standalone {
        return Err("suppression directive must be a standalone comment".into());
    }
    let Some(body) = content
        .strip_prefix("evenflow:")
        .map(str::trim_start)
        .and_then(|body| body.strip_prefix("allow("))
    else {
        return Err(
            "malformed suppression directive; expected evenflow: allow(RULE_ID): reason".into(),
        );
    };
    let Some((id, reason)) = body.split_once("):") else {
        return Err("malformed suppression directive; expected a rule ID and reason".into());
    };
    if id.is_empty() || id.trim() != id || registry.get(id).is_none() {
        return Err("unknown rule ID in suppression directive".into());
    }
    if reason.trim().is_empty() {
        return Err("suppression directive requires a non-empty reason".into());
    }
    Ok(Directive {
        rule: id.into(),
        span: comment.span,
    })
}

/// Suppress only exact annotated targets before converting findings to output.
fn apply_findings(
    context: &SourceContext,
    id: &str,
    severity: Severity,
    findings: Vec<Finding>,
    directives: &[Directive],
    used: &mut BTreeSet<usize>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut suppressed_targets = BTreeSet::new();
    // Identify target identities before emitting any findings so a directive
    // consistently handles multiple findings belonging to that exact target.
    for finding in &findings {
        for (index, directive) in directives.iter().enumerate() {
            if directive.rule == id && finding.annotations.contains(&directive.span) {
                used.insert(index);
                suppressed_targets.insert(finding.target);
            }
        }
    }
    for finding in findings {
        if suppressed_targets.contains(&finding.target) {
            continue;
        }
        diagnostics.push(Diagnostic {
            path: Some(context.path.clone()),
            rule: id.into(),
            kind: DiagnosticKind::Lint,
            severity,
            message: finding.message,
            range: Some(source_range(context, finding.span)),
            category: None,
        });
    }
}

/// Translate finding offsets using the file's indexed original source lines.
fn source_range(context: &SourceContext, span: ByteSpan) -> SourceRange {
    let (start_line, start_column) = context.location(span.start);
    let (end_line, end_column) = context.location(span.end);
    SourceRange {
        start: Position {
            line: start_line,
            column: start_column,
        },
        end: Position {
            line: end_line,
            column: end_column,
        },
    }
}
