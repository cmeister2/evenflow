//! Parse command arguments and render shared reports as text or structured JSON.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::model::{Diagnostic, DiagnosticKind, Level, Report, Severity};
use crate::registry::{OptionKind, Registry};
use crate::runner;

/// Command-line arguments including the explicit subcommand.
#[derive(Parser)]
#[command(
    name = "evenflow",
    version,
    about = "Opinionated source-only Rust linting alongside Clippy"
)]
struct Arguments {
    /// Select checking or inspection of the bundled rule catalog.
    #[command(subcommand)]
    command: Command,
}

/// Commands supported by the first-release executable.
#[derive(Subcommand)]
enum Command {
    /// Check Rust files or directories without building the target project.
    Check {
        /// Rust source files or directories, relative to the working directory.
        paths: Vec<PathBuf>,
        /// Promote enabled lint warnings to errors for this run.
        #[arg(long)]
        strict: bool,
        /// Diagnostic output format, including structured failures.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// List bundled rule IDs, descriptions, default levels, and supported options.
    Rules {
        /// Catalog output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
}

/// Supported user-facing output encodings.
#[derive(Clone, Copy, Default, ValueEnum)]
enum OutputFormat {
    /// Human-readable diagnostics split by operational versus advisory output.
    #[default]
    Text,
    /// One versioned JSON object on stdout, even when the command fails.
    Json,
}

/// Execute a complete CLI invocation including the executable's first argument.
pub fn execute(arguments: impl IntoIterator<Item = OsString>) -> i32 {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    let requested_json = arguments.iter().any(|argument| argument == "--format=json")
        || arguments
            .windows(2)
            .any(|pair| pair[0] == "--format" && pair[1] == "json");
    let parsed = match Arguments::try_parse_from(arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            // Help and version retain the standard successful clap behavior.
            // Invalid commands use the same structured failure contract as scans.
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                return if error.print().is_ok() { 0 } else { 2 };
            }
            let mut report = Report::new(0);
            report
                .diagnostics
                .push(Diagnostic::error(None, "input", error.to_string(), None));
            report.finish();
            return render_report(
                &report,
                if requested_json {
                    OutputFormat::Json
                } else {
                    OutputFormat::Text
                },
            );
        }
    };
    match parsed.command {
        Command::Rules { format } => render_rules(format),
        Command::Check {
            paths,
            strict,
            format,
        } => {
            let report = match std::env::current_dir() {
                Ok(working_directory) => runner::run(&working_directory, &paths, strict),
                Err(error) => {
                    // Preserve the report contract even when process state
                    // prevents configuration discovery from starting.
                    let mut report = Report::new(0);
                    report.diagnostics.push(Diagnostic::error(
                        None,
                        "input",
                        format!("cannot determine working directory: {error}"),
                        None,
                    ));
                    report.finish();
                    report
                }
            };
            render_report(&report, format)
        }
    }
}

/// Render one report and return its exit status unless output itself fails.
fn render_report(report: &Report, format: OutputFormat) -> i32 {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut stdout = stdout.lock();
    let mut stderr = stderr.lock();
    let result = match format {
        OutputFormat::Json => write_json(&mut stdout, report),
        OutputFormat::Text => {
            // Operational failures use stderr while source findings remain
            // available on stdout for ordinary command-line processing.
            let mut result = Ok(());
            for diagnostic in &report.diagnostics {
                let destination: &mut dyn Write = if diagnostic.kind == DiagnosticKind::Error {
                    &mut stderr
                } else {
                    &mut stdout
                };
                // Preserve deterministic ordering within each destination and
                // stop writing after the first I/O failure without panicking.
                if result.is_ok() {
                    result = write_diagnostic(destination, diagnostic);
                }
            }
            result
        }
    };
    if result.is_err() {
        2
    } else {
        report.exit_code()
    }
}

/// Serialize a JSON value followed by a single terminating newline.
///
/// # Errors
///
/// Returns the writer error or a serialization error converted into an I/O error.
fn write_json(writer: &mut impl Write, value: &impl serde::Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(io::Error::other)?;
    writeln!(writer)
}

/// Render a diagnostic using one-based positions supplied by the analyzer.
///
/// # Errors
///
/// Returns an error if the output destination cannot accept the rendered text.
fn write_diagnostic(writer: &mut dyn Write, diagnostic: &Diagnostic) -> io::Result<()> {
    match (&diagnostic.path, &diagnostic.range) {
        (Some(path), Some(range)) => write!(
            writer,
            "{path}:{}:{}: ",
            range.start.line, range.start.column
        )?,
        (Some(path), None) => write!(writer, "{path}: ")?,
        (None, _) => {}
    }
    let severity = match diagnostic.severity {
        Severity::Note => "note",
        Severity::Warning => "warning",
        Severity::Error => "error",
    };
    let message = diagnostic.message.replace(['\r', '\n'], " ");
    writeln!(writer, "{severity} {}: {message}", diagnostic.rule)
}

/// Display the production catalog without loading project settings or scanning.
fn render_rules(format: OutputFormat) -> i32 {
    let registry = match Registry::bundled() {
        Ok(registry) => registry,
        Err(message) => {
            // Invalid bundled metadata is an operational failure with the same
            // report shape as a scan, including for catalog-only commands.
            let mut report = Report::new(0);
            report
                .diagnostics
                .push(Diagnostic::error(None, "registry", message, None));
            report.finish();
            return render_report(&report, format);
        }
    };
    let definitions = registry.definitions();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    match format {
        OutputFormat::Json => {
            // Expose the same registered catalog facts in a versioned
            // document without requiring any source selection.
            let rules: Vec<_> = definitions
                .iter()
                .map(|definition| {
                    let options: Vec<_> = definition
                        .options
                        .iter()
                        .map(|option| {
                            serde_json::json!({
                                "name": option.name,
                                "kind": option_kind(&option.kind),
                                "description": option.description,
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "id": definition.id,
                        "name": definition.name,
                        "description": definition.description,
                        "default": level_name(definition.default_level),
                        "options": options,
                    })
                })
                .collect();
            let document = serde_json::json!({"schema_version": 1, "rules": rules});
            if write_json(&mut stdout, &document).is_ok() {
                0
            } else {
                2
            }
        }
        OutputFormat::Text => {
            for definition in definitions {
                if writeln!(
                    stdout,
                    "{} [{}] {}: {}",
                    definition.id,
                    level_name(definition.default_level),
                    definition.name,
                    definition.description
                )
                .is_err()
                {
                    return 2;
                }
                if definition.options.is_empty() {
                    if writeln!(stdout, "  options: none").is_err() {
                        return 2;
                    }
                } else {
                    // Rule descriptions supply option semantics while the
                    // shared catalog prints their names and declared types.
                    for option in definition.options {
                        if writeln!(
                            stdout,
                            "  {} ({}): {}",
                            option.name,
                            option_kind(&option.kind),
                            option.description
                        )
                        .is_err()
                        {
                            return 2;
                        }
                    }
                }
            }
            0
        }
    }
}

/// Map registered default levels onto their stable configuration spelling.
fn level_name(level: Level) -> &'static str {
    match level {
        Level::Off => "off",
        Level::Warning => "warning",
        Level::Error => "error",
    }
}

/// Map typed rule options onto readable catalog type names.
fn option_kind(kind: &OptionKind) -> &'static str {
    match kind {
        OptionKind::Boolean => "boolean",
        OptionKind::Integer => "integer",
        OptionKind::String => "string",
        OptionKind::StringList => "string-list",
    }
}
