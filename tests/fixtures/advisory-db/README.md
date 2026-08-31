# Advisory database fixture

A minimal stand-in for the RustSec advisory database, laid out the way
`rustsec::Database::open` expects (`crates/<package>/<RUSTSEC-ID>.md`).

`tests/cli.rs` points every `cargo-upkeep` invocation at this directory via
`UPKEEP_ADVISORY_DB`, so the integration tests never clone, fetch, or lock the
shared `~/.cargo/advisory-db`. Analyzer tests also open it directly to exercise
advisory and informational-warning mapping without running the separate
crates.io yanked-package lookup. That cache is mutable state shared with every
other tool on the machine. Concurrent rustsec-based runs serialize on an outer
flock rather than colliding, but they wait up to five minutes to do it; and a
stale `.git/index.lock` from a killed process hard-fails an audit outright,
because `gix` takes that inner lock with a single attempt and no retry.

**Every advisory in here is fabricated.** None of them describe a real
vulnerability in any crate, and none should ever be copied anywhere that a human
or a tool might mistake them for genuine. They exist so analyzer tests can pin
advisory mapping against a local database. `Cargo.toml` excludes this directory
from the published package so the fabrication never ships.

The `2099` in the ID and date is chosen to stay clear of anything RustSec will
allocate, but it is not arbitrarily far out: rustsec enforces a hard
`YEAR_MAX = 2100` on both the `date` field and the year embedded in the ID, so
2099 sits one year inside the ceiling. A later year fails to parse.
