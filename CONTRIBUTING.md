# Contributing to Mainstay

Thank you for your interest in contributing to Mainstay.

## How to contribute
- Report bugs with clear reproduction steps.
- Request new features with motivation and acceptance criteria.
- Keep pull requests focused and linked to the relevant issue or task.

## Pull request process
- Base PRs on `main`.
- Provide a concise summary of the change.
- Describe how the change was tested or validated.
- Include any relevant docs updates.
- **Update CHANGELOG.md** with your changes (see [Changelog Updates](#changelog-updates) below).
- Ensure secret scanning passes on PRs for `.env` and other configuration files.

## Code review requirements

### Branch protection on `main`
The `main` branch is protected:
- **At least 1 approved review** is required before merging.
- **All CI checks must pass** (build, test, Clippy, rustfmt, cargo audit).
- **Force pushes and branch deletion are disabled** to preserve history.

### Required reviewers for contracts/
All changes under `contracts/` — including new contracts, dependency bumps, and
logic changes — must be approved by a code owner listed in
[`.github/CODEOWNERS`](.github/CODEOWNERS) before merge. This rule is enforced
automatically by GitHub's CODEOWNERS mechanism.

If you are adding a new contract subdirectory, update `.github/CODEOWNERS` in
the same PR so the new path is covered from the first commit.

## Secret scanning
- The repository uses `gitleaks` on PRs to detect real secrets before merge.
- The `.gitleaks.toml` configuration contains standard secret rules plus false-positive allowlists for placeholder values.
- Do not commit real API keys, tokens, or credential files to any branch.
- If a false positive is detected, update `.gitleaks.toml` only after verifying the content is safe.

## Changelog updates

All notable changes must be documented in `CHANGELOG.md` following the [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) format.

### What to document

Document changes in the following categories (when applicable):
- **Added**: New features or functionality
- **Changed**: Changes to existing functionality
- **Deprecated**: Features marked for removal
- **Removed**: Removed features or functionality
- **Fixed**: Bug fixes
- **Security**: Security-related changes

### How to update the changelog

1. Locate the `## [Unreleased]` section at the top of `CHANGELOG.md`
2. Add an entry under the appropriate category
3. Use clear, user-facing language (not implementation details)
4. Link to relevant issues or PRs when applicable

### Example

```markdown
### Added
- New collateral scoring algorithm for improved asset valuation
- Support for multi-signature authorization on lending contracts
```

## Code style
- Maintain consistent formatting.
- Document new workflows, APIs, or UI changes.
- Keep changes minimal and reviewable.

Please refer to this file when creating issues or pull requests for the project.
