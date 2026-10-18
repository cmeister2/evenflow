# Bundled rules

Run `evenflow rules` to list the bundled rule catalog, default severities,
descriptions, and available options. Configure severities in
[`evenflow.toml`](../README.md#configuration-and-selection), or use a reasoned
[local suppression](../README.md#local-suppressions) for a specific body.
See [scope and coverage](SCOPE.md) for the source Evenflow inspects and its
analysis limits.

## FC001: explain substantial nested bodies

FC001 defaults to `warning` and has no tunable options. `--strict` promotes its
warnings to errors; it does not enable a disabled rule.

### Qualifying bodies

A non-function executable body with **three or more direct executable
statements** must start with an ordinary flow comment. A trailing expression
counts once; a multiline call or nested control construct also counts once at
its parent level. Comments, attributes, and local item definitions do not count.
Function and method outer bodies are exempt, while their nested bodies are checked.

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

### Comment placement

Put the flow comment first inside the block. Other explanatory comments are
allowed anywhere: a comment above an `if` can explain the decision or both arms,
while each substantial arm still needs its own opening flow comment.

```rust
fn update(ready: bool) {
    // Commit only when ready; otherwise discard the staged changes.
    if ready {
        // Preserve the write order until the transaction is committed.
        prepare();
        write();
        commit();
    } else {
        // Clear the staged work before reporting the failed attempt.
        rollback();
        reset();
        report_failure();
    }
}
```

To satisfy FC001, a standalone ordinary `//` or `/* ... */` explanation must be
the first thing after the opening brace, apart from whitespace. Comments elsewhere
remain valid; they simply do not satisfy this requirement. Adjacent comment lines
form one explanation; only the opening comment group counts, so a later
explanation cannot repair an initial license header or
placeholder group. There is no required prefix or prose-quality scoring;
reviewers assess whether the explanation is useful.
Documentation comments, trailing comments, license headers, suppression
directives, and comments consisting only of TODO/FIXME placeholders do not count.
An explanation belongs to one body; children, sibling branches, and blocks inside
conditions need their own opening explanations when they qualify. Reasoned
[suppressions](../README.md#local-suppressions) retain their separate placement
rules.

### Exemptions

Bodies made entirely of recognized logging invocations, plain field-copy
assignments, or both are exempt. A plain field copy assigns to a field from an
identifier, field, literal, or reference to one of these. Calls, computed values,
compound assignments, and control flow do not qualify. Logging counts normally
when mixed with other work. See [recognized macro paths](SCOPE.md#macros-and-coverage-notes)
and [excluded contexts](SCOPE.md#const-evaluation-contexts) for the full scope.

Findings use the shared [diagnostic formats and exit statuses](DIAGNOSTICS.md).

## LG001: entry logging

LG001 defaults to `off`. Enable it to require an allowed logging entry at the
start of each substantial body:

```toml
[rules]
LG001 = "warning"

[options.LG001]
allowed = ["tracing::info!", "crate::logging::enter()"]
```

LG001 uses the same [qualifying bodies](#qualifying-bodies), statement threshold,
and [exemptions](#exemptions) as FC001. The entry call itself counts as a direct
statement. Function and method outer bodies remain exempt; qualifying nested
bodies are checked independently.

### Entry placement

The first non-comment syntax item inside the block must be an allowed macro
invocation or direct function call. Opening comments are ignored by LG001, so an
entry call can follow the flow comment required by FC001:

```rust
use tracing::info as entered;

fn handle_job(job: Job) {
    if job.is_ready() {
        // Record the job entry before decoding and persisting its payload.
        entered!("processing job");
        let payload = decode(job);
        persist(payload);
    }
}
```

An attribute, `use` declaration, local definition, empty semicolon, or other
statement before the call produces LG001. A later logging call does not repair
the missing entry. Parentheses, `?`, and `.await` around an otherwise allowed call
are transparent. A call nested inside another call, a `let` or `return`, an async
block, or an ordinary block does not qualify as the direct entry.

### Allowed paths

Without a custom `allowed` list, the defaults are `log::trace!`, `log::debug!`,
`log::info!`, `tracing::trace!`, `tracing::debug!`, and `tracing::info!`. A custom
list replaces these defaults.

Each entry must be a qualified path with at least two components, ending in `!`
for a macro or `()` for a function. For example, `tracing::info!` allows that
macro and `crate::logging::enter()` allows that function; a bare configured
`info!` is invalid. The suffix identifies the call kind; arguments are not
inspected. Use full external or `crate::` paths in configuration rather than
`self::`, `super::`, or `Self::` paths. An empty list, malformed path, unknown
option, or wrong value type is a configuration error even when LG001 is disabled.

The rule follows explicit source imports and aliases, as in the example above,
and compares the complete qualified path and invocation kind. See
[entry-call path resolution](SCOPE.md#entry-call-path-resolution) for the bounded
resolution model. A non-call, disallowed call, or unresolved first item produces
an ordinary LG001 finding; inability to resolve an entry is not an analysis error.

LG001 and FC001 are independent: a flow comment does not satisfy entry logging,
and a logging call does not satisfy the flow-comment requirement. Reasoned
[local suppressions](../README.md#local-suppressions) retain their existing
placement and exact-body behavior. Suppressing one rule does not suppress the
other. With LG001 configured as `warning`, `--strict` promotes its findings to
errors; strict mode does not enable it while it remains `off`.
