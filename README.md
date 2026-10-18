# Evenflow

Evenflow is an opinionated Rust source linter that runs alongside Clippy. It uses
Tree-sitter to check source structure and comments without building your project.

## Install and run

```sh
cargo install --locked evenflow
evenflow check
evenflow check src/ worker/src/
evenflow check . --strict
evenflow check . --format json
evenflow rules
```

Prebuilt binaries are available from
[GitHub Releases](https://github.com/cmeister2/evenflow/releases).

## GitHub Action

Call Evenflow from another repository's workflow after checking out that
repository:

```yaml
name: Evenflow

on: [push, pull_request]

permissions:
  contents: read

jobs:
  lint:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - uses: cmeister2/evenflow@main
```

The action downloads a prebuilt binary, verifies it against the release's
`SHA256SUMS`, and runs `evenflow check` with text diagnostics. No Rust toolchain
or project build is required. Supported runners are Linux x64, macOS ARM64, and
Windows x64; other operating system and architecture combinations fail with an
error.

| Input | Default | Meaning |
| --- | --- | --- |
| `version` | `latest` | Latest stable release, or an exact bare version such as `0.1.1` without a `v` prefix. |
| `paths` | `.` | Files or directories, one per line, relative to `working-directory`. |
| `working-directory` | `.` | Directory to scan from, relative to `GITHUB_WORKSPACE`. |
| `strict` | `true` | Promote enabled warnings to errors. Set `false` to respect configured rule severities. |
| `token` | `${{ github.token }}` | Token used only for GitHub API reads of releases and assets. |

For example, select a release, scan part of a repository, and allow configured
warnings:

```yaml
- uses: cmeister2/evenflow@main
  with:
    version: '0.1.1'
    working-directory: crates/service
    paths: |
      src/
      examples/request.rs
    strict: 'false'
```

Configuration discovery starts from `working-directory` using the same
`evenflow.toml` rules as the CLI. The action preserves CLI exit statuses: `0`
passes, `1` fails for error-level lint findings, and `2` fails for tool or analysis
errors. Its default strict mode also fails on warnings.

The examples use `@main` because the action has not yet been tagged. For
reproducible CI, replace `main` with a full commit SHA containing the action and
set `version` to an exact release. Pinning the action alone still uses the latest
binary unless `version` is set.

## Documentation

- [Bundled rules](docs/RULES.md): behavior, examples, and exemptions.
- [Scope and coverage](docs/SCOPE.md): checked code, exclusions, and macro limitations.
- [Diagnostics and exit status](docs/DIAGNOSTICS.md): text output, JSON schema, and exit codes.
- [Development](DEVELOPMENT.md): prerequisites, local commands, and adding rules.

## Configuration and selection

Evenflow discovers the nearest ancestor of the working directory containing
`evenflow.toml`; that directory becomes the scan root. Without a configuration
file, the working directory is the root. Ancestor configurations are not merged,
and the target project's Cargo manifest is not required.

```toml
exclude = ["src/bindings/**", "src/test_support/**"]
include-tests = false
```

Set rule levels to `off`, `warning`, or `error` in `[rules]`, using IDs listed by
`evenflow rules`. Omitted rules use their registered defaults. Warnings do not
fail a run unless `--strict` promotes them to errors. Strict mode does not enable
disabled rules. Unknown settings, rule IDs, options, and invalid types are errors,
even when the affected rule is off. `evenflow rules` also lists defaults,
descriptions, and available options.

`check` accepts Rust files or directories and defaults to the working directory.
It recursively selects `.rs` files, applies exclusions, deduplicates overlapping
inputs, and scans in stable path order. Exclusion globs use `/` relative to the
root; `**/` matches zero or more directories. Directory symlinks are not followed,
and resolved paths must stay within the root. Explicit missing, unreadable, or
non-Rust inputs, and a selection with no files after exclusions, are errors.

## Local suppressions

Use a standalone ordinary comment with one rule ID and a non-empty reason.
In this template, replace `RULE_ID` with an ID listed by `evenflow rules`:

```text
// evenflow: allow(RULE_ID): Explain why this exception is needed.
```

A directive can precede the body-owning construct or its first executable
statement. It suppresses findings for that exact body, not descendants or
siblings. Adjacent directives may address different rules. Mentioning `evenflow:`
inside other prose does not activate one.

Malformed, trailing, misplaced, unknown-ID, and unused directives are analysis
errors. Directives for known disabled rules are inactive rather than unused;
unused checks run only for successfully analyzed files with the rule enabled.
Analysis errors and coverage notes cannot be suppressed. Use explicit file
exclusions for unsupported source. File-wide and baseline suppressions are not
supported.

## License

Licensed under either [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT), at your option.
