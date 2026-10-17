//! Exercise discovery, configuration, diagnostics, and exit policy through the executable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

/// Distinguish parallel fixture directories within one test process.
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// An isolated project directory deleted after its test finishes.
struct Project {
    /// Absolute directory passed to the command as its working directory.
    root: PathBuf,
}

impl Project {
    /// Create an isolated directory outside any project configuration.
    fn new() -> Self {
        let number = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("evenflow-cli-{}-{number}", std::process::id()));
        fs::create_dir_all(&root).expect("create temporary project");
        Self { root }
    }

    /// Write a project file, creating any parent directories first.
    fn write(&self, path: &str, content: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().expect("fixture file has parent"))
            .expect("create fixture parent");
        fs::write(path, content).expect("write fixture");
    }

    /// Run the built executable from this project directory.
    fn run(&self, arguments: &[&str]) -> Output {
        self.run_from(&self.root, arguments)
    }

    /// Run the executable from a particular directory under the fixture.
    fn run_from(&self, working_directory: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_evenflow"))
            .args(arguments)
            .current_dir(working_directory)
            .output()
            .expect("execute evenflow")
    }
}

impl Drop for Project {
    /// Remove temporary fixtures without masking a prior assertion failure.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A missing explanation in a qualifying branch produces one FC001 finding.
const MISSING_COMMENT: &str = "fn work() { if ready() { let a = 1; let b = 2; consume(a, b); } }\n";

/// Decode an output document and retain command errors in assertion context.
fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON ({error}): stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Check that a JSON response contains a diagnostic with the requested ID.
fn has_id(document: &Value, id: &str) -> bool {
    document["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .any(|diagnostic| diagnostic["rule"] == id)
}

/// Warnings are advisory until the caller enables strict enforcement.
#[test]
fn strict_promotes_warnings_and_reports_json() {
    let project = Project::new();
    project.write("src/main.rs", MISSING_COMMENT);
    let advisory = project.run(&["check", "--format", "json"]);
    assert_eq!(advisory.status.code(), Some(0));
    assert!(has_id(&json(&advisory), "FC001"));
    let strict = project.run(&["check", ".", "--strict", "--format", "json"]);
    assert_eq!(strict.status.code(), Some(1));
    assert!(has_id(&json(&strict), "FC001"));
}

/// Strict mode retains the configured disabled state of a rule.
#[test]
fn off_rule_remains_off_in_strict_mode() {
    let project = Project::new();
    project.write("src/main.rs", MISSING_COMMENT);
    project.write("evenflow.toml", "[rules]\nFC001 = \"off\"\n");
    let output = project.run(&["check", "--strict", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(!has_id(&json(&output), "FC001"));
}

/// Overlapping file and directory inputs select a file exactly once.
#[test]
fn overlapping_inputs_are_deduplicated() {
    let project = Project::new();
    project.write("src/a.rs", MISSING_COMMENT);
    let output = project.run(&["check", ".", "src", "src/a.rs", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let document = json(&output);
    assert_eq!(document["summary"]["selected_files"], 1);
    assert_eq!(document["summary"]["analyzed_files"], 1);
    assert_eq!(document["diagnostics"].as_array().unwrap().len(), 1);
}

/// Configuration discovery uses the nearest ancestor and root-relative globs.
#[test]
fn nearest_configuration_sets_scan_root_without_merging() {
    let project = Project::new();
    project.write("evenflow.toml", "[rules]\nFC001 = \"off\"\n");
    project.write(
        "nested/evenflow.toml",
        "exclude = [\"src/skip/**\"]\n[rules]\nFC001 = \"error\"\n",
    );
    project.write("nested/src/skip/ignored.rs", "this is not Rust");
    project.write("nested/src/main.rs", MISSING_COMMENT);
    let output = project.run_from(
        &project.root.join("nested/src"),
        &["check", "--format", "json"],
    );
    assert_eq!(output.status.code(), Some(1));
    let document = json(&output);
    assert_eq!(document["summary"]["selected_files"], 1);
    assert_eq!(document["diagnostics"][0]["path"], "src/main.rs");
}

/// A malformed nearest configuration cannot fall back to a valid ancestor.
#[test]
fn invalid_nearest_configuration_is_not_ignored() {
    let project = Project::new();
    project.write("evenflow.toml", "[rules]\nFC001 = \"off\"\n");
    project.write("nested/evenflow.toml", "include-tests = \"wrong type\"\n");
    project.write("nested/main.rs", "fn main() {}\n");
    let output = project.run_from(&project.root.join("nested"), &["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let document = json(&output);
    assert!(has_id(&document, "EF000"));
    assert_eq!(document["summary"]["selected_files"], 0);
}

/// Build output, generated code, and test paths are skipped before parsing.
#[test]
fn default_path_exclusions_prevent_parsing_invalid_fixtures() {
    let project = Project::new();
    project.write("src/lib.rs", "fn production() {}\n");
    for path in [
        "target/broken.rs",
        "generated/broken.rs",
        "tests/broken.rs",
        "src/tests.rs",
        "src/bindings_autogenerated.rs",
    ] {
        project.write(path, "not valid Rust");
    }
    let output = project.run(&["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output)["summary"]["selected_files"], 1);
}

/// Test inclusion does not lift generated-code or build-output exclusions.
#[test]
fn include_tests_only_lifts_test_exclusions() {
    let project = Project::new();
    project.write("evenflow.toml", "include-tests = true\n");
    project.write("tests/check.rs", MISSING_COMMENT);
    project.write("target/broken.rs", "not Rust");
    project.write("generated/broken.rs", "not Rust");
    let output = project.run(&["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let document = json(&output);
    assert_eq!(document["summary"]["selected_files"], 1);
    assert!(has_id(&document, "FC001"));
}

/// Production items remain checked in a file containing both test mechanisms.
#[test]
fn inline_tests_do_not_hide_neighboring_production_code() {
    let project = Project::new();
    project.write(
        "src/lib.rs",
        "fn production() { if ready() { prepare(); save(); commit(); } }\n\
         #[cfg(test)] mod tests {\n\
             fn helper() { if ready() { prepare(); save(); commit(); } }\n\
         }\n\
         #[test] fn independent_test() { if ready() { prepare(); save(); commit(); } }\n",
    );
    let excluded = project.run(&["check", "--format", "json"]);
    assert_eq!(excluded.status.code(), Some(0));
    assert_eq!(json(&excluded)["summary"]["counts_by_id"]["FC001"], 1);
    project.write("evenflow.toml", "include-tests = true\n");
    let included = project.run(&["check", "--format", "json"]);
    assert_eq!(included.status.code(), Some(0));
    assert_eq!(json(&included)["summary"]["counts_by_id"]["FC001"], 3);
}

/// Double-star directory globs include the zero-directory case.
#[test]
fn double_star_globs_match_files_at_root_and_in_subdirectories() {
    let project = Project::new();
    project.write("evenflow.toml", "exclude = [\"**/skip.rs\"]\n");
    project.write("skip.rs", "not Rust");
    project.write("nested/skip.rs", "not Rust");
    project.write("keep.rs", "fn keep() {}\n");
    let output = project.run(&["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output)["summary"]["selected_files"], 1);
}

/// Unknown top-level configuration keys never silently change behavior.
#[test]
fn unknown_configuration_settings_fail_before_discovery() {
    let project = Project::new();
    project.write("evenflow.toml", "include_test = true\n");
    project.write("main.rs", "fn main() {}\n");
    let output = project.run(&["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let document = json(&output);
    assert!(has_id(&document, "EF000"));
    assert_eq!(document["summary"]["selected_files"], 0);
    assert_eq!(document["summary"]["analysis_completed"], false);
}

/// Disabled rules still reject unknown options and unknown IDs remain errors.
#[test]
fn invalid_rule_settings_fail_before_discovery() {
    for configuration in [
        "[rules]\nUNKNOWN = \"warning\"\n",
        "[rules]\nFC001 = \"quiet\"\n",
        "[rules]\nFC001 = \"off\"\n[options.FC001]\nthreshold = 99\n",
        "[options.UNKNOWN]\nenabled = true\n",
    ] {
        let project = Project::new();
        project.write("evenflow.toml", configuration);
        project.write("main.rs", "fn main() {}\n");
        let output = project.run(&["check", "--format", "json"]);
        assert_eq!(output.status.code(), Some(2), "{configuration}");
        let document = json(&output);
        assert!(has_id(&document, "EF000"));
        assert_eq!(document["summary"]["selected_files"], 0);
    }
}

/// Explicit invalid inputs and an empty final selection produce tool failures.
#[test]
fn missing_non_rust_and_empty_inputs_are_errors() {
    let project = Project::new();
    project.write("notes.txt", "not Rust");
    for arguments in [
        vec!["check", "missing.rs", "--format", "json"],
        vec!["check", "notes.txt", "--format", "json"],
        vec!["check", "--format", "json"],
    ] {
        let output = project.run(&arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(has_id(&json(&output), "EF000"));
    }
}

/// Independent files continue after one parsing failure and errors beat lints.
#[test]
fn file_failures_do_not_hide_independent_lints() {
    let project = Project::new();
    project.write("a_bad.rs", "fn broken( {\n");
    project.write("z_good.rs", MISSING_COMMENT);
    let output = project.run(&["check", "--strict", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let document = json(&output);
    assert!(has_id(&document, "EF000"));
    assert!(has_id(&document, "FC001"));
    assert_eq!(document["summary"]["selected_files"], 2);
    assert_eq!(document["summary"]["analysis_completed"], false);
}

/// File encoding failures are structured analysis errors.
#[test]
fn invalid_utf8_is_not_a_clean_scan() {
    let project = Project::new();
    fs::write(project.root.join("invalid.rs"), [0xff, 0xfe]).unwrap();
    let output = project.run(&["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(has_id(&json(&output), "EF000"));
}

/// Explicit unreadable source is a tool error while independent files still finish.
#[cfg(unix)]
#[test]
fn unreadable_explicit_files_fail_without_hiding_other_findings() {
    use std::os::unix::fs::PermissionsExt;

    let project = Project::new();
    project.write("private.rs", "fn private() {}\n");
    project.write("public.rs", MISSING_COMMENT);
    let private = project.root.join("private.rs");
    fs::set_permissions(&private, fs::Permissions::from_mode(0o0)).unwrap();
    if fs::read(&private).is_ok() {
        // Privileged test runners may bypass permission bits; the observable
        // unreadable-file behavior requires a process subject to those bits.
        return;
    }
    let output = project.run(&["check", "private.rs", "public.rs", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let document = json(&output);
    assert!(has_id(&document, "EF000"));
    assert!(has_id(&document, "FC001"));
    assert_eq!(document["summary"]["selected_files"], 2);
    assert_eq!(document["summary"]["analyzed_files"], 1);
    assert_eq!(document["summary"]["analysis_completed"], false);

    project.write("evenflow.toml", "exclude = [\"private.rs\"]\n");
    let excluded = project.run(&["check", "private.rs", "public.rs", "--format", "json"]);
    let document = json(&excluded);
    assert_eq!(excluded.status.code(), Some(2));
    assert!(has_id(&document, "EF000"));
    assert!(has_id(&document, "FC001"));
    assert_eq!(document["summary"]["selected_files"], 1);
}

/// Source findings are stable regardless of the order of command inputs.
#[test]
fn diagnostics_are_sorted_by_normalized_path() {
    let project = Project::new();
    project.write("z.rs", MISSING_COMMENT);
    project.write("a.rs", MISSING_COMMENT);
    let output = project.run(&["check", "z.rs", "a.rs", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let document = json(&output);
    assert_eq!(document["diagnostics"][0]["path"], "a.rs");
    assert_eq!(document["diagnostics"][1]["path"], "z.rs");
}

/// The rule catalog describes the shipped rule and excludes speculative rules.
#[test]
fn rules_lists_the_bundled_catalog() {
    let project = Project::new();
    let output = project.run(&["rules"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("FC001"));
    assert!(text.contains("warning"));
    assert!(!text.contains("FC002"));
}

/// Coverage notes remain advisory when strict enforcement is enabled.
#[test]
fn macro_coverage_notes_do_not_fail_strict_runs() {
    let project = Project::new();
    project.write("main.rs", "fn main() { custom!({ do_work(); }); }\n");
    let output = project.run(&["check", "--strict", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let document = json(&output);
    assert!(has_id(&document, "EF100"));
    assert_eq!(document["summary"]["analysis_completed"], true);
}

/// Human-readable output keeps operational failures on a separate stream.
#[test]
fn text_streams_separate_findings_and_operational_errors() {
    let project = Project::new();
    project.write("a.rs", MISSING_COMMENT);
    project.write("b.rs", "fn broken( {\n");
    let output = project.run(&["check"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stdout).contains("FC001"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("EF000"));
}

/// CLI argument errors retain the same valid JSON failure envelope as analysis.
#[test]
fn invalid_arguments_still_return_a_json_document() {
    let project = Project::new();
    let output = project.run(&["check", "--unknown-option", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let document = json(&output);
    assert!(has_id(&document, "EF000"));
    assert_eq!(document["summary"]["selected_files"], 0);
    assert_eq!(document["summary"]["analyzed_files"], 0);
    assert_eq!(document["summary"]["analysis_completed"], false);
}

/// Invalid exclusion syntax fails before selecting any project source.
#[test]
fn invalid_exclusion_patterns_are_configuration_errors() {
    let project = Project::new();
    project.write("evenflow.toml", "exclude = [\"[invalid\"]\n");
    project.write("main.rs", "fn main() {}\n");
    let output = project.run(&["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let document = json(&output);
    assert!(has_id(&document, "EF000"));
    assert_eq!(document["summary"]["selected_files"], 0);
}

/// A prebuilt executable scans source without finding a Rust toolchain on PATH.
#[test]
fn scanning_does_not_invoke_rust_or_cargo() {
    let project = Project::new();
    project.write("main.rs", "fn main() {}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_evenflow"))
        .args(["check", "--format", "json"])
        .current_dir(&project.root)
        .env("PATH", "/evenflow-no-programs-here")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output)["summary"]["analyzed_files"], 1);
}

/// Canonical paths deduplicate an in-root source file and its file symlink.
#[cfg(unix)]
#[test]
fn file_symlink_aliases_do_not_duplicate_analysis() {
    use std::os::unix::fs::symlink;

    let project = Project::new();
    project.write("main.rs", MISSING_COMMENT);
    symlink(project.root.join("main.rs"), project.root.join("alias.rs")).unwrap();
    let output = project.run(&["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let document = json(&output);
    assert_eq!(document["summary"]["selected_files"], 1);
    assert_eq!(document["diagnostics"].as_array().unwrap().len(), 1);
}

/// Directory symlinks are not traversed and escaping file symlinks fail visibly.
#[cfg(unix)]
#[test]
fn symlink_discovery_cannot_escape_the_scan_root() {
    use std::os::unix::fs::symlink;

    let project = Project::new();
    let outside = Project::new();
    outside.write("external.rs", MISSING_COMMENT);
    project.write("main.rs", "fn main() {}\n");
    symlink(&outside.root, project.root.join("linked_directory")).unwrap();
    let output = project.run(&["check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output)["summary"]["selected_files"], 1);
    symlink(
        outside.root.join("external.rs"),
        project.root.join("escaped.rs"),
    )
    .unwrap();
    let escaped = project.run(&["check", "escaped.rs", "--format", "json"]);
    assert_eq!(escaped.status.code(), Some(2));
    assert!(has_id(&json(&escaped), "EF000"));
}
