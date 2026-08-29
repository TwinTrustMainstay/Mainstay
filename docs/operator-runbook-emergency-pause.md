# Operator Runbook: Emergency Pause

This runbook provides operators with the procedures for triggering, verifying, and recovering from an emergency pause across all Mainstay contracts.

## Overview

Each Mainstay contract provides a `pause`/`unpause` mechanism gated by the contract admin. When paused, all state-mutating (write) operations are rejected with a `ContractError::Paused` error. Read-only (view) functions remain accessible so operators and integrators can still inspect contract state during an incident.

### Contracts & Error Codes

| Contract | Pause error code | Storage key |
|---|---|---|
| `asset-registry` | `7` (`AssetRegistry::Paused`) | `PAUSED` (persistent) |
| `engineer-registry` | `8` (`EngineerRegistry::Paused`) | `PAUSED` (persistent) |
| `lifecycle` | `9` (`Lifecycle::Paused`) | `PAUSED` (persistent) |
| `lending` | `13` (`Lending::ContractPaused`) | `PAUSED` (persistent) |

Each contract pauses independently. Pausing one contract does **not** pause any other contract.

---

## When to Trigger an Emergency Pause

### Immediate-trigger conditions (pause now, investigate later)

1. **Suspected exploit or unauthorized access**
   - Unexpected asset registrations or ownership transfers
   - Unauthorized maintenance records appearing on assets
   - Loans being requested or repaid without borrower consent
   - Vouches appearing from unknown addresses

2. **Critical contract bug discovered**
   - A vulnerability has been responsibly disclosed or publicly reported
   - Unexpected panic/revert patterns observed in production
   - Score calculation returning incorrect values

3. **Oracle or external dependency failure**
   - The Stellar network itself is experiencing instability
   - A critical dependency (token contract, issuer registry) is compromised

4. **Admin key compromise**
   - Any indication that the admin private key has been exposed

### Evaluation conditions (investigate first, then decide)

1. **Unusual activity patterns**
   - Spike in maintenance submissions from a single engineer
   - Rapid succession of loan requests from new borrowers
   - Multiple lien records created in a short window

2. **Score anomalies**
   - Collateral scores jumping to 100 or dropping to 0 unexpectedly
   - Assets not decaying as expected

### Decision matrix

| Condition | Severity | Action | Pause which contracts? |
|---|---|---|---|
| Unauthorized asset registration | High | Pause `asset-registry` immediately | `asset-registry` |
| Loan exploit suspected | Critical | Pause `lending` immediately | `lending` |
| Maintenance fraud (engineer compromise) | High | Pause `lifecycle` immediately | `lifecycle` |
| Admin key compromise | Critical | Pause **all** contracts | All four |
| Network-level incident | High | Pause `lending` first, then others | Prioritize `lending` |
| Unexpected contract panic | Medium | Pause the affected contract | Affected contract |

---

## Step-by-Step Pause Procedure

### Prerequisites

- Access to the admin private key for the contract(s) you need to pause
- Stellar CLI installed and configured for the correct network
- Contract ID(s) for the contract(s) to be paused

### Pause a single contract

```bash
# Replace <CONTRACT_ID> with the actual contract ID
# Replace <ADMIN_SECRET> with the admin's secret key (or use a hardware wallet)

stellar contract invoke \
  --id <CONTRACT_ID> \
  --network <mainnet|testnet> \
  --source <ADMIN_IDENTITY> \
  -- \
  pause \
  --admin <ADMIN_ADDRESS>
```

**Example** — pausing the lending contract:
```bash
stellar contract invoke \
  --id CBC7G4QJ... \
  --network mainnet \
  --source admin \
  -- \
  pause \
  --admin GABC...DEF
```

### Pause all contracts (full emergency)

```bash
#!/bin/bash
# emergency-pause-all.sh — pauses every Mainstay contract
set -euo pipefail

NETWORK="${1:-mainnet}"

echo "⚠️  Pausing all Mainstay contracts on $NETWORK"

for CONTRACT_ID in "$AR_ID" "$ER_ID" "$LC_ID" "$LENDING_ID"; do
  echo "Pausing $CONTRACT_ID ..."
  stellar contract invoke \
    --id "$CONTRACT_ID" \
    --network "$NETWORK" \
    --source admin \
    -- \
    pause \
    --admin "$ADMIN_ADDRESS"
  echo "  ✅ $CONTRACT_ID paused"
done

echo "All contracts paused."
```

### What happens when paused

