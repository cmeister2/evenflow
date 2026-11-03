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

## Python development and packaging

Use CPython 3.10 or newer with the same Rust and C build prerequisites above:

```sh
python -m venv .venv
. .venv/bin/activate
python -m pip install 'maturin>=1.12,<2' pytest
maturin develop --locked
python -m pytest python/tests
python -m evenflow check . --strict
cargo clippy --locked --all-targets --features pyo3 -- -D warnings
maturin build --release --locked --out dist
maturin sdist --out dist
```

On Windows, activate with `.venv\Scripts\Activate.ps1`. The mixed Python/Rust
layout follows [Maturin's project layout](https://www.maturin.rs/project_layout).
`python/evenflow` supplies the public API and console entry point;
`src/python.rs` binds the shared Rust runner and CLI through PyO3. Wheels use the
CPython stable ABI starting at Python 3.10.

## Python release publishing

[workflow.yml](.github/workflows/workflow.yml) builds CPython 3.10+ stable-ABI
wheels for these platforms:

| Platform | Architectures |
| --- | --- |
| Linux glibc 2.28+ (manylinux) | x86_64, i686, aarch64, ARMv7, s390x, ppc64le |
| Linux musl 1.2+ (musllinux) | x86_64, i686, aarch64, ARMv7 |
| macOS | ARM64, Intel x86_64 |
| Windows | x64, x86 |

Native Linux x64, macOS ARM64, and Windows jobs install and test their wheels on
Python 3.10 and 3.15. Intel macOS uses Python 3.10: its Python 3.15 job is excluded
because setup-python repeatedly stalls before the build starts. The Intel abi3
wheel still supports CPython 3.10+, but Intel Python 3.15 runtime testing is
currently disabled. Native wheel jobs limit Python setup to ten minutes.
Additional Linux wheels use Maturin's target-specific Docker build
containers; the x86_64 musllinux wheel is also installed and tested on Alpine
Python 3.10. Other cross-built Linux wheels are checked during compilation and
Maturin's portability audit, but are not runtime-tested on their target CPUs.
The success gate requires every wheel build. The source distribution is installed
and tested using Rust 1.90. Unsupported platforms can build from the source
distribution with Rust and a C compiler.

The existing semantic-release plan stamps Cargo.toml and Cargo.lock before all
builds. Maturin derives the Python version from Cargo.toml. The combined success
gate requires Rust, Python, and GitHub Action checks. After the crates.io and
GitHub release succeeds, the PyPI job uploads the tested wheels and source
distribution using [trusted publishing](https://docs.pypi.org/trusted-publishers/using-a-publisher/).
No PyPI token is needed. Its publisher identity is repository
`cmeister2/evenflow`, workflow `workflow.yml`, environment `pypi`; these must
match the pending publisher on PyPI. The existing crates.io release still uses
`CARGO_REGISTRY_TOKEN`.

Publication across registries is not atomic. If PyPI publishing fails after the
Rust release, rerun the failed PyPI job in that workflow run to reuse its tested
artifacts. Do not start a fresh release to repair a failed upload.
