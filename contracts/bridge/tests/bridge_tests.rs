//! Tests for the cross-chain bridge contract (#1743, #1744).

use bridge::{Bridge, BridgeClient, ChainConfig, ReleaseParams, SwapStatus};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Bytes, BytesN, Env, Symbol,
};

const ETH: u32 = 1;

struct Ctx<'a> {
    env: Env,
    client: BridgeClient<'a>,
    admin: Address,
    token: Address,
    relayers: [Address; 3],
}

fn eth_config(env: &Env) -> ChainConfig {
    ChainConfig {
        name: Symbol::new(env, "ETH"),
        enabled: true,
        min_amount: 10,
        max_amount: 1_000,
        required_confirmations: 12,
        window_secs: 3_600,
        window_cap: 5_000,
        alert_threshold: 4_000,
    }
}

fn setup<'a>() -> Ctx<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(Bridge, ());
    let client = BridgeClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let relayers = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    client.set_relayers(
        &admin,
        &vec![
            &env,
            relayers[0].clone(),
            relayers[1].clone(),
            relayers[2].clone(),
        ],
        &2,
    );
    client.add_chain(&admin, &ETH, &eth_config(&env));

    Ctx {
        env,
        client,
        admin,
        token,
        relayers,
    }
}

fn mint(ctx: &Ctx, to: &Address, amount: i128) {
    StellarAssetClient::new(&ctx.env, &ctx.token).mint(to, &amount);
}

fn balance(ctx: &Ctx, who: &Address) -> i128 {
    TokenClient::new(&ctx.env, &ctx.token).balance(who)
}

fn attest(ctx: &Ctx, relayer: &Address, nonce: u64, recipient: &Address, amount: i128) {
    ctx.client.attest_release(
        relayer,
        &ETH,
        &nonce,
        &ReleaseParams {
            token: ctx.token.clone(),
            recipient: recipient.clone(),
            amount,
            confirmations: 12,
        },
    );
}

fn release(ctx: &Ctx, nonce: u64, recipient: &Address, amount: i128) {
    attest(ctx, &ctx.relayers[0], nonce, recipient, amount);
    attest(ctx, &ctx.relayers[1], nonce, recipient, amount);
}

fn dest(ctx: &Ctx) -> Bytes {
    Bytes::from_slice(&ctx.env, b"0xdeadbeef")
}

// ── Outbound transfers ───────────────────────────────────────

#[test]
fn test_bridge_out_locks_tokens_and_increments_nonce() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    let n1 = ctx
        .client
        .bridge_out(&user, &ctx.token, &100, &ETH, &dest(&ctx));
    let n2 = ctx
        .client
        .bridge_out(&user, &ctx.token, &50, &ETH, &dest(&ctx));

    assert_eq!(n1, 1);
    assert_eq!(n2, 2);
    assert_eq!(ctx.client.get_outbound_nonce(&ETH), 2);
    assert_eq!(balance(&ctx, &user), 350);
    assert_eq!(balance(&ctx, &ctx.client.address), 150);
}

#[test]
fn test_bridge_out_rejects_disabled_chain() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);
    ctx.client.set_chain_enabled(&ctx.admin, &ETH, &false);

    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &ETH, &dest(&ctx))
        .is_err());
    assert_eq!(balance(&ctx, &user), 500);
}

#[test]
fn test_bridge_out_rejects_amount_out_of_range() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 5_000);

    // Below min (10) and above max (1000)
    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &5, &ETH, &dest(&ctx))
        .is_err());
    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &2_000, &ETH, &dest(&ctx))
        .is_err());
}

#[test]
fn test_bridge_out_rejects_empty_destination() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    let empty = Bytes::new(&ctx.env);
    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &ETH, &empty)
        .is_err());
}

#[test]
fn test_bridge_out_rejects_unknown_chain() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &99, &dest(&ctx))
        .is_err());
}

// ── Inbound releases ─────────────────────────────────────────

#[test]
fn test_release_executes_only_at_threshold() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    attest(&ctx, &ctx.relayers[0], 1, &recipient, 300);
    let pending = ctx.client.get_release(&ETH, &1).unwrap();
    assert_eq!(pending.approvals.len(), 1);
    assert!(!pending.executed);
    assert_eq!(balance(&ctx, &recipient), 0);

    attest(&ctx, &ctx.relayers[1], 1, &recipient, 300);
    let pending = ctx.client.get_release(&ETH, &1).unwrap();
    assert!(pending.executed);
    assert_eq!(balance(&ctx, &recipient), 300);
}

