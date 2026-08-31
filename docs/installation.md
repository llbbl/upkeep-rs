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

The install script resolves one release tag, downloads and verifies the matching binary archive, and installs the companion Claude Code skills from that same tag unless you opt out. This keeps the binary and its skill instructions on the same version.

Binary plus skills:

```bash
curl -fsSL https://raw.githubusercontent.com/llbbl/upkeep-rs/main/scripts/install.sh | bash
```

Binary only:

```bash
SKIP_SKILLS=1 curl -fsSL https://raw.githubusercontent.com/llbbl/upkeep-rs/main/scripts/install.sh | bash
```

The script also accepts:

- `VERSION` to pin the binary, checksum, and skills to an exact release tag instead of resolving `latest`
- `INSTALL_DIR` to choose the binary install path
- `SKILLS_DIR` to change where the Claude Code skills are installed

After installation, the script lists skills under separate `installed`, `skipped`, and `failed` headings. If any requested skill fails, the script exits with a nonzero status after printing that summary; the verified binary remains installed. `SKIP_SKILLS=1` bypasses skill downloads and reports all companion skills as skipped.

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
