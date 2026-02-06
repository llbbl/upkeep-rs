---
name: upkeep-rs-audit
version: 0.1.7
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
1. Run `cargo upkeep audit` to scan for vulnerabilities.
2. For each vulnerability:
   - Explain the issue in plain terms and affected versions.
   - Check for patched versions.
   - If patch exists, guide upgrade steps.
   - If no patch, suggest mitigations or alternatives.
3. Provide RustSec advisory links for each finding.
4. Create a security fix branch and commit changes.
5. Open a PR with vulnerability details.

## Severity Handling
- Critical: Immediate action required, prioritize fix now.
- High: Fix soon, schedule promptly.
- Moderate: Plan to fix in the next cycle.
- Low: Informational, track for later.

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
