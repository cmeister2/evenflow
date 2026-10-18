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

### Comment placement

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

### Exemptions

Bodies made entirely of recognized logging invocations, plain field-copy
assignments, or both are exempt. A plain field copy assigns to a field from an
identifier, field, literal, or reference to one of these. Calls, computed values,
compound assignments, and control flow do not qualify. Logging counts normally
when mixed with other work. See [recognized macro paths](SCOPE.md#macros-and-coverage-notes)
and [excluded contexts](SCOPE.md#const-evaluation-contexts) for the full scope.

Findings use the shared [diagnostic formats and exit statuses](DIAGNOSTICS.md).