| Contract | Blocked operations |
|---|---|
| `asset-registry` | `register_asset`, `batch_register_assets`, `update_asset_metadata`, `transfer_asset`, `decommission_asset`, `deprecate_asset` |
| `engineer-registry` | `register_engineer`, `revoke_credential`, `renew_credential`, `add_trusted_issuer` |
| `lifecycle` | `submit_maintenance`, `batch_submit_maintenance`, `authorize_engineer`, `decay_score` |
| `lending` | `request_loan`, `repay`, `vouch` |

**Read-only functions remain accessible** — `get_asset`, `get_collateral_score`, `get_loan`, `get_liens`, `is_paused`, etc.

---

## Verification Steps

After pausing, verify the pause took effect before declaring the incident contained.

### 1. Verify pause state

For each paused contract, confirm `is_paused` returns `true`:

```bash
stellar contract invoke \
  --id <CONTRACT_ID> \
  --network <NETWORK> \
  --source any \
  -- \
  is_paused
# Expected output: true
```

### 2. Verify writes are blocked

Attempt a write operation and confirm it returns a `Paused` error:

**Asset Registry:**
```bash
# Attempt to register a test asset — must fail with error code 7
stellar contract invoke \
  --id "$AR_ID" \
  --network mainnet \
  --source any \
  -- \
  register_asset \
  --asset_type GENSET \
  --metadata "PAUSE_TEST" \
  --serial_number "SN-PAUSE-VERIFY-001" \
  --owner "$TEST_OWNER"
# Expected: contract error 7 (Paused)
```

**Lifecycle:**
```bash
# Attempt to submit maintenance — must fail with error code 9
stellar contract invoke \
  --id "$LC_ID" \
  --network mainnet \
  --source any \
  -- \
  submit_maintenance \
  --asset_id 1 \
  --task_type OIL_CHG \
  --notes "Pause verification" \
  --engineer "$TEST_ENGINEER"
# Expected: contract error 9 (Paused)
```

**Lending:**
```bash
# Attempt to request a loan — must fail with error code 13
stellar contract invoke \
  --id "$LENDING_ID" \
  --network mainnet \
  --source any \
  -- \
  request_loan \
  --borrower "$TEST_BORROWER" \
  --amount 1
# Expected: contract error 13 (ContractPaused)
```

### 3. Verify reads still work

```bash
# Asset lookup must still return data
stellar contract invoke \
  --id "$AR_ID" \
  --network mainnet \
  --source any \
  -- \
  get_asset \
  --asset_id 1

# Collateral score must still be queryable
stellar contract invoke \
  --id "$LC_ID" \
  --network mainnet \
  --source any \
  -- \
  get_collateral_score \
  --asset_id 1
```

### 4. Check for TTL expiry risk

While the contract is paused, persistent storage entries may approach TTL expiry. Extend storage if the pause is expected to last more than a few days:

```bash
# Extend instance storage by 30 days for all contracts
for ID in "$AR_ID" "$ER_ID" "$LC_ID" "$LENDING_ID"; do
  stellar contract storage extend \
    --id "$ID" \
    --network mainnet \
    --durability persistent \
    --ledgers-to-extend 518400
done
```

---

## Resuming (Unpause)

Only unpause once you have confirmed:
- The root cause of the incident is fully understood
- A fix or mitigation has been deployed (if applicable)
- No unauthorized state changes occurred while the contract was paused

### Step-by-step unpause

```bash
# Replace <CONTRACT_ID> and admin credentials
stellar contract invoke \
  --id <CONTRACT_ID> \
  --network <NETWORK> \
  --source admin \
  -- \
  unpause \
  --admin <ADMIN_ADDRESS>
```

### Verify after unpause

```bash
# 1. Confirm unpause state
stellar contract invoke --id <CONTRACT_ID> --network mainnet --source any -- is_paused
# Expected: false

# 2. Smoke test: perform a legitimate write
stellar contract invoke \
  --id "$AR_ID" \
  --network mainnet \
  --source owner \
  -- \
  register_asset \
  --asset_type GENSET \
  --metadata "Post-unpause smoke test" \
  --serial_number "SN-SMOKE-$(date +%s)" \
  --owner "$OWNER_ADDRESS"
# Expected: returns a new asset_id

# 3. Clean up test data if possible
stellar contract invoke \
  --id "$AR_ID" \
  --network mainnet \
  --source owner \
  -- \
  deprecate_asset \
  --owner "$OWNER_ADDRESS" \
  --asset_id <SMOKE_TEST_ASSET_ID> \
  --reason "Post-unpause smoke test cleanup"
```

