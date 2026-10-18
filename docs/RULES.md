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
