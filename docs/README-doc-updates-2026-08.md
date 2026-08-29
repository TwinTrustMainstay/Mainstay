# Documentation Update Summary (2026-08-29)

This branch (`docs/q3-documentation-updates`) addresses four documentation gaps, one commit per issue.

## 1. `docs/credentialing.md` (Medium priority)
- Added an ASCII credential lifecycle diagram: Issuance → Valid → Grace Period → Hard-Expired → Revoked.
- Documented the 7-day post-expiry grace period (`GRACE_PERIOD_SECS`): when it applies, how `set_grace_period` configures it (1–90 day bounds), and its effect on maintenance submission and renewal timing.
- Documented specialization-based task matching (`verify_engineer_for_task`, `add_specialization`) and how a `SpecializationNotCovered` error blocks maintenance submissions from engineers lacking the required task-type qualification.

## 2. `docs/iot-integration-design.md` + `docs/roadmap.md` (Low priority)
- Created `docs/iot-integration-design.md`: a design proposal for v2.0 IoT sensor integration, covering the proposed `submit_sensor_event` interface, a trust model that keeps IoT Attestors weaker-trust than credentialed engineers (sensors can flag risk but not unilaterally raise collateral scores), and an on-chain `SensorEvent` data format. Includes an explicit feedback section for IoT integrators.
- Created `docs/roadmap.md`, which was linked from `README.md` but did not previously exist, and referenced the new design doc from it.

## 3. `docs/deployment-runbook.md` (Medium priority)
- Added a "Post-Deploy Configuration" section (4.5) documenting `max_submissions_per_hour`: what `0` means (rate limiting disabled), recommended values by fleet size, and a verification step using `get_config` and `check_engineer_submission_rate`.

## 4. `docs/threat-model.md` (High priority)
- Added threat entry T-AR-11 / DI-03: a single compromised admin key can propose and execute a malicious contract upgrade via `propose_upgrade`/`execute_upgrade`.
- Documented `set_admin_quorum` multisig as the primary mitigation and the existing upgrade timelock as a secondary, defense-in-depth mitigation, including the residual risk if an attacker compromises quorum-threshold-many keys at once.

Each commit's diff is under 150 lines; this README was added per the task's size-reporting convention.
