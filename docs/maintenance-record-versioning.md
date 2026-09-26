# Maintenance record corrections

Maintenance records remain immutable after submission. An administrator can
publish a correction with `correct_maintenance_record`, which appends a
versioned `MaintenanceCorrection` linked to the original asset and history
index. The original record is never overwritten, so auditors can reconstruct
both the submitted value and every subsequent correction.

Use `get_maintenance_record_versions(asset_id, record_index)` to retrieve
corrections in ascending version order. Consumers should apply the last
correction as the current interpretation while retaining all earlier versions
for audit and dispute workflows. Each correction emits `MNT_CORR`.
