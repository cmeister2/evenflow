# Evenflow

Evenflow is an opinionated Rust source linter that runs alongside Clippy. It uses
Tree-sitter to inspect source structure and ordinary comments, without compiling
the project being checked. The first bundled rule, **FC001**, asks for a leading
flow comment on substantial nested executable bodies.

Evenflow is implemented in Rust and checks its own production source in CI. Its
rule interface is internal; external plugins and a stable library API are not
part of this release. Entry logging is a possible future rule, not an enabled
check.

## Install and run

Source builds require Rust **1.90.0 or newer**, Cargo, and a C compiler/linker for
the bundled Tree-sitter parser and Rust grammar. Typical prerequisites are the
platform's C development tools: `build-essential` on Ubuntu, Xcode Command Line
Tools on macOS, or Visual Studio C++ Build Tools on Windows. The Cargo package uses
Rust edition 2024 and includes `Cargo.lock`.

Install the published package with locked dependencies:

```sh
cargo install --locked evenflow
```

From a source checkout, use `cargo install --locked --path .`. Run the linter with:

```sh
evenflow check
evenflow check src/ worker/src/
evenflow check . --strict
evenflow check . --format json
evenflow rules
```

Download prebuilt executables from
[GitHub Releases](https://github.com/cmeister2/evenflow/releases) for Linux x86_64
(`x86_64-unknown-linux-gnu`), macOS Apple Silicon (`aarch64-apple-darwin`), and
Windows x86_64 (`x86_64-pc-windows-msvc`). Unix archives are named
`evenflow-VERSION-TARGET.tar.gz`; Windows uses `evenflow-VERSION-TARGET.zip`.
Each release also includes `evenflow-VERSION.crate` and `SHA256SUMS`. Extract the
matching executable and place it on `PATH`. A prebuilt executable needs the
matching operating system/runtime, but no Rust compiler, Cargo, Node.js,
separately installed parser, or grammar. Linux archives are built on Ubuntu 22.04
and use its system C runtime. The Rust package is also published to
[crates.io](https://crates.io/crates/evenflow).

A scan is offline: it reads source and local configuration, and does not contact
a service or build the checked project. Source installation may download Cargo
dependencies. After those dependencies are cached, use
`cargo install --locked --offline --path .` for an offline source build.

## FC001: explain substantial nested bodies

A non-function executable body with **three or more direct executable
statements** needs a leading ordinary comment. A trailing expression counts once;
a multiline call or nested control construct also counts once at its parent
level. Comments, attributes, and local item definitions do not count. Function
and method outer bodies are exempt, while their nested bodies are checked.

For example, this branch has three direct statements and produces FC001:

```rust
fn save_if_dirty(state: &mut State) -> Result<()> {
    if state.is_dirty() {
        let snapshot = state.snapshot();
        persist(snapshot)?;
        state.mark_clean();
    }
    Ok(())
}
```

Explain the reason for the sequence before its owning construct:

```rust
fn save_if_dirty(state: &mut State) -> Result<()> {
    // Preserve the dirty marker until persistence succeeds so failures can retry.
    if state.is_dirty() {
        let snapshot = state.snapshot();
        persist(snapshot)?;
        state.mark_clean();
    }
    Ok(())
}
```

A standalone ordinary `//` or `/* ... */` explanation can instead precede the
body's first executable statement. Whitespace and attributes may intervene.
Adjacent comment lines form one explanation. There is no required prefix or
prose-quality scoring; reviewers assess whether the explanation is useful.
Documentation comments, trailing comments, license headers, suppression
directives, and comments consisting only of TODO/FIXME placeholders do not count.
An explanation belongs to one body; children and sibling branches need their own
explanations when they qualify. Before `if` or `while`, it explains the primary
body, not an executable block inside the condition.

Bodies made entirely of recognized logging invocations, plain field-copy
assignments, or both are exempt. A plain field copy assigns to a field from an
identifier, field, literal, or reference to one of these. Calls, computed values,
compound assignments, and control flow do not qualify. Logging counts normally
when mixed with other work.

## Scope and coverage

The syntax tree includes functions and provided trait methods, branches, loop
bodies, `let ... else` bodies, block match arms, block closures, async blocks,
runtime unsafe blocks, and explicit runtime scopes. Expression-only closures and
match arms do not gain synthetic bodies. Struct literals, item definitions, and
the braces around a match arm list are not executable bodies.

FC001 skips const-evaluation contexts: const functions, inline const blocks,
const/static initializer subtrees, array lengths, const generic expressions, and
enum discriminants. This includes runtime closures nested in static initializers
such as `LazyLock`; they are outside first-release coverage.

By default, production source is checked, including inactive production `cfg`
branches. These paths are excluded before parsing:

- `target` and `generated` path components, and `*_autogenerated.rs` files;
- `tests` path components and files named `tests.rs`.

Functions with `#[test]`, `#[tokio::test]`, or `#[async_std::test]`, and definitely
test-only items under `cfg`, are skipped. Inline test modules do not hide nearby
production code. The test filter treats `test` as false and other predicates as
unknown through nested `all`, `any`, and `not`: `all(test, feature = "x")` is
excluded, but `any(test, feature = "x")` stays in scope. Attribute aliases,
`cfg_attr`, and module resolution are not evaluated. `include-tests = true`
includes test paths and items; generated/build exclusions still apply.

Every selected file must parse completely, including code later excluded by
item-level test or const policy. Syntax errors, missing parser nodes, and invalid
UTF-8 produce analysis errors. Exclude an unsupported file explicitly while
upgrading the grammar. An error never becomes a successful empty lint result.

Evenflow does not resolve names or types, expand macros, or check whether a log
invocation emits output at runtime. Recognized logging paths are bare `trace`,
`debug`, `info`, `warn`, and `error`, their `log::`/`tracing::` forms, bare `crit`,
and all six levels under `slog::`. A leading `::` is normalized; aliases are not
resolved. Logging arguments are opaque. These exact data macros are also opaque:
`matches`, `std::matches`, `core::matches`, `vec`, `std::vec`, `alloc::vec`, `json`,
and `serde_json::json`. Their arguments can hide executable code.

Other macro invocations containing brace-delimited token groups receive one
**EF100 coverage note**, including `custom! { ... }`, `custom!({ ... })`, and
`tokio::select!`/bare `select!` with such groups. Braces inside string literals do
not count. Macro definitions and expansions are outside coverage. EF100 remains
advisory under `--strict`; successful analysis does not imply macro-internal
coverage.

## Configuration and selection

Evenflow discovers the nearest ancestor of the working directory containing
`evenflow.toml`; that directory becomes the scan root. Without a configuration
file, the working directory is the root. Ancestor configurations are not merged,
and the target project's Cargo manifest is not required.

```toml
exclude = ["src/bindings/**", "src/test_support/**"]
include-tests = false

[rules]
FC001 = "warning"
```

Rules accept `off`, `warning`, and `error`. Omitted rules use their registered
defaults; FC001 defaults to `warning`. Warnings do not fail a run unless
`--strict` promotes them to errors. Strict mode does not enable disabled rules.
`[options.RULE_ID]` is reserved for each registered rule's typed options; FC001
has no tunable options. Unknown settings, rule IDs, options, and invalid types
are errors, even when the affected rule is off. `evenflow rules` lists the
production catalog, defaults, descriptions, and available options.

`check` accepts Rust files or directories and defaults to the working directory.
It recursively selects `.rs` files, applies exclusions, deduplicates overlapping
inputs, and scans in stable path order. Exclusion globs use `/` relative to the
root; `**/` matches zero or more directories. Directory symlinks are not followed,
and resolved paths must stay within the root. Explicit missing, unreadable, or
non-Rust inputs, and a selection with no files after exclusions, are errors.

## Local suppressions

Use a standalone ordinary comment with one rule ID and a non-empty reason:

```rust
// evenflow: allow(FC001): Keep this transaction's straightforward steps together.
if ready {
    prepare();
    write();
    commit();
}
```

A directive can precede the body-owning construct or its first executable
statement. It suppresses findings for that exact body, not descendants or
siblings. Adjacent directives may address different rules. Directives do not
count as flow explanations; mentioning `evenflow:` inside other prose does not
activate one.

Malformed, trailing, misplaced, unknown-ID, and unused directives are analysis
errors. Directives for known disabled rules are inactive rather than unused;
unused checks run only for successfully analyzed files with the rule enabled.
Analysis errors and coverage notes cannot be suppressed. Use explicit file
exclusions for unsupported source. File-wide and baseline suppressions are not
supported.

## Diagnostics and exit status

Text findings and coverage notes go to stdout; operational errors go to stderr:

```text
src/worker.rs:42:9: warning FC001: branch body has 4 direct statements; add a leading flow comment
```

Diagnostics have normalized paths, an ID, kind, effective severity, message, and
source range. Lines and Unicode code-point columns are one-based; internal
locations use byte offsets. Unlocated errors sort first, followed by path,
position, and ID. The framework uses **EF000** for configuration, input, encoding,
parsing, suppression, and internal-rule errors.

With `--format json`, stdout is one JSON object even on failure. Schema version 1
has this shape (an example successful scan with no findings):

```json
{
  "schema_version": 1,
  "diagnostics": [],
  "summary": {
    "selected_files": 1,
    "analyzed_files": 1,
    "counts_by_id": {},
    "counts_by_kind": {},
    "analysis_completed": true
  }
}
```

Each diagnostic has `path`, `rule` (the diagnostic ID), `kind` (`lint`, `coverage`,
or `error`), `severity` (`note`, `warning`, or `error`), `message`, `range`, and
`category`. The range contains `start` and `end`, each with `line` and `column`.
Path, range, and category may be `null`. Failures before discovery have zero
file counts and `analysis_completed: false`. Completion means selected files
finished without analysis errors, not that opaque macros were inspected.

| Exit | Meaning |
| --- | --- |
| 0 | Analysis completed without error-level lints; warnings/coverage notes may exist. |
| 1 | Analysis completed with error-level lints. |
| 2 | Tool/analysis failure, including empty selections. |

Analysis failures take precedence over lint failures. Independent files continue
after a file-level error, and the report marks analysis incomplete.

## Development and CI

Run the same checks as CI:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo run --locked -- check . --strict
```

The repository's `evenflow.toml` keeps FC001 enabled. Test fixtures under `tests/`
are excluded from the production-source scan; they independently define expected
behavior. The package also denies Rust `missing_docs` and Clippy
`missing_docs_in_private_items`, `missing_errors_doc`, and `missing_panics_doc`.
Documentation checks complement FC001's ordinary flow-comment checks.

To add a bundled rule, implement the internal rule contract, register its unique
ID/default/options, and add fixtures and documentation. Use the shared source
context, body facts, comments, attributes, spans, and suppression targets. Rules
return structured findings; parsing, discovery, configuration, diagnostics,
suppression validation, and exit policy remain shared infrastructure. The test
suite includes an unrelated test-only rule to exercise this contract.

Build a source archive and verify a release executable outside the checkout:

```sh
cargo package --locked --allow-dirty
cargo build --locked --release
bash scripts/release-smoke.sh target/release/evenflow
```

The smoke script copies the executable and fixtures to a temporary directory,
runs with an empty `PATH` and environment, and checks clean, lint-failing, and
syntax-failing JSON scans. The CI workflow additionally installs from the
packaged source with `cargo install --locked`, then checks that executable.
Nothing invokes Evenflow recursively from a build script or during analysis.

## Releases

CI runs semantic-release on pushes to `main`. Publication waits for all checks:
stable and minimum-supported Rust verification, tests and standalone smoke
checks on all three native platforms, and installation from the packaged Cargo
source. The required `Success` check aggregates these results and gates
publication. CI calculates the release version, stamps it into `Cargo.toml` and
`Cargo.lock` before building, and checks each executable's `--version` output.
The verified source package is published to crates.io before the version is
tagged. GitHub Releases then receives the native archives, the same published
source archive, checksums, and generated release notes.

Configure the `CARGO_REGISTRY_TOKEN` repository Actions secret with permission to
publish `evenflow`, including new-crate publication for the initial release. The
token is passed only to the main-branch publication step. Missing credentials or
registry errors stop publication before tag creation. A retry accepts an already
published version only when its package checksum matches the prepared package.

Pull requests run a semantic-release dry run, show the proposed version and notes
in the workflow summary, and use that version for the native-platform and
source-installation checks. If no release is proposed, builds use the checked-in
development version. PR source-package checks also run `cargo publish --dry-run`
without a registry token. Publishing runs only on `main`.

Use Conventional Commits for changes merged to `main`:

| Commit | Release |
| --- | --- |
| `fix: ...` | Patch |
| `feat: ...` | Minor |
| `feat!: ...` or a `BREAKING CHANGE:` footer | Major |
| `docs: ...`, `chore: ...`, `ci: ...` | No release |

The checked-in Cargo version remains the development version, currently
`0.1.0`; release builds receive the calculated version without a version-bump
commit. Semantic-release calculates release versions from the latest Git tag and
the Conventional Commits after it.

Node.js **24.15.0 or newer** and npm are needed only when working on release
automation. Install its locked dependencies and run its checks with:

```sh
npm ci --ignore-scripts
npm run test:release
```

Preview the next version and release notes with:

```sh
npm run release:dry-run
```

The preview uses a temporary local Git repository and needs no publishing
credentials. It works on feature branches and pull requests, preserves the source
checkout and remote, and skips publication. Use a full-history clone with tags so
version calculation includes previous releases.

## License

Licensed under either [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT), at your option.