#[test]
fn test_release_cannot_be_replayed() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    release(&ctx, 7, &recipient, 200);
    assert!(ctx
        .client
        .try_attest_release(
            &ctx.relayers[2],
            &ETH,
            &7,
            &ReleaseParams {
                token: ctx.token.clone(),
                recipient: recipient.clone(),
                amount: 200,
                confirmations: 12
            }
        )
        .is_err());
    assert_eq!(balance(&ctx, &recipient), 200);
}

#[test]
fn test_non_relayer_cannot_attest() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    let outsider = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    assert!(ctx
        .client
        .try_attest_release(
            &outsider,
            &ETH,
            &1,
            &ReleaseParams {
                token: ctx.token.clone(),
                recipient: recipient.clone(),
                amount: 100,
                confirmations: 12
            }
        )
        .is_err());
}

#[test]
fn test_duplicate_attestation_rejected() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    attest(&ctx, &ctx.relayers[0], 2, &recipient, 100);
    assert!(ctx
        .client
        .try_attest_release(
            &ctx.relayers[0],
            &ETH,
            &2,
            &ReleaseParams {
                token: ctx.token.clone(),
                recipient: recipient.clone(),
                amount: 100,
                confirmations: 12
            }
        )
        .is_err());
}

#[test]
fn test_mismatched_attestation_rejected() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    let other = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    attest(&ctx, &ctx.relayers[0], 3, &recipient, 100);
    // Second relayer disagrees on the recipient
    assert!(ctx
        .client
        .try_attest_release(
            &ctx.relayers[1],
            &ETH,
            &3,
            &ReleaseParams {
                token: ctx.token.clone(),
                recipient: other.clone(),
                amount: 100,
                confirmations: 12
            }
        )
        .is_err());
    assert_eq!(balance(&ctx, &recipient), 0);
}

#[test]
fn test_insufficient_confirmations_rejected() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    // Chain requires 12 confirmations
    assert!(ctx
        .client
        .try_attest_release(
            &ctx.relayers[0],
            &ETH,
            &4,
            &ReleaseParams {
                token: ctx.token.clone(),
                recipient: recipient.clone(),
                amount: 100,
                confirmations: 11
            }
        )
        .is_err());
}

#[test]
fn test_window_cap_blocks_excess_outflow() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 10_000);

    // Cap is 5000 per window; five releases of 1000 fill it exactly.
    for nonce in 1..=5u64 {
        release(&ctx, nonce, &recipient, 1_000);
    }
    assert_eq!(ctx.client.get_window(&ETH).outflow, 5_000);

    assert!(ctx
        .client
        .try_attest_release(
            &ctx.relayers[0],
            &ETH,
            &6,
            &ReleaseParams {
                token: ctx.token.clone(),
                recipient: recipient.clone(),
                amount: 1_000,
                confirmations: 12
            }
        )
        .is_ok()); // first attestation alone does not execute
    assert!(ctx
        .client
        .try_attest_release(
            &ctx.relayers[1],
            &ETH,
            &6,
            &ReleaseParams {
                token: ctx.token.clone(),
                recipient: recipient.clone(),
                amount: 1_000,
                confirmations: 12
            }
        )
        .is_err());
    assert_eq!(balance(&ctx, &recipient), 5_000);
}

#[test]
fn test_window_resets_after_window_elapses() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 10_000);

    for nonce in 1..=5u64 {
        release(&ctx, nonce, &recipient, 1_000);
    }

    ctx.env
        .ledger()
        .set_timestamp(ctx.env.ledger().timestamp() + 3_600);
    release(&ctx, 6, &recipient, 1_000);
    assert_eq!(balance(&ctx, &recipient), 6_000);
    assert_eq!(ctx.client.get_window(&ETH).outflow, 1_000);
}

// ── Atomic swaps (hashed timelock) ───────────────────────────

fn open_swap(ctx: &Ctx, preimage: &Bytes, expiry: u64) -> (u64, Address, Address) {
    let sender = Address::generate(&ctx.env);
    let recipient = Address::generate(&ctx.env);
    mint(ctx, &sender, 500);
    let hashlock: BytesN<32> = ctx.env.crypto().sha256(preimage).into();
    let id = ctx
        .client
        .create_swap(&sender, &recipient, &ctx.token, &200, &hashlock, &expiry);
    (id, sender, recipient)
}

#[test]
fn test_swap_claim_with_correct_preimage_pays_recipient() {
    let ctx = setup();
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, sender, recipient) = open_swap(&ctx, &preimage, 1_000);

    assert_eq!(balance(&ctx, &sender), 300);
    ctx.client.claim_swap(&id, &preimage);

    assert_eq!(balance(&ctx, &recipient), 200);
    assert_eq!(ctx.client.get_swap(&id).status, SwapStatus::Claimed);
}

