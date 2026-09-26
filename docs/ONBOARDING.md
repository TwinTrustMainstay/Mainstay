# Developer Onboarding Guide

Welcome to Mainstay! This guide will help you get up and running quickly. You'll learn how to set up the environment, run tests, deploy to testnet, and contribute effectively.

**Time estimate:** 30-45 minutes for complete setup

## Prerequisites

Before you begin, ensure you have:
- Rust 1.70+ with `wasm32-unknown-unknown` target
- Stellar CLI 22.5.0+ (for contract deployment)
- Docker (for local test ledger)
- Git
- A code editor (VS Code recommended)

## Quick Start (5 minutes)

### 1. Clone and Install Dependencies

```bash
git clone https://github.com/TwinTrustMainstay/Mainstay.git
cd Mainstay

# Install Rust toolchain and WASM target
rustup update
rustup target add wasm32-unknown-unknown

# Install Stellar CLI (macOS example; see stellar.org for other OS)
brew install stellar/stellar-dev/stellar-cli
```

### 2. Run Tests

```bash
# Run all tests
cargo test --workspace

# Run a specific test file
cargo test --test test_reputation_score

# Run with output
cargo test --workspace -- --nocapture
```

### 3. Build Contracts

```bash
./scripts/build.sh  # Builds all contracts to target/wasm32-unknown-unknown/release/
```

### 4. Deploy to Local Testnet

```bash
# Start local Stellar network
docker run -d --name stellar-local -p 8000:8000 stellar/quickstart:latest --local --enable-soroban-rpc

# Wait for it to be ready
sleep 10

# Configure Stellar CLI
stellar network add local \
  --rpc-url http://localhost:8000/soroban/rpc \
  --network-passphrase "Standalone Network ; February 2017"

# Generate deployer account and fund it
stellar keys generate deployer --network local
stellar keys fund deployer --network local

# Deploy contracts
stellar contract deploy --wasm target/wasm32-unknown-unknown/release/asset_registry.wasm --network local --source deployer
stellar contract deploy --wasm target/wasm32-unknown-unknown/release/engineer_registry.wasm --network local --source deployer
stellar contract deploy --wasm target/wasm32-unknown-unknown/release/lifecycle.wasm --network local --source deployer

# Cleanup when done
docker rm -f stellar-local
```

## Project Structure

Understanding the codebase layout helps you navigate effectively:

```
Mainstay/
├── contracts/               # Smart contract implementations
│   ├── shared/             # Common types and utilities
│   ├── asset-registry/     # Asset registration and tracking
│   ├── engineer-registry/  # Engineer reputation and credentials
│   ├── lending/            # Lending and loan management
│   └── lifecycle/          # Asset lifecycle state machine
│
├── sdk/rust/               # Rust SDK for contract interaction
│
├── tests/                  # Integration tests
│   ├── test_reputation_score.rs
│   ├── test_collateral_model.rs
│   ├── test_full_loan_lien_lifecycle_e2e.rs
│   └── ... (40+ test files)
│
├── docs/                   # Documentation
│   ├── adr/               # Architecture Decision Records
│   ├── architecture.md    # System design overview
│   ├── ONBOARDING.md      # This file
│   ├── deployment-runbook.md
│   └── api-reference.md
│
└── scripts/               # Build and deployment scripts
    ├── build.sh
    └── check_*.sh         # Validation scripts
```

## Module Breakdown

### Asset Registry (`contracts/asset-registry/`)

Manages asset registration and metadata. An asset represents a physical or digital item eligible for lending collateral.

**Key Functions:**
- `register_asset()` — Register a new asset
- `get_asset()` — Retrieve asset details
- `add_asset_type()` — Add a new asset type to allowlist
- `decommission_asset()` — Mark asset as no longer eligible

**Tests:** `tests/test_*.rs` matching `asset*`

### Engineer Registry (`contracts/engineer-registry/`)

Tracks engineer credentials and reputation scores.

**Key Functions:**
- `add_engineer()` — Add engineer to registry
- `issue_credential()` — Issue time-limited credential
- `revoke_credential()` — Revoke engineer credential
- `get_reputation_score()` — Fetch engineer's current score

**Tests:** `tests/test_reputation_score.rs`, `tests/test_engineer_*.rs`

### Lifecycle (`contracts/lifecycle/`)

Implements asset state machine and maintenance record submission.

**Key Functions:**
- `submit_maintenance()` — Record maintenance activity
- `get_maintenance_history()` — Query history for an asset
- `pause()` / `resume()` — Emergency pause mechanism
- `propose_upgrade()` / `execute_upgrade()` — Contract upgrades

**Tests:** `tests/test_lifecycle_*.rs`, `tests/test_full_*.rs`

### Lending (`contracts/lending/`)

Manages loans backed by registered assets.

**Key Functions:**
- `create_loan()` — Initiate a loan with collateral
- `repay_loan()` — Make repayment toward loan
- `get_loan()` — Retrieve loan details
- `liquidate_collateral()` — Enforce default

