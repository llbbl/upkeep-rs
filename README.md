# cargo-upkeep

![CI](https://github.com/llbbl/upkeep-rs/actions/workflows/ci.yml/badge.svg)
![crates.io](https://img.shields.io/crates/v/cargo-upkeep.svg)

`cargo-upkeep` is a Rust maintenance CLI that combines the checks you usually run separately into one cargo subcommand. Instead of stitching together `cargo-outdated`, `cargo-audit`, `cargo-machete`, `cargo-geiger`, and `cargo clippy`, it gives you one interface, one JSON contract, workspace-aware dependency resolution, and one quality signal you can feed into CI.

## Install

```bash
cargo install cargo-upkeep
```

For `cargo-binstall`, the install script, and source builds, see [docs/installation.md](./docs/installation.md). Versioned script installs use the same release tag for the binary and companion skills, and report any partial skill failure explicitly.

## Why use it

- One command surface for dependency freshness, RustSec vulnerabilities and informational warnings, yanked resolved crates, unused dependencies, unsafe code, dependency trees, and a graded quality summary.
- One JSON shape per subcommand, with stdout reserved for machine-readable output and diagnostics kept on stderr.
- Workspace-aware dependency reporting: `deps` groups by crate name plus resolved version, and tells you which members actually own each result.
- A single `quality` grade that stays honest about partial runs through `complete`, `measured_weight`, and `unavailable`.

## Real examples

These are selected-field excerpts from real runs on this repository, not the complete command payloads. For the full canonical JSON for each command, see [docs/commands.md](./docs/commands.md).

The project-health pass is the shortest end-to-end workflow:

```bash
cargo upkeep quality --json
```

```json
{
  "score": 100.0,
  "grade": "A",
  "complete": false,
  "measured_weight": 0.45,
  "unavailable": [
    {
      "name": "Security",
      "weight": 0.25,
      "reason": "failed",
      "detail": "failed to fetch RustSec advisory database"
    },
    {
      "name": "Unused dependencies",
      "weight": 0.15,
      "reason": "not_installed",
      "detail": "cargo-machete is not installed; install with `cargo install cargo-machete`"
    },
    {
      "name": "Unsafe code",
      "weight": 0.15,
      "reason": "not_installed",
      "detail": "cargo-geiger is not installed; install with `cargo install cargo-geiger`"
    }
  ]
}
```

Selected fields from a real run on this repository on August 30, 2026. The full `quality` contract, including `breakdown`, recommendation ordering, and CI guidance, is in [docs/commands.md#quality](./docs/commands.md#quality).

Security-aware dependency checks use the same interface:

```bash
cargo upkeep deps --json --security
```

```json
{
  "total": 15,
  "checked": 15,
  "outdated": 0,
  "major": 0,
  "minor": 0,
  "patch": 0,
  "warnings": [
    "security scan uses Cargo.lock and reports direct workspace dependencies only"
  ],
  "security": {
    "summary": {
      "critical": 0,
      "high": 0,
      "moderate": 0,
      "low": 0,
      "total": 0
    },
    "packages": []
  }
}
```

Selected fields from a real run on this repository on August 30, 2026. The full `deps` contract, including `packages`, `skipped_packages`, and workspace attribution rules, is in [docs/commands.md#deps](./docs/commands.md#deps).

For the full security picture, `cargo upkeep audit` reports vulnerabilities
separately from informational `notice`, `unmaintained`, and `unsound` advisories
and yanked resolved versions. Warnings are actionable findings, but they are not
vulnerabilities and do not change the vulnerability summary or `quality` grade.

## Docs

- [docs/installation.md](./docs/installation.md): crates.io, `cargo-binstall`, install script, and source installs
- [docs/commands.md](./docs/commands.md): full command reference plus canonical JSON examples for every subcommand
- [docs/spec.md](./docs/spec.md): CLI contract, comparison with the underlying tools, rate limiting, and test-tooling notes
- [docs/releasing.md](./docs/releasing.md): automated releases, conventional commits, and the pre-1.0 version policy

## Invocation forms

The normal form is `cargo upkeep <command>`. The direct binary form, the compatibility alias, and the exact contract around them are documented once in [docs/spec.md#cli-contract](./docs/spec.md#cli-contract).

## Project links

Contributing starts with the open [GitHub issues](https://github.com/llbbl/upkeep-rs/issues) and a local `just check`. Release history lives in [CHANGELOG.md](./CHANGELOG.md). Licensing and credits live in [LICENSE](./LICENSE) and [docs/spec.md](./docs/spec.md).
