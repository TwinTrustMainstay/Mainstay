# Mainstay — Proof of Maintenance for Industrial Assets

A decentralized physical infrastructure network (DePIN) built on Stellar Soroban smart contracts, creating verifiable maintenance audit trails for heavy industrial machinery.

Mainstay solves the **Information Asymmetry** problem in asset financing — lenders don't know the true physical condition of a machine. By anchoring every maintenance event on-chain, Mainstay creates an immutable, verifiable lifecycle record that transforms industrial assets into credible DeFi collateral.

## 🎯 What is Mainstay?

Mainstay is a "digital twin" registry for heavy machinery (generators, turbines, vehicles, and other industrial assets). Each asset gets an on-chain identity whose state is updated exclusively by certified engineers — verified through a Federated Engineering credential system.

Every maintenance task is:

- **Signed** by a certified engineer with a verified on-chain credential
- **Recorded** as an immutable event on the Stellar blockchain
- **Aggregated** into a Soroban-based Life-Cycle Contract that reflects the asset's true condition

This makes Mainstay:

✅ **Trustless** (no central maintenance authority needed)  
✅ **Tamper-proof** (maintenance history cannot be altered or backdated)  
✅ **Credentialed** (only verified engineers can sign maintenance events)  
✅ **Collateral-ready** (assets with verified records qualify for DeFi lending)

## 🚀 Features

- **Asset Registry**: Register industrial assets with unique on-chain identities
- **Engineer Verification**: Federated credentialing system for certified maintenance engineers
- **Maintenance Signing**: Engineers cryptographically sign and submit maintenance records
- **Life-Cycle Contracts**: Soroban contracts that track full asset maintenance history
- **Collateral Scoring**: On-chain health score derived from verified maintenance completeness
- **DeFi Integration**: Assets with verified records usable as collateral for Stellar-based lending protocols

## 🛠️ Quick Start

### Prerequisites

- Rust (1.70+)
- Soroban CLI
- Stellar CLI

### Build

```bash
./scripts/build.sh
```

### Test

From the repository root, run the full workspace test suite (same as CI):

```bash
./scripts/test.sh
```

Optional arguments are forwarded to `cargo test`, for example:

```bash
./scripts/test.sh -p lifecycle
./scripts/test.sh -p lifecycle my_test_name -- --nocapture
```

On Windows (PowerShell):

```powershell
.\scripts\test.ps1
.\scripts\test.ps1 -p lifecycle
```

### Setup Environment

Copy the example environment file:

```bash
cp .env.example .env
```

Configure your environment variables in `.env`:

```bash
# Network configuration
STELLAR_NETWORK=testnet
STELLAR_RPC_URL=https://soroban-testnet.stellar.org

# Contract addresses (after deployment)
CONTRACT_ASSET_REGISTRY=<your-contract-id>
CONTRACT_ENGINEER_REGISTRY=<your-contract-id>
CONTRACT_LIFECYCLE=<your-contract-id>

# Frontend configuration
VITE_STELLAR_NETWORK=testnet
VITE_STELLAR_RPC_URL=https://soroban-testnet.stellar.org
```

Network configurations are defined in `environments.toml`:

- `testnet` — Stellar testnet
- `mainnet` — Stellar mainnet
- `futurenet` — Stellar futurenet
- `standalone` — Local development

### Deploy to Testnet

```bash
# Configure your testnet identity first
stellar keys generate deployer --network testnet

# Deploy
./scripts/deploy_testnet.sh
```

## 📖 Documentation

- [Architecture Overview](docs/architecture.md)
- [Life-Cycle Contract Design](docs/lifecycle-contract.md)
- [Engineer Credentialing](docs/credentialing.md)
- [Collateral Scoring Model](docs/collateral-scoring.md)
- [Threat Model & Security](docs/threat-model.md)
- [Roadmap](docs/roadmap.md)

## 🎓 Smart Contract API

### Asset Registry

```rust
register_asset(asset_id, asset_type, metadata) -> u64
get_asset(asset_id) -> Asset
get_lifecycle_score(asset_id) -> u32
```

### Engineer Registry

```rust
register_engineer(engineer_address, credential_hash, issuer)
verify_engineer(engineer_address) -> bool
revoke_credential(engineer_address)
```

### Maintenance Records

