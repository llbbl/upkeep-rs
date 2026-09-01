# Python maintenance output schema

This page is the contract for the Python maintenance adapter. It exists before
the adapter does, so that each manager backend normalizes *into* a shape that
was designed once, rather than inheriting whichever tool happened to be
implemented first.

Nothing here is wired to a command yet. The types live in
`src/core/python.rs` and the JSON examples below are pinned to them by tests.

## Why `cargo-upkeep` owns this schema

The Python tools this adapter will read do not offer a stable machine contract.
`uv audit` and `uv tree` both emit `"schema": {"version": "preview"}` and `uv
audit` prints an experimental-tool warning to stderr; Poetry and pip-tools emit
nothing comparable at all. A CI gate cannot be built on any of that, and it
certainly cannot be built on all of it at once.

So the contract is ours. `schema_version` is a `cargo-upkeep` integer. It is
never a passthrough of an upstream tool's own version field, and an upstream
schema change is a normalization problem for the adapter, not a contract change
for the caller.

## Compatibility rule for `schema_version`

`schema_version` is the first key in every payload. It is a plain integer,
currently `1`.

**Additive changes do not bump it.** A new field, a new enum variant, a new
capability, or a new manager backend all leave the version alone. Consumers must
therefore ignore unknown object keys and must not treat an unrecognized enum
string as a parse failure.


Ignoring an unrecognized enum string does not mean discarding the value that
carried it — that would let a new severity slip past a gate. Each enum has a
designed "we do not know" member, and an unrecognized string maps to it:

| Field | Unrecognized string is treated as |
| --- | --- |
| `severity` | `unknown` — and `unknown` satisfies every `--fail-on-vulnerability` threshold |
| `update_type` | `unclassified` |
| `scope` | `unknown` |
| `manager.name`, `reason`, `capability` | passed through as an opaque string; never dropped |

**These bump it:**

- removing or renaming a field
- changing a field's type, including making a non-nullable field nullable
- changing what an existing value *means* — for example, redefining which
  version differences count as `major`
- changing which exit code an existing condition produces

The version describes the payload, not the crate. A release that changes nothing
in this document leaves `schema_version` where it is, and a single release emits
exactly one version — there is no negotiation flag and no multi-version output
mode.

A consumer that pins a version should compare for equality and refuse to parse a
higher one. A higher number means fields it relies on may have been redefined
underneath it, which is exactly the case where guessing is worse than stopping.

## Envelope

```text
schema_version   integer, ours
manager          which Python manager this run normalized from
complete         true only when every capability was measured
capabilities[]   one entry per capability, with whether it was measured
unavailable[]    the capabilities that were not, and why
outdated         the outdated report, or null when not measured
security         the security report, or null when not measured
warnings[]       non-fatal notes, including upstream instability disclaimers
```

