# API performance integration

Mainstay's API service is deployed separately from this repository. The
contract workspace provides bounded aggregation and batch primitives that an
API gateway can expose without issuing one RPC request per related record.

## Aggregate asset reads

`LifecycleContract::get_asset_full_snapshots(asset_ids)` returns the same
complete asset state as `get_asset_full_snapshot`, including registry metadata,
collateral score, valuation, and maintenance summary, for up to 50 asset IDs
in one Soroban invocation. A GraphQL resolver can map this function to an
`assets(ids: ...)` field and compose the returned records with other API
fields. Unknown asset IDs retain the single-record endpoint's
`AssetNotFound` behavior.

The deployed API image (`ghcr.io/mainstay/api-server`) is not part of this
repository, so GraphQL schema and resolver changes must be made in that
service.

## Bulk asset reads

`AssetRegistry::batch_get_assets(asset_ids)` resolves up to 50 asset IDs in
one invocation and omits IDs that are not registered. API clients should use
this endpoint for bulk hydration instead of calling `get_asset` sequentially.

## Request tracing

The regional Nginx proxy accepts an optional `X-Request-ID`. If the caller
does not provide one, Nginx generates an ID and returns it on the response.
The same value is forwarded to the API container and written to the access
log, so clients can include it in support reports and operators can correlate
the request across the proxy and application logs.
