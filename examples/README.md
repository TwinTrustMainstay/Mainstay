# Examples

Runnable Stellar CLI scripts that exercise one happy-path flow per Mainstay
contract. They are intentionally small so integrators can copy them as a
starting point.

| Script | Contract | Flow |
|---|---|---|
| `credential_management.sh` / `.ps1` | Engineer Registry | Register an engineer, verify, rotate credential |
| `governance_proposal.sh` | Governance | Mint shares, create proposal, vote, finalize, execute |
| `bridge_atomic_swap.sh` | Bridge | Create a hashed-timelock swap, claim with the preimage |
| `marketplace_auction.sh` | Marketplace | Read dynamic price, create open auction, place bid, finalize |

## Running

Each script reads its required inputs from environment variables and
documents them at the top. The shared defaults are:

- `STELLAR_NETWORK` — default `testnet`
- `STELLAR_RPC_URL` — default `https://soroban-testnet.stellar.org`

Deploy and initialize the target contract first (see `scripts/` or the
contract's README), export its address as `CONTRACT_<NAME>`, then run the
script:

```bash
export CONTRACT_GOVERNANCE=C...
export ADMIN_ADDRESS=G...
export HOLDER_ADDRESS=G...
export PARAM_NAME=fee_bps
export PARAM_VALUE=150
./examples/governance_proposal.sh
```

The scripts are deliberately conservative — they create a single artefact
and verify it with a read-back. They are not benchmarks.
