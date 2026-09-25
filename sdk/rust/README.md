# Mainstay Rust SDK

`mainstay-sdk` is the official typed Rust facade for the Mainstay Asset
Registry Soroban contract. It wraps the generated contract client and keeps
application code independent of generated-client details.

```rust
use mainstay_sdk::{MainstayClient, SearchFilter};

let client = MainstayClient::new(&env, &asset_registry_id);
let page = client.search_assets(&SearchFilter {
    asset_type: None,
    manufacturer: None,
    min_age_months: None,
    max_age_months: None,
    sort: None,
    lifecycle_contract: None,
});
println!("{} matching assets", page.total);
```

The crate is included in the workspace, so it can be tested with
`cargo test -p mainstay-sdk`.
