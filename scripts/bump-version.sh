#!/usr/bin/env bash
# bump-version.sh — cut a YYYY.WW.BUILD release through a pull request.
#
# master only changes through PRs, so a release takes two runs:
#
#   scripts/bump-version.sh          Compute the next version from git tags, bump Cargo.toml
#                                    on a new branch release/vX off origin/master, commit,
#                                    push the branch and open a PR.
#   scripts/bump-version.sh --tag    After that PR is squash-merged: put an annotated tag vX
#                                    on the release commit of origin/master and push it, which
#                                    starts .github/workflows/release.yml.
#
# Source of truth: git tags `vYYYY.WW.BUILD`. Operator checklist: docs/releasing.md.
#
# Usage:
#   scripts/bump-version.sh [--tag] [--dry-run]
#
#   --tag       Tag the merged release instead of preparing a new one.
#   --dry-run   Print what would happen and exit. No side effects.
#
# Exit codes:
#   0  success
#   1  dirty tree, unexpected repository state, or a git/cargo/gh failure
#   2  usage error

set -euo pipefail

MODE=prepare
DRY_RUN=0
REMOTE=origin
BASE=master

usage() {
    cat <<EOF
Usage: $(basename "$0") [--tag] [--dry-run]

Without --tag: computes the next YYYY.WW.BUILD from git tags, bumps Cargo.toml on a
new branch release/vYYYY.WW.BUILD off ${REMOTE}/${BASE}, commits, pushes it and opens a PR.

With --tag: after the release PR is merged, tags its commit on ${REMOTE}/${BASE} as
vYYYY.WW.BUILD and pushes the tag, which starts the release workflow.
EOF
}

err() {
    echo "error: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --tag)      MODE=tag ;;
        --dry-run)  DRY_RUN=1 ;;
        -h|--help)  usage; exit 0 ;;
        *)          echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

REPO_ROOT=$(git rev-parse --show-toplevel)
cd "$REPO_ROOT"

