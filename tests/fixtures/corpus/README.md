# Representative corpus

These authored samples model persistence, worker queues, recovery, field copying,
logging, and asynchronous dispatch. They are raw-source fixtures and intentionally
leave application types and functions unresolved. They are not a measurement of
warning frequency in third-party projects.

| File | Lines | Expected FC001 | Expected EF100 | Evaluation |
| --- | ---: | ---: | ---: | --- |
| `workflow.rs` | 62 | 0 | 0 | Explanations at the start of each qualifying body and the narrow logging/field-copy exemptions avoid unnecessary findings. Short bodies, expression-only arms, and compile-time contexts remain clean. |
| `queue.rs` | 38 | 3 | 0 | Queue processing, the successful recovery arm, and the inner retry each lack their own explanation. The explained error arm and outer branch remain clean. |
| `macros.rs` | 23 | 0 | 2 | A custom macro and `tokio::select!` expose coverage gaps. Logging/data macros are opaque exceptions; braces inside a string do not create a gap. |

The 123-line corpus produces three intentional lint findings (2.44 per 100
physical lines) and no unexpected findings. This deliberately balanced sample
checks warning usefulness and placement; its density does not predict adoption
cost in a real repository. Macro interiors, resolved names and types, and runtime
logging effects remain outside the analysis contract.

`tests/corpus.rs` checks these expectations, equality between CLI and in-memory
analysis, deterministic ordering, and unchanged rule conclusions after `rustfmt`.
Contributor checks require the `rustfmt` component of the stable Rust toolchain.
Set `RUSTFMT` to an explicit formatter executable when checking a different
toolchain without its own `rustfmt` component installed.
The same test target generates 2,000 explained bodies/comments plus a final
unexplained body, exercising source indexing and exact ownership without a
machine-dependent timing assertion.

Run the checks with:

```bash
cargo test --locked --test corpus
```

An initial unoptimized local run of the four corpus tests took approximately
0.36 seconds on 2026-10-17, excluding compilation. Use this only as a starting
observation; compare profiles on the same machine and build configuration before
changing parser or traversal architecture.
