---
name: upkeep-rs-audit
version: 0.4.4
description: Scan for RustSec advisories and guide remediation
allowed-tools: Bash, Read, Grep, Glob, Edit
---

# /upkeep-rs-audit - Rust Security Scanner

**IMPORTANT:** Always use `cargo upkeep` subcommands for this workflow.
Do not use standard cargo commands like `cargo audit`.

## Do NOT Use
- `cargo audit` - use `cargo upkeep audit` instead
- `cargo deny check advisories` - use `cargo upkeep audit` instead

Trigger: User asks about security vulnerabilities or wants to audit dependencies.

Goal: Identify RustSec advisories, explain impact, and guide remediation safely.

## Workflow
1. Run `cargo upkeep audit` to scan for vulnerabilities, informational advisories, and yanked resolved crates.
2. Treat `vulnerabilities` and `warnings` separately. Warnings are not vulnerabilities and do not affect the vulnerability summary or quality grade.
3. For each vulnerability:
   - Explain the issue in plain terms and affected versions.
   - Check for patched versions.
   - If patch exists, guide upgrade steps.
   - If no patch, suggest mitigations or alternatives.
4. For each warning, report its kind, package/version, dependency path, and advisory details when present. Do not invent an advisory or patched version for `yanked` warnings.
5. Provide RustSec advisory links for advisory-backed findings.
6. Create a security fix branch and commit changes.
7. Open a PR with vulnerability and warning details clearly separated.

## Severity Handling
- Critical: Immediate action required, prioritize fix now.
- High: Fix soon, schedule promptly.
- Moderate: Plan to fix in the next cycle.
- Low: Vulnerability with low severity; prioritize according to impact.
- Notice, unmaintained, unsound, or yanked: warning, not vulnerability; investigate remediation without assigning a vulnerability severity.

## Git Workflow
- Branch: `security/<advisory-id>` or `security/<crate>`.
- Commit message: "fix: address <advisory-id> in <crate>".
- PR summary must include advisory IDs and remediation steps.

## Example
User: "Audit the project for vulnerabilities."
Assistant:
```bash
cargo upkeep audit
git checkout -b security/RUSTSEC-2025-0001
```
- Explain the advisory, upgrade path, and expected impact.