### Unpause all contracts

```bash
#!/bin/bash
# emergency-unpause-all.sh
set -euo pipefail

NETWORK="${1:-mainnet}"

echo "▶️  Unpausing all Mainstay contracts on $NETWORK"

for CONTRACT_ID in "$AR_ID" "$ER_ID" "$LC_ID" "$LENDING_ID"; do
  echo "Unpausing $CONTRACT_ID ..."
  stellar contract invoke \
    --id "$CONTRACT_ID" \
    --network "$NETWORK" \
    --source admin \
    -- \
    unpause \
    --admin "$ADMIN_ADDRESS"
  echo "  ✅ $CONTRACT_ID unpaused"
done

echo "All contracts unpaused. Run verification steps from §Resuming above."
```

---

## Rollback & Recovery Procedures

### Scenario 1: Accidental pause (no actual incident)

1. Confirm no writes occurred during the pause window (review event logs)
2. Verify contract state is consistent (check last-known-good snapshots)
3. Unpause using the procedure above
4. Run smoke tests
5. Document the false-alarm in the incident log

### Scenario 2: Exploit confirmed, attacker changed state

1. **Do not unpause** until all affected state is audited
2. Identify every state change the attacker made:
   - List all events emitted during the incident window
   - Compare storage snapshots before/after
3. Prepare a remediation transaction that reverses unauthorized changes:
   - Revoke fraudulent engineer credentials
   - Transfer assets back to legitimate owners
   - Cancel fraudulent loans (mark as defaulted)
   - Release unauthorized liens
4. Deploy a **fixed contract version** if the exploit was a code bug
5. Execute the remediation transaction
6. Unpause only after remediation is verified on-chain

### Scenario 3: Contract upgrade required

1. Audit the pause window — all state must be consistent
2. Deploy the patched WASM to the same contract ID:
   ```bash
   stellar contract install --wasm fixed_contract.wasm
   stellar contract deploy \
     --wasm-hash <NEW_WASM_HASH> \
     --network mainnet \
     --source deployer \
     --id <EXISTING_CONTRACT_ID>
   ```
3. Verify that existing storage is readable by the new code
4. Unpause the upgraded contract
5. Run the full smoke test suite

---

## Incident Log Template

After every pause event, complete this log entry:

```markdown
### Incident #<NUMBER> — <DATE>

- **Trigger**: [What caused the pause?]
- **Severity**: [Critical / High / Medium]
- **Contracts paused**: [e.g., lending only]
- **Time paused**: [UTC timestamp]
- **Time unpaused**: [UTC timestamp]
- **Duration**: [HH:MM]
- **Root cause**: [Brief description]
- **State changes during pause**: [None / list of changes]
- **Remediation actions**: [What was done before unpausing]
- **Post-mortem link**: [URI]
```

---

## Admin Key Management

### Best practices

- Use a **multisig account** for the admin role on mainnet
- Store admin keys in a hardware wallet (Ledger) or HSM
- Maintain a separate "emergency admin" key for pause-only operations
- Never share the admin key; use the two-step admin transfer to rotate

### Admin transfer procedure

```bash
# 1. Current admin initiates transfer
stellar contract invoke \
  --id <CONTRACT_ID> \
  --network mainnet \
  --source current_admin \
  -- \
  transfer_admin \
  --admin <CURRENT_ADMIN> \
  --new_admin <NEW_ADMIN>

# 2. New admin accepts (after timelock expires)
stellar contract invoke \
  --id <CONTRACT_ID> \
  --network mainnet \
  --source new_admin \
  -- \
  accept_admin \
  --new_admin <NEW_ADMIN>
```

---

## Quick Reference Card

| Action | Command |
|---|---|
| **Pause lending** | `stellar contract invoke --id $LENDING_ID --network mainnet --source admin -- pause --admin $ADMIN` |
| **Pause all** | Run `emergency-pause-all.sh mainnet` |
| **Check pause state** | `stellar contract invoke --id $ID --network mainnet --source any -- is_paused` |
| **Unpause** | `stellar contract invoke --id $ID --network mainnet --source admin -- unpause --admin $ADMIN` |
| **Extend TTL** | `stellar contract storage extend --id $ID --network mainnet --durability persistent --ledgers-to-extend 518400` |
| **View events** | `stellar events --network mainnet --contract-id $ID --start-ledger <FIRST> --count 100` |

---

*This runbook is maintained alongside the Mainstay smart contract system. Review and rehearse the pause procedure at least once per quarter.*
