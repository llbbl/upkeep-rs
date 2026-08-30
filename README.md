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
  "minor": 0,
  "patch": 1,
  "packages": [
    {
      "name": "serde",
      "alias": null,
      "current": "1.0.197",
      "latest": "1.0.204",
      "required": "^1.0",
      "update_type": "patch",
      "dependency_type": "normal",
      "members": ["my-crate"]
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

`update_type` follows cargo's compatibility rule rather than raw semver field
comparison: the leftmost non-zero component is the one that carries breakage.

| Current | Latest | `update_type` | Why |
|---------|--------|---------------|-----|
| `1.2.3` | `2.0.0` | `major` | major differs |
| `1.2.3` | `1.3.0` | `minor` | compatible feature bump |
| `1.2.3` | `1.2.4` | `patch` | compatible fix |
| `0.8.5` | `0.10.2` | `major` | for `0.x` the minor carries breakage |
| `0.8.1` | `0.8.5` | `patch` | compatible within `0.8` |
| `0.0.1` | `0.0.2` | `major` | nothing is compatible under `0.0.z` |

So a `0.x` dependency moving to a new minor is reported `major`, because cargo
will not take that upgrade without a manifest change.

Entries are grouped by `(name, current)` and sorted by that pair. Each entry's
`members` array names the workspace members that declared it, sorted and
deduplicated; for a single-crate project it holds that crate's own name. A
workspace whose members resolve one crate to semver-incompatible versions
therefore produces one entry per resolved version:

```json
[
  {
    "name": "rand",
    "current": "0.8.5",
    "required": "^0.8",
    "members": ["core-lib"]
  },
  {
    "name": "rand",
    "current": "0.9.2",
    "required": "^0.9",
    "members": ["cli-app"]
  }
]
```

When members declare different requirement strings that resolve to the same
version, `required` and `alias` come from a single representative edge: the one
with the smallest `(member, required, alias)` tuple. The member name is the first
key, so entries are attributed to the first member in sorted order; ties on the
member are broken by **byte order on the requirement string**, not by semver. So
for one member declaring both `=0.2.100` and `^0.1`, `=0.2.100` wins — `=` is
`0x3D` and `^` is `0x5E`. If a group spans several dependency kinds,
`dependency_type` follows the precedence normal > build > dev.

Each entry under `security.packages` carries the same `members` array, so a
vulnerable version is attributed to the members that actually resolved to it.

### audit

Scan for RustSec advisories.

> **Scope:** advisories are matched only against dependencies resolved from
> crates.io. Path, git, vendored, and alternate-registry dependencies are
> skipped. This is upstream `rustsec` behaviour as of 0.33, which avoids false
> positives from local crates whose names collide with advisory crates. The
> same scope applies to `deps --security` and to the `quality` grade.

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