```rust
submit_maintenance(asset_id, task_type, notes, engineer_signature)
get_maintenance_history(asset_id) -> Vec<MaintenanceRecord>
get_last_service(asset_id) -> MaintenanceRecord
```

### Collateral

```rust
get_collateral_score(asset_id) -> u32
is_collateral_eligible(asset_id) -> bool
```

## 🧪 Testing

Comprehensive test suite covering:

✅ Asset registration and metadata  
✅ Engineer credential issuance and verification  
✅ Maintenance record submission and signing  
✅ Life-cycle contract state transitions  
✅ Collateral score calculation  
✅ Error handling and edge cases  
✅ TTL extension verification  

Run tests:

```bash
cargo test
```

## ⏱️ TTL (Time-To-Live) Strategy

Soroban persistent storage entries have a limited Time-To-Live (TTL) and will expire if not extended. To prevent silent data loss, all three contracts automatically extend the TTL of persistent storage entries after every write operation.

### TTL Configuration

- **Extension Threshold**: 518,400 ledgers (~30 days)
- **Extension Target**: 518,400 ledgers (~30 days)
- **Strategy**: Extend on every write to ensure data remains accessible

### Protected Data

**Asset Registry:**
- Asset records (asset ID → Asset struct)
- Deduplication keys (owner + metadata hash → asset ID)

**Engineer Registry:**
- Engineer credentials (address → Engineer struct)

**Lifecycle Contract:**
- Maintenance history (asset ID → Vec<MaintenanceRecord>)
- Collateral scores (asset ID → u32)

### Why This Matters

Without TTL extension, critical data could silently expire:
- Asset records would disappear, breaking the entire system
- Engineer credentials would be lost, preventing maintenance verification
- Maintenance histories would vanish, destroying the audit trail
- Collateral scores would reset to zero, invalidating DeFi collateral

The automatic TTL extension ensures that all data remains accessible as long as the contract is actively used, preventing data loss and maintaining system integrity.

For a detailed breakdown of all storage keys and our extension strategy, see [docs/ttl-strategy.md](docs/ttl-strategy.md).

## 🌍 Why This Matters

**The Problem — Information Asymmetry in Asset Financing:**

Industrial assets like generators, turbines, and heavy vehicles represent trillions of dollars in potential collateral globally. Yet lenders routinely reject or under-value these assets because there is no reliable way to verify their physical condition. Maintenance records are paper-based, easily falsified, and siloed.

**The Blockchain Solution:**

- No need for a trusted third-party inspector
- Transparent, append-only maintenance history
- Cryptographic proof that a certified engineer performed each task
- Accessible to any DeFi lender with a Stellar wallet

**Target Users:**

- Industrial asset owners seeking financing
- DeFi lenders needing verifiable collateral
- Certified maintenance engineers building on-chain reputation
- Equipment leasing and fleet management companies

## 🗺️ Roadmap

- **v1.0 (Current)**: Asset registry, engineer credentialing, basic maintenance records
- **v1.1**: Collateral scoring engine, DeFi lender API
- **v2.0**: IoT sensor integration (automated maintenance triggers)
- **v3.0**: Frontend dashboard with wallet integration
- **v4.0**: Mobile app for field engineers, multi-asset portfolio view

See [docs/roadmap.md](docs/roadmap.md) for details.

## 🛡️ Security

We take the security of Mainstay very seriously. If you discover a vulnerability, please refer to our [Security Policy](SECURITY.md) for reporting instructions.

### Dependency Vulnerability Scanning
- **Automated Scanning**: CI workflow runs `cargo audit` on every push and PR
- **Failure Handling**: Build fails if high-severity vulnerabilities are detected
- **Purpose**: Automatically detect known vulnerabilities in Soroban SDK and dependencies
- **Action Required**: Review and update dependencies if audit fails

### Security Best Practices
- **Regular Updates**: Keep dependencies updated to latest secure versions
- **Review Process**: All dependency changes undergo security review
- **Vulnerability Disclosure**: Report security issues through responsible disclosure

## 🤝 Contributing

We welcome contributions! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

See our [Code of Conduct](CODE_OF_CONDUCT.md) and [Contributing Guidelines](CONTRIBUTING.md).

## 📄 License

This project is licensed under the MIT License — see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- [Stellar Development Foundation](https://stellar.org) for Soroban
- The global engineering community for maintaining the machines that power the world
