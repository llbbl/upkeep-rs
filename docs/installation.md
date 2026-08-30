# Installation

`cargo-upkeep` can be installed from crates.io, from prebuilt release assets, or from source.

## crates.io

```bash
cargo install cargo-upkeep
```

## cargo-binstall

If you use [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) for prebuilt binaries:

```bash
cargo binstall cargo-upkeep
```

## Install script

The install script downloads the matching release archive, verifies its SHA-256 checksum, installs the binary, and also installs the companion Claude Code skills unless you opt out.

Binary plus skills:

```bash
curl -fsSL https://raw.githubusercontent.com/llbbl/upkeep-rs/main/scripts/install.sh | bash
```

Binary only:

```bash
SKIP_SKILLS=1 curl -fsSL https://raw.githubusercontent.com/llbbl/upkeep-rs/main/scripts/install.sh | bash
```

The script also accepts:

- `VERSION` to pin a release tag instead of `latest`
- `INSTALL_DIR` to choose the binary install path
- `SKILLS_DIR` to change where the Claude Code skills are installed

## From source

Source installs use the toolchain declared in [Cargo.toml](../Cargo.toml).

```bash
git clone https://github.com/llbbl/upkeep-rs
cd upkeep-rs
cargo install --path .
```

## After install

The normal invocation is:

```bash
cargo upkeep <command>
```

For the direct binary form and compatibility alias, see [docs/spec.md#cli-contract](./spec.md#cli-contract).
