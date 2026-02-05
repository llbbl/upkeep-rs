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

# Run tests with output
test-verbose:
  cargo test -- --nocapture

# Run tests and watch for changes
test-watch:
  cargo watch -x test

# Run clippy linter
lint:
  cargo clippy -- -D warnings

# Run clippy and fix issues
lint-fix:
  cargo clippy --fix --allow-dirty --allow-staged

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
  NEW="$major.$minor.$patch"
  sed -i '' "s/^version = \"$CURRENT\"/version = \"$NEW\"/" Cargo.toml
  # Update skill versions
  for skill in skills/upkeep-rs-*/SKILL.md; do
    sed -i '' "s/^version: .*/version: $NEW/" "$skill"
  done
  echo "Bumped version: $CURRENT -> $NEW"

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
