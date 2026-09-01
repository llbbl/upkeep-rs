#!/usr/bin/env bash
# Bring local `main` up to date with `origin/main`, waiting out an in-flight
# auto-release only for as long as it can still move `main`.
#
# Merging a releasable PR triggers `auto-release.yml`, whose `Compute version`
# job pushes a `chore(release)` bump commit. A branch cut in that window starts
# stale. Only that first job moves any ref, so this waits on it alone rather
# than on the ~13-minute full release.
#
# Safe from a dirty feature branch: it never checks out, stashes, or rebases.
set -euo pipefail

WORKFLOW="auto-release.yml"
BUMP_JOB="Compute version"
REMOTE="origin"
MAIN="main"
# The `Compute version` job itself runs 9-190s, but the wait is wall-clock and
# `auto-release.yml` sets `concurrency: cancel-in-progress: false`, so a run can
# queue behind an entire prior release. Measured over 22 real runs: 20 finished
# within 191s, one took 600s and one 1232s, both dominated by queue time. The cap
# is set past that worst case so a healthy-but-queued release is never abandoned.
WAIT_TIMEOUT_SECONDS=1800
POLL_INTERVAL_SECONDS=15
# Consecutive unreadable `gh run view` responses before giving up. Without this,
# an expired token or a network drop is indistinguishable from a job that has not
# been created yet, and the script polls a dead endpoint until the cap.
MAX_API_FAILURES=3
# A run is created within seconds of a push, but not instantly. If `main`'s tip
# has no run yet, re-check a few times before concluding nothing is in flight.
DETECT_RETRIES=3
DETECT_RETRY_SECONDS=5

EXIT_CODE=0
REPORT_MAY_BE_STALE=""

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/.." && pwd)
cd -- "$REPO_ROOT"

say() { printf '==> %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }

# Format seconds as e.g. `2m34s`.
elapsed_hms() {
  local total=$1
  printf '%dm%02ds' $((total / 60)) $((total % 60))
}

subject_of() {
  git log -1 --format='%s' "$1" 2>/dev/null || printf '(unknown)'
}

# --- 1. Where is local `main` right now? --------------------------------------
# Tolerate `main` not existing locally; a fresh or single-branch clone may not
# have it, and step 5 creates it.
OLD_MAIN=$(git rev-parse --verify --quiet "refs/heads/$MAIN" || true)

# On a detached HEAD this yields the literal string `HEAD`, which is handled
# where it matters rather than treated as a branch name.
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || printf 'HEAD')
if [ "$CURRENT_BRANCH" = "HEAD" ]; then
  say "On a detached HEAD (no branch checked out)"
else
  say "On branch: $CURRENT_BRANCH"
fi

# --- 2. Fetch -----------------------------------------------------------------
# `--tags` matters beyond the tag refs themselves: `just next-version` derives
# the bump from the newest tag in the LOCAL clone, so stale tags make it predict
# against the wrong baseline without warning.
say "Fetching $REMOTE (branches and tags)..."
if ! git fetch --tags "$REMOTE"; then
  warn "git fetch $REMOTE failed; cannot sync. Check your network or remote."
  exit 1
fi

# --- 3. Is an auto-release in flight? -----------------------------------------
RUN_ID=""
DETECTION_SKIPPED=""

if ! command -v gh >/dev/null 2>&1; then
  DETECTION_SKIPPED="the GitHub CLI (gh) is not installed"
elif ! gh auth status >/dev/null 2>&1; then
  DETECTION_SKIPPED="gh is installed but not authenticated (run: gh auth login)"
elif ! command -v jq >/dev/null 2>&1; then
  DETECTION_SKIPPED="jq is not installed"
else
  MAIN_TIP=$(git rev-parse --verify --quiet "refs/remotes/$REMOTE/$MAIN" || true)
  MAIN_TIP_SUBJECT=$(subject_of "$REMOTE/$MAIN")
  DETECT_ATTEMPT=0

  while :; do
    RUNS_JSON=$(gh run list \
      --workflow="$WORKFLOW" \
      --branch="$MAIN" \
      --limit 20 \
      --json databaseId,createdAt,status,headSha 2>/dev/null || true)
    if [ -z "$RUNS_JSON" ]; then
      DETECTION_SKIPPED="could not query workflow runs (gh API error or no access)"
      break
    fi

    # Anything not yet `completed` — queued, in_progress, waiting, requested —
    # can still move `main`. Take the newest, which settles `main` last when
    # several PRs merged in quick succession. `|| true` matters: gh can return a
    # non-JSON error body, and an unguarded substitution would abort the script
    # under `set -e` with `main` left un-synced.
    RUN_ID=$(printf '%s' "$RUNS_JSON" \
      | jq -r '[.[] | select(.status != "completed")]
               | sort_by(.createdAt)
               | last
               | .databaseId // empty' 2>/dev/null || true)
    if [ -n "$RUN_ID" ]; then
      break
    fi

    # Nothing in flight — but a run created seconds ago may not be listed yet,
    # and that is the one silently-wrong path: we would sync to the pre-bump
    # commit and exit 0. Every push to `main` gets a run, EXCEPT the workflow's
    # own `chore(release)` bump: that is pushed with GITHUB_TOKEN, which by
    # design does not trigger workflows. So "tip has no run" means the run has
    # not registered yet, unless the tip is a bump commit.
    if [ -z "$MAIN_TIP" ]; then
      break
    fi
    case "$MAIN_TIP_SUBJECT" in
      "chore(release):"*) break ;;
    esac

    TIP_RUNS=$(printf '%s' "$RUNS_JSON" \
      | jq -r --arg sha "$MAIN_TIP" '[.[] | select(.headSha == $sha)] | length' \
        2>/dev/null || printf '1')
    if [ "$TIP_RUNS" != "0" ]; then
      break
    fi

    DETECT_ATTEMPT=$((DETECT_ATTEMPT + 1))
    if [ "$DETECT_ATTEMPT" -ge "$DETECT_RETRIES" ]; then
      warn "no workflow run has registered for $MAIN's tip $(git rev-parse --short "$MAIN_TIP")."
      warn "If a merge just landed, its release may not have started yet — re-run 'just sync'."
      DETECT_UNCERTAIN="yes"
      REPORT_MAY_BE_STALE="yes"
      break
    fi
    note "No run registered yet for $MAIN's tip; re-checking in ${DETECT_RETRY_SECONDS}s..."
    sleep "$DETECT_RETRY_SECONDS"
  done
