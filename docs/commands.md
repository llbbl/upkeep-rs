# Command reference

All examples below use the recommended cargo-subcommand form, `cargo upkeep <command>`. For the direct binary form and compatibility alias, see [docs/spec.md#cli-contract](./spec.md#cli-contract).

## Global flags

Every command accepts:

- `-v`, `--verbose`
- `--json`
- `--log-level <level>`

## Exit codes

| Status | Meaning |
| --- | --- |
| `0` | The command ran and produced its result. |
| `1` | The command failed, or `quality` rejected its own result. Stdout is empty except in the `quality` cases below, which print the full report first. The diagnostic goes to stderr either way, as a JSON error object under `--json` and as a plain line otherwise. |
| `2` | The arguments were rejected. Usage goes to stderr. |

Findings are not failures. `audit` exits 0 with vulnerabilities in its report, `deps` exits 0 with outdated crates, and `unused` exits 0 with unused dependencies. Every command works this way, and `quality` is the only one that adds anything to it.

### `quality`

`quality` also exits nonzero when its own analysis did not measure enough to stand behind:

| Result | Status |
| --- | --- |
| `complete: true` | `0` |
| `complete: false`, a `score` was still produced | `0` — or `1` with `--require-complete` |
| `score: null`, nothing could be measured at all | `1`, always |

The report is printed first and in full, on stdout and unchanged, so a nonzero status never costs you the output that explains it. The reason is added on stderr, alongside the report rather than in place of it.

A *partial* analysis exiting 0 is the backward-compatible default: it produced a real number over a stated `measured_weight`, and a pipeline that accepts that is making a defensible choice. Pass `--require-complete` to reject it. A *total* failure is different in kind — there is no result for `complete` to qualify — so it exits nonzero either way, flag or no flag.

`--require-complete` means what it says: every metric must have run, including the ones that depend on optional tooling — `cargo-machete`, `cargo-geiger`, the `clippy` component, and a reachable RustSec advisory database. A runner without them fails the flag permanently, reporting missing tools rather than anything about the project. Install what you intend to measure in the job before gating on it, or gate on `measured_weight` instead.

This is the signal for callers that do not parse the JSON. If you do parse it, gate on `complete` and `measured_weight` as before; the fields are unchanged and remain the more precise control.

## detect

Detect workspace, package, tooling, and CI metadata for the current project.

```bash
cargo upkeep detect --json
```

<!-- cargo-upkeep-example:detect -->
```json
{
  "edition": "2021",
  "msrv": "1.70",
  "workspace": true,
  "members": [
    "core",
    "upkeep"
  ],
  "package": "upkeep",
  "version": "0.1.0",
  "dependencies": 3,
  "features": [
    "default"
  ],
  "targets": [
    "bin"
  ],
  "tooling": [
    "clippy"
  ],
  "ci": [
    "github-actions"
  ]
}
```

## deps

Report outdated dependencies and classify each update as `major`, `minor`, or `patch`. Add `--security` to attach RustSec findings for direct workspace dependencies resolved through `Cargo.lock`.

```bash
cargo upkeep deps --json --security
```

<!-- cargo-upkeep-example:deps -->
```json
{
  "total": 2,
  "checked": 2,
  "outdated": 1,
  "major": 0,
  "minor": 0,
  "patch": 1,
  "packages": [
    {
      "name": "serde",
      "alias": null,
      "current": "1.0.0",
      "latest": "1.0.1",
      "required": "^1.0",
      "update_type": "patch",
      "dependency_type": "normal",
      "members": [
        "core"
      ]
    }
  ],
  "skipped": 1,
  "skipped_packages": [
    {
      "name": "serde",
      "alias": null,
      "required": "^1.0",
      "reason": "target_specific",
      "dependency_type": "normal",
      "source": null,
      "target": "x86_64-unknown-linux-gnu"
    }
  ],
  "warnings": [
    "security scan uses Cargo.lock and reports direct workspace dependencies only"
  ],
  "security": {
    "summary": {
      "critical": 0,
      "high": 1,
      "moderate": 0,
      "low": 0,
      "total": 1
    },
    "packages": [
      {
        "name": "serde",
        "alias": null,
        "current": "1.0.0",
        "dependency_type": "normal",
        "members": [
          "core"
        ],
        "vulnerabilities": [
          {
            "advisory_id": "RUSTSEC-0000-0000",
            "severity": "high",
            "title": "Example",
            "fix_available": true
          }
        ]
      }
    ]
  },
  "workspace": true,
  "members": [
    "core"
  ],
  "skipped_members": [
    "legacy"
  ]
}
```

Notes:

- `--security` requires `Cargo.lock`. If it is missing, generate it with `cargo generate-lockfile` before rerunning the command.
- `--security` adds the advisory summary and package list, and warns that the scan is lockfile-based and limited to direct workspace dependencies.

### How to read `total` and `checked`

`total` counts declared dependency edges: each declaration by each workspace member, with no deduplication across normal, build, or dev sections.

`checked` counts the grouped freshness comparisons that actually reached an answer. The grouping key is `(name, resolved version)`, which means one crate declared twice can still count as one checked dependency, and one crate resolved to two versions in a workspace can count as two.

Because those are different units, `total - skipped` is not a valid substitute for `checked`. Subtracting a grouped skip count from an edge count can invent comparisons that never happened.

### Update classification

`update_type` follows Cargo compatibility rules, not raw semver field names. The leftmost non-zero component is the breaking boundary.

| Current | Latest | `update_type` | Why |
| --- | --- | --- | --- |
| `1.2.3` | `2.0.0` | `major` | major differs |
| `1.2.3` | `1.3.0` | `minor` | compatible feature bump |
| `1.2.3` | `1.2.4` | `patch` | compatible fix |
| `0.8.5` | `0.10.2` | `major` | in `0.x`, the minor carries breakage |
| `0.8.1` | `0.8.5` | `patch` | compatible within `0.8` |
| `0.0.1` | `0.0.2` | `major` | nothing is compatible under `0.0.z` |

### Grouping and workspace attribution

- `packages` are grouped by `(name, current)` and sorted by that pair.
- `members` on each package entry names the workspace members that actually resolved to that version.
- If two workspace members resolve the same crate name to semver-incompatible versions, `deps` emits one row per resolved version.
- When several edges collapse into one grouped row, `required` and `alias` come from one representative edge: the smallest `(member, required, alias)` tuple.
- If a grouped row spans several dependency kinds, `dependency_type` follows the precedence `normal > build > dev`.

### Registry failures and denominator rules

Some skipped dependencies are still considered checked because there was no meaningful registry comparison to make, such as `non_registry`, `target_specific`, and `optional_not_activated`. Registry-related skips are excluded from `checked` because the freshness question was never answered:

- `unsupported_registry` means the dependency comes from an alternate registry. `cargo-upkeep` preserves its `source` and `target` in `skipped_packages` but does not send its name to crates.io.
- `registry_unavailable` means the crates.io request for that specific crate failed because of an HTTP, status, or response-decoding error.
- `registry_metadata_missing` means crates.io responded successfully but provided no usable version.

Registry lookups are failure-tolerant. If one crate fails, successful sibling lookups are still compared and freshness is measured over that checked subset. `warnings` contains one deterministic, crate-named entry for each failed lookup. If none of the owed comparisons can be completed, `quality` reports dependency freshness as unavailable instead of a measured 100.

`missing_resolve` is also an unanswered freshness comparison: Cargo metadata contained the declaration but no resolved version to compare. It is excluded from `checked`, and a run containing only unresolved dependencies makes dependency freshness unavailable. Local path dependencies remain `non_registry`; their names are never sent to crates.io.

`ambiguous_resolve` is the same kind of unanswered comparison, for a narrower cause: one member resolved a package name to several distinct versions and the declaration matched none of the resolve-graph keys that would say which instance it meant — typically a crate with a custom `[lib] name` pulled in twice at different versions. Rather than report an arbitrary instance's version against that declaration's own requirement, `deps` skips the dependency. Because the precedence below is deliberate, it can also appear for an inactive optional or foreign-target declaration whose package name is ambiguous for unrelated reasons, with no custom `[lib] name` involved. Like `missing_resolve`, it is excluded from `checked` and can make dependency freshness unavailable; it takes precedence over `optional_not_activated` and `target_specific` so a refused comparison never re-enters the denominator as "not applicable".

That distinction is what `quality` uses for dependency freshness. If the registry could not answer, the denominator shrinks; those dependencies do not become implicitly healthy.

## audit

Report RustSec advisories for the resolved lockfile.

```bash
cargo upkeep audit --json
```

<!-- cargo-upkeep-example:audit -->
```json
{
  "vulnerabilities": [
    {
      "id": "RUSTSEC-0000-0000",
      "package": "serde",
      "package_version": "1.0.0",
      "severity": "high",
      "title": "Example",
      "path": [
        "root",
        "serde"
      ],
      "fix_available": true
    }
  ],
  "warnings": [
    {
      "kind": "unmaintained",
      "package": "example-old",
      "package_version": "0.1.0",
      "advisory_id": "RUSTSEC-0000-0001",
      "title": "Example crate is unmaintained",
      "path": [
        "root",
        "example-old"
      ],
      "fix_available": false
    }
  ],
  "summary": {
    "critical": 0,
    "high": 1,
    "moderate": 0,
    "low": 0,
    "total": 1
  }
}
```

Scope notes:

- Vulnerability and informational advisories are matched against resolved crates.io dependencies from the lockfile.
- `warnings` contains RustSec `notice`, `unmaintained`, and `unsound` advisories plus yanked resolved versions. Yanked entries have `null` advisory metadata and `fix_available` because a yank alone does not identify a replacement.
- Warnings are not vulnerabilities. They do not contribute to `summary`, and they do not affect the vulnerability-based security grade in `quality`.
- Standalone `audit` refreshes the crates.io index to check yanked versions. An index or per-package lookup failure fails the command rather than returning a false clean warning list; JSON errors are written to stderr.
- Path, git, vendored, and alternate-registry dependencies are not reported as advisory matches.
- `deps --security` and the security metric inside `quality` intentionally run the vulnerability-only scan: they do not fetch the index for yanked checks and do not consume informational warnings.

## quality

Compute a weighted project-health grade across dependency freshness, security, unused dependencies, unsafe code, clippy, and the declared Rust version contract.

```bash
cargo upkeep quality --json
```

Flags:

- `--require-complete` — exit nonzero when any metric could not be measured. See [Exit codes](#exit-codes).

<!-- cargo-upkeep-example:quality -->
```json
{
  "score": 97.65,
  "grade": "A",
  "complete": false,
  "measured_weight": 0.85,
  "breakdown": [
    {
      "name": "Dependency freshness",
      "score": 90.0,
      "weight": 0.2
    },
    {
      "name": "Security",
      "score": 100.0,
      "weight": 0.25
    },
    {
      "name": "Unused dependencies",
      "score": 100.0,
      "weight": 0.15
    },
    {
      "name": "Unsafe code",
      "score": null,
      "weight": 0.15
    },
    {
      "name": "Clippy",
      "score": 100.0,
      "weight": 0.15
    },
    {
      "name": "MSRV",
      "score": 100.0,
      "weight": 0.1
    }
  ],
  "unavailable": [
    {
      "name": "Unsafe code",
      "weight": 0.15,
      "reason": "not_installed",
      "detail": "cargo-geiger is not installed; install with `cargo install cargo-geiger`"
    }
  ],
  "recommendations": []
}
```

Notes:

- `complete` tells you whether every metric ran.
- `measured_weight` tells you how much of the total grade weight is actually represented.
- When a metric cannot run, `breakdown[].score` is `null` for that metric and the metric also appears under `unavailable`.
- The MSRV metric recognizes `package.rust-version` and, for virtual workspaces, `workspace.package.rust-version`, including member declarations that inherit it with `rust-version.workspace = true`.

### How partial results work

`quality` scores only the metrics that actually ran, then renormalizes that weighted total back onto a 0-100 scale. Unavailable metrics give neither penalty nor credit: their weight leaves the denominator entirely.

That is why `complete` and `measured_weight` are contract fields, not decoration. A partial `A` is only meaningful alongside how much of the total weight was actually measured.

### `not_installed` vs `failed`

- `not_installed` means an optional external tool such as `cargo-machete` or `cargo-geiger` is absent.
- `failed` means the analyzer ran but could not produce a valid measurement.

Those cases should be handled differently by callers, but neither one should be mistaken for a healthy project metric.

### No measurement means no score

If nothing at all can be measured, `score` and `grade` are `null`. The command does not substitute a default number because any number would read as a real grade.

### Recommendation ordering

`recommendations` are ordered by weighted impact, not by raw metric score. A slightly weaker high-weight metric can outrank a much worse low-weight metric because it changes the overall grade more.

### Freshness inside `quality`

The dependency freshness metric uses the grouped `checked` subset from `deps`, not declared edges and not `total - skipped`. Registry failures and unsupported registries therefore reduce coverage instead of inflating freshness. Partial results remain measured over successful comparisons; zero completed owed comparisons make the metric unavailable.

### CI guidance

Do not gate CI on `grade` alone. Gate on `complete == true`, or on a `measured_weight` threshold you explicitly accept in your pipeline.

If your job does not parse the JSON, run `cargo upkeep quality --require-complete` and let the exit status carry the same gate. Either way, a run that measured nothing fails on its own — see [Exit codes](#exit-codes).

## tree

Render a dependency tree with optional depth limits, duplicate-only filtering, reverse lookups, feature expansion, and dev-dependency suppression.

```bash
cargo upkeep tree --json --features
```

Flags:

- `--depth <depth>`
- `--duplicates`
- `--invert <crate>`
- `--features`
- `--no-dev`

<!-- cargo-upkeep-example:tree -->
```json
{
  "root": {
    "name": "root",
    "version": "0.1.0",
    "package_id": "root 0.1.0",
    "features": [
      "default"
    ],
    "dependencies": [
      {
        "name": "dep",
        "version": "1.2.3",
        "package_id": "dep 1.2.3",
        "features": [],
        "dependencies": [],
        "is_dev": false,
        "is_build": false,
        "duplicate": false
      }
    ],
    "is_dev": false,
    "is_build": false,
    "duplicate": false
  },
  "stats": {
    "total_crates": 2,
    "direct_deps": 1,
    "transitive_deps": 0,
    "duplicate_crates": 0
  }
}
```

## unused

Normalize `cargo-machete` findings into a stable JSON shape.

```bash
cargo upkeep unused --json
```

`unused` requires `cargo-machete` to be installed:

```bash
cargo install cargo-machete
```

<!-- cargo-upkeep-example:unused -->
```json
{
  "unused": [
    {
      "name": "tokio",
      "dependency_type": "dev",
      "confidence": "high"
    }
  ],
  "possibly_unused": [
    "serde"
  ]
}
```

## unsafe-code

Normalize `cargo-geiger` findings into a stable JSON shape.

```bash
cargo upkeep unsafe-code --json
```

`cargo upkeep unsafe --json` is supported as an alias for the same command.

`unsafe-code` requires `cargo-geiger` to be installed:

```bash
cargo install cargo-geiger
```

<!-- cargo-upkeep-example:unsafe-code -->
```json
{
  "summary": {
    "packages": 1,
    "unsafe_functions": 2,
    "unsafe_impls": 1,
    "unsafe_traits": 0,
    "unsafe_blocks": 3,
    "unsafe_expressions": 1,
    "total_unsafe": 7
  },
  "packages": [
    {
      "name": "ffi",
      "version": "0.1.0",
      "package_id": "ffi 0.1.0 (path+file://...)",
      "unsafe_functions": 2,
      "unsafe_impls": 1,
      "unsafe_traits": 0,
      "unsafe_blocks": 3,
      "unsafe_expressions": 1,
      "total_unsafe": 7
    }
  ]
}
```
