# Asset Lifecycle

This document describes the full lifecycle of an industrial asset in Mainstay, focusing on the distinction between **deprecation** and **deletion (deregistration)**.

## Lifecycle States

Every asset carries a `deprecation_status` field embedded directly in the `Asset` struct:

| State            | Value | Who sets it      | Description                                                        |
|------------------|-------|------------------|--------------------------------------------------------------------|
| `Active`         | 0     | System (default) | Asset is operational and eligible for collateral scoring.          |
| `Deprecated`     | 1     | Asset owner      | Asset has reached end-of-life. Audit trail preserved; score = 0.  |
| `Decommissioned` | 2     | Reserved         | Planned for formal decommission workflow integration.              |

Assets also have a separate admin-controlled decommission flag (via `decommission_asset`) that freezes the collateral score at its last known value.

## State Transitions

```
         register_asset()
                │
                ▼
           [ Active ]
                │
  deprecate_asset() (owner-only)
                │
                ▼
         [ Deprecated ]   ← terminal; cannot be undone
```

Once an asset is `Deprecated`, calling `deprecate_asset` again returns `AssetAlreadyDeprecated`.

## Deprecation vs Deletion

| Dimension               | Deprecation                                | Deletion (Deregistration)                    |
|-------------------------|--------------------------------------------|----------------------------------------------|
| **Who can do it**       | Asset owner (self-service)                 | Owner or admin (with 48-hr timelock)         |
| **Data retention**      | Full audit trail preserved on-chain        | Asset record removed from storage            |
| **Collateral score**    | Returns 0 immediately                      | Asset no longer queryable                    |
| **Reversibility**       | One-way (terminal state)                   | Permanent removal                            |
| **Maintenance history** | History readable; no new records accepted  | History remains until TTL expiry             |
| **Use case**            | End-of-life machinery still under lien     | Clerical errors, test asset cleanup          |

## When to Deprecate

Use `deprecate_asset` when:

- Machinery has reached end-of-service life but its maintenance history must be legally preserved (e.g., under a financing agreement).
- An asset should no longer qualify as DeFi collateral while keeping the on-chain audit trail intact for compliance or dispute resolution.
- The owner wants to signal to lenders that the machine is no longer operational without requiring admin involvement.

## When to Deregister

Use `propose_deregister_asset` → `execute_deregister_asset` (48-hr timelock) when:

- The asset was registered in error (wrong serial number, test data).
- You need to free up the serial-number dedup slot for a corrected registration.
- Full removal is acceptable.

## Transfer Rules by Deprecation State

`transfer_asset` and `initiate_ownership_transfer` (the propose step of the two-step
transfer flow) both reject any asset whose `deprecation_status` is not `Active`:

| `deprecation_status` | Transferable? | Error on attempt         |
|-----------------------|----------------|---------------------------|
| `Active`               | Yes            | —                          |
| `Deprecated`           | No             | `AssetDecommissioned`    |
| `Decommissioned`       | No             | `AssetDecommissioned`    |

This applies regardless of the asset's separate admin-controlled decommission flag —
either lifecycle state alone is sufficient to block a transfer.

## API Reference

### Asset Registry

```rust
/// Mark an asset as deprecated (owner-only).
deprecate_asset(owner: Address, asset_id: u64, reason: String)
```

| Parameter  | Type      | Description                             |
|------------|-----------|-----------------------------------------|
| `owner`    | `Address` | Must match the stored asset owner       |
| `asset_id` | `u64`     | Unique asset identifier                 |
| `reason`   | `String`  | Human-readable end-of-life explanation  |

**Errors:**
- `AssetNotFound` — no asset with the given ID exists
- `UnauthorizedOwner` — caller is not the asset owner
- `AssetAlreadyDeprecated` — asset is already `Deprecated` or `Decommissioned`

**Event:** `(DEPRECATED, asset_id)` → `(owner, reason, timestamp)`

### Lifecycle Contract

`get_collateral_score(asset_id)` returns `0` immediately for any asset whose `deprecation_status != Active`. No decay computation is performed.

## TTL Considerations

`deprecate_asset` extends the TTL of both the asset record and the stored deprecation reason for 30 days. See [docs/ttl-strategy.md](ttl-strategy.md) for the full TTL strategy.
