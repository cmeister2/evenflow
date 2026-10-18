//! Discover and deserialize one project configuration without ancestor merging.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::{Diagnostic, Level};

/// The configuration filename searched for in each working-directory ancestor.
const CONFIG_FILENAME: &str = "evenflow.toml";

/// Validated configuration shape together with the root that gives paths meaning.
pub(crate) struct LoadedConfig {
    /// Canonical directory containing the configuration, or the working directory.
    pub(crate) root: PathBuf,
    /// Optional canonical configuration location for reporting invalid settings.
    pub(crate) path: Option<PathBuf>,
    /// Slash-separated exclusion patterns relative to the scan root.
    pub(crate) exclude: Vec<String>,
    /// Whether test paths and definitely test-only items are included.
    pub(crate) include_tests: bool,
    /// Explicit rule levels; omitted rules retain their registered defaults.
    pub(crate) rules: BTreeMap<String, Level>,
    /// Rule-defined typed option tables, validated by the registry.
    pub(crate) options: BTreeMap<String, toml::Table>,
}

/// The strictly deserialized contents of an optional configuration file.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct FileConfig {
    /// Additional exclusion patterns relative to the configuration directory.
    exclude: Vec<String>,
    /// Opt into test source paths and test-only syntax items.
    include_tests: bool,
    /// Per-rule overrides expressed as the registered level enum.
    rules: BTreeMap<String, Level>,
    /// Tables whose keys and value types belong to their registered rules.
    options: BTreeMap<String, toml::Table>,
}

/// Load the nearest configuration and retain its directory as the scan root.
///
/// # Errors
///
/// Returns an input diagnostic for an inaccessible working directory, or a
/// configuration diagnostic when discovering, reading, or decoding settings fails.
pub(crate) fn load(working_directory: &Path) -> Result<LoadedConfig, Box<Diagnostic>> {
    let working_directory = fs::canonicalize(working_directory).map_err(|error| {
        Box::new(Diagnostic::error(
            None,
            "input",
            format!("cannot resolve working directory: {error}"),
            None,
        ))
    })?;
    for ancestor in working_directory.ancestors() {
        // Select one nearest configuration so a child project's policy never
        // silently inherits a different ancestor's rule settings.
        let candidate = ancestor.join(CONFIG_FILENAME);
        let metadata = match fs::metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(configuration_error(format!(
                    "cannot inspect {}: {error}",
                    candidate.display()
                )));
            }
        };
        // Once the nearest candidate exists, report its own errors instead of
        // falling back to a different ancestor's configuration.
        if !metadata.is_file() {
            return Err(configuration_error(format!(
                "{} must be a regular file",
                candidate.display()
            )));
        }
        let source = fs::read_to_string(&candidate).map_err(|error| {
            configuration_error(format!("cannot read {}: {error}", candidate.display()))
        })?;
        let contents: FileConfig = toml::from_str(&source).map_err(|error| {
            configuration_error(format!("invalid {}: {error}", candidate.display()))
        })?;
        return Ok(LoadedConfig {
            root: ancestor.to_path_buf(),
            path: Some(candidate),
            exclude: contents.exclude,
            include_tests: contents.include_tests,
            rules: contents.rules,
            options: contents.options,
        });
    }
    Ok(LoadedConfig {
        root: working_directory,
        path: None,
        exclude: Vec::new(),
        include_tests: false,
        rules: BTreeMap::new(),
        options: BTreeMap::new(),
    })
}

/// Construct an unlocated framework diagnostic for invalid project settings.
fn configuration_error(message: String) -> Box<Diagnostic> {
    Box::new(Diagnostic::error(None, "configuration", message, None))
}
