# Development

Use Rust 1.90.0 or newer with Cargo, rustfmt, Clippy, and a C compiler/linker for
the bundled Tree-sitter parser and Rust grammar. Run the commands below from the
repository root.

## Run from the checkout

```sh
cargo run --locked -- rules
cargo run --locked -- check src/ --strict
cargo run --locked -- check src/flow.rs --format json
```

The arguments after `--` go to Evenflow. Scanning reads source and local
configuration without building the project being checked.

## Check changes

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo run --locked -- check . --strict
```

The final command checks Evenflow's production source using
[evenflow.toml](evenflow.toml). Test fixtures under `tests/` define expected
findings and are excluded from that scan.

[Cargo.toml](Cargo.toml) denies missing documentation for public and private
items, missing error documentation, and missing panic documentation. These checks
complement FC001's ordinary flow comments.

For changes to JavaScript helpers, use Node.js 24.15.0 or newer and npm:

```sh
npm ci --ignore-scripts
npm run test:release
npm run test:action
```

The root [action.yml](action.yml) exposes the GitHub Action implemented in
[.github/action/main.mjs](.github/action/main.mjs). It uses the Node.js 24 action
runtime and built-in modules, so there are no runtime npm dependencies or bundled
JavaScript artifacts to regenerate. The action downloads release binaries and
verifies `SHA256SUMS` before running the linter in the caller's checkout.

Run `npm run test:action` when changing action inputs, release installation, or
CLI invocation. Keep the action's input defaults and supported platforms in sync
with the [usage documentation](README.md#github-action).

## Add a bundled rule

Implement the internal [`Rule`](src/registry.rs) contract in a Rust module and
declare that module in [src/lib.rs](src/lib.rs). Supply a `RuleDefinition` with a
unique stable ID, name, description, default level, and typed options. IDs beginning
with `EF` are reserved for framework diagnostics. Register the implementation in
`Registry::bundled()`.

Override `Rule::validate_options` for semantic checks beyond option types; these
checks also run when the rule is disabled.

Use [`SourceContext`](src/source.rs) for the parsed tree, body facts, comments,
attributes, and original source spans. Return structured `Finding` values with
the diagnostic span, suppression target, and eligible annotation spans. Rules
inspect the shared context without performing file I/O; the framework handles
configuration, severity, suppression validation, rendering, and exit status.

See [src/flow.rs](src/flow.rs) for the production rule and
[tests/framework.rs](tests/framework.rs) for an unrelated test rule. Add passing
and failing examples, including option validation and suppression behavior where
applicable, and document the rule's user-visible behavior.

## Check a standalone executable

On a Unix-like development environment with Bash and Python 3:

```sh
cargo build --locked --release
bash scripts/release-smoke.sh target/release/evenflow
```

The smoke script copies the executable and fixtures to a temporary directory,
clears the executable's environment and `PATH`, and checks clean, lint-failing,
and syntax-failing JSON scans.
