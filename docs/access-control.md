# Access Control Model

## submit_maintenance

Maintenance submission uses owner-approved per-asset authorization.

An engineer must:

1. Hold a valid credential in the configured engineer registry.
2. Be explicitly authorized for the asset by the current asset owner through `authorize_engineer(owner, asset_id, engineer)`.

The owner can revoke this relationship with `revoke_engineer_auth(owner, asset_id, engineer)`.

This prevents a credentialed engineer from submitting records for assets they have no owner-approved relationship with.
