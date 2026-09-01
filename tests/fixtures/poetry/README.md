# Poetry output fixtures

Real output from `Poetry 2.4.2`, captured against a throwaway Python project
created for the purpose. Nothing here comes from any user's project.

The throwaway project pinned deliberately stale versions so that the listing had
something to report, and left `six` unpinned so that one direct dependency would
be *current*:

```toml
[project]
name = "demo-app"
version = "0.1.0"
requires-python = ">=3.10,<4.0"
dependencies = [
    "requests[socks] (==2.19.1)",
    "pyyaml (==5.3)",
    "flask (==2.0.0)",
    "six",
]

[project.optional-dependencies]
extra-feature = ["packaging (==21.0)"]

[dependency-groups]
dev = ["click (==8.0.1)"]

[tool.poetry.group.docs.dependencies]
markupsafe = "2.0.0"

[build-system]
requires = ["poetry-core>=2.0.0,<3.0.0"]
build-backend = "poetry.core.masonry.api"
```

`jinja2`, `werkzeug`, `itsdangerous`, `urllib3`, `idna`, `chardet`, `certifi`,
`pyparsing`, and `pysocks` are all transitive, which is what makes the scope diff
below testable.

## `show-latest.json`

`poetry show --latest --format json`, run with `POETRY_VIRTUALENVS_CREATE=false`
and after `poetry install`. Pretty-printed with `jq '.'` — Poetry emits it as one
long line — and otherwise **verbatim**: no entries removed, no fields edited.

Three things it is here to pin:

- the payload is exactly `{name, installed_status, version, latest_version,
  description}`. There is no `groups`, no `extras`, and no `marker` **anywhere**
  in it, which is why the adapter reports all three as `null` / `not_reported`
  rather than as `[]`. That negative is the single most load-bearing fact in this
  directory.
- `latest_version` is **present on every entry**, including packages that are
  already current — the opposite of `uv tree`, which omits the field to mean
  "up to date". The adapter compares the two versions itself.
- `--latest` lists all sixteen packages, not just the ten that are behind. That
  is why the adapter runs `--latest` rather than `--outdated`: under `--outdated`,
  `checked` would equal `outdated` on every run and the denominator would say
  nothing. Verified against this project that the derived outdated subset is
  exactly the ten names `poetry show --outdated` returns.

## `show-latest-top-level.json`

`poetry show --latest --top-level --format json`, captured the same way and
pretty-printed the same way. Seven entries, the project's seven direct
dependencies across `[project.dependencies]`, `[project.optional-dependencies]`,
`[dependency-groups]`, and `[tool.poetry.group]`.

This file is the *only* source of direct-versus-transitive: Poetry's JSON carries
no such field, so the adapter diffs the two listings by normalized name.

`six` is the entry that makes the pairing meaningful. It is direct **and** up to
date, so it appears here while appearing in no outdated listing at all. Without
it, a `--top-level` that actually returned "the outdated direct dependencies"
would be indistinguishable from one that returns direct dependencies, and the
scope test would pass against a wrong premise.

## What is deliberately absent

There is no security fixture, and there is no command to capture one from.
Poetry ships no vulnerability scanner — `poetry check` validates
`pyproject.toml` against the lockfile — so the adapter reports `security` as
`unsupported` rather than as a missing tool or a clean scan.
