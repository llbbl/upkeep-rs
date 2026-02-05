# cargo-upkeep

Unified Rust project maintenance CLI.

## Status

Work in progress.

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

Global flags:

```bash
--json
--verbose
--log-level <level>
```

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
