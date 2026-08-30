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
  "checked": 8,
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

**`total` and `checked` are different units.** `total` counts dependency
*edges* — every declaration by every workspace member, dev and build kinds
included, with no deduplication — so one crate listed in both `[dependencies]`
and `[dev-dependencies]` counts twice. `checked` counts what `outdated` and
`skipped_packages` count: *groups*, where edges are merged by
`(name, resolved version)`, so that same crate counts once.

`checked` is the number of dependencies the freshness question was actually
settled for — those compared against crates.io, plus those with no registry
release to be behind (git, path, target-specific and inactive-optional
dependencies). Dependencies the registry could not answer for are excluded.
`total - skipped` is **not** a substitute: it subtracts a group count from an
edge count, and can report comparisons that never happened.

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
  "score": 89.29,
  "grade": "B",
  "complete": false,
  "measured_weight": 0.85,
  "breakdown": [
    { "name": "Dependency freshness", "score": 80.0, "weight": 0.2 },
    { "name": "Security", "score": 100.0, "weight": 0.25 },
    { "name": "Unused dependencies", "score": null, "weight": 0.15 },
    { "name": "Unsafe code", "score": 90.0, "weight": 0.15 },
    { "name": "Clippy", "score": 76.0, "weight": 0.15 },
    { "name": "MSRV", "score": 100.0, "weight": 0.1 }
  ],
  "unavailable": [
    {
      "name": "Unused dependencies",
      "weight": 0.15,
      "reason": "not_installed",
      "detail": "cargo-machete is not installed; install with `cargo install cargo-machete`"
    }
  ],
  "recommendations": [
    "Update outdated dependencies.",
    "Fix clippy warnings and errors.",
    "Reduce unsafe code usage."
  ]
}
```

#### Interpreting a partial result

Six metrics contribute to the grade, each with a fixed weight. When an analyzer
cannot run, that metric is **excluded and the rest renormalized** — its weight
leaves the denominator rather than contributing a default value. `score` is
therefore always "of what we could measure, rescaled to 0-100".

The example above is that arithmetic in full. `cargo-machete` was not installed,
so its 0.15 leaves the denominator and 0.85 of the weight remains:

```text
0.20*80 + 0.25*100 + 0.15*90 + 0.15*76 + 0.10*100 = 75.9
75.9 / 0.85                                       = 89.29   -> B
```

Both alternatives are worse. Counting the unmeasured metric as 0 gives
`75.9` — a `C` for not having installed an optional tool. Defaulting it to 100,
which this tool used to do, gives `90.9` — an `A` awarded partly for a check
that never ran.

`recommendations` are ordered by weighted impact, not by score: dependency
freshness is the first entry at `(100-80)*0.20 = 4.0`, ahead of clippy's
`(100-76)*0.15 = 3.6`, even though clippy scored far worse.

There is no penalty for an unavailable metric: not having `cargo-geiger`
installed does not make a project worse. But there is no credit either, which is
the point — an unmeasured check must not read as a passed one.

| Field | Meaning |
|-------|---------|
| `score` | Weighted mean over the measured metrics, or `null` if none could be measured |
| `grade` | Letter for `score`, or `null` for the same reason |
| `complete` | `true` only when all six metrics were measured |
| `measured_weight` | Fraction of total weight behind `score`, from `0.0` to `1.0` |
| `breakdown[].score` | `null` for a metric that was not measured — never a substituted number |
| `unavailable[].reason` | `not_installed` (an optional tool is absent) or `failed` (the analyzer ran and failed) |

**If you gate CI on the grade, check `complete` as well.** An `A` over 40% of the
weight is not an `A`, and `grade` alone cannot tell you which you have. Gate on
`complete == true`, or on `measured_weight` clearing a threshold you accept.

`reason` distinguishes the two causes because only one is yours to fix:
`not_installed` is resolved by installing the tool named in `detail`, which
carries the exact `cargo install` command. `failed` means an analyzer that
should have worked did not. Neither says anything about project health.

When nothing at all could be measured, `score` and `grade` are `null` rather
than a number — there is no honest value to report. The text output says
`Score: unavailable` in that case, and states any incompleteness above the score
rather than below it.

The same rule applies *within* a metric. Dependency freshness is scored over the
dependencies whose latest version was actually fetched — the `checked` count
from `deps`, not the declared `total`. If crates.io is unreachable for some of
them, those leave that metric's denominator instead of counting as up to date,
and if none could be checked the metric is `failed` rather than a perfect 100.

So when the registry is the only thing unreachable, the run reports
`complete: false` with 0.80 of the weight measured, not an `A`. A genuinely
offline run measures less than that: the security metric fetches the RustSec
advisory database over the network and fails too, taking another 0.25 with it.
Read `measured_weight` rather than assuming a figure.

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

1. Create or pick up a [GitHub issue](https://github.com/llbbl/upkeep-rs/issues).
2. Keep changes focused and add tests for new behavior.
3. Run `cargo fmt`, `cargo clippy`, and `cargo test` before submitting.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.

This project uses [Conventional Commits](https://www.conventionalcommits.org/) and [git-cliff](https://git-cliff.org/) for automated changelog generation.

## License and credits

MIT licensed. See `LICENSE`.
Inspired by the JS/TS [upkeep](https://github.com/llbbl/upkeep) project and the Rust maintenance tool ecosystem.
