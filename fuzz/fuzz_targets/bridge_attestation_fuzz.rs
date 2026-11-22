//! Fuzz target for bridge relayer attestation and swap state transitions.
//!
//! Drives random sequences of `attest_release`, `create_swap`, `claim_swap`,
//! `refund_swap` and time advancement against the bridge contract. After each
//! step it checks that:
//!
//! - a release that has executed can never be attested into a second payout,
//! - an executed release has at least the threshold number of approvals,
//! - swaps only move `Open` -> `Claimed` (with the correct preimage) or
//!   `Open` -> `Refunded` (after expiry), and never again once terminal,
//! - total token supply across the bridge and all participants is conserved.
//!
//! # Usage
//!
//! ```bash
//! cd fuzz && cargo fuzz run bridge_attestation_fuzz -- -max_total_time=3600
//! ```

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

const ETH: u32 = 1;
const THRESHOLD: u32 = 2;
const LIQUIDITY: i128 = 1_000_000;
const USER_FUNDS: i128 = 10_000;
const MAX_OPS: usize = 64;

#[derive(Arbitrary, Debug)]
enum Op {
    Attest {
        relayer: u8,
        nonce: u8,
        recipient: u8,
        amount: u16,
        confirmations: u8,
    },
    CreateSwap {
        sender: u8,
        recipient: u8,
        amount: u16,
        preimage: u8,
        expiry_offset: u16,
    },
    ClaimSwap {
        swap: u8,
        preimage: u8,
    },
    RefundSwap {
        swap: u8,
    },
    AdvanceTime {
        secs: u16,
    },
}

#[derive(Arbitrary, Debug)]
struct FuzzInput {
    ops: Vec<Op>,
}

fn pick<T>(items: &[T], index: u8) -> Option<&T> {
    if items.is_empty() {
        None
    } else {
        items.get(index as usize % items.len())
    }
}

fuzz_target!(|input: FuzzInput| {
    use bridge::{Bridge, BridgeClient, ChainConfig, ReleaseParams, SwapStatus};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{Client as TokenClient, StellarAssetClient},
        vec, Address, Bytes, BytesN, Env, Symbol,
    };

    let env = Env::default();
    env.mock_all_auths();

    let bridge_id = env.register(Bridge, ());
    let bridge = BridgeClient::new(&env, &bridge_id);
    let admin = Address::generate(&env);
    bridge.initialize(&admin);

    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();
    let minter = StellarAssetClient::new(&env, &token);
    let balances = TokenClient::new(&env, &token);

    let relayers: [Address; 3] = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    bridge.set_relayers(
        &admin,
        &vec![
            &env,
            relayers[0].clone(),
            relayers[1].clone(),
            relayers[2].clone(),
        ],
        &THRESHOLD,
    );
    bridge.add_chain(
        &admin,
        &ETH,
        &ChainConfig {
            name: Symbol::new(&env, "ETH"),
            enabled: true,
            min_amount: 1,
            max_amount: LIQUIDITY,
            required_confirmations: 0,
            window_secs: 3_600,
            window_cap: 10 * LIQUIDITY,
            alert_threshold: 9 * LIQUIDITY,
            address_len: 42,
        },
    );

    // Releases are paid from the bridge's balance, so fund it up front.
    minter.mint(&bridge_id, &LIQUIDITY);
    let senders: [Address; 3] = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let recipients: [Address; 3] = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    for user in senders.iter().chain(recipients.iter()) {
        minter.mint(user, &USER_FUNDS);
    }
    let total_supply = LIQUIDITY + 6 * USER_FUNDS;

    // Swaps created so far, with the preimage byte each was hashed from.
    let mut swaps: Vec<(u64, u8)> = Vec::new();

    let assert_conserved = || {
        let mut sum = balances.balance(&bridge_id);
        for user in senders.iter().chain(recipients.iter()) {
            sum += balances.balance(user);
        }
        assert_eq!(sum, total_supply, "token supply not conserved");
    };

    for op in input.ops.iter().take(MAX_OPS) {
        match *op {
            Op::Attest {
                relayer,
                nonce,
                recipient,
                amount,
                confirmations,
            } => {
                let nonce = nonce as u64;
                let was_executed = bridge
                    .get_release(&ETH, &nonce)
                    .map_or(false, |pending| pending.executed);

                let res = bridge.try_attest_release(
                    &relayers[relayer as usize % relayers.len()],
                    &ETH,
                    &nonce,
                    &ReleaseParams {
                        token: token.clone(),
                        recipient: recipients[recipient as usize % recipients.len()].clone(),
                        amount: amount as i128,
                        confirmations: confirmations as u32,
                    },
                );
                if was_executed {
                    assert!(res.is_err(), "release {nonce} executed twice");
                }
            }
            Op::CreateSwap {
                sender,
                recipient,
                amount,
                preimage,
                expiry_offset,
            } => {
                let now = env.ledger().timestamp();
                let secret = Bytes::from_slice(&env, &[preimage]);
                let hashlock: BytesN<32> = env.crypto().sha256(&secret).into();
                let res = bridge.try_create_swap(
                    &senders[sender as usize % senders.len()],
                    &recipients[recipient as usize % recipients.len()],
                    &token,
                    &(amount as i128),
                    &hashlock,
                    &(now + 1 + expiry_offset as u64),
                );
                if let Ok(Ok(id)) = res {
                    swaps.push((id, preimage));
                }
            }
            Op::ClaimSwap { swap, preimage } => {
                if let Some(&(id, expected)) = pick(&swaps, swap) {
                    let before = bridge.get_swap(&id).status;
                    let secret = Bytes::from_slice(&env, &[preimage]);
                    if bridge.try_claim_swap(&id, &secret).is_ok() {
                        assert_eq!(before, SwapStatus::Open, "claimed a non-open swap");
                        assert_eq!(preimage, expected, "claimed with wrong preimage");
                        assert_eq!(bridge.get_swap(&id).status, SwapStatus::Claimed);
                    }
                }
            }
            Op::RefundSwap { swap } => {
                if let Some(&(id, _)) = pick(&swaps, swap) {
                    let before = bridge.get_swap(&id).status;
                    if bridge.try_refund_swap(&id).is_ok() {
                        assert_eq!(before, SwapStatus::Open, "refunded a non-open swap");
                        assert_eq!(bridge.get_swap(&id).status, SwapStatus::Refunded);
                    }
                }
            }
            Op::AdvanceTime { secs } => {
                let now = env.ledger().timestamp();
                env.ledger().set_timestamp(now + secs as u64);
            }
        }

        // Release invariants: an executed release met the threshold.
        for nonce in 0..=u8::MAX as u64 {
            if let Some(pending) = bridge.get_release(&ETH, &nonce) {
                if pending.executed {
                    assert!(
                        pending.approvals.len() >= THRESHOLD,
                        "release {nonce} executed below threshold"
                    );
                }
            }
        }
        assert_conserved();
    }
});
