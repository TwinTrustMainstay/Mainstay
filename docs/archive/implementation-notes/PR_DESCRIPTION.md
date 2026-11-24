# Fix broken documentation links and declare the project license

## Summary

- Replace stale or incorrectly based documentation links with paths to existing guides.
- Correct case-sensitive and relative-path references in the event indexer, monitoring, architecture, onboarding, and ADR documentation.
- Add the MIT license, consistent with the Rust SDK package metadata, and remove the README reference to a nonexistent code of conduct.

## Commits

1. `docs: fix maintenance documentation links`
2. `docs: repair monitoring and event indexer links`
3. `docs: fix architecture and onboarding references`
4. `docs: add MIT license and correct README references`

## Validation

- Verified the changed relative links resolve to files in the repository.
- `git diff --check` passes.
