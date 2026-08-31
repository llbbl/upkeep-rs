# Releasing

Releases are automated. A releasable commit merged to `main` makes the release
workflow calculate a version, run `just check`, update versioned files and the
changelog, tag the commit, and publish it. Publishing to crates.io cannot be
undone, so check the result before merging:

```bash
just next-version
just changelog-preview
```

Do not bump versions or create tags by hand during the normal release flow.

The bump rules are verified against the pinned `git-cliff 2.13.1` used by both
pull-request CI and the release workflow. Run the same isolated fixture locally
when changing `cliff.toml` or release automation:

```bash
just test-release-policy
```

This check downloads no project dependencies, but it requires that exact
`git-cliff` version. It is deliberately separate from `just check`, so the
ordinary Rust development loop and Cargo cache do not depend on a release tool.

## Pre-1.0 version policy

The project remains on the current minor line until Logan explicitly marks the
next minor milestone. While the crate is pre-1.0:

- `feat:` is an honest feature entry in the changelog and bumps **patch** (for
  example, `0.4.0` to `0.4.1`).
- `feat!:` or a `BREAKING CHANGE` footer is an explicit breaking feature and
  bumps **minor** (for example, `0.4.7` to `0.5.0`). Merge one only when the next
  minor milestone has been approved.
- `fix:`, `perf:`, `refactor:`, `revert:`, `build:`, and releasable `chore:`
  commits bump patch.
- `docs:`, `style:`, `test:`, `ci:`, `chore(release):`, and routine
  `chore(deps):` commits do not release. A security-relevant `chore(deps):`
  remains releasable.

Keep prefixes truthful; do not label a feature as a fix to control the version.
Single-commit pull requests preserve that conventional commit when merged.
Multi-commit pull requests are squash-merged, so their final squash title and
body must carry the intended prefix and any breaking-change marker.

The release workflow caps automatic major bumps. Before 1.0 this means a
breaking change can advance the minor version but can never automatically
publish `1.0.0`.
