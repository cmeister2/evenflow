//! Select source files and apply the in-memory analyzer without output side effects.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use walkdir::{DirEntry, WalkDir};

use crate::analyzer::{AnalysisSettings, analyze_source};
use crate::config::{self, LoadedConfig};
use crate::model::{Diagnostic, Report};
use crate::registry::Registry;

/// Selected canonical files indexed by normalized root-relative diagnostic paths.
struct Selection {
    /// Canonical paths sorted by the corresponding displayed path.
    files: BTreeMap<String, PathBuf>,
    /// Discovery failures that must not prevent checking independent inputs.
    diagnostics: Vec<Diagnostic>,
}

/// Analyze selected files using one configuration and the bundled rule registry.
pub(crate) fn run(working_directory: &Path, inputs: &[PathBuf], strict: bool) -> Report {
    let registry = match Registry::bundled() {
        Ok(registry) => registry,
        Err(message) => {
            return failure(Diagnostic::error(None, "registry", message, None));
        }
    };
    let configuration = match config::load(working_directory) {
        Ok(configuration) => configuration,
        Err(diagnostic) => return failure(*diagnostic),
    };
    if let Err(message) = registry.validate_settings(&configuration.rules, &configuration.options) {
        return failure(Diagnostic::error(
            configuration
                .path
                .as_ref()
                .map(|_| "evenflow.toml".to_owned()),
            "configuration",
            message,
            None,
        ));
    }
    let exclusions = match exclusions(&configuration.exclude) {
        Ok(exclusions) => exclusions,
        Err(message) => {
            return failure(Diagnostic::error(
                configuration
                    .path
                    .as_ref()
                    .map(|_| "evenflow.toml".to_owned()),
                "configuration",
                message,
                None,
            ));
        }
    };
    let selection = discover(working_directory, inputs, &configuration, &exclusions);
    let mut report = Report::new(selection.files.len());
    report.diagnostics = selection.diagnostics;
    let settings = AnalysisSettings {
        include_tests: configuration.include_tests,
        strict,
        levels: configuration.rules,
        options: configuration.options,
    };
    for (path, file) in selection.files {
        // A failed file remains selected, and independent files still contribute
        // their findings so an operational error cannot conceal useful results.
        match fs::read(&file) {
            Ok(source) => {
                // Only completed analyses increment the success count;
                // findings and failures both remain in the final report.
                let analysis = analyze_source(&source, &path, &settings, &registry);
                if analysis.completed {
                    report.summary.analyzed_files += 1;
                }
                report.diagnostics.extend(analysis.diagnostics);
            }
            Err(error) => report.diagnostics.push(Diagnostic::error(
                Some(path),
                "input",
                format!("cannot read Rust source: {error}"),
                None,
            )),
        }
    }
    report.finish();
    report
}

/// Return a completed error document when work cannot reach file discovery.
fn failure(diagnostic: Diagnostic) -> Report {
    let mut report = Report::new(0);
    report.diagnostics.push(diagnostic);
    report.finish();
    report
}

/// Compile root-relative globs with slash-sensitive matching and double stars.
///
/// # Errors
///
/// Returns a message identifying an invalid pattern or unsupported path syntax.
fn exclusions(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        // Reject nonportable paths before compiling each pattern with the same
        // root-relative semantics used during file discovery.
        // Configuration paths use a portable root-relative syntax, avoiding
        // platform-dependent absolute paths and traversal outside the root.
        if pattern.starts_with('/')
            || pattern.contains('\\')
            || pattern.split('/').any(|part| part == "..")
        {
            return Err(format!(
                "exclusion must be a slash-separated root-relative glob: {pattern}"
            ));
        }
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|error| format!("invalid exclusion {pattern:?}: {error}"))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| format!("invalid exclusions: {error}"))
}

/// Discover explicit files or recursively selected directories in stable order.
fn discover(
    working_directory: &Path,
    inputs: &[PathBuf],
    configuration: &LoadedConfig,
    exclusions: &GlobSet,
) -> Selection {
    let mut selection = Selection {
        files: BTreeMap::new(),
        diagnostics: Vec::new(),
    };
    let default_input = vec![PathBuf::from(".")];
    let inputs = if inputs.is_empty() {
        &default_input
    } else {
        inputs
    };
    for input in inputs {
        // Resolve each explicit selection independently so one invalid input does
        // not prevent analyzing other files in the same invocation.
        let original = if input.is_absolute() {
            input.clone()
        } else {
            working_directory.join(input)
        };
        let canonical = match fs::canonicalize(&original) {
            Ok(path) => path,
            Err(error) => {
                selection.diagnostics.push(input_error(format!(
                    "cannot resolve {}: {error}",
                    original.display()
                )));
                continue;
            }
        };
        // Root containment is checked even for explicit inputs before any walk
        // or source read, including when a symlink points outside the project.
        if !canonical.starts_with(&configuration.root) {
            selection.diagnostics.push(input_error(format!(
                "input resolves outside scan root: {}",
                original.display()
            )));
            continue;
        }
        let metadata = match fs::metadata(&original) {
            Ok(metadata) => metadata,
            Err(error) => {
                selection.diagnostics.push(input_error(format!(
                    "cannot inspect {}: {error}",
                    original.display()
                )));
                continue;
            }
        };
        if let Some(link) = directory_symlink(&original, metadata.is_dir()) {
            selection.diagnostics.push(input_error(format!(
                "directory symlinks are not followed: {}",
                link.display()
            )));
            continue;
        }
        if metadata.is_dir() {
            walk_directory(&canonical, configuration, exclusions, &mut selection);
        } else if metadata.is_file() {
            select_file(&original, configuration, exclusions, &mut selection, true);
        } else {
            selection.diagnostics.push(input_error(format!(
                "input is not a regular file or directory: {}",
                original.display()
            )));
        }
    }
    if selection.files.is_empty() {
        selection.diagnostics.push(input_error(
            "selection contains no Rust files after exclusions".to_owned(),
        ));
    }
    selection
}

