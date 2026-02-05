# cargo-upkeep

![CI](https://github.com/llbbl/upkeep-rs/actions/workflows/ci.yml/badge.svg)
![crates.io](https://img.shields.io/crates/v/cargo-upkeep.svg)

Unified Rust project maintenance CLI.

One install, one interface, unified output for common maintenance tasks like dependency updates,
security audits, and project health scoring.

## Status

Work in progress.

## Installation

### From crates.io

```bash
cargo install cargo-upkeep
```

### Using cargo-binstall

Requires cargo-binstall (https://github.com/cargo-bins/cargo-binstall):

```bash
cargo install cargo-binstall
```

```bash
cargo binstall cargo-upkeep
```

### From install script

Installs the binary and Claude Code skills:

```bash
curl -fsSL https://raw.githubusercontent.com/llbbl/upkeep-rs/main/scripts/install.sh | bash
```

Binary only (skip skills):

```bash
SKIP_SKILLS=1 curl -fsSL https://raw.githubusercontent.com/llbbl/upkeep-rs/main/scripts/install.sh | bash
```

### From source (requires Rust 1.70+)

```bash
git clone https://github.com/llbbl/upkeep-rs
cd upkeep-rs
cargo install --path .
```

## Usage

```bash
cargo upkeep <command>
```

Direct binary invocation also works:

```bash
cargo-upkeep upkeep <command>
```

Global flags:

```bash
--json
--verbose
--log-level <level>
```

### detect

Detect project configuration (edition, workspace, features).

```bash
cargo upkeep detect --json
```

```json
{
  "command": "detect",
  "workspace": true,
  "edition": "2021",
  "members": 3
}
```

### deps

Report outdated dependencies with semver classification.

`deps --security` requires `Cargo.lock`. If it's missing, generate one with:

```bash
cargo generate-lockfile
```

```bash
cargo upkeep deps --json
```

```json
{
  "total": 10,
  "outdated": 1,
  "major": 0,
  "minor": 1,
  "patch": 0,
  "packages": [
    {
      "name": "serde",
      "alias": null,
      "current": "1.0.197",
      "latest": "1.0.204",
      "required": "^1.0",
      "update_type": "minor",
      "dependency_type": "normal"
    }
  ],
  "skipped": 0,
  "skipped_packages": [],
  "warnings": [],
  "workspace": false,
  "members": [],
  "skipped_members": []
}
```

### audit

Scan for RustSec advisories.

```bash
cargo upkeep audit --json
```

```json
{
  "command": "audit",
  "vulnerabilities": [
    {
      "crate": "time",
      "advisory": "RUSTSEC-2020-0071",
      "severity": "high",
      "patched": "0.2.23"
    }
  ]
}
```

### quality

Generate a project health grade with breakdown.

```bash
cargo upkeep quality --json
```

```json
{
  "command": "quality",
  "grade": "B",
  "scores": {
    "dependencies": 82,
    "security": 95,
    "clippy": 70,
    "msrv": 80
  }
}
```

### tree

Enhanced dependency tree output.

```bash
cargo upkeep tree --json
```

```json
{
  "command": "tree",
  "root": "cargo-upkeep",
  "dependencies": [
    {
      "name": "clap",
      "version": "4.5.1",
      "direct": true
    }
  ]
}
```

### unused

Detect unused dependencies using cargo-machete.

Requires cargo-machete to be installed:

```bash
cargo install cargo-machete
```

```bash
cargo upkeep unused --json
```

```json
{
  "unused": [
    {
      "name": "some-crate",
      "dependency_type": "normal",
      "confidence": "high"
    }
  ],
  "possibly_unused": ["another-crate"]
}
```

### unsafe-code

Analyze unsafe code usage in dependencies using cargo-geiger.

Requires cargo-geiger to be installed:

```bash
cargo install cargo-geiger
```

```bash
cargo upkeep unsafe-code --json
```

```json
{
  "summary": {
    "packages": 5,
    "unsafe_functions": 10,
    "unsafe_impls": 2,
    "unsafe_traits": 0,
    "unsafe_blocks": 15,
    "unsafe_expressions": 3,
    "total_unsafe": 30
  },
  "packages": [
    {
      "name": "libc",
      "version": "0.2.155",
      "package_id": "libc 0.2.155 (registry+https://github.com/rust-lang/crates.io-index)",
      "unsafe_functions": 10,
      "unsafe_impls": 2,
      "unsafe_traits": 0,
      "unsafe_blocks": 15,
      "unsafe_expressions": 3,
      "total_unsafe": 30
    }
  ]
}
```

## Claude Code skills

Use the companion Claude Code skills for guided workflows:

- `/upkeep-rs-deps`: `skills/upkeep-rs-deps/SKILL.md`
- `/upkeep-rs-audit`: `skills/upkeep-rs-audit/SKILL.md`
- `/upkeep-rs-quality`: `skills/upkeep-rs-quality/SKILL.md`

## Comparison

| Tool | Focus | Where cargo-upkeep fits |
| --- | --- | --- |
| cargo-audit | RustSec vulnerability scanning | `cargo upkeep audit` wraps advisory scanning with unified output |
| cargo-outdated | Outdated dependencies | `cargo upkeep deps` reports with semver classification |

## Rate limiting

Crates.io requests are serialized and rate-limited to roughly one request per second.
Large dependency sets will take at least one second per crate, plus network time.

## Test tooling

- Some integration tests use `httpmock` (dev dependency only) for crates.io client behavior.
- Full test coverage for `unused` and `unsafe-code` requires `cargo-machete` and `cargo-geiger`.

Optional tooling installs:

```bash
cargo install cargo-machete
cargo install cargo-geiger
```

## Contributing

1. Create or pick up a task in `bd`.
2. Keep changes focused and add tests for new behavior.
3. Run `cargo fmt`, `cargo clippy`, and `cargo test` before submitting.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.

This project uses [Conventional Commits](https://www.conventionalcommits.org/) and [git-cliff](https://git-cliff.org/) for automated changelog generation.

## License and credits

MIT licensed. See `LICENSE`.
Inspired by the JS/TS [upkeep](https://github.com/llbbl/upkeep) project and the Rust maintenance tool ecosystem.
