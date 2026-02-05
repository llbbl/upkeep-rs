#!/usr/bin/env bash
set -euo pipefail

mkdir -p coverage
cargo llvm-cov --workspace --all-features --lcov --output-path coverage/lcov.info
