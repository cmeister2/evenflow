//! Preserve representative corpus behavior independently of the implementation source.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use evenflow::analyzer::{AnalysisSettings, analyze_source};
use evenflow::model::{Diagnostic, DiagnosticKind, Report};
use evenflow::registry::Registry;
use tempfile::TempDir;

/// Representative source samples with stable normalized paths and source contents.
const CORPUS: [(&str, &str); 3] = [
    ("macros.rs", include_str!("fixtures/corpus/macros.rs")),
    ("queue.rs", include_str!("fixtures/corpus/queue.rs")),
    ("workflow.rs", include_str!("fixtures/corpus/workflow.rs")),
];

/// Analyze every corpus input and normalize its combined report like the runner.
fn analyze_corpus(files: &[(&str, &str)]) -> Report {
    let registry = Registry::bundled().expect("bundled metadata must be valid");
    let mut report = Report::new(files.len());
    for (path, source) in files {
        let analysis = analyze_source(
            source.as_bytes(),
            path,
            &AnalysisSettings::default(),
            &registry,
        );
        assert!(analysis.completed, "{path}: {:?}", analysis.diagnostics);
        report.summary.analyzed_files += 1;
        report.diagnostics.extend(analysis.diagnostics);
    }
    report.finish();
    report
}

/// Extract path and rule identities without coupling to incidental source offsets.
fn identities(diagnostics: &[Diagnostic]) -> Vec<(String, String)> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.path.clone().expect("corpus finding has a path"),
                diagnostic.rule.clone(),
            )
        })
        .collect()
}

/// The corpus contains three actionable explanations and two known macro gaps.
#[test]
fn representative_corpus_has_useful_findings_and_clean_cases() {
    let report = analyze_corpus(&CORPUS);
    assert_eq!(report.exit_code(), 0);
    assert!(report.summary.analysis_completed);
    assert_eq!(
        report.summary.counts_by_id,
        BTreeMap::from([("EF100".to_owned(), 2), ("FC001".to_owned(), 3)])
    );
    assert_eq!(
        report.summary.counts_by_kind,
        BTreeMap::from([("coverage".to_owned(), 2), ("lint".to_owned(), 3)])
    );
    assert!(report.diagnostics.iter().all(|diagnostic| {
        let expected_path = if diagnostic.kind == DiagnosticKind::Lint {
            "queue.rs"
        } else {
            "macros.rs"
        };
        diagnostic.path.as_deref() == Some(expected_path)
    }));
}

/// Shared in-memory analysis and executable discovery return exactly the same report.
#[test]
fn corpus_cli_agrees_with_source_analysis_and_sorts_inputs() {
    let project = TempDir::new().expect("create corpus project");
    for (path, source) in CORPUS {
        fs::write(project.path().join(path), source).expect("write corpus file");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_evenflow"))
        .args([
            "check",
            "workflow.rs",
            "queue.rs",
            "macros.rs",
            "--format",
            "json",
        ])
        .current_dir(project.path())
        .output()
        .expect("execute corpus check");
    assert_eq!(output.status.code(), Some(0));
    let cli: Report = serde_json::from_slice(&output.stdout).expect("decode corpus report");
    assert_eq!(cli, analyze_corpus(&CORPUS));
    assert_eq!(
        identities(&cli.diagnostics),
        vec![
            ("macros.rs".to_owned(), "EF100".to_owned()),
            ("macros.rs".to_owned(), "EF100".to_owned()),
            ("queue.rs".to_owned(), "FC001".to_owned()),
            ("queue.rs".to_owned(), "FC001".to_owned()),
            ("queue.rs".to_owned(), "FC001".to_owned()),
        ]
    );
}

/// Format an isolated source file using the documented contributor Rust toolchain.
fn format_source(path: &Path) {
    let formatter = std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let output = Command::new(formatter)
        .args(["--edition", "2024"])
        .arg(path)
        .output()
        .expect("rustfmt must be installed for contributor checks");
    assert!(
        output.status.success(),
        "rustfmt failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Formatting compact control flow preserves which structural boundaries need explanations.
#[test]
fn rustfmt_layout_changes_preserve_rule_conclusions() {
    let project = TempDir::new().expect("create formatting fixture");
    let compact = "fn work(){if ready(){let first=load();validate(&first);save(first);}\n// This explanation belongs to the following loop.\nfor item in items(){let decoded=decode(item);validate(&decoded);save(decoded);}}\n";
    let path = project.path().join("layout.rs");
    fs::write(&path, compact).expect("write unformatted fixture");
    let before = analyze_corpus(&[("layout.rs", compact)]);
    assert_eq!(before.summary.counts_by_id.get("FC001"), Some(&1));
    format_source(&path);
    let formatted = fs::read_to_string(path).expect("read formatted fixture");
    assert_ne!(compact, formatted);
    let after = analyze_corpus(&[("layout.rs", &formatted)]);
    assert_eq!(
        identities(&before.diagnostics),
        identities(&after.diagnostics)
    );
    assert_eq!(before.summary.counts_by_id, after.summary.counts_by_id);
    assert_eq!(
        before.summary.analysis_completed,
        after.summary.analysis_completed
    );
}

/// A comment-heavy scan exercises indexing and ownership at useful corpus scale.
#[test]
fn thousands_of_comments_preserve_body_ownership_and_complete() {
    let mut source = String::from("fn many_boundaries() {\n");
    for index in 0..2_000 {
        source.push_str(&format!(
            "// Keep operation {index} ordered before its commit marker.\nif ready() {{ let value = load(); save(value); commit(); }}\n"
        ));
    }
    source.push_str("if final_ready() { finish(); commit(); publish(); }\n}\n");
    let report = analyze_corpus(&[("comment_heavy.rs", &source)]);
    assert!(report.summary.analysis_completed);
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].rule, "FC001");
    assert_eq!(report.diagnostics[0].range.unwrap().start.line, 4_002);
    assert_eq!(report.summary.analyzed_files, 1);
}
