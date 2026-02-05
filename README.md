# cargo-upkeep

Unified Rust project maintenance CLI.

## Status

Work in progress.

## Installation

### From source (requires Rust 1.70+)

```bash
git clone https://github.com/llbbl/cargo-upkeep
cd cargo-upkeep
cargo install --path .
```

### From crates.io (once published)

```bash
cargo install cargo-upkeep
```

### Using cargo-binstall (once published)

```bash
cargo binstall cargo-upkeep
```

## Usage

```bash
cargo upkeep <command>
```

Direct binary invocation also works:

```bash
cargo-upkeep upkeep <command>
```

Available commands:

```bash
cargo upkeep detect
cargo upkeep audit
cargo upkeep deps
cargo upkeep quality
cargo upkeep unused
cargo upkeep unsafe-code
cargo upkeep tree
```

Deps flags:

```bash
cargo upkeep deps --security
```

The security scan uses RustSec data from `Cargo.lock` and reports direct workspace dependencies.

Global flags:

```bash
--json
--verbose
--log-level <level>
```

## Rate limiting

Crates.io requests are serialized and rate-limited to roughly one request per second.
Large dependency sets will take at least one second per crate, plus network time.

## Test tooling

- Some integration tests use `httpmock` for crates.io client behavior.
- Full test coverage for `unused` and `unsafe-code` requires `cargo-machete` and `cargo-geiger`.

## Development

- Rust 1.70+
- `cargo build`

### Coverage

Install coverage tooling:

```bash
cargo install cargo-llvm-cov
```

Run coverage (LCOV output in `coverage/lcov.info`):

```bash
./scripts/coverage.sh
```