fi

if [ -n "$DETECTION_SKIPPED" ]; then
  warn "release detection skipped: $DETECTION_SKIPPED"
  note "Fetching and reporting anyway; an in-flight release could still move $MAIN after this."
  REPORT_MAY_BE_STALE="yes"
elif [ -n "${DETECT_UNCERTAIN:-}" ]; then
  say "No auto-release run is visible yet for $MAIN's tip — continuing without waiting."
elif [ -z "$RUN_ID" ]; then
  say "No auto-release in flight on $MAIN — nothing to wait for."
else
  # --- 4. Wait on the `Compute version` job only ------------------------------
  say "Auto-release run $RUN_ID is in flight. Waiting on the '$BUMP_JOB' job only."
  note "(The remaining ~10 minutes of a release move no refs and are not waited on.)"
  note "Run: $(gh run view "$RUN_ID" --json url --jq '.url' 2>/dev/null || printf 'id %s' "$RUN_ID")"

  WAIT_START=$(date +%s)
  API_FAILURES=0
  JOB_STATUS=""
  JOB_CONCLUSION=""
  while :; do
    JOBS_JSON=$(gh run view "$RUN_ID" --json jobs 2>/dev/null || true)
    JOB_LINE=""
    if [ -n "$JOBS_JSON" ]; then
      # `|| true` for the same reason as above: a parse error here would abort
      # mid-wait, after the user has been told to wait, leaving `main` stale.
      JOB_LINE=$(printf '%s' "$JOBS_JSON" \
        | jq -r --arg name "$BUMP_JOB" \
            '.jobs[]? | select(.name == $name) | "\(.status) \(.conclusion // "")"' \
          2>/dev/null | head -1 || true)
    fi

    if [ -n "$JOB_LINE" ]; then
      API_FAILURES=0
      read -r JOB_STATUS JOB_CONCLUSION <<<"$JOB_LINE"
    else
      # Either the job has not been created yet, or the response was unreadable.
      # Those are different problems and only the second is counted: polling a
      # dead endpoint for the full cap and then blaming the job is a lie. Valid
      # JSON that simply lacks the job is the benign case, so the test is
      # parseability — an empty body AND a 502 HTML body both count as failures.
      if [ -z "$JOBS_JSON" ] || ! printf '%s' "$JOBS_JSON" | jq -e . >/dev/null 2>&1; then
        API_FAILURES=$((API_FAILURES + 1))
        if [ "$API_FAILURES" -ge "$MAX_API_FAILURES" ]; then
          warn "gh returned $API_FAILURES unreadable responses in a row; giving up on the wait."
          warn "This is a gh/network problem, not a stuck release. Check: gh auth status"
          EXIT_CODE=1
          REPORT_MAY_BE_STALE="yes"
          break
        fi
      else
        API_FAILURES=0
      fi
      JOB_STATUS="pending"
      JOB_CONCLUSION=""
    fi

    NOW=$(date +%s)
    WAITED=$((NOW - WAIT_START))

    if [ "$JOB_STATUS" = "completed" ]; then
      note "[$(elapsed_hms "$WAITED")] $BUMP_JOB: completed (${JOB_CONCLUSION:-unknown})"
      break
    fi

    if [ "$WAITED" -ge "$WAIT_TIMEOUT_SECONDS" ]; then
      warn "gave up after $(elapsed_hms "$WAITED") waiting for '$BUMP_JOB' (last status: $JOB_STATUS)."
      warn "$MAIN may still move when that job finishes. Re-run 'just sync' later."
      EXIT_CODE=1
      REPORT_MAY_BE_STALE="yes"
      break
    fi

    note "[$(elapsed_hms "$WAITED")] waiting for '$BUMP_JOB' — status: $JOB_STATUS (cap $(elapsed_hms "$WAIT_TIMEOUT_SECONDS"))"
    sleep "$POLL_INTERVAL_SECONDS"
  done

  if [ "$JOB_STATUS" = "completed" ] && [ "$JOB_CONCLUSION" != "success" ]; then
    warn "'$BUMP_JOB' concluded '$JOB_CONCLUSION' — the release did NOT complete normally."
    warn "$MAIN may not have moved, and no tag may exist. Inspect: gh run view $RUN_ID"
    EXIT_CODE=1
    REPORT_MAY_BE_STALE="yes"
  fi

  # The bump commit lands during that job; re-fetch to pick it up.
  say "Re-fetching $REMOTE after the release job..."
  if ! git fetch --tags "$REMOTE"; then
    warn "re-fetch failed; the report below may be stale."
    REPORT_MAY_BE_STALE="yes"
  fi
