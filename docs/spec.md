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

`audit` checks RustSec advisories against the resolved lockfile. In practice that means resolved crates.io packages: path, git, vendored, and alternate-registry dependencies are not reported as advisory matches. The same effective RustSec scope applies to `deps --security` and to the security component inside `quality`.

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

## Testing notes

- The canonical JSON examples in [commands.md](./commands.md) are checked in Rust tests against serialized representative output values.
- Network-dependent dependency tests skip when crates.io is unavailable unless the environment explicitly requires them.
- Full behavior coverage for `unused` and `unsafe-code` requires the matching optional cargo subcommands to be installed.

## Source of truth

- Build requirements and the declared Rust toolchain floor: [Cargo.toml](../Cargo.toml)
- Release history: [CHANGELOG.md](../CHANGELOG.md)
- License: [LICENSE](../LICENSE)
- Inspiration: the JS/TS [upkeep](https://github.com/llbbl/upkeep) project and the Rust maintenance tools listed above