**Tests:** `tests/test_lending_*.rs`, `tests/test_collateral_*.rs`

## Common Tasks

### Add a New Asset Type

1. Open `contracts/asset-registry/src/types.rs`
2. Add the new type to the `AssetType` enum
3. Update the allowlist in `initialize()` or `add_asset_type()`
4. Add tests in `tests/test_asset_registry.rs`

Example:
```rust
// In contracts/asset-registry/src/types.rs
pub enum AssetType {
    Generator,
    Pump,
    Motor,
    NewAssetType,  // Add here
}
```

### Add a New Maintenance Type

1. Open `contracts/lifecycle/src/types.rs`
2. Add to the `MaintenanceType` enum
3. Register in lifecycle initialization: `add_maintenance_type()`
4. Test in `tests/test_maintenance_types.rs`

### Create a New Test

1. Create file: `tests/test_my_feature.rs`
2. Follow the pattern from existing tests:

```rust
#[test]
fn test_my_feature() {
    let e = Env::default();
    e.mock_all_auths();

    // Setup: deploy contracts, initialize
    let admin = Address::generate(&e);
    let asset_id = e.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(&e, &asset_id);
    client.initialize_admin(&admin, &admin);

    // Execute: call contract functions
    let result = client.my_function();

    // Assert: verify behavior
    assert_eq!(result, expected_value);
}
```

3. Run: `cargo test --test test_my_feature`

## Debugging Tips

### View Contract Events

```bash
# Run a test with output to see contract logs
cargo test --test test_reputation_score -- --nocapture
```

### Check Test Coverage

```bash
# Install tarpaulin
cargo install cargo-tarpaulin

# Generate coverage report
cargo tarpaulin --workspace --out Html --output-dir coverage/
```

### Validate Your Code

```bash
# Run linter
cargo clippy --workspace --all-targets -- -D warnings

# Format code
cargo fmt --all

# Run all checks (what CI runs)
./scripts/check_error_reference.sh
./scripts/check_symbol_short.sh
./scripts/check_error_discriminants.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Common Issues

### "Soroban RPC not ready"
The Stellar test ledger takes time to start. Increase the wait time in deployment script.

```bash
# Wait up to 60 seconds instead of 30
for i in $(seq 1 60); do
  curl -s http://localhost:8000/health && break
  sleep 1
done
```

### "WASM target not found"
Install the WASM compilation target:
```bash
rustup target add wasm32-unknown-unknown
```

### Tests pass locally but fail in CI
Check for non-deterministic tests, especially those relying on timing or random order.

### "cargo test" hangs
Some tests start the Stellar network. Ensure Docker is running:
```bash
docker ps  # Should show no error
```

## Understanding the Codebase

### Key Concepts

1. **Assets:** Physical items eligible as loan collateral (generators, pumps, etc.)
2. **Engineers:** Professionals who perform maintenance and build reputation
3. **Reputation Score:** 0-1000, reflecting engineer trustworthiness
4. **Maintenance Records:** History of work performed on assets
5. **Loans:** Borrowing against asset collateral
6. **Lifecycle:** State machine tracking asset operational status

### Data Flow Example: Creating a Loan

1. Engineer registers asset via **Asset Registry**
2. Engineer builds reputation through **Lifecycle** maintenance submissions
3. Engineer queries reputation score from **Engineer Registry**
4. Engineer initiates loan in **Lending** contract with asset as collateral
5. **Lending** contract verifies collateral via **Asset Registry** and checks reputation in **Engineer Registry**
6. If approved, loan is created; engineer receives funds

### Contract Dependencies

```
Lending
├─ Asset Registry (verify collateral eligibility)
├─ Engineer Registry (check reputation score)
└─ Lifecycle (track maintenance history)

Lifecycle
├─ Asset Registry (verify asset exists)
└─ Engineer Registry (update reputation on maintenance)

Asset Registry
└─ (independent, no internal dependencies)

Engineer Registry
└─ (independent, no internal dependencies)
```

## References

- **Architecture:** See [docs/architecture.md](./architecture.md) for system design
- **ADRs:** See [docs/adr/README.md](./adr/README.md) for design decisions
- **Deployment:** See [docs/deployment-runbook.md](./deployment-runbook.md) for testnet/mainnet procedures
- **API Reference:** See [docs/api-reference.md](./api-reference.md) for contract functions

## Getting Help

1. **Search the code:** Use `grep -r "your_search_term" .` to find implementations
2. **Check tests:** Most features have test files showing usage examples
3. **Read ADRs:** [Architecture Decision Records](./adr/README.md) explain the "why"
4. **Ask:** Post in team chat or open an issue with questions

## Next Steps

- [ ] Set up development environment
- [ ] Run tests successfully
- [ ] Deploy to local testnet
- [ ] Read [Architecture Overview](./architecture.md)
- [ ] Explore one contract module in depth
- [ ] Make a small code change and test it
- [ ] Submit your first PR!

---

**Questions?** Check [docs/ONBOARDING.md](.) or open an issue.