#[test]
fn test_swap_claim_with_wrong_preimage_rejected() {
    let ctx = setup();
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, _, recipient) = open_swap(&ctx, &preimage, 1_000);

    let wrong = Bytes::from_slice(&ctx.env, b"guess");
    assert!(ctx.client.try_claim_swap(&id, &wrong).is_err());
    assert_eq!(balance(&ctx, &recipient), 0);
    assert_eq!(ctx.client.get_swap(&id).status, SwapStatus::Open);
}

#[test]
fn test_swap_claim_after_expiry_rejected() {
    let ctx = setup();
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, _, _) = open_swap(&ctx, &preimage, 1_000);

    ctx.env.ledger().set_timestamp(1_000);
    assert!(ctx.client.try_claim_swap(&id, &preimage).is_err());
}

#[test]
fn test_swap_refund_only_after_expiry() {
    let ctx = setup();
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, sender, _) = open_swap(&ctx, &preimage, 1_000);

    assert!(ctx.client.try_refund_swap(&id).is_err());

    ctx.env.ledger().set_timestamp(1_000);
    ctx.client.refund_swap(&id);

    assert_eq!(balance(&ctx, &sender), 500);
    assert_eq!(ctx.client.get_swap(&id).status, SwapStatus::Refunded);
    // A refunded swap can no longer be claimed
    assert!(ctx.client.try_claim_swap(&id, &preimage).is_err());
}

#[test]
fn test_swap_cannot_be_opened_with_past_expiry() {
    let ctx = setup();
    ctx.env.ledger().set_timestamp(500);
    let sender = Address::generate(&ctx.env);
    mint(&ctx, &sender, 500);
    let hashlock = BytesN::from_array(&ctx.env, &[9u8; 32]);
    assert!(ctx
        .client
        .try_create_swap(
            &sender,
            &Address::generate(&ctx.env),
            &ctx.token,
            &100,
            &hashlock,
            &400
        )
        .is_err());
}

#[test]
fn test_unknown_swap_is_not_found() {
    let ctx = setup();
    assert!(ctx.client.try_get_swap(&42).is_err());
}

// ── Admin controls and pause ─────────────────────────────────

#[test]
fn test_pause_blocks_new_transfers_but_allows_refund() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, sender, _) = open_swap(&ctx, &preimage, 1_000);

    ctx.client.pause(&ctx.admin);
    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &ETH, &dest(&ctx))
        .is_err());
    assert!(ctx
        .client
        .try_create_swap(
            &user,
            &Address::generate(&ctx.env),
            &ctx.token,
            &100,
            &BytesN::from_array(&ctx.env, &[1u8; 32]),
            &2_000,
        )
        .is_err());

    ctx.env.ledger().set_timestamp(1_000);
    ctx.client.refund_swap(&id);
    assert_eq!(balance(&ctx, &sender), 500);

    ctx.client.unpause(&ctx.admin);
    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &ETH, &dest(&ctx))
        .is_ok());
}

#[test]
fn test_non_admin_cannot_manage_bridge() {
    let ctx = setup();
    let impostor = Address::generate(&ctx.env);

    assert!(ctx.client.try_pause(&impostor).is_err());
    assert!(ctx
        .client
        .try_set_chain_enabled(&impostor, &ETH, &false)
        .is_err());
    assert!(ctx
        .client
        .try_add_chain(&impostor, &2, &eth_config(&ctx.env))
        .is_err());
    assert!(ctx.client.get_chain(&ETH).enabled);
}

#[test]
fn test_add_chain_rejects_duplicate_and_invalid_config() {
    let ctx = setup();
    assert!(ctx
        .client
        .try_add_chain(&ctx.admin, &ETH, &eth_config(&ctx.env))
        .is_err());

    let mut bad = eth_config(&ctx.env);
    bad.min_amount = 0;
    assert!(ctx.client.try_add_chain(&ctx.admin, &2, &bad).is_err());

    let mut bad_alert = eth_config(&ctx.env);
    bad_alert.alert_threshold = bad_alert.window_cap + 1;
    assert!(ctx
        .client
        .try_add_chain(&ctx.admin, &3, &bad_alert)
        .is_err());
}

#[test]
fn test_set_relayers_validates_threshold_and_duplicates() {
    let ctx = setup();
    let a = Address::generate(&ctx.env);
    assert!(ctx
        .client
        .try_set_relayers(&ctx.admin, &vec![&ctx.env, a.clone()], &0)
        .is_err());
    assert!(ctx
        .client
        .try_set_relayers(&ctx.admin, &vec![&ctx.env, a.clone()], &2)
        .is_err());
    assert!(ctx
        .client
        .try_set_relayers(&ctx.admin, &vec![&ctx.env, a.clone(), a.clone()], &1)
        .is_err());
}
