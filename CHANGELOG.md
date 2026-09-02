# Changelog

All notable changes to this project will be documented in this file.

## [0.4.4] - 2026-09-02

### Features

- **python:** Detect pip and pip-tools projects and refuse to guess
## [0.4.3] - 2026-09-01

### Features

- **python:** Add a Poetry backend for cargo upkeep python
## [0.4.2] - 2026-09-01

### Features

- **python:** Add cargo upkeep python with a uv adapter
## [0.4.1] - 2026-09-01

### Features

- **quality:** Add --require-complete and fail when nothing was measured
## [0.4.0] - 2026-08-31

### Features

- **audit:** Surface informational advisories and yanked crates
## [0.3.15] - 2026-08-31

### Miscellaneous

- **package:** Exclude dev-only files from published crate
## [0.3.14] - 2026-08-31

### Bug Fixes

- **unsafe:** Report an outdated cargo-geiger instead of bad JSON
## [0.3.13] - 2026-08-31

### Bug Fixes

- **analyzers:** Unbreak unused and unsafe against released tools
## [0.3.12] - 2026-08-31

### Bug Fixes

- **deps:** Key resolved package names by dependency kind
## [0.3.11] - 2026-08-31

### Bug Fixes

- **analyzers:** Match unknown-flag wording per line
## [0.3.10] - 2026-08-31

### Bug Fixes

- **audit:** Let the advisory database be read from a local path
## [0.3.9] - 2026-08-31

### Bug Fixes

- **deps:** Poison ambiguous package names instead of guessing
## [0.3.8] - 2026-08-31

### Bug Fixes

- **quality:** Collapse the duplicated clippy penalty formula
## [0.3.7] - 2026-08-31

### Bug Fixes

- **analyzers:** Stop misattributing external tool failures
## [0.3.6] - 2026-08-31

### Bug Fixes

- **quality:** Honor virtual workspace MSRV
## [0.3.5] - 2026-08-31

### Bug Fixes

- **deps:** Key resolution by dependency kind
## [0.3.4] - 2026-08-31

### Bug Fixes

- **installer:** Pin skills to release version
## [0.3.3] - 2026-08-31

### Bug Fixes

- **deps:** Make registry lookups failure-tolerant
## [0.3.2] - 2026-08-30

### Bug Fixes

- **tree:** Preserve shared dependency subtrees
## [0.3.1] - 2026-08-30

### Bug Fixes

- **quality:** Exclude unmeasured metrics from the score instead of assuming health
## [0.3.0] - 2026-08-30

### Bug Fixes

- **deps:** Resolve dependencies per workspace member
- **deps:** Classify 0.x updates by cargo semver compatibility
- **lint:** Lint test targets and fix io_other_error

### Features

- **release:** Bump and publish versions automatically
## [0.2.0] - 2026-08-30

### Miscellaneous

- Remove beads task tracking

### Security

- **deps:** Bulk dependency update and raise MSRV to 1.96
## [0.1.7] - 2026-02-06

### Bug Fixes

- **ci:** Regenerate lockfile on version bump
## [0.1.6] - 2026-02-06

### Bug Fixes

- **ci:** Use --locked and env var for crates.io publish
## [0.1.4] - 2026-02-05

### Security

- **deps:** Update all dependencies to latest versions
## [0.1.3] - 2026-02-05

### Features

- **install:** Add Claude Code skills installation and version sync
## [0.1.2] - 2026-02-05

### Features

- **skills:** Rename skills to upkeep-rs-* for uniqueness
## [0.1.1] - 2026-02-05

### Bug Fixes

- **ci:** Fix matrix ci
- **ci:** Use correct cross installation action
## [0.1.0] - 2026-02-05

### Miscellaneous

- Apply cargo fmt to detect.rs
- Add conventional commits, Just task runner, and fix repo URLs

