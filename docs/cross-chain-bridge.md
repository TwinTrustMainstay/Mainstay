# Cross-Chain Asset Bridge

Implements issues #1743 and #1744 (these two issues have identical bodies and are
delivered together). Source: `contracts/bridge/`.

## Scope

This crate is the **Soroban (Stellar) side** of the bridge. The counterpart contract
on other chains (for example an EVM contract written in Solidity) is **not** included
here. It needs its own toolchain and audit, so it should be a separate change.

## Components

| Feature | Entry points | Notes |
|---|---|---|
| Chain registry and constraints | `add_chain`, `update_chain`, `set_chain_enabled`, `get_chain` | Per-chain enablement, min/max amount, required confirmations, rolling outflow window, window cap, alert threshold |
| Outbound transfers | `bridge_out` | Locks tokens and emits `BR_OUT` with a per-destination nonce for relayers |
| Inbound releases | `set_relayers`, `attest_release`, `get_release` | M-of-N relayer attestations. Each `(source_chain, nonce)` executes once. Mismatched parameters are rejected |
| Monitoring | `get_window`, `BR_ALERT` event | Emits an alert when outflow crosses `alert_threshold`. Rejects releases that would exceed `window_cap` |
| Atomic swaps (HTLC) | `create_swap`, `claim_swap`, `refund_swap`, `get_swap` | Claim needs the SHA-256 preimage before expiry. Refund is possible after expiry |
| Emergency control | `pause`, `unpause` | Blocks new transfers. Claims and refunds stay available so users can reclaim funds |

## Events

| Topic | Payload |
|---|---|
| `BR_OUT` | `(dest_chain, nonce, sender, token, amount, dest_address)` |
| `BR_REL` | `(source_chain, nonce, recipient, token, amount)` |
| `BR_ALERT` | `(source_chain, window_outflow, window_cap)` |
| `BR_SWP_N` | `(swap_id, sender, hashlock, expiry, amount)` |
| `BR_SWP_C` | `(swap_id, preimage)` |
| `BR_SWP_R` | `(swap_id)` |

## Errors

Discriminants are defined in `contracts/bridge/src/lib.rs` (`ContractError`, 1–24).

## Trust model

Releases depend on the relayer set and threshold chosen by the admin. A compromised
threshold of relayers can drain the bridge up to the per-window cap. The window cap
and alert threshold limit the damage and give operators time to pause the bridge.
Relayers attest only after the source-chain confirmations are reached.
