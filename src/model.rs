//! Owned diagnostics and execution results shared by analysis and presentation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An original-source byte interval, with an exclusive end offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteSpan {
    /// First byte of the interval.
    pub start: usize,
    /// First byte after the interval.
    pub end: usize,
}

impl ByteSpan {
    /// Construct an original-source interval.
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// Configured enforcement of an ordinary lint rule.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Do not execute the rule.
    Off,
    /// Report findings without making a normal scan fail.
    #[default]
    Warning,
    /// Report findings and fail a completed scan.
    Error,
}

impl Level {
    /// Return the stable configuration spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// The independent purpose of a diagnostic.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticKind {
    /// An opinionated rule violation.
    Lint,
    /// A visible boundary of supported analysis.
    Coverage,
    /// An operational or analysis failure.
    Error,
}

impl DiagnosticKind {
    /// Return the stable summary key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lint => "lint",
            Self::Coverage => "coverage",
            Self::Error => "error",
        }
    }
}

/// Effective presentation severity after applying enforcement policy.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational coverage boundary.
    Note,
    /// Advisory lint violation.
    Warning,
    /// Failing lint or tool error.
    Error,
}

impl Severity {
    /// Return the text-output spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A one-based source location using Unicode code-point columns.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Position {
    /// One-based line number.
    pub line: usize,
    /// One-based Unicode code-point column.
    pub column: usize,
}

/// The original source range for a diagnostic.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SourceRange {
    /// Inclusive beginning of the diagnostic range.
    pub start: Position,
    /// Exclusive end of the diagnostic range.
    pub end: Position,
}

impl SourceRange {
    /// Translate original byte offsets to one-based Unicode locations.
    pub fn from_span(source: &str, span: ByteSpan) -> Self {
        Self {
            start: position(source, span.start),
            end: position(source, span.end),
        }
    }
}

/// Convert a byte offset without assuming that externally supplied spans align.
fn position(source: &str, offset: usize) -> Position {
    let mut boundary = offset.min(source.len());
    while !source.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let prefix = &source[..boundary];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    Position { line, column }
}

/// A complete diagnostic suitable for text or JSON output.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    /// Normalized path relative to the scan root, when known.
    pub path: Option<String>,
    /// Stable rule or framework diagnostic ID.
    pub rule: String,
    /// Whether this is a lint, coverage note, or analysis failure.
    pub kind: DiagnosticKind,
    /// Effective diagnostic severity.
    pub severity: Severity,
    /// Actionable explanation of the finding or failure.
    pub message: String,
    /// Original source range, when known.
    pub range: Option<SourceRange>,
    /// Machine-readable category for a framework failure.
    pub category: Option<String>,
}

impl Diagnostic {
    /// Create an EF000 analysis or operational error.
    pub fn error(
        path: Option<String>,
        category: &str,
        message: impl Into<String>,
        range: Option<SourceRange>,
    ) -> Self {
        Self {
            path,
            rule: "EF000".into(),
            kind: DiagnosticKind::Error,
            severity: Severity::Error,
            message: message.into(),
            range,
            category: Some(category.into()),
        }
    }
}

/// A source-analysis failure before diagnostic rendering and policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisIssue {
    /// Stable failure category, such as parsing or encoding.
    pub category: String,
    /// Explanation of the failure.
    pub message: String,
    /// Original-source interval when the failure has a location.
    pub span: Option<ByteSpan>,
}

/// Results of analyzing one source buffer, independent of filesystem discovery.
#[derive(Clone, Debug, Default)]
pub struct Analysis {
    /// Lints, coverage notes, and failures for this source buffer.
    pub diagnostics: Vec<Diagnostic>,
    /// Whether the file completed analysis without tool errors.
    pub completed: bool,
}

/// Aggregate scan counts and completion status.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Summary {
    /// Number of deduplicated files selected after exclusions.
    pub selected_files: usize,
    /// Number of files whose analysis completed successfully.
    pub analyzed_files: usize,
    /// Diagnostic counts by rule or framework ID.
    pub counts_by_id: BTreeMap<String, usize>,
    /// Diagnostic counts by lint, coverage, or error kind.
    pub counts_by_kind: BTreeMap<String, usize>,
    /// Whether every selected file completed and no operational error occurred.
    pub analysis_completed: bool,
}

/// Versioned JSON report for a complete command invocation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Report {
    /// Version of this diagnostic schema.
    pub schema_version: u32,
    /// Deterministically ordered diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// File counts, diagnostic counts, and completion status.
    pub summary: Summary,
}

impl Report {
    /// Create an empty report with a known number of selected files.
    pub fn new(selected_files: usize) -> Self {
        Self {
            schema_version: 1,
            diagnostics: Vec::new(),
            summary: Summary {
                selected_files,
                ..Summary::default()
            },
        }
    }

    /// Sort diagnostics and derive aggregate counters and completion status.
    pub fn finish(&mut self) {
        self.diagnostics.sort_by(|left, right| {
            let location = |diagnostic: &Diagnostic| {
                diagnostic
                    .range
                    .map(|range| (range.start.line, range.start.column))
            };
            (left.range.is_some(), &left.path, location(left), &left.rule).cmp(&(
                right.range.is_some(),
                &right.path,
                location(right),
                &right.rule,
            ))
        });
        self.summary.counts_by_id.clear();
        self.summary.counts_by_kind.clear();
        // Rebuild counts from final diagnostics so repeated rendering cannot
        // accumulate values or hide an operational error behind lint findings.
        for diagnostic in &self.diagnostics {
            *self
                .summary
                .counts_by_id
                .entry(diagnostic.rule.clone())
                .or_default() += 1;
            *self
                .summary
                .counts_by_kind
                .entry(diagnostic.kind.as_str().into())
                .or_default() += 1;
        }
        self.summary.analysis_completed = self.summary.selected_files > 0
            && self.summary.analyzed_files == self.summary.selected_files
            && !self
                .diagnostics
                .iter()
                .any(|item| item.kind == DiagnosticKind::Error);
    }

    /// Return the documented scan exit code, giving tool errors precedence.
    pub fn exit_code(&self) -> i32 {
        if self
            .diagnostics
            .iter()
            .any(|item| item.kind == DiagnosticKind::Error)
        {
            return 2;
        }
        if self
            .diagnostics
            .iter()
            .any(|item| item.kind == DiagnosticKind::Lint && item.severity == Severity::Error)
        {
            return 1;
        }
        0
    }
}
