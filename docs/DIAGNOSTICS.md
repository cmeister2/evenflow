# Diagnostics and exit status

Evenflow reports [rule findings](RULES.md), [coverage notes](SCOPE.md#macros-and-coverage-notes),
and analysis errors. [Configuration](../README.md#configuration-and-selection)
controls rule severity; [local suppressions](../README.md#local-suppressions)
apply to findings for a specific body.

## Text output

Text findings and coverage notes go to stdout; operational errors go to stderr:

```text
src/worker.rs:42:9: warning FC001: branch body has 4 direct statements; add a flow comment at the start of the block
```

## Locations and identifiers

Diagnostics have normalized paths, an ID, kind, effective severity, message, and
source range. Lines and Unicode code-point columns are one-based; internal
locations use byte offsets. Unlocated errors sort first, followed by path,
position, and ID. The framework uses **EF000** for configuration, input, encoding,
parsing, suppression, and internal-rule errors.

The flow-comment rule uses **FC001**. The optional entry-logging rule uses
**LG001** and defaults to `off`. A missing, disallowed, or unresolved first entry
is an LG001 lint finding, not an analysis error. **EF100** identifies advisory
macro coverage notes. See [rules](RULES.md) and [scope and coverage](SCOPE.md) for
their meaning and limits.

## JSON output

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

## Exit status

| Exit | Meaning |
| --- | --- |
| 0 | Analysis completed without error-level lints; warnings/coverage notes may exist. |
| 1 | Analysis completed with error-level lints. |
| 2 | Tool/analysis failure, including empty selections. |

Analysis failures take precedence over lint failures. Independent files continue
after a file-level error, and the report marks analysis incomplete.

Warnings do not fail a run unless `--strict` promotes them to errors. Strict mode
does not enable disabled rules, and EF100 coverage notes remain advisory.
