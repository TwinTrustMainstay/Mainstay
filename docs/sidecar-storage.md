# Sidecar Storage for Large Assets

Closes #1760.

## Purpose

Large attachments such as photos and reports bloat contract storage. A sidecar
keeps the bytes off-chain in content-addressed storage (IPFS or Arweave) and
stores only a hash and metadata on-chain.

## On-chain record

Each sidecar is a `SidecarRecord`:

| Field | Meaning |
|-------|---------|
| `content_hash` | SHA-256 digest of the bytes |
| `uri` | Content address, e.g. `ipfs://...` or `ar://...` |
| `size_bytes` | Size of the blob, used for cost tracking |
| `added_at` | Ledger timestamp when added |
| `retain_until` | Timestamp after which anyone may expire it (`0` = no expiry) |
| `deleted` | `true` once removed or expired |

## Operations

- `add_sidecar(caller, asset_id, content_hash, uri, size_bytes, retain_until) -> index`
  Owner-only. Stores the record and adds `size_bytes` to the tracked total.
- `verify_sidecar(asset_id, index, content) -> bool`
  Recomputes SHA-256 over the downloaded bytes and compares it to `content_hash`.
- `remove_sidecar(caller, asset_id, index)`
  Owner-only. Marks the record deleted and releases its tracked bytes.
- `expire_sidecar(asset_id, index)`
  Callable by anyone once `retain_until` has passed, so retention is enforced
  without the owner's cooperation. Fails with `RetentionNotElapsed` before then.
- `get_sidecars(asset_id)` and `get_sidecar_bytes(asset_id)`
  Read the records and the total live off-chain bytes for cost tracking.

## Off-chain responsibilities

The contract cannot delete or pin bytes on IPFS or Arweave. Operators must:
- Pin the blob before calling `add_sidecar`, and unpin it after
  `remove_sidecar` or `expire_sidecar`.
- Verify downloads with `verify_sidecar` before trusting them.

## Limitations

- Deletion is logical on-chain. Content that was already replicated by an
  external network may still be retrievable there.
- `get_sidecar_bytes` tracks declared sizes, not measured storage fees.