# The `version` of the [package] section of a Cargo.toml read from stdin (dependency
# version constraints are never touched). The awk scripts here read their whole input:
# exiting early would SIGPIPE the producer and fail the pipeline under pipefail.
package_version() {
    awk '
        /^\[package\][[:space:]]*$/ { in_pkg = 1; next }
        /^\[/ { in_pkg = 0 }
        in_pkg && !found && /^version[[:space:]]*=/ {
            sub(/^version[[:space:]]*=[[:space:]]*"/, ""); sub(/".*$/, ""); print; found = 1
        }
    '
}

# Same format check as the release workflow: BUILD starts at 1.
valid_version() {
    printf '%s' "$1" | grep -Eq '^[0-9]{4}\.(0[1-9]|[1-4][0-9]|5[0-3])\.[1-9][0-9]*$'
}

# Reading refs is harmless, but a dry run must not even update them.
if [ "$DRY_RUN" -eq 0 ]; then
    git fetch --quiet --tags "$REMOTE" "$BASE"
fi
BASE_REF="refs/remotes/${REMOTE}/${BASE}"
git rev-parse -q --verify "$BASE_REF" >/dev/null || err "${REMOTE}/${BASE} not found; fetch it first"

if [ "$MODE" = tag ]; then
    VERSION=$(git show "${BASE_REF}:Cargo.toml" | package_version)
    valid_version "$VERSION" \
        || err "${REMOTE}/${BASE} has version '${VERSION}', not a release; merge a release PR first"
    TAG="v${VERSION}"

    if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null \
        || git ls-remote --exit-code --tags "$REMOTE" "refs/tags/${TAG}" >/dev/null; then
        err "tag ${TAG} already exists"
    fi

    # The squash-merge commit is titled after the PR: "release: vX (#N)".
    COMMIT=$(git log --first-parent --format='%H%x09%s' "$BASE_REF" \
        | awk -F '\t' -v t="release: ${TAG}" '!found && ($2 == t || index($2, t " (#") == 1) { print $1; found = 1 }')
    [ -n "$COMMIT" ] || err "no 'release: ${TAG}' commit on ${REMOTE}/${BASE}; merge the release PR first"
    [ "$(git show "${COMMIT}:Cargo.toml" | package_version)" = "$VERSION" ] \
        || err "commit ${COMMIT} does not set version ${VERSION} in Cargo.toml"

    if [ "$DRY_RUN" -eq 1 ]; then
        echo "would tag $(git log -1 --format='%h %s' "$COMMIT") as ${TAG} and push it"
        exit 0
    fi

    git tag -a "$TAG" -m "Release ${VERSION}" "$COMMIT"
    git push "$REMOTE" "refs/tags/${TAG}"
    echo "Pushed ${TAG} -> $(git log -1 --format='%h %s' "$COMMIT")"
    echo "The release workflow is starting; follow it with: gh run watch --exit-status"
    exit 0
fi

YEAR=$(date -u +%G)
WEEK=$(date -u +%V)
LATEST_TAG=$(git tag -l "v${YEAR}.${WEEK}.*" --sort=-v:refname | head -n 1 || true)
if [ -z "$LATEST_TAG" ]; then
    BUILD=1
else
    LATEST_BUILD=${LATEST_TAG##*.}
    printf '%s' "$LATEST_BUILD" | grep -Eq '^[0-9]+$' \
        || err "could not parse BUILD number from tag '$LATEST_TAG'"
    BUILD=$((LATEST_BUILD + 1))
fi
VERSION="${YEAR}.${WEEK}.${BUILD}"
TAG="v${VERSION}"
BRANCH="release/${TAG}"

# A merged but untagged release would otherwise be prepared a second time.
BASE_VERSION=$(git show "${BASE_REF}:Cargo.toml" | package_version)
if valid_version "$BASE_VERSION" \
    && ! git rev-parse -q --verify "refs/tags/v${BASE_VERSION}" >/dev/null; then
    err "${REMOTE}/${BASE} is at untagged release ${BASE_VERSION}; run $(basename "$0") --tag"
fi

if [ "$DRY_RUN" -eq 1 ]; then
    echo "$VERSION"
    exit 0
fi

if ! git diff --quiet || ! git diff --cached --quiet \
    || [ -n "$(git ls-files --others --exclude-standard)" ]; then
    err "working tree is dirty; commit or stash changes first"
fi
if git rev-parse -q --verify "refs/heads/${BRANCH}" >/dev/null \
    || git ls-remote --exit-code --heads "$REMOTE" "$BRANCH" >/dev/null; then
    err "branch ${BRANCH} already exists; finish or delete that release first"
fi

START=$(git symbolic-ref -q --short HEAD || git rev-parse HEAD)
git switch --quiet -c "$BRANCH" "$BASE_REF"

TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT
awk -v new="$VERSION" '
    BEGIN { in_pkg = 0; done = 0 }
    /^\[package\][[:space:]]*$/ { in_pkg = 1; print; next }
    /^\[/ { in_pkg = 0 }
    in_pkg && !done && /^version[[:space:]]*=/ { print "version = \"" new "\""; done = 1; next }
    { print }
    END { if (!done) { print "error: no [package] version line in Cargo.toml" > "/dev/stderr"; exit 1 } }
' Cargo.toml > "$TMP"
mv "$TMP" Cargo.toml
trap - EXIT

# Refresh Cargo.lock so the root package entry carries the new version.
cargo check --quiet --locked 2>/dev/null || cargo check --quiet

git add Cargo.toml Cargo.lock
git commit --quiet -m "release: ${TAG}"
git push --quiet -u "$REMOTE" "$BRANCH"

if command -v gh >/dev/null 2>&1; then
    gh pr create --base "$BASE" --head "$BRANCH" --title "release: ${TAG}" \
        --body "Bumps the version to \`${VERSION}\`. After the squash-merge, run \`scripts/bump-version.sh --tag\` to tag it and start the release workflow."
else
    echo "gh not found: open a PR from ${BRANCH} into ${BASE} titled 'release: ${TAG}'"
fi
git switch --quiet "$START" 2>/dev/null || git switch --quiet --detach "$START"

echo "Prepared ${VERSION} on ${BRANCH}."
echo "Next: squash-merge the PR, then run: scripts/bump-version.sh --tag"
