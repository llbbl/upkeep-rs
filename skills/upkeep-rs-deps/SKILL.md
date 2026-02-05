# /upkeep-rs-deps - Rust Dependency Updater

Trigger: User asks to update dependencies or check outdated crates.

Goal: Safely update Rust dependencies with risk assessment, verification, and clear reporting.

## Workflow
1. Run `cargo upkeep deps` to list outdated crates.
2. Assess risk per dependency (major/minor/patch).
3. For each dependency (one at a time):
   - Create a feature branch named `deps/<crate>`.
   - Update the version in `Cargo.toml`.
   - Run `cargo build` to ensure compilation.
   - Run `cargo test` to validate behavior.
   - If success, commit changes with a concise message.
   - If failure, rollback changes and report the failure details.
4. Open a PR with `gh` per dependency.
5. Summarize changes and risks in the final report.

## Safety Rules
- Never update multiple major versions at once.
- Always run tests before committing.
- One dependency per commit and per PR.
- Warn about breaking changes and link to release notes when available.
- Do not add Claude attribution in commits or PRs.

## Reporting
- Provide a table: crate, current, target, risk, notes.
- Flag majors as high risk, minors as medium, patches as low.
- Call out any build or test failures with next steps.

## Example
User: "Update dependencies for this repo."
Assistant:
```bash
cargo upkeep deps
git checkout -b deps/serde
```
- Update `serde` in `Cargo.toml` to the latest compatible version.
- Run `cargo build` and `cargo test`.
- Commit and open a PR with a summary of the change and risk.