fi

# --- 5. Update local `main` without touching the worktree ---------------------
if ! git rev-parse --verify --quiet "refs/remotes/$REMOTE/$MAIN" >/dev/null; then
  warn "$REMOTE/$MAIN does not exist; nothing to sync to."
  exit 1
fi

# Local commits on `main` that origin does not have. Distinguishes "you have
# unpushed work here" from "a dirty file is in the way", which need different
# fixes and which git reports only in prose.
main_ahead_by() {
  git rev-list --count "$REMOTE/$MAIN..$MAIN" 2>/dev/null || printf '0'
}

if [ "$CURRENT_BRANCH" = "$MAIN" ]; then
  # `git fetch origin main:main` is refused when main is the checked-out branch,
  # so fast-forward in place instead. --ff-only never rewrites and never merges,
  # so a dirty worktree is left alone unless the fast-forward itself would touch
  # a modified file, in which case git declines and we report it.
  if ! git merge --ff-only --quiet "$REMOTE/$MAIN"; then
    AHEAD=$(main_ahead_by)
    if [ "$AHEAD" -gt 0 ]; then
      warn "could not fast-forward $MAIN: it has $AHEAD local commit(s) not on $REMOTE/$MAIN."
      warn "Push or rebase them yourself; this script never rewrites history."
    else
      warn "could not fast-forward $MAIN to $REMOTE/$MAIN (git's reason is above)."
      warn "Most likely an uncommitted change is in the way. Resolve by hand."
    fi
    EXIT_CODE=1
  fi
else
  # Updates the ref directly. Never touches the index or worktree, so this is
  # safe with uncommitted changes on a feature branch.
  if ! git fetch "$REMOTE" "$MAIN:$MAIN"; then
    AHEAD=$(main_ahead_by)
    if [ "$AHEAD" -gt 0 ]; then
      warn "could not fast-forward local $MAIN: it has $AHEAD commit(s) not on $REMOTE/$MAIN."
      warn "Push or rebase them yourself; this script never rewrites history."
    else
      warn "could not fast-forward local $MAIN from $REMOTE/$MAIN."
      warn "It may be checked out in another worktree. Resolve by hand."
    fi
    EXIT_CODE=1
  fi
fi

# --- 6. Report ----------------------------------------------------------------
NEW_MAIN=$(git rev-parse --verify --quiet "refs/heads/$MAIN" || true)

printf '\n'
if [ -z "$NEW_MAIN" ]; then
  warn "local $MAIN still does not exist."
elif [ -z "$OLD_MAIN" ]; then
  say "Created local $MAIN at $(git rev-parse --short "$NEW_MAIN")"
  note "$(subject_of "$NEW_MAIN")"
elif [ "$OLD_MAIN" = "$NEW_MAIN" ]; then
  say "$MAIN did not move — still $(git rev-parse --short "$NEW_MAIN")"
  note "$(subject_of "$NEW_MAIN")"
else
  say "$MAIN moved: $(git rev-parse --short "$OLD_MAIN") -> $(git rev-parse --short "$NEW_MAIN")"
  note "$(subject_of "$NEW_MAIN")"
fi

if [ -n "$REPORT_MAY_BE_STALE" ]; then
  warn "the state above is provisional: a release may still move $MAIN after this run."
fi

if [ "$CURRENT_BRANCH" = "$MAIN" ]; then
  say "You are on $MAIN; branch away from here and it will not start stale."
elif [ -n "$NEW_MAIN" ]; then
  if [ "$CURRENT_BRANCH" = "HEAD" ]; then
    WHAT="detached HEAD"
  else
    WHAT="$CURRENT_BRANCH"
  fi
  BEHIND=$(git rev-list --count "HEAD..$MAIN" 2>/dev/null || printf '0')
  if [ "$BEHIND" -eq 0 ]; then
    say "$WHAT is not behind $MAIN."
  else
    if [ "$BEHIND" -eq 1 ]; then
      say "$WHAT is behind $MAIN by 1 commit."
    else
      say "$WHAT is behind $MAIN by $BEHIND commits."
    fi
    note "Not rebasing automatically. To rebase, run:"
    note ""
    note "    git rebase $MAIN"
    note ""
  fi
fi

exit "$EXIT_CODE"
