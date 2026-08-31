# cargo-upkeep Behavior Notes

This page carries the details that do not belong above the fold in the repository README.

## CLI contract

`cargo-upkeep` supports three invocation forms:

- `cargo upkeep <command>`: the normal cargo-subcommand workflow
- `cargo-upkeep <command>`: direct binary invocation
- `cargo-upkeep upkeep <command>`: supported compatibility alias

Use the first form in user-facing examples unless you are documenting the direct binary explicitly. The compatibility alias exists so both parser shapes stay supported, but it should only be explained once.

Global flags are shared across both forms:

- `-v`, `--verbose`
- `--json`
- `--log-level <level>`

## Comparison

`cargo-upkeep` is an integration layer over established Rust tooling, not a claim that those tools are obsolete.

| Tool | Primary focus | Where `cargo-upkeep` fits |
| --- | --- | --- |
| `cargo-outdated` | Latest available crate versions | `cargo upkeep deps` wraps freshness checks in a stable JSON contract and adds workspace-member attribution plus update classification |
| `cargo-audit` | RustSec advisory scanning | `cargo upkeep audit` exposes advisories in the same JSON style used by the rest of the CLI |
| `cargo-machete` | Unused dependency detection | `cargo upkeep unused` normalizes `cargo-machete` findings into a consistent output shape and feeds them into `quality` when installed |
| `cargo-geiger` | Unsafe code counting | `cargo upkeep unsafe-code` reports unsafe totals in a consistent schema and lets `quality` include them when installed |
| `cargo clippy` | Linting and code-quality diagnostics | `cargo upkeep quality` incorporates clippy results into a single grade instead of asking callers to merge lint output themselves |

The point is not that `cargo-upkeep` replaces deeper tool-specific workflows. It gives one entrypoint, one stdout contract, and one CI-friendly health summary for the common maintenance pass.

## Runtime notes

### Dependency freshness and rate limiting

Crates.io lookups are serialized and delayed to roughly one request per second. Large manifests therefore take at least one second per uncached crate, plus network time.

### Security scope

`audit` checks RustSec advisories against the resolved lockfile. In practice that means resolved crates.io packages: path, git, vendored, and alternate-registry dependencies are not reported as advisory matches. Vulnerabilities remain in `vulnerabilities` and its severity `summary`; RustSec `notice`, `unmaintained`, and `unsound` informational advisories are reported separately in `warnings`. Standalone `audit` also refreshes the crates.io index and reports yanked resolved versions as warnings. A registry or per-package lookup failure fails the standalone command, so an unavailable yanked scan cannot look clean.

Warnings are not vulnerabilities and never contribute to `AuditSummary`. The `deps --security` output and the security component inside `quality` intentionally run a vulnerability-only scan, so informational/yanked findings and crates.io-index availability cannot change the quality grade.

### Advisory database source

By default the advisory database is cloned or fetched into the shared
`~/.cargo/advisory-db`. That directory is mutable state shared with every other
tool on the machine, and it fails in two distinct ways.

Concurrent `cargo-upkeep` runs do not corrupt it: rustsec takes an outer file
lock on `~/.cargo/advisory-db..lock` and waits on it for up to five minutes, so
simultaneous audits serialize. The cost is a stall long enough to resemble a
hung job, not a failure.

A stale `.git/index.lock` — left behind by a killed process, or held by a
non-rustsec git client — is the hard failure. That lock is taken underneath the
outer one, and `gix` makes a single attempt with no retry or backoff, so the
audit errors immediately.

Setting `UPKEEP_ADVISORY_DB` to a local advisory-database checkout reads that
path instead and fetches no advisory data. The layout is the advisory-db repository's own
(`crates/<package>/<RUSTSEC-ID>.md`), and the caller owns keeping it current —
`cargo-upkeep` will not update it. Standalone `audit` still refreshes the
crates.io index for yanked-package detection when the lockfile contains
crates.io packages; `UPKEEP_ADVISORY_DB` does not disable that check. The
vulnerability-only scans used by `quality` and `deps --security` do not perform
the yanked lookup.

A set-but-empty value is an error rather than a fallback to fetching.

### Optional tooling

Two commands depend on external cargo subcommands:

- `unused` requires `cargo-machete`
- `unsafe-code` requires `cargo-geiger`

When either tool is missing, `quality` reports that as an unavailable metric instead of pretending the project passed that check.

### Dependency freshness semantics

`deps` and the freshness portion of `quality` deliberately distinguish two units:

- `total`: declared dependency edges
- `checked`: grouped freshness comparisons that actually reached an answer

That distinction matters in workspaces and in repeated declarations. One crate listed in both `[dependencies]` and `[dev-dependencies]` is two declared edges but one grouped comparison. Because of that, `total - skipped` is not a valid denominator for freshness scoring.

The grouped `checked` count also excludes registry failures. If crates.io is unavailable, those dependencies were not measured and must not be scored as up to date.

### Quality semantics

`quality` renormalizes over the metrics that actually ran. An unavailable metric gives neither penalty nor credit: its weight leaves the denominator, `breakdown[].score` becomes `null`, and the metric is listed under `unavailable`.

If nothing can be measured, `score` and `grade` are `null`. If only some metrics run, treat `complete` and `measured_weight` as part of the public contract, not as optional metadata.

The exit status is part of that contract too. `quality` exits nonzero when `score` is `null`, and when `--require-complete` is passed and `complete` is false; every other run exits 0, including a partial one that still produced a score. In both failing cases the report is written to stdout in full before the process fails, so the status is added to the analysis rather than substituted for it. The reason goes to stderr — a JSON error object under `--json`, a plain line otherwise. Exit codes are tabulated in [commands.md](./commands.md#exit-codes).

## Testing notes

- The canonical JSON examples in [commands.md](./commands.md) are checked in Rust tests against serialized representative output values.
- Network-dependent dependency tests skip when crates.io is unavailable unless the environment explicitly requires them.
- The CLI tests point `UPKEEP_ADVISORY_DB` at a committed fixture database, so they never fetch or lock `~/.cargo/advisory-db`. Analyzer tests open the fixture directly for advisory-warning mapping; yanked mapping and failure behavior use synthetic lookup results and never contact the crates.io index.
- Full behavior coverage for `unused` and `unsafe-code` requires the matching optional cargo subcommands to be installed.

## Source of truth

- Build requirements and the declared Rust toolchain floor: [Cargo.toml](../Cargo.toml)
- Release history: [CHANGELOG.md](../CHANGELOG.md)
- License: [LICENSE](../LICENSE)
- Inspiration: the JS/TS [upkeep](https://github.com/llbbl/upkeep) project and the Rust maintenance tools listed above