/// Walk a real directory while pruning excluded subtrees and directory symlinks.
fn walk_directory(
    directory: &Path,
    configuration: &LoadedConfig,
    exclusions: &GlobSet,
    selection: &mut Selection,
) {
    let entries = WalkDir::new(directory)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !excluded(entry.path(), configuration, exclusions));
    for entry in entries {
        match entry {
            Ok(entry) if selectable_entry(&entry) => {
                select_file(entry.path(), configuration, exclusions, selection, false);
            }
            Ok(_) => {}
            Err(error) => selection.diagnostics.push(input_error(format!(
                "cannot inspect selected directory: {error}"
            ))),
        }
    }
}

/// Identify regular source files and file symlinks without following directories.
fn selectable_entry(entry: &DirEntry) -> bool {
    entry.file_type().is_file() || (entry.file_type().is_symlink() && !entry.path().is_dir())
}

/// Find directory symlinks in an explicit input, including its parent components.
fn directory_symlink(path: &Path, is_directory: bool) -> Option<PathBuf> {
    let first = if is_directory {
        Some(path)
    } else {
        path.parent()
    }?;
    first
        .ancestors()
        .find(|ancestor| {
            fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink())
        })
        .map(Path::to_path_buf)
}

/// Add one canonical Rust file after applying logical and resolved exclusions.
fn select_file(
    original: &Path,
    configuration: &LoadedConfig,
    exclusions: &GlobSet,
    selection: &mut Selection,
    explicit: bool,
) {
    if original
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("rs")
    {
        if explicit {
            selection.diagnostics.push(input_error(format!(
                "explicit input is not a Rust .rs file: {}",
                original.display()
            )));
        }
        return;
    }
    let canonical = match fs::canonicalize(original) {
        Ok(path) => path,
        Err(error) => {
            selection.diagnostics.push(input_error(format!(
                "cannot resolve {}: {error}",
                original.display()
            )));
            return;
        }
    };
    if !canonical.starts_with(&configuration.root) {
        selection.diagnostics.push(input_error(format!(
            "source resolves outside scan root: {}",
            original.display()
        )));
        return;
    }
    // Explicit unreadable inputs remain errors even when exclusion prevents
    // parsing their content; discovered excluded files are simply skipped.
    if excluded(original, configuration, exclusions)
        || excluded(&canonical, configuration, exclusions)
    {
        if explicit && let Err(error) = fs::File::open(original) {
            selection.diagnostics.push(input_error(format!(
                "cannot read explicitly requested Rust source {}: {error}",
                original.display()
            )));
        }
        return;
    }
    match normalized_path(&canonical, &configuration.root) {
        Some(path) => {
            selection.files.insert(path, canonical);
        }
        None => selection.diagnostics.push(input_error(format!(
            "source path is not valid UTF-8: {}",
            original.display()
        ))),
    }
}

/// Apply unconditional build exclusions, optional test exclusions, and user globs.
fn excluded(path: &Path, configuration: &LoadedConfig, exclusions: &GlobSet) -> bool {
    let Ok(relative) = path.strip_prefix(&configuration.root) else {
        return false;
    };
    for component in relative.components() {
        let Component::Normal(component) = component else {
            continue;
        };
        if component == "target"
            || component == "generated"
            || (!configuration.include_tests && component == "tests")
        {
            return true;
        }
    }
    let name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name.ends_with("_autogenerated.rs")
        || (!configuration.include_tests && name == "tests.rs")
        || exclusions.is_match(relative)
}

/// Convert an in-root path to portable slash-separated UTF-8 components.
fn normalized_path(path: &Path, root: &Path) -> Option<String> {
    let components: Option<Vec<_>> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect();
    Some(components?.join("/"))
}

/// Construct a discovery failure without inventing a source-code location.
fn input_error(message: String) -> Diagnostic {
    Diagnostic::error(None, "input", message, None)
}
