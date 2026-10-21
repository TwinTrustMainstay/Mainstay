# Documentation Versioning and Maintenance

> **Last updated:** 2026-10-16 · **Doc version:** 0.1.0

This policy keeps Mainstay's documentation in step with the contracts it describes. It covers how docs are versioned, how their age is shown, how old versions are archived, how deprecations are announced, and who owns each area. It implements issue [#1724](https://github.com/TwinTrustMainstay/Mainstay/issues/1724).

## 1. Version Docs to Match Contract Releases

Every doc in `docs/` carries a **Doc version** line. The value must equal the release version of the contracts it describes.

- All contract crates (`contracts/*/Cargo.toml`) and `sdk/rust/Cargo.toml` are released in lockstep and share one version.
- `scripts/check_doc_metadata.sh` fails CI if the crates disagree, or if a doc's version differs from them.
- When a release bumps the version, update the **Doc version** line in every doc whose content changed for that release. Docs that did not change may keep their older version, but then they must still match the release they were last verified against.

## 2. "Last Updated" Timestamps

Every doc has a metadata line directly under its title:

```markdown
# Title

> **Last updated:** 2026-10-16 · **Doc version:** 0.1.0
```

- The date is the date of the last substantive edit (not a typo fix), in ISO format `YYYY-MM-DD`.
- `scripts/check_doc_metadata.sh` fails CI if the line is missing or malformed.

## 3. Archiving Old Versions

When a release changes behavior described in a doc, copy the previous version before editing:

```
docs/archive/<doc-version>/<doc-name>.md
```

For example, before releasing `0.2.0` changes to `docs/error-reference.md`, copy the `0.1.0` text to `docs/archive/0.1.0/error-reference.md`. Archived copies are read-only snapshots. They are not checked for metadata, and they are not linked from the live docs except through deprecation notices.

See [`docs/archive/README.md`](archive/README.md) for the archive index.

## 4. Deprecation Notices

When a doc or section is superseded, add a banner directly under the metadata line:

```markdown
> **Deprecated:** This page describes the 0.0.x upgrade flow. See [upgrade-testing.md](upgrade-testing.md) for the current process.
```

Rules:

- A deprecation banner must link to the replacement document.
- Deprecated docs stay in place for one full release before they are moved to `docs/archive/`.
- `scripts/check_doc_metadata.sh` fails CI if a `Deprecated` banner has no link.

## 5. Documentation Owners

Each documentation area has one owner who reviews changes to it and keeps it current. Owners are listed in [`DOC_OWNERS.md`](DOC_OWNERS.md). An area without a named owner is listed as **Unassigned** and must be assigned before the next release.

## 6. Review Checklist for Doc Changes

- [ ] The metadata line is updated (date and doc version).
- [ ] Links to other docs and to [`GLOSSARY.md`](GLOSSARY.md) still resolve.
- [ ] If the change alters behavior from the previous release, the old version is archived.
- [ ] `bash scripts/check_doc_metadata.sh` and `bash scripts/check_glossary.sh` pass.
