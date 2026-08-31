# Changelog

All notable changes to this project will be documented in this file.

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

