# Asset Search API

`search_assets` is a contract-level function on the **Asset Registry** that filters and sorts registered assets in real time, eliminating the need to index the entire registry off-chain.

## Function Signature

```rust
pub fn search_assets(env: Env, filter: SearchFilter) -> SearchPage
```

## SearchFilter

| Field | Type | Description |
|---|---|---|
| `asset_type` | `Option<Symbol>` | Exact match on asset type (e.g. `"TURBINE"`). Omit to match all types. |
| `manufacturer` | `Option<String>` | Case-sensitive substring match against the asset's `metadata` field. |
| `min_age_months` | `Option<u32>` | Only return assets registered **at least** this many months ago (1 month = 30 days). |
| `max_age_months` | `Option<u32>` | Only return assets registered **at most** this many months ago. |
| `sort` | `Option<SortOrder>` | Sort order applied to matched results. |
| `lifecycle_contract` | `Option<Address>` | Required when `sort = ByCollateralScore`. Address of the Lifecycle contract. |

All fields are optional. Omitting a field applies no constraint for that dimension.

## SortOrder

| Variant | Description |
|---|---|
| `ByCollateralScore` | Descending by on-chain collateral score. Requires `lifecycle_contract` to be set. |
| `ByMaintenanceDate` | Descending by `metadata_updated_at` (most recently updated first). |

## SearchPage

| Field | Type | Description |
|---|---|---|
| `assets` | `Vec<Asset>` | Matched assets (capped at **100**). |
| `total` | `u32` | Total match count before the 100-result cap. |

## Limits

- Maximum **100** assets returned per call.
- `total` always reflects the full match count so callers can detect truncation.

## Examples

### Filter by type

```json
{
  "asset_type": "TURBINE",
  "manufacturer": null,
  "min_age_months": null,
  "max_age_months": null,
  "sort": null,
  "lifecycle_contract": null
}
```

### Filter by manufacturer substring

```json
{
  "asset_type": null,
  "manufacturer": "Siemens",
  "min_age_months": null,
  "max_age_months": null,
  "sort": null,
  "lifecycle_contract": null
}
```

### Assets registered between 6 and 24 months ago, sorted by collateral score

```json
{
  "asset_type": null,
  "manufacturer": null,
  "min_age_months": 6,
  "max_age_months": 24,
  "sort": "ByCollateralScore",
  "lifecycle_contract": "<lifecycle-contract-address>"
}
```

## Notes

- The `manufacturer` filter matches anywhere in the `metadata` string. Store manufacturer name as part of the metadata (e.g. `"Siemens SGT-800 Turbine"`) to make this useful.
- `ByMaintenanceDate` sorts by `metadata_updated_at`. This is the best available proxy within the Asset Registry; for precise last-maintenance timestamps use the Lifecycle contract directly.
- Cross-contract calls for `ByCollateralScore` add per-asset invocation cost. Prefer this sort only when the result set is small.
