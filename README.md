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

## Documentation

- [Bundled rules](docs/RULES.md): FC001 flow comments, with examples and exemptions.
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

## License

Licensed under either [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT), at your option.
