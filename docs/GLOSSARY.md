# Glossary of Terms

> **Last updated:** 2026-10-16 · **Doc version:** 0.1.0

This glossary defines the domain terms used across Mainstay's documentation and contracts. Each entry has a stable anchor so it can be linked from any document, for example `[lien](GLOSSARY.md#lien)`.

**Searching:** use your editor's or browser's find-in-page, or search the term name directly. Every term is also listed in the [index](#index), and aliases are listed under each term.

**Cross-references:** when a term first appears in a guide, link it to its entry here. Guides that already do so include [collateral-lien-guide.md](collateral-lien-guide.md) and [lender-integration-guide.md](lender-integration-guide.md).

---

## Index

[Asset](#asset) · [Collateral](#collateral) · [Collateral score](#collateral-score) · [Covenant](#covenant) · [Credential](#credential) · [Decay](#decay) · [Decommission](#decommission) · [Engineer](#engineer) · [Grace period](#grace-period) · [Lien](#lien) · [Lifecycle](#lifecycle) · [Liquidation](#liquidation) · [Loan](#loan) · [Maintenance record](#maintenance-record) · [Proof of maintenance](#proof-of-maintenance) · [Slash](#slash) · [Timelock](#timelock) · [Upgrade](#upgrade) · [Version history](#version-history)

---

## Asset

**Pronunciation:** AS-et

An industrial machine (for example a generator, turbine or vehicle) registered on-chain in the Asset Registry and given a unique asset ID. An asset's maintenance history and score are attached to that ID.

*Aliases:* machine, digital twin (informal).

*See also:* [Lifecycle](#lifecycle), [Collateral](#collateral).

## Collateral

**Pronunciation:** kuh-LAT-er-uhl

An asset pledged to secure a loan. In Mainstay, collateral is an asset whose verified maintenance history makes it eligible to back a loan. Eligibility is decided from the [collateral score](#collateral-score), not from the asset's market value.

*Aliases:* security (in lending contexts).

*See also:* [Lien](#lien), [Loan](#loan).

## Collateral score

**Pronunciation:** kuh-LAT-er-uhl skor

A number derived on-chain from an asset's verified [maintenance records](#maintenance-record). It rises as maintenance is recorded and falls over time through [decay](#decay). Lenders read it to decide whether an asset is eligible as collateral. The exact formula is in [collateral-scoring-formula.md](collateral-scoring-formula.md).

*Aliases:* health score (in some views), score.

## Covenant

**Pronunciation:** KUV-uh-nuhnt

A condition written into a loan agreement that the borrower must keep to remain in good standing. In Mainstay, a covenant is typically a minimum collateral score or a required maintenance cadence. Breaching a covenant can allow a lender to take action, such as a [liquidation](#liquidation).

*Aliases:* loan covenant, loan condition.

*See also:* [Loan](#loan), [Grace period](#grace-period).

## Credential

**Pronunciation:** kruh-DEN-shuhl

The verifiable record that an [engineer](#engineer) is certified to sign maintenance work. A credential has an issuer, an expiry date and a status (valid, grace period, expired, revoked or suspended). Only engineers with a valid credential can submit maintenance records.

*Aliases:* engineer credential, certification.

*See also:* [Engineer](#engineer), [Grace period](#grace-period).

## Decay

**Pronunciation:** dih-KAY

The gradual reduction of an asset's [collateral score](#collateral-score) as time passes without new maintenance. Decay is computed from elapsed time, so an asset's effective score can fall without any transaction.

*See also:* [Collateral score](#collateral-score), [Maintenance record](#maintenance-record).

## Decommission

**Pronunciation:** dee-kuh-MISH-uhn

The permanent retirement of an asset. A decommissioned asset cannot accept new maintenance records.

*See also:* [Asset](#asset), [Lien](#lien).

## Engineer

**Pronunciation:** en-juh-NEER

A person or organization that signs maintenance records for assets. Engineers are registered in the Engineer Registry and must hold a valid [credential](#credential) to submit work.

*See also:* [Credential](#credential), [Maintenance record](#maintenance-record).

## Grace period

**Pronunciation:** grays PEER-ee-uhd

A window after a [credential](#credential) expires during which the engineer can still submit work, usually with a warning. After the grace period ends, the credential becomes hard-expired and submissions are rejected.

*See also:* [Credential](#credential).

## Lien

**Pronunciation:** leen

A recorded legal or contractual claim by a lender over an asset that secures a [loan](#loan). A lien is stored on-chain as a `LienRecord` that names the lender and the asset. While a lien is active, the asset cannot be transferred, and its lien status is shown to other integrators. A lien is released when the loan is repaid or the lender authorizes it.

*Aliases:* encumbrance, lien record.

*See also:* [Collateral](#collateral), [Liquidation](#liquidation), [Collateral lien lifecycle](collateral-lien-guide.md).

## Lifecycle

**Pronunciation:** LYF-sy-kuhl

The full history of an asset from registration to retirement. In the codebase, *Lifecycle* also names the contract that records maintenance, computes the [collateral score](#collateral-score) and enforces state changes.

*See also:* [Asset](#asset), [Maintenance record](#maintenance-record), [architecture.md](architecture.md).

## Liquidation

**Pronunciation:** lik-wuh-DAY-shuhn

The enforced sale or seizure of collateral after a borrower defaults on a [loan](#loan) or breaches a [covenant](#covenant). Mainstay records the default status and the matching [lien](#lien); the lender then takes action off-chain or through a lending protocol.

*Aliases:* liquidate (verb), default enforcement.

*See also:* [Lien](#lien), [Covenant](#covenant), [lender-integration-guide.md](lender-integration-guide.md).

## Loan

**Pronunciation:** lohn

A borrowed amount given to an asset owner, secured by the owner's [collateral](#collateral). Loan terms, including [covenants](#covenant) and deadlines, are handled by the lending contract.

*See also:* [Collateral](#collateral), [Covenant](#covenant), [Liquidation](#liquidation).

## Maintenance record

**Pronunciation:** MAIN-tuh-nuhns REK-erd

An immutable entry showing that an [engineer](#engineer) performed a specific task on an [asset](#asset) at a point in time. Each record includes a task type, notes and the engineer's address. Records feed the [collateral score](#collateral-score).

*Aliases:* maintenance event, maintenance task.

*See also:* [Proof of maintenance](#proof-of-maintenance), [Engineer](#engineer).

## Proof of maintenance

**Pronunciation:** proof uhv MAIN-tuh-nuhns

The core idea behind Mainstay: maintenance is signed by a credentialed engineer and recorded on a blockchain, so a lender can verify an asset's real condition without trusting the owner.

*See also:* [Maintenance record](#maintenance-record), [README.md](../README.md).

## Slash

**Pronunciation:** slash

A penalty that lowers an asset's [collateral score](#collateral-score) after a serious event, such as a failed inspection or a fraud finding. Slashing a collateralized asset can release or trigger a [lien](#lien) depending on the lending rules.

*See also:* [Collateral score](#collateral-score), [Lien](#lien).

## Timelock

**Pronunciation:** TIME-lok

A required waiting period between an admin proposing a sensitive change and that change taking effect. Timelocks give users time to react before a change such as an [upgrade](#upgrade) executes.

*See also:* [Upgrade](#upgrade), [access-control.md](access-control.md).

## Upgrade

**Pronunciation:** up-GRAYD

A replacement of a contract's WASM code while keeping its stored data. Upgrades are proposed by the admin, wait for the [timelock](#timelock), and are then executed. Testing is described in [upgrade-testing.md](upgrade-testing.md).

*See also:* [Timelock](#timelock), [upgrade-testing.md](upgrade-testing.md).

## Version history

Changes to terms are recorded here. Add an entry whenever a term's meaning, aliases or pronunciation changes.

| Date | Term | Change |
|------|------|--------|
| 2026-10-16 | All | Initial glossary created (issue #1723). |