`complete`, `unavailable[]`, `reason`, and `detail` are deliberately the same
vocabulary `quality` uses (see [commands.md](./commands.md#quality)). A Python
payload should read like the rest of this CLI, not like a foreign schema
bolted on.

<!-- cargo-upkeep-example:python -->
```json
{
  "schema_version": 1,
  "manager": {
    "name": "uv",
    "version": "0.0.0"
  },
  "complete": true,
  "capabilities": [
    {
      "name": "outdated",
      "measured": true
    },
    {
      "name": "security",
      "measured": true
    }
  ],
  "unavailable": [],
  "outdated": {
    "checked": 12,
    "outdated": 2,
    "counts": {
      "epoch": 0,
      "major": 1,
      "minor": 0,
      "patch": 0,
      "qualifier": 1,
      "unclassified": 0
    },
    "packages": [
      {
        "name": "example-http",
        "current": "1.4.2",
        "latest": "2.0.0",
        "update_type": "major",
        "scope": "direct",
        "groups": [
          "main"
        ],
        "extras": [
          "socks"
        ],
        "marker": {
          "status": "reported",
          "expression": "python_version >= '3.10'"
        }
      },
      {
        "name": "example-parser",
        "current": "0.9.0",
        "latest": "0.9.0.post1",
        "update_type": "qualifier",
        "scope": "transitive",
        "groups": [],
        "extras": null,
        "marker": {
          "status": "absent"
        }
      }
    ]
  },
  "security": {
    "summary": {
      "critical": 0,
      "high": 1,
      "moderate": 0,
      "low": 0,
      "unknown": 1,
      "total": 2
    },
    "findings": [
      {
        "id": "GHSA-0000-0000-0000",
        "aliases": [
          "CVE-0000-00000"
        ],
        "package": "example-http",
        "installed_version": "1.4.2",
        "severity": "high",
        "title": "Example advisory",
        "scope": "direct",
        "fixed_versions": [
          "1.4.3",
          "2.0.0"
        ]
      },
      {
        "id": "PYSEC-0000-0000",
        "aliases": null,
        "package": "example-parser",
        "installed_version": "0.9.0",
        "severity": "unknown",
        "title": null,
        "scope": "transitive",
        "fixed_versions": null
      }
    ]
  },
  "warnings": [
    "uv documents its own JSON output as unstable; this report is normalized into cargo-upkeep schema_version 1"
  ]
}
```

## Capability gaps

A capability is one question this adapter can answer: `outdated` or `security`.
Every capability appears in `capabilities[]` on every run, so a caller never has
to infer from absence.

When a capability was not measured, three things happen together and must stay
consistent:

- its `capabilities[]` entry has `"measured": false`
- it gains an `unavailable[]` entry naming the `reason` and a human `detail`
- its top-level report field is **`null`**, never `{}` and never an empty list

That last point is the whole design. An empty `findings` list means the scanner
ran and found nothing. `"security": null` means nobody looked. Collapsing those
two is the bug `quality` already had once (#10, #34), where an unmeasured metric
defaulted to a healthy value and a broken toolchain graded as an `A`.

`null` is written explicitly rather than omitted, because an omitted key is
indistinguishable from a key a future version stopped emitting.

### Unavailability reasons

| `reason` | Meaning |
| --- | --- |
| `not_installed` | The tool that would answer this is not installed. Actionable by the user, and says nothing about the project. |
| `failed` | The tool ran and did not produce a usable result. Something is genuinely broken. |
| `unsupported` | The detected manager has no way to answer this at all. Installing something else will not help; a different tool is needed. |

The first two match `quality`'s `not_installed` / `failed` labels
([commands.md](./commands.md#not_installed-vs-failed)) on purpose. `unsupported`
is new here because the Rust side has no equivalent: every Rust metric has a
tool that *could* be installed, whereas Poetry simply does not scan for
vulnerabilities, and no install fixes that.

<!-- cargo-upkeep-example:python-capability-gap -->
```json
{
  "schema_version": 1,
  "manager": {
    "name": "poetry",
    "version": null
  },
  "complete": false,
  "capabilities": [
    {
      "name": "outdated",
      "measured": true
    },
    {
      "name": "security",
      "measured": false
    }
  ],
  "unavailable": [
    {
      "name": "security",
      "reason": "unsupported",
      "detail": "the detected manager reports no vulnerability data; run a dedicated scanner and gate on that instead"
    }
  ],
  "outdated": {
    "checked": 4,
    "outdated": 0,
    "counts": {
      "epoch": 0,
      "major": 0,
      "minor": 0,
      "patch": 0,
      "qualifier": 0,
      "unclassified": 0
    },
    "packages": []
  },
  "security": null,
  "warnings": []
}
```

Read that payload carefully: `"outdated": 0` is a real, measured zero, and
`"security": null` is not a zero at all. A pipeline that treats this run as
clean has drawn a conclusion the data does not support, which is why `complete`
is `false` and why `--require-complete` exists.

## Fields a source may not report

Python dependency metadata is uneven. `uv` can report dependency groups and
environment markers; `pip list --outdated` reports neither. The schema refuses
to paper over that.

Four fields carry the distinction — `groups`, `extras`, `aliases`, and
`fixed_versions` — and they use the same encoding the payload already uses at
the top level for `outdated` and `security`: `null` versus the empty value.

```json
"extras": ["socks"]   // the source reports extras, and there is one
"extras": []          // the source reports extras, and there are none
"extras": null        // the source does not report extras at all
```

`[]` says the source reported the field and there was nothing in it. `null` says
the source does not report this field, so this run cannot answer the question.
Collapsing the two would let a consumer read "unknown" as "none", which is the
same class of mistake as a defaulted health score.

One encoding, one rule, at every level: `"security": null` is an unmeasured
capability and `"extras": null` is an unreported attribute, and in both cases the
empty value means the opposite. None of these fields is ever omitted — an absent
key cannot be told apart from a key a future version stopped emitting — so the
`null` is always written.

`marker` is the single exception, because it is single-valued and needs three
states. `null` would have to mean both "markers are not reported" and "this
dependency has no marker", which are different facts, so this one field is
tagged:

```json
{ "status": "reported", "expression": "python_version >= '3.10'" }
{ "status": "absent" }
{ "status": "not_reported" }
```

`absent` means the source reports markers and this dependency has none.
## Outdated entries

| Field | Meaning |
| --- | --- |
| `name` | The package name, normalized per PEP 503 (lowercase, runs of `-`, `_`, `.` collapsed to `-`). |
| `current` | The installed or locked version, as reported. |
| `latest` | The newest version the source offers. |
| `update_type` | See [Update classification](#update-classification). |
| `scope` | `direct`, `transitive`, or `unknown`. |
| `groups` | Dependency groups this package belongs to. |
| `extras` | Extras this package was pulled in with. |
| `marker` | PEP 508 environment marker. |

`scope` has an explicit `unknown` because several sources genuinely do not say.
An entry that cannot be established as direct must not be recorded as
transitive, or as direct, just to fill the field.

`checked` counts the distinct packages the freshness question was actually
settled for, and it is the denominator. It is not a count of declarations, and
it is not `total - skipped`; the Rust side documents at length
([commands.md](./commands.md#how-to-read-total-and-checked)) why mixing those
units invents comparisons that never happened. The same rule holds here.

`counts` breaks `outdated` down by classification, so a pipeline can gate
without parsing every entry. `counts.unclassified` is deliberately visible: a
run where the classifier gave up should be obvious from the summary alone.

## Update classification

Python is not semver, so the Rust `update_type` rule
([commands.md](./commands.md#update-classification)) does **not** transfer.
Cargo's "leftmost non-zero component is the breaking boundary" convention has no
counterpart in PEP 440, and applying it would fabricate compatibility claims.

PEP 440 accepts a good deal more spelling variation than its normalized form
suggests. `v1.0`, `1.0-1`, `1.0beta2`, `1.0.alpha1`, `1.0-rc1`, `1.0rev1`,
`1.0-dev`, `1.0preview1`, and `01.0` are all **valid** PEP 440 versions, and all
normalize to something shorter (`1.0`, `1.0.post1`, `1.0b2`, `1.0a1`, `1.0rc1`,
`1.0.post1`, `1.0.dev0`, `1.0rc1`, `1.0`). An implementation that treats the
normalized grammar as its *input* grammar will emit `unclassified` for versions
that are perfectly valid — and because `unclassified` is defined as an honest
"we could not tell", that wrongness would be invisible.

So: accept the full PEP 440 version scheme, **normalize before comparing**, and
classify on the normalized form `[N!]N(.N)*[{a|b|rc}N][.postN][.devN][+local]`.
Release segments are zero-padded to equal length, and then:
| `update_type` | Rule |
| --- | --- |
| `epoch` | The epoch differs. |
| `major` | Same epoch, first release component differs. |
| `minor` | First component equal, second differs. |
| `patch` | First two equal, third or later differs. |
| `qualifier` | Release segments are identical; the difference is only in a pre-release, post-release, dev, or local segment. |
| `unclassified` | Either version cannot be parsed as PEP 440, or the two normalize to equal versions (`1.0` and `1.0.0` are equal under PEP 440, so there is no difference to classify). |

Worked examples:

| Current | Latest | `update_type` |
| --- | --- | --- |
| `1.4.2` | `2.0.0` | `major` |
| `1.4.2` | `1.5.0` | `minor` |
| `1.4.2` | `1.4.3` | `patch` |
| `1.4` | `1.4.1` | `patch` (padded to `1.4.0`) |
| `0.9.0` | `0.9.0.post1` | `qualifier` |
| `2.0.0rc1` | `2.0.0` | `qualifier` |
| `1.0` | `1!1.0` | `epoch` |
| `2026.4` | `2026.9` | `minor` |
| `1.0` | `not-a-version` | `unclassified` |
| `v1.0` | `1.0-1` | `qualifier` (normalized to `1.0` and `1.0.post1`) |
| `1.0` | `1.0.0` | `unclassified` (equal after normalization) |


`update_type` describes the difference between two version numbers and assumes
`latest` is the newer of the two. A source that reports a `latest` *below*
`current` — Poetry does this when the newest stable release is older than an
installed pre-release — must classify the entry `unclassified` rather than
letting a downgrade be counted as an available `major` update.

Two things this table is careful *not* to claim.

`epoch` is its own class rather than a flavour of `major` because an epoch bump
declares that the project's whole versioning scheme changed. There is no
meaningful comparison to make across it, and folding it into `major` would hide
that.

`major` means "the first release component differs" and nothing more. It is not
a prediction of breakage. A calendar-versioned project bumps its first component
every January, and `cargo-upkeep` has no reliable way to detect CalVer, so it
does not guess — see `2026.4` → `2026.9` above, which is called `minor` on
position alone. Read `update_type` as a description of the version numbers, not
as a compatibility promise. PEP 440 does not carry one.

`unclassified` is a first-class outcome, not an error. A source that hands back
a version string this crate cannot parse gets an honest "we do not know" rather
than a defaulted `patch` that reads as safe.

## Security findings

| Field | Meaning |
| --- | --- |
| `id` | The advisory identifier as issued, such as a `GHSA-` or `PYSEC-` id. |
| `aliases` | Other identifiers for the same advisory. Empty when none are published. |
| `package` | PEP 503 normalized package name. |
| `installed_version` | The version the finding was matched against. |
| `severity` | `critical`, `high`, `moderate`, `low`, or `unknown`. |
| `title` | Short human-readable summary. |
| `scope` | `direct`, `transitive`, or `unknown`, as in outdated entries. |
| `fixed_versions` | Versions the advisory names as fixed. |

`severity` carries `unknown` because Python advisory sources frequently publish
no severity at all. The Rust `Severity` has no such variant, and adding one there
would be a change to an existing contract; a finding with no severity is
represented as `unknown` here rather than being downgraded into `low`.

`summary` counts `critical`, `high`, `moderate`, `low`, `unknown`, and `total`.
`unknown` gets its own bucket for the same reason: a payload where the four
graded buckets are zero but `unknown` is not must not read as clean.

`fixed_versions` uses the `null`/`[]` encoding described above.
`[]` means the advisory names no fix; `null` means the source reported none. There is
no `fix_available` boolean, because the version list answers the same question
without collapsing "no fix exists" into "no fix was reported".


Both summaries are contracts, not conveniences, because they exist so a pipeline
can gate without walking every entry. These invariants always hold, and are
asserted in the crate's tests:

- `outdated == packages.length` — `packages` is never truncated relative to the count
- `outdated == epoch + major + minor + patch + qualifier + unclassified`
- `checked >= outdated` — an outdated package was necessarily checked
- `summary.total == findings.length` — `total` counts *findings*, not deduplicated advisories
- `summary.total == critical + high + moderate + low + unknown`

One advisory affecting three packages is three findings and counts as three.

## Exit codes

The base contract is the one every command in this CLI already follows
([commands.md](./commands.md#exit-codes)):

| Status | Meaning |
| --- | --- |
| `0` | The command ran and produced its report. |
| `1` | The run failed, or an opt-in policy gate rejected the result. |
| `2` | The arguments were rejected. |

**Findings are not failures.** Outdated packages exit 0. Vulnerabilities exit 0.
The report is the deliverable, and a pipeline that wants a gate asks for one.

Two conditions fail without any flag, because there is no report to stand on:

- No supported Python manager could be detected. Note this path emits a JSON error object rather than a `PythonOutput`, so it carries no `schema_version`; a consumer pinning the version cannot learn that its exit semantics changed, and `schema_version` is bumped for such a change precisely so the release notes carry it instead.
- A manager was detected but every capability is unavailable — `complete` is
  false and both reports are `null`. There is nothing here for `complete` to
  qualify, exactly as `quality` treats `score: null`.

The report, when one exists, is written to stdout in full *before* the process
exits nonzero. A failing status never costs the caller the output that explains
it, and the reason goes to stderr — a JSON error object under `--json`, a plain
line otherwise.

### Opt-in policy gates

| Gate | Fails when |
| --- | --- |
| `--require-complete[=<capabilities>]` | A required capability was not measured. |
| `--fail-on-vulnerability <threshold>` | Any finding is at or above `threshold`, which is one of `critical`, `high`, `moderate`, `low`, or `any`. |

`--require-complete` takes an optional comma-separated capability list. This is
not decoration — it resolves a real conflict with the compatibility rule above.

Adding a capability is an additive change that does **not** bump
`schema_version`, but `complete` is true only when *every* capability was
measured. So a bare `--require-complete` would start failing on unchanged code
the first time a release adds a capability whose tool the runner lacks, with no
version signal to have warned anyone. That is the `quality` #10/#34 lesson
inverted: not "unmeasured reads as healthy", but "unmeasured reads as broken",
arriving unannounced.

Naming the capabilities pins the gate to what the pipeline actually asked for:

```bash
cargo upkeep python --require-complete=outdated,security
```

The bare form means "every capability this version knows about", which is a set
that can grow between releases. Use it interactively; name the capabilities in
CI.

Either form is a *coverage* gate, so a runner missing an optional scanner fails
it and reports the missing tool rather than anything about the project. Install
what you intend to measure before gating on it.

`--fail-on-vulnerability` has one rule worth stating outright: **an `unknown`
severity satisfies every threshold.** A finding whose severity was never
established cannot be shown to be below the bar, and silently excluding it would
turn a missing severity into a passing build. If that is too noisy for a
pipeline, gate on the parsed JSON instead — the `summary.unknown` bucket exists
so that decision can be made deliberately rather than by default.

Because `unknown` satisfies everything and every graded severity is at or above
`low`, the `low` and `any` thresholds accept the same set today. Both names are
kept because they say different things about intent, and a future severity below
`low` would separate them.

Neither gate changes the report. They only change the exit status.
