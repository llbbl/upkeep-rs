---
name: upkeep-rs-quality
version: 0.4.2
description: Generate Rust project health grade with improvement recommendations
allowed-tools: Bash, Read, Grep, Glob, Edit
---

# /upkeep-rs-quality - Rust Project Health Report

**IMPORTANT:** Always use `cargo upkeep` subcommands for this workflow.
Do not run individual tools separately.

## Do NOT Use
- `cargo clippy` alone - use `cargo upkeep quality` for integrated scoring
- `cargo outdated` - use `cargo upkeep deps` instead
- `cargo audit` - use `cargo upkeep audit` instead
- `cargo geiger` alone - use `cargo upkeep quality` for integrated scoring

Trigger: User asks about project health or quality assessment.

Goal: Generate a health report, explain the grade, and produce a prioritized action plan.

## Workflow
1. Run `cargo upkeep quality` to generate the report.
2. Check `complete` before presenting anything. If it is `false`, the grade
   covers only part of the project — see [Partial results](#partial-results).
3. Present the overall grade (A-F) with a metric breakdown, qualified as partial
   whenever `complete` is `false`.
4. For each low-scoring metric, suggest concrete improvements:
   - Dependencies: run `/upkeep-rs-deps`.
   - Security: run `/upkeep-rs-audit`.
   - Clippy: fix warnings.
   - MSRV: add `rust-version` under `[package]`, or under `[workspace.package]` for a virtual workspace.
   - Unused deps: remove with `cargo-machete`.
   - Unsafe code: audit and document safety invariants.
   The Security metric is vulnerability-only. Informational and yanked warnings
   from standalone `cargo upkeep audit` do not affect this grade.
5. Compare with previous runs when available — but only compare `score` between
   runs with the same `measured_weight`, since it is renormalized (see below).
6. Celebrate improvements and highlight regressions.
7. Provide a prioritized action plan.

## Partial results

Unmeasured metrics are **excluded and the remainder renormalized**, not defaulted
to a healthy value. `score` means "of what we could measure, rescaled to 0-100".

| Field | How to read it |
|-------|----------------|
| `complete` | `true` only when all six metrics ran. Check this first. |
| `measured_weight` | Fraction of total weight behind the score, `0.0`-`1.0`. |
| `breakdown[].score` | `null` means not measured. Never report it as a number. |
| `unavailable[]` | One entry per metric that did not run, with `reason` and `detail`. |
| `unavailable[].reason` | `not_installed` = optional tool absent; `failed` = analyzer broke. |

Rules:

- **Never present a partial grade as the project's health.** A `B` over 55% of
  the weight is a `B` for a little over half the project. Say so, and say
  `measured_weight`.
- **`score` and `grade` are `null` when nothing could be measured.** Report that
  the analysis failed. Do not substitute a number, and do not infer a grade.
- **A nonzero exit status does not mean there is no report.** `quality` exits
  nonzero when nothing was measured, and when `--require-complete` is passed and
  the run was partial. The full JSON report is still on stdout in both cases, and
  the reason is on stderr. Read stdout before reporting a failure — the exit
  status is added to the analysis, not substituted for it.
- **Do not turn an unavailable metric into a finding.** `not_installed` says
  nothing about project health; a missing `cargo-geiger` is not unsafe code.
- **Route the two reasons differently.** For `not_installed`, surface the
  `cargo install` command from `detail` as a setup step. For `failed`, treat it
  as a real problem to investigate — an analyzer that should have worked did not.
- **Never re-derive a "full" score** by assuming values for unmeasured metrics.

## Prioritization
- High: Security findings, critical updates.
- Medium: Code quality, test coverage, linting.
- Low: Style, documentation.

## Reporting
- Summarize the score drivers.
- List top 3 improvements for the next sprint.
- State `complete`, and when it is `false` list every `unavailable` metric with
  its `reason` — separating "install this tool" from "this analyzer failed" —
  and give `measured_weight` so the reader knows how much the grade covers.

## Example
User: "How healthy is this Rust project?"
Assistant:
```bash
cargo upkeep quality
```
- Report grade and metrics, then propose an action plan.
