# cargo-upkeep

![CI](https://github.com/llbbl/cargo-upkeep/actions/workflows/ci.yml/badge.svg)
![crates.io](https://img.shields.io/crates/v/cargo-upkeep.svg)
![License](https://img.shields.io/crates/l/cargo-upkeep.svg)

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

```bash
curl -fsSL https://raw.githubusercontent.com/llbbl/cargo-upkeep/main/scripts/install.sh | bash
```

### From source (requires Rust 1.70+)

```bash
git clone https://github.com/llbbl/cargo-upkeep
cd cargo-upkeep
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
  "command": "deps",
  "outdated": [
    {
      "name": "serde",
      "current": "1.0.197",
      "latest": "1.0.204",
      "kind": "minor"
    }
  ]
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

## Claude Code skills

Use the companion Claude Code skills for guided workflows:

- `/upkeep-deps`: `skills/upkeep-deps/SKILL.md`
- `/upkeep-audit`: `skills/upkeep-audit/SKILL.md`
- `/upkeep-quality`: `skills/upkeep-quality/SKILL.md`

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

## License and credits

MIT licensed. See `LICENSE`.
Inspired by the JS/TS `upkeep` project and the Rust maintenance tool ecosystem.
