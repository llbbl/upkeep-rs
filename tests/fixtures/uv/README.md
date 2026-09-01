# `uv` output fixtures

Real output from `uv 0.12.8`, captured against a throwaway Python project
created for the purpose. Nothing here comes from any user's project.

The throwaway project pinned deliberately stale versions so that both commands
had something to report:

```toml
[project]
name = "demo-app"
requires-python = ">=3.10"
dependencies = [
    "requests[socks]==2.19.1",
    "jinja2==2.11.2",
    "pyyaml==5.3",
    "colorama==0.4.1 ; sys_platform == 'win32'",
]

[project.optional-dependencies]
extra-feature = ["packaging==21.0"]

[dependency-groups]
dev = ["click==7.0"]
docs = ["markupsafe==1.1.1"]
```

## `tree-outdated.json`

`uv tree --outdated --frozen --format json`, with two edits:

- every package's `wheels` array was removed — it is several kilobytes of URLs
  and hashes that the adapter never reads
- the absolute project path was rewritten to `/workspace/demo`

Everything the adapter reads is verbatim. Three things it is here to pin:

- `resolution` is a **map** keyed by node id, not a flat `packages` array
- `kind` is either the string `"package"` or one of `{"group": …}` /
  `{"extra": …}` / `"workspace"`, and the group and extra nodes are *aliases* of
  a real package rather than packages of their own
- `latest_version` is **absent** on a package that is already current

Note what is not here. The `docs` group is missing because it is not a default
dependency group, and `colorama` is missing because its `sys_platform == 'win32'`
marker does not hold on the machine that captured this. Both are uv's own
default view of the project, and the adapter reports what uv reports.

## `audit.json`

`uv audit --frozen --output-format json`, reduced to six findings across six
packages, with `summary.vulnerabilities` recomputed to match and each
`description` truncated to 120 characters. The six were chosen to cover a null
`summary`, a three-entry `aliases` list, and a two-entry `fix_versions` list.

The shape is verbatim, and the thing it pins hardest is a *negative*: **no
finding carries a severity**, because `uv audit` publishes none. Every finding
therefore normalizes to `unknown`, which satisfies every
`--fail-on-vulnerability` threshold.
