# Soroban Storage & TTL Strategy

Soroban persistent storage entries expire if their Time-To-Live (TTL) is not extended. To prevent silent data loss, Mainstay contracts follow a standardized TTL management approach.

## Storage Types

- **Instance Storage**: Used for shared contract configuration (admin address, registry bindings, etc.). Instance storage is automatically extended when the contract is called, but should still be managed for longevity.
- **Persistent Storage**: Used for all asset-specific data, maintenance records, and scores. **Requires explicit extension** to ensure longevity.

## TTL Parameters

Mainstay uses a standardized 30-day extension policy:
- **Threshold**: 518,400 ledgers (~30 days at 5s/ledger)
- **Target**: 518,400 ledgers (~30 days)

## Contract Storage Keys

### 1. Asset Registry

| Key Pattern | Storage Type | Description |
| ----------- | ------------ | ----------- |
| `(Symbol("ASSET"), id: u64)` | Persistent | Full `Asset` record (metadata, owner, etc.) |
| `(Symbol("DEDUP"), owner: Address, hash: BytesN<32>)` | Persistent | Mapping of unique metadata to active asset IDs |
| `Symbol("A_COUNT")` | Instance | Global counter for total registered assets |
| `Symbol("ADMIN")` | Instance | Admin address authorized for deregistration |

### 2. Engineer Registry

| Key Pattern | Storage Type | Description |
| ----------- | ------------ | ----------- |
| `(Symbol("ENG"), addr: Address)` | Persistent | `Engineer` record (credential hash, active status) |
| `(Symbol("TRUSTED"), issuer: Address)` | Instance | Registry of trusted credential issuers |
| `Symbol("ADMIN")` | Instance | Admin address authorized for trust management |

### 3. Lifecycle Contract

| Key Pattern | Storage Type | Description |
| ----------- | ------------ | ----------- |
| `(Symbol("HIST"), asset_id: u64)` | Persistent | `Vec<MaintenanceRecord>` of all verified events |
| `(Symbol("SCORE"), asset_id: u64)` | Persistent | Current maintenance health score (0-100) |
| `(Symbol("L_UPD"), asset_id: u64)` | Persistent | Timestamp of the last maintenance or decay event |
| `Symbol("REGISTRY")` | Instance | Linked Asset Registry contract address |
| `Symbol("ENG_REG")` | Instance | Linked Engineer Registry contract address |
| `Symbol("CONFIG")` | Instance | `Config` record (max history, decay rates, etc.) |

## Extension Logic

- **Automatic Extension**: All `persistent` entries should be extended upon every `set` operation.
- **Manual Extension**: Use the Soroban CLI to extend entries if they are near expiration but no write operations are expected.

```bash
# Example manual extension
stellar contract storage extend --id <CONTRACT_ID> \
  --key '<KEY_XDR>' \
  --durability persistent \
  --ledgers-to-extend 518400
```
