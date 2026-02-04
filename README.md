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
cargo upkeep unsafe
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
