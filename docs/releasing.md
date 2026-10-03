# Releasing hw

`hw` uses calendar-based versioning: **`YYYY.WW.BUILD`**, where `YYYY` is the UTC year, `WW` is the ISO week number (01–53), and `BUILD` is a monotonic counter within that week starting at 1. The source of truth is git tags of the form `vYYYY.WW.BUILD` (e.g. `v2026.15.1`).

Releases are fully automated: pushing a matching tag triggers the release workflow, which cross-compiles for Linux / macOS / Windows, uploads the archives plus SHA-256 sums to a new GitHub Release, and generates release notes from merged pull requests.

This document is the **operator checklist** for cutting a release. It does not describe internal workflow details — for those, read [`.github/workflows/release.yml`](../.github/workflows/release.yml) directly.

## Prerequisites

Before starting:

- The local clone has no uncommitted changes. The script always works from `origin/master`, so the
  current branch doesn't matter.
- `gh` is installed and logged in (`gh auth status`): the script opens the release PR with it.
- You can push branches and tags to `origin` and create releases on `mazuninky/hookah-work-cli`.

## Pre-release review (at least a few hours before)

1. Confirm the last CI run on `master` is green:
   ```sh
   gh run list --branch master --workflow ci.yml --limit 5
   ```
2. Skim merged PRs since the previous release (for the first release, everything on `master`):
   ```sh
   git fetch origin --tags
   git log --oneline $(git describe --tags --abbrev=0 origin/master 2>/dev/null && echo ..)origin/master
   ```
   If anything looks risky, either back it out or postpone the release.
3. GitHub generates the release notes from PR titles. Fix any poor PR titles **now**: a merged PR's
   title can still be edited, and the notes pick up the corrected version.

## Cut the release

`master` only changes through pull requests (squash-merge), so the version bump goes through a PR
and the tag is put on its merge commit afterwards.

1. **Prepare the release PR.**
   ```sh
   ./scripts/bump-version.sh --dry-run   # prints the next version, touches nothing
   ./scripts/bump-version.sh
   ```
   The script:
   - Computes the next `YYYY.WW.BUILD` from the latest `vYYYY.WW.*` tag (the first release of a week
     gets `BUILD` 1).
   - Creates the branch `release/vYYYY.WW.BUILD` off `origin/master`.
   - Rewrites `version = "…"` in the `[package]` section of `Cargo.toml` and refreshes
     `Cargo.lock` (`cargo check`).
   - Commits `release: vYYYY.WW.BUILD`, pushes the branch and opens a PR with that title, then
     switches back to your branch.

2. **Merge the PR.** Wait for CI, then squash-merge it. Keep the title: the squash commit is titled
   `release: vYYYY.WW.BUILD (#N)`, and the next step looks for it.

3. **Tag the merged release.**
   ```sh
   ./scripts/bump-version.sh --tag --dry-run   # shows the commit it would tag
   ./scripts/bump-version.sh --tag
   ```
   The script reads the version from `Cargo.toml` on `origin/master`, finds the `release: vX`
   commit there (later merges don't matter) and pushes an annotated tag `vYYYY.WW.BUILD` on it.

4. **Watch the release workflow.** The pushed tag triggers `.github/workflows/release.yml`:
   ```sh
   gh run watch --exit-status
   ```
   The workflow will:
   - Validate that the pushed tag matches `vYYYY.WW.BUILD` format **and** matches the `version` in `Cargo.toml`. A mismatch fails fast with a clear error.
   - Cross-compile three targets in parallel: `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`.
   - Produce `hw-<version>-<target>.{tar.gz,zip}` plus a sibling `*.sha256`, and attest their build provenance.
   - Create a GitHub Release with `generate_release_notes: true` and attach every archive.

5. **Verify the release.**
   ```sh
   gh release view vYYYY.WW.BUILD
   gh release download vYYYY.WW.BUILD --pattern 'hw-*-aarch64-apple-darwin.tar.gz' --dir /tmp/hw-release
   gh attestation verify /tmp/hw-release/hw-*.tar.gz --repo mazuninky/hookah-work-cli
   ```
   Check that all three archives and their `.sha256` companions are attached, and that the generated notes list the PRs you expect.

6. **Smoke-test the installer.**
   ```sh
   curl -sSfL https://raw.githubusercontent.com/mazuninky/hookah-work-cli/master/scripts/install.sh | sh
   hw --version   # should print YYYY.WW.BUILD
   ```

## If something goes wrong

A version number is never reused: once `--tag` has pushed `vX`, that tag stays, even if its release
fails or is withdrawn. A fixed release is the next number, cut from step 1 of
[Cut the release](#cut-the-release) after the fix lands on `master` through a PR. The bump script
counts existing tags, so it picks the next number by itself.

### Workflow failed before the GitHub Release was created

The tag exists but no release is attached.

- **Transient failure** (runner timeout, flaky download): re-run the failed jobs.
  ```sh
  gh run rerun <run-id> --failed
  ```
- **Fixable in code:** leave the tag alone, land the fix through a PR and cut the next release. The
  tag without a release is harmless: the installer and `releases/latest` only see published
  releases.

### GitHub Release exists but is wrong (missing asset, bad notes, wrong commit)

1. **Delete the release, keep the tag:**
   ```sh
   gh release delete vYYYY.WW.BUILD --yes
   ```
2. Land the fix on `master` through a PR and cut the next release.

Notes alone can be fixed in place with `gh release edit vYYYY.WW.BUILD --notes-file …`.

### Version in Cargo.toml disagrees with the tag

This is what `verify-version` catches. It means someone created a tag without `scripts/bump-version.sh --tag`, or edited `Cargo.toml` manually after the bump. Always cut releases through the script — do not hand-craft tags.

## What *not* to do

- **Do not hand-edit `Cargo.toml` to bump the version.** The script is the only supported path.
- **Do not move or delete release tags.** Tags are immutable from users' perspective; cut the next version instead.
- **Do not skip the pre-release CI check.** The release workflow does build-test as part of the cross-compile, but a broken test on `master` means a broken release.
- **Do not push the version bump straight to `master`.** It goes through the release PR like every other change; the script never pushes to `master`.
- **Do not tag a commit that is not on `master`.** `--tag` only tags the merged `release: vX` commit of `origin/master`.
