//! Command-line entry point for the Evenflow Rust source linter.

/// Run the shared command implementation and preserve its documented exit code.
fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(evenflow::cli::execute(std::env::args_os()) as u8)
}
