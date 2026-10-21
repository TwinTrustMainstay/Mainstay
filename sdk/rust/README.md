# Mainstay Rust SDK

`mainstay-sdk` provides typed clients for Mainstay's Asset Registry, Engineer
Registry, Lifecycle, and Lending Soroban contracts. Its bindings are generated
from the SDK's contract interface declarations and do not depend on the
contracts' implementation crates.

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

Credentialing, maintenance submission, collateral scoring, and lending use
separate client instances because each contract is deployed at its own address:

```rust
use mainstay_sdk::{EngineerRegistryClient, LendingClient, LifecycleClient};

let engineers = EngineerRegistryClient::new(&env, &engineer_registry_id);
let status = engineers.get_credential_status(&engineer);

let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
let score = lifecycle.get_collateral_score(&asset_id);

let lending = LendingClient::new(&env, &lending_id);
let loan = lending.get_loan(&borrower);
```

The crate is included in the workspace, so it can be tested with
`cargo test -p mainstay-sdk`.
