set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
  @just --list

help:
  @just --list

# Build the project
build:
  cargo build

# Build release binary
build-release:
  cargo build --release

# Run the CLI with arguments
run *args:
  cargo run -- {{args}}

# Run tests
test:
  cargo test
  just test-installer

# Run fixture-driven installer tests without network access
test-installer:
  bash ./tests/install.sh

# Run tests with output
test-verbose:
  cargo test -- --nocapture

# Run tests and watch for changes
test-watch:
  cargo watch -x test

# Run clippy linter
lint:
  cargo clippy --locked --all-targets -- -D warnings

# Run clippy and fix issues
lint-fix:
  cargo clippy --all-targets --fix --allow-dirty --allow-staged

# Format code
format:
  cargo fmt

# Check formatting without changing files
format-check:
  cargo fmt --check

# Run all checks (lint, format, test)
check:
  just format-check
  just lint
  just test

# Full CI pipeline
ci:
  just check
  just build-release

# Clean build artifacts
clean:
  cargo clean

# Generate changelog
changelog:
  git cliff -o CHANGELOG.md

# Preview unreleased changes
changelog-preview:
  git cliff --unreleased

# Bump patch version (0.0.x)
bump-patch:
  just bump-version patch

# Bump minor version (0.x.0)
bump-minor:
  just bump-version minor

# Bump major version (x.0.0)
bump-major:
  just bump-version major

# Write an explicit version to every place it appears
set-version version:
  #!/usr/bin/env bash
  set -euo pipefail
  CURRENT=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
  NEW="{{version}}"
  # `sed -i.bak` is the portable spelling. Bare `-i ''` is macOS-only and bare `-i`
  # is GNU-only; this recipe runs both locally and on ubuntu in CI.
  sed -i.bak "s/^version = \"$CURRENT\"/version = \"$NEW\"/" Cargo.toml && rm -f Cargo.toml.bak
  # Update only this crate's own lockfile entry. `cargo generate-lockfile` re-resolves
  # the whole graph, which can pull newer transitive deps into a release commit — and
  # resolver v2 is not MSRV-aware, so that can silently break the declared rust-version.
  cargo update --workspace
  # Update skill versions
  for skill in skills/upkeep-rs-*/SKILL.md; do
    sed -i.bak "s/^version: .*/version: $NEW/" "$skill" && rm -f "$skill.bak"
  done
  echo "Set version: $CURRENT -> $NEW"

# Print the next version from conventional commits, capped at minor (stdout = version only)
next-version:
  #!/usr/bin/env bash
  set -euo pipefail
  CURRENT=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
  NEXT=$(git cliff --bumped-version 2>/dev/null | tail -1 | sed 's/^v//')
  if [ -z "$NEXT" ]; then
    echo "could not compute a next version from git cliff" >&2
    exit 1
  fi
  IFS='.' read -r cmaj cmin _ <<< "$CURRENT"
  IFS='.' read -r nmaj _ _ <<< "$NEXT"
  # Never emit a major bump automatically. While the crate is 0.x this is also
  # semantically right, because cargo already treats a 0.x minor bump as breaking.
  if [ "$nmaj" -gt "$cmaj" ]; then
    NEXT="$cmaj.$((cmin + 1)).0"
    echo "warning: commits imply a major bump; capping at minor -> $NEXT" >&2
    if [ "$cmaj" -ne 0 ]; then
      echo "warning: the crate is past 1.0, so this cap now understates a breaking change" >&2
    fi
  fi
  echo "$NEXT"

# Bump the version from conventional commits, capped at minor
bump-auto:
  #!/usr/bin/env bash
  set -euo pipefail
  CURRENT=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
  NEXT=$(just next-version)
  if [ "$NEXT" = "$CURRENT" ]; then
    echo "No releasable commits since v$CURRENT; nothing to bump."
    exit 0
  fi
  just set-version "$NEXT"

# Bump version by type (patch, minor, major)
bump-version bump:
  #!/usr/bin/env bash
  set -euo pipefail
  CURRENT=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
  IFS='.' read -r major minor patch <<< "$CURRENT"
  case "{{bump}}" in
    patch) patch=$((patch + 1)) ;;
    minor) minor=$((minor + 1)); patch=0 ;;
    major) major=$((major + 1)); minor=0; patch=0 ;;
    *) echo "Invalid bump type: {{bump}}"; exit 1 ;;
  esac
  just set-version "$major.$minor.$patch"

# Commit version bump and create tag
commit-version:
  #!/usr/bin/env bash
  set -euo pipefail
  VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
  git add Cargo.toml Cargo.lock skills/upkeep-rs-*/SKILL.md
  git commit -m "chore(release): bump version to v$VERSION"
  git tag "v$VERSION"
  echo "Created tag v$VERSION"
  echo "Push with: git push origin main --tags"

# Show current version
show-version:
  @grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/'

# Install the CLI locally
install:
  cargo install --path .

# Uninstall the CLI
uninstall:
  cargo uninstall cargo-upkeep

# Run security audit
audit:
  cargo upkeep audit

# Check outdated dependencies
deps:
  cargo upkeep deps

# Run quality check
quality:
  cargo upkeep quality
