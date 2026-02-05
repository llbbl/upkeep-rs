---
name: upkeep-rs-quality
version: 0.1.3
description: Generate Rust project health grade with improvement recommendations
allowed-tools: Bash, Read, Grep, Glob, Edit
---

# /upkeep-rs-quality - Rust Project Health Report

Trigger: User asks about project health or quality assessment.

Goal: Generate a health report, explain the grade, and produce a prioritized action plan.

## Workflow
1. Run `cargo upkeep quality` to generate the report.
2. Present the overall grade (A-F) with a metric breakdown.
3. For each low-scoring metric, suggest concrete improvements:
   - Dependencies: run `/upkeep-rs-deps`.
   - Security: run `/upkeep-rs-audit`.
   - Clippy: fix warnings.
   - MSRV: add `rust-version` to `Cargo.toml`.
   - Unused deps: remove with `cargo-machete`.
   - Unsafe code: audit and document safety invariants.
4. Compare with previous runs when available.
5. Celebrate improvements and highlight regressions.
6. Provide a prioritized action plan.

## Prioritization
- High: Security findings, critical updates.
- Medium: Code quality, test coverage, linting.
- Low: Style, documentation.

## Reporting
- Summarize the score drivers.
- List top 3 improvements for the next sprint.
- Note any metrics blocked by missing tools.

## Example
User: "How healthy is this Rust project?"
Assistant:
```bash
cargo upkeep quality
```
- Report grade and metrics, then propose an action plan.
