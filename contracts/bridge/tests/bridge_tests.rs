//! Tests for the cross-chain bridge contract (#1743, #1744).

use bridge::{
    Bridge, BridgeClient, ChainConfig, ContractError, ReleaseParams, SwapStatus,
    MAX_DEST_ADDRESS_LEN, MAX_PREIMAGE_LEN, SWAP_SCOPE,
};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    symbol_short,
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Bytes, BytesN, Env, Error, IntoVal, Symbol, Val, Vec,
};

fn is_contract_error<T, C, I>(
    res: Result<Result<T, C>, Result<Error, I>>,
    err: ContractError,
) -> bool {
    matches!(res, Err(Ok(e)) if e == Error::from_contract_error(err as u32))
}

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
        attestation_window_secs: 600,
        address_len: 42,
    }
}

/// A 56-character alphanumeric destination chain with a different address format.
fn xlm_config(env: &Env) -> ChainConfig {
    ChainConfig {
        name: Symbol::new(env, "XLM"),
        enabled: true,
        min_amount: 10,
        max_amount: 1_000,
        required_confirmations: 1,
        window_secs: 3_600,
        window_cap: 5_000,
        alert_threshold: 4_000,
        address_len: 56,
        attestation_window_secs: 600,
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
    // Token allowlists are per scope: bridge-out (ETH) and swaps (SWAP_SCOPE).
    client.set_token_allowed(&admin, &ETH, &token, &true);
    client.set_token_allowed(&admin, &SWAP_SCOPE, &token, &true);

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
    Bytes::from_slice(&ctx.env, b"0x00000000000000000000000000000000deadbeef")
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
fn test_bridge_out_rejects_wrong_length_destination_address() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    // One byte short of the 42-byte ETH address format.
    let short = Bytes::from_slice(&ctx.env, b"0x00000000000000000000000000000000deadbee");
    let res = ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &ETH, &short);
    assert!(is_contract_error(
        res,
        ContractError::InvalidDestinationAddress
    ));
    assert_eq!(balance(&ctx, &user), 500);
}

#[test]
fn test_bridge_out_rejects_non_alphanumeric_destination_address() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    // Right length, but contains a space.
    let bad = Bytes::from_slice(&ctx.env, b"0x0000000000000000000000000000000 deadbeef");
    let res = ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &ETH, &bad);
    assert!(is_contract_error(
        res,
        ContractError::InvalidDestinationAddress
    ));
    assert_eq!(balance(&ctx, &user), 500);
}

#[test]
fn test_bridge_out_validates_address_per_destination_chain() {
    let ctx = setup();
    ctx.client.add_chain(&ctx.admin, &2, &xlm_config(&ctx.env));
    ctx.client.set_token_allowed(&ctx.admin, &2, &ctx.token, &true);
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    // The ETH 42-byte format is invalid for the 56-byte XLM chain.
    let eth_style = Bytes::from_slice(&ctx.env, b"0x00000000000000000000000000000000deadbeef");
    let res = ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &2, &eth_style);
    assert!(is_contract_error(
        res,
        ContractError::InvalidDestinationAddress
    ));

    // A 56-byte alphanumeric address is valid on XLM and rejected on ETH.
    let xlm_style = Bytes::from_slice(
        &ctx.env,
        b"GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN7",
    );
    assert_eq!(
        ctx.client
            .bridge_out(&user, &ctx.token, &100, &2, &xlm_style),
        1
    );
    let res = ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &ETH, &xlm_style);
    assert!(is_contract_error(
        res,
        ContractError::InvalidDestinationAddress
    ));
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
fn test_release_event_lists_approving_relayers() {
    use soroban_sdk::{IntoVal, TryFromVal};

    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);
    release(&ctx, 7, &recipient, 300);

    let br_rel = Symbol::new(&ctx.env, "BR_REL");
    let events = ctx.env.events().all();
    let (_, _, data) = events
        .iter()
        .find(|(_, topics, _)| {
            topics
                .get(0)
                .and_then(|t| Symbol::try_from_val(&ctx.env, &t).ok())
                == Some(br_rel.clone())
        })
        .expect("BR_REL event emitted on execution");

    let payload: (u32, u64, Address, Address, i128, soroban_sdk::Vec<Address>) =
        data.into_val(&ctx.env);
    assert_eq!(payload.0, ETH);
    assert_eq!(payload.1, 7);
    assert_eq!(payload.2, recipient);
    assert_eq!(payload.4, 300);
    assert_eq!(
        payload.5,
        vec![&ctx.env, ctx.relayers[0].clone(), ctx.relayers[1].clone()]
    );
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
fn test_swap_refund_emits_sender_token_and_amount() {
    let ctx = setup();
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, sender, _) = open_swap(&ctx, &preimage, 1_000);

    ctx.env.ledger().set_timestamp(1_000);
    ctx.client.refund_swap(&id);

    // BR_SWP_R is published after the payout transfer, so it is the last event.
    let (contract, topics, data) = ctx.env.events().all().last().unwrap();
    assert_eq!(contract, ctx.client.address);
    assert_eq!(
        topics,
        vec![&ctx.env, symbol_short!("BR_SWP_R").into_val(&ctx.env)]
    );
    let emitted: (u64, Address, Address, i128) = data.into_val(&ctx.env);
    assert_eq!(emitted, (id, sender, ctx.token.clone(), 200_i128));
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
fn test_swap_expiry_capped_by_max_duration() {
    let ctx = setup();
    ctx.env.ledger().set_timestamp(1_000);
    ctx.client.set_max_swap_duration(&ctx.admin, &3_600);
    assert_eq!(ctx.client.get_max_swap_duration(), 3_600);

    let sender = Address::generate(&ctx.env);
    mint(&ctx, &sender, 500);
    let hashlock = BytesN::from_array(&ctx.env, &[9u8; 32]);
    let recipient = Address::generate(&ctx.env);

    // Exactly at the boundary is accepted.
    ctx.client.create_swap(
        &sender,
        &recipient,
        &ctx.token,
        &100,
        &hashlock,
        &(1_000 + 3_600),
    );
    // One second beyond the boundary is rejected.
    assert!(ctx
        .client
        .try_create_swap(
            &sender,
            &recipient,
            &ctx.token,
            &100,
            &hashlock,
            &(1_000 + 3_601)
        )
        .is_err());
}

#[test]
fn test_default_max_swap_duration_is_30_days() {
    let ctx = setup();
    assert_eq!(ctx.client.get_max_swap_duration(), 30 * 24 * 60 * 60);
}

#[test]
fn test_set_max_swap_duration_is_admin_only_and_nonzero() {
    let ctx = setup();
    let impostor = Address::generate(&ctx.env);
    assert!(ctx
        .client
        .try_set_max_swap_duration(&impostor, &3_600)
        .is_err());
    assert!(ctx
        .client
        .try_set_max_swap_duration(&ctx.admin, &0)
        .is_err());
    assert_eq!(ctx.client.get_max_swap_duration(), 30 * 24 * 60 * 60);
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

    let mut zero_len = eth_config(&ctx.env);
    zero_len.address_len = 0;
    assert!(ctx.client.try_add_chain(&ctx.admin, &4, &zero_len).is_err());
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

// ── Issue #1920: approvals from removed relayers must not count ──

#[test]
fn test_removed_relayer_approval_does_not_count_toward_release() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    // relayers[0] attests, then is removed from the set mid-release.
    attest(&ctx, &ctx.relayers[0], 5, &recipient, 300);
    let new_relayer = Address::generate(&ctx.env);
    ctx.client.set_relayers(
        &ctx.admin,
        &vec![
            &ctx.env,
            ctx.relayers[1].clone(),
            ctx.relayers[2].clone(),
            new_relayer.clone(),
        ],
        &2,
    );

    // Only relayers[2] counts now: 1 approval, below threshold 2.
    attest(&ctx, &ctx.relayers[2], 5, &recipient, 300);
    let pending = ctx.client.get_release(&ETH, &5).unwrap();
    assert!(!pending.executed);
    assert_eq!(pending.approvals.len(), 1);
    assert_eq!(balance(&ctx, &recipient), 0);

    // A second current relayer reaches the threshold and the release executes.
    attest(&ctx, &new_relayer, 5, &recipient, 300);
    assert!(ctx.client.get_release(&ETH, &5).unwrap().executed);
    assert_eq!(balance(&ctx, &recipient), 300);
}

// ── Issue #1921: pending releases expire ─────────────────────

fn set_time(ctx: &Ctx, t: u64) {
    ctx.env.ledger().with_mut(|li| li.timestamp = t);
}

#[test]
fn test_attestation_accepted_at_window_deadline() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    set_time(&ctx, 1_000);
    attest(&ctx, &ctx.relayers[0], 7, &recipient, 300);

    // Window is 600s from created_at = 1_000, so 1_600 is still open.
    set_time(&ctx, 1_600);
    attest(&ctx, &ctx.relayers[1], 7, &recipient, 300);
    assert!(ctx.client.get_release(&ETH, &7).unwrap().executed);
    assert_eq!(balance(&ctx, &recipient), 300);
}

#[test]
fn test_attestation_rejected_after_window() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    set_time(&ctx, 1_000);
    attest(&ctx, &ctx.relayers[0], 8, &recipient, 300);

    // One second past the deadline (1_601) the second approval is refused.
    set_time(&ctx, 1_601);
    let res = ctx.client.try_attest_release(
        &ctx.relayers[1],
        &ETH,
        &8,
        &ReleaseParams {
            token: ctx.token.clone(),
            recipient: recipient.clone(),
            amount: 300,
            confirmations: 12,
        },
    );
    assert!(res.is_err());
    let pending = ctx.client.get_release(&ETH, &8).unwrap();
    assert!(!pending.executed);
    assert_eq!(pending.approvals.len(), 1);
    assert_eq!(balance(&ctx, &recipient), 0);
}

#[test]
fn test_expire_release_only_after_window_then_removes_it() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    set_time(&ctx, 1_000);
    attest(&ctx, &ctx.relayers[0], 9, &recipient, 300);

    // Still open at the deadline: cannot expire yet.
    set_time(&ctx, 1_600);
    assert!(ctx.client.try_expire_release(&ETH, &9).is_err());
    assert!(ctx.client.get_release(&ETH, &9).is_some());

    // Past the deadline: cleanup removes the record.
    set_time(&ctx, 1_601);
    ctx.client.expire_release(&ETH, &9);
    assert!(ctx.client.get_release(&ETH, &9).is_none());
}

#[test]
fn test_expire_release_keeps_executed_release_as_replay_guard() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    set_time(&ctx, 1_000);
    release(&ctx, 10, &recipient, 300);

    set_time(&ctx, 10_000);
    assert!(ctx.client.try_expire_release(&ETH, &10).is_err());
    assert!(ctx.client.get_release(&ETH, &10).unwrap().executed);
}

#[test]
fn test_expire_release_unknown_nonce_is_not_found() {
    let ctx = setup();
    assert!(ctx.client.try_expire_release(&ETH, &404).is_err());
}

#[test]
fn test_add_chain_rejects_zero_attestation_window() {
    let ctx = setup();
    let mut cfg = eth_config(&ctx.env);
    cfg.attestation_window_secs = 0;
    assert!(ctx
        .client
        .try_add_chain(&ctx.admin, &4, &cfg)
        .is_err());
}

#[test]
fn test_initialize_requires_admin_authorization() {
    let env = Env::default();
    let contract_id = env.register(Bridge, ());
    let client = BridgeClient::new(&env, &contract_id);

    // No auth is mocked, so the admin never signed the initialize call.
    let admin = Address::generate(&env);
    assert!(client.try_initialize(&admin).is_err());
}

// ── Admin and configuration events ───────────────────────────

/// Topic and payload of the most recent event emitted by the bridge.
fn last_event(ctx: &Ctx) -> (Vec<Val>, Val) {
    let (_, topics, data) = ctx.env.events().all().last().unwrap();
    (topics, data)
}

#[test]
fn test_pause_and_unpause_emit_admin_events() {
    let ctx = setup();

    ctx.client.pause(&ctx.admin);
    let (topics, data) = last_event(&ctx);
    assert_eq!(
        topics,
        vec![&ctx.env, symbol_short!("BR_PAUSE").into_val(&ctx.env)]
    );
    let (who,): (Address,) = data.into_val(&ctx.env);
    assert_eq!(who, ctx.admin);

    ctx.client.unpause(&ctx.admin);
    let (topics, data) = last_event(&ctx);
    assert_eq!(
        topics,
        vec![&ctx.env, symbol_short!("BR_RESUME").into_val(&ctx.env)]
    );
    let (who,): (Address,) = data.into_val(&ctx.env);
    assert_eq!(who, ctx.admin);
}

#[test]
fn test_set_relayers_emits_threshold_and_relayer_set() {
    let ctx = setup();
    let fresh = [Address::generate(&ctx.env), Address::generate(&ctx.env)];
    let new_set = vec![&ctx.env, fresh[0].clone(), fresh[1].clone()];

    ctx.client.set_relayers(&ctx.admin, &new_set, &1);

    let (topics, data) = last_event(&ctx);
    assert_eq!(
        topics,
        vec![&ctx.env, symbol_short!("BR_RELAYS").into_val(&ctx.env)]
    );
    let emitted: (Address, u32, Vec<Address>) = data.into_val(&ctx.env);
    assert_eq!(emitted, (ctx.admin.clone(), 1, new_set));
}

#[test]
fn test_add_update_and_enable_chain_emit_events() {
    let ctx = setup();
    let base_cfg = eth_config(&ctx.env);

    let mut new_cfg = eth_config(&ctx.env);
    new_cfg.name = Symbol::new(&ctx.env, "POLY");
    ctx.client.add_chain(&ctx.admin, &2, &new_cfg);
    let (topics, data) = last_event(&ctx);
    assert_eq!(
        topics,
        vec![&ctx.env, symbol_short!("BR_CH_ADD").into_val(&ctx.env)]
    );
    let emitted: (Address, u32, ChainConfig) = data.into_val(&ctx.env);
    assert_eq!(emitted, (ctx.admin.clone(), 2, new_cfg.clone()));

    let mut updated = base_cfg.clone();
    updated.max_amount = 2_000;
    ctx.client.update_chain(&ctx.admin, &ETH, &updated);
    let (topics, data) = last_event(&ctx);
    assert_eq!(
        topics,
        vec![&ctx.env, symbol_short!("BR_CH_UPD").into_val(&ctx.env)]
    );
    let emitted: (Address, u32, ChainConfig) = data.into_val(&ctx.env);
    assert_eq!(emitted, (ctx.admin.clone(), ETH, updated));

    ctx.client.set_chain_enabled(&ctx.admin, &ETH, &false);
    let (topics, data) = last_event(&ctx);
    assert_eq!(
        topics,
        vec![&ctx.env, symbol_short!("BR_CH_ENB").into_val(&ctx.env)]
    );
    let emitted: (Address, u32, bool) = data.into_val(&ctx.env);
    assert_eq!(emitted, (ctx.admin.clone(), ETH, false));
}

// ── Relayer-observed confirmations ───────────────────────────

fn attest_with_confirmations(
    ctx: &Ctx,
    relayer: &Address,
    nonce: u64,
    recipient: &Address,
    amount: i128,
    confirmations: u32,
) {
    ctx.client.attest_release(
        relayer,
        &ETH,
        &nonce,
        &ReleaseParams {
            token: ctx.token.clone(),
            recipient: recipient.clone(),
            amount,
            confirmations,
        },
    );
}

#[test]
fn test_disagreeing_confirmations_are_stored_per_relayer() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    // The release pays out of the bridge's own balance, so fund it first.
    mint(&ctx, &ctx.client.address, 100);

    attest_with_confirmations(&ctx, &ctx.relayers[0], 7, &recipient, 100, 12);
    // Stays pending until the threshold is reached, so the record is readable.
    let pending = ctx.client.get_release(&ETH, &7).unwrap();
    assert_eq!(pending.confirmations, vec![&ctx.env, 12]);
    assert!(!pending.executed);

    attest_with_confirmations(&ctx, &ctx.relayers[1], 7, &recipient, 100, 30);

    let pending = ctx.client.get_release(&ETH, &7).unwrap();
    assert_eq!(pending.approvals.len(), 2);
    assert_eq!(pending.confirmations, vec![&ctx.env, 12, 30]);
    assert!(pending.executed);
    assert_eq!(balance(&ctx, &recipient), 100);
}

#[test]
fn test_attestation_below_requirement_does_not_count_toward_threshold() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 100);

    // Relayer 0 reports 12, which meets the requirement. Relayer 1 reports 11,
    // which is below it, so its attestation is refused outright.
    attest_with_confirmations(&ctx, &ctx.relayers[0], 8, &recipient, 100, 12);
    assert!(ctx
        .client
        .try_attest_release(
            &ctx.relayers[1],
            &ETH,
            &8,
            &ReleaseParams {
                token: ctx.token.clone(),
                recipient: recipient.clone(),
                amount: 100,
                confirmations: 11,
            },
        )
        .is_err());

    let pending = ctx.client.get_release(&ETH, &8).unwrap();
    assert_eq!(pending.approvals.len(), 1);
    assert!(!pending.executed);
    assert_eq!(balance(&ctx, &recipient), 0);
}

// ── Allowlist, swap TTL and execution tests (PR #2062) ─────

const DAY_SECS: u64 = 86_400;
const DAY_LEDGERS: u32 = 17_280;

fn advance_days(ctx: &Ctx, days: u32) {
    ctx.env.ledger().with_mut(|li| {
        li.sequence_number += days * DAY_LEDGERS;
        li.timestamp += days as u64 * DAY_SECS;
    });
}

fn swap_hashlock(ctx: &Ctx, preimage: &Bytes) -> BytesN<32> {
    ctx.env.crypto().sha256(preimage).into()
}

#[test]
fn test_allowlist_is_scoped_per_chain() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);
    ctx.client.add_chain(&ctx.admin, &2, &eth_config(&ctx.env));

    // Allowed for ETH (1) but not for chain 2.
    assert!(!ctx.client.is_token_allowed(&2, &ctx.token));
    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &2, &dest(&ctx))
        .is_err());
}

#[test]
fn test_bridge_allowlist_does_not_authorize_swaps() {
    let ctx = setup();
    // Revoke the swap-scope entry from setup, leaving only the ETH bridge-out entry.
    ctx.client.set_token_allowed(&ctx.admin, &SWAP_SCOPE, &ctx.token, &false);
    // ctx.token is allowed for bridge_out to ETH, but not for swaps.
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let sender = Address::generate(&ctx.env);
    mint(&ctx, &sender, 500);
    assert!(ctx
        .client
        .try_create_swap(
            &sender,
            &Address::generate(&ctx.env),
            &ctx.token,
            &200,
            &swap_hashlock(&ctx, &preimage),
            &1_000,
        )
        .is_err());
    assert_eq!(balance(&ctx, &sender), 500);
}

#[test]
fn test_bridge_out_accepts_listed_token_and_respects_revocation() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    assert!(ctx.client.is_token_allowed(&ETH, &ctx.token));
    assert_eq!(
        ctx.client
            .bridge_out(&user, &ctx.token, &100, &ETH, &dest(&ctx)),
        1
    );

    ctx.client
        .set_token_allowed(&ctx.admin, &ETH, &ctx.token, &false);
    assert!(ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &ETH, &dest(&ctx))
        .is_err());
}

#[test]
fn test_bridge_out_rejects_unlisted_token() {
    let ctx = setup();
    let user = Address::generate(&ctx.env);
    let other_token = ctx
        .env
        .register_stellar_asset_contract_v2(Address::generate(&ctx.env))
        .address();
    StellarAssetClient::new(&ctx.env, &other_token).mint(&user, &500);

    assert!(!ctx.client.is_token_allowed(&ETH, &other_token));
    assert!(ctx
        .client
        .try_bridge_out(&user, &other_token, &100, &ETH, &dest(&ctx))
        .is_err());
    assert_eq!(TokenClient::new(&ctx.env, &other_token).balance(&user), 500);
    assert_eq!(ctx.client.get_outbound_nonce(&ETH), 0);
}

#[test]
fn test_bump_swap_keeps_open_swap_alive_until_expiry() {
    let ctx = setup();
    // These tests hold swaps open for 60 days; raise the default 30-day cap.
    ctx.client.set_max_swap_duration(&ctx.admin, &(90 * DAY_SECS));
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, _, _) = open_swap(&ctx, &preimage, 60 * DAY_SECS);

    advance_days(&ctx, 30);
    // Permissionless: a stranger with no auth can bump the swap.
    ctx.client.bump_swap(&id);

    advance_days(&ctx, 25);
    assert_eq!(ctx.client.get_swap(&id).status, SwapStatus::Open);
}

#[test]
fn test_bump_swap_rejects_unknown_and_closed_swaps() {
    let ctx = setup();
    assert!(ctx.client.try_bump_swap(&42).is_err());

    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, _, _) = open_swap(&ctx, &preimage, 1_000);
    ctx.client.claim_swap(&id, &preimage);
    assert!(ctx.client.try_bump_swap(&id).is_err());
}

#[test]
fn test_create_swap_accepts_listed_token() {
    let ctx = setup();
    ctx.client
        .set_token_allowed(&ctx.admin, &SWAP_SCOPE, &ctx.token, &true);
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    let (id, sender, recipient) = open_swap(&ctx, &preimage, 1_000);

    ctx.client.claim_swap(&id, &preimage);
    assert_eq!(balance(&ctx, &recipient), 200);
    assert_eq!(balance(&ctx, &sender), 300);
}

#[test]
fn test_create_swap_rejects_unlisted_token() {
    let ctx = setup();
    let sender = Address::generate(&ctx.env);
    let other_token = ctx
        .env
        .register_stellar_asset_contract_v2(Address::generate(&ctx.env))
        .address();
    StellarAssetClient::new(&ctx.env, &other_token).mint(&sender, &500);
    let hashlock = swap_hashlock(&ctx, &Bytes::from_slice(&ctx.env, b"secret"));

    assert!(ctx
        .client
        .try_create_swap(
            &sender,
            &Address::generate(&ctx.env),
            &other_token,
            &200,
            &hashlock,
            &1_000,
        )
        .is_err());
    assert_eq!(
        TokenClient::new(&ctx.env, &other_token).balance(&sender),
        500
    );
}

#[test]
fn test_execute_release_cannot_replay_executed_release() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    release(&ctx, 5, &recipient, 200);
    assert_eq!(balance(&ctx, &recipient), 200);
    assert!(ctx.client.try_execute_release(&ETH, &5).is_err());
    assert_eq!(balance(&ctx, &recipient), 200);
}

#[test]
fn test_execute_release_rejects_below_threshold() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    attest(&ctx, &ctx.relayers[0], 4, &recipient, 250);
    assert!(ctx.client.try_execute_release(&ETH, &4).is_err());
    assert_eq!(balance(&ctx, &recipient), 0);
    assert!(!ctx.client.get_release(&ETH, &4).unwrap().executed);
}

#[test]
fn test_execute_release_unknown_release_rejected() {
    let ctx = setup();
    assert!(ctx.client.try_execute_release(&ETH, &99).is_err());
}

#[test]
fn test_lowered_threshold_lets_anyone_execute_qualifying_release() {
    let ctx = setup();
    let recipient = Address::generate(&ctx.env);
    mint(&ctx, &ctx.client.address, 1_000);

    // One approval is below the threshold of 2, so nothing is paid yet.
    attest(&ctx, &ctx.relayers[0], 3, &recipient, 250);
    assert_eq!(balance(&ctx, &recipient), 0);

    // Admin lowers the threshold to 1. The pending release now qualifies.
    ctx.client.set_relayers(
        &ctx.admin,
        &vec![
            &ctx.env,
            ctx.relayers[0].clone(),
            ctx.relayers[1].clone(),
            ctx.relayers[2].clone(),
        ],
        &1,
    );
    assert_eq!(balance(&ctx, &recipient), 0);

    // Any account can execute it without another relayer attestation.
    ctx.client.execute_release(&ETH, &3);

    assert_eq!(balance(&ctx, &recipient), 250);
    assert!(ctx.client.get_release(&ETH, &3).unwrap().executed);
}

#[test]
fn test_only_admin_can_set_token_allowlist() {
    let ctx = setup();
    let stranger = Address::generate(&ctx.env);
    let other_token = Address::generate(&ctx.env);
    assert!(ctx
        .client
        .try_set_token_allowed(&stranger, &ETH, &other_token, &true)
        .is_err());
    assert!(!ctx.client.is_token_allowed(&ETH, &other_token));
}

#[test]
fn test_open_swap_record_survives_idle_past_old_ttl() {
    let ctx = setup();
    // These tests hold swaps open for 60 days; raise the default 30-day cap.
    ctx.client.set_max_swap_duration(&ctx.admin, &(90 * DAY_SECS));
    let preimage = Bytes::from_slice(&ctx.env, b"secret");
    // Expiry is 60 days out. The old fixed 30-day TTL would archive the record
    // at day 30, long before expiry.
    let (id, _, _) = open_swap(&ctx, &preimage, 60 * DAY_SECS);

    advance_days(&ctx, 45);
    let swap = ctx.client.get_swap(&id);
    assert_eq!(swap.status, SwapStatus::Open);
    assert_eq!(swap.expiry, 60 * DAY_SECS);
}

#[test]
fn test_swap_scope_cannot_be_registered_as_chain() {
    let ctx = setup();
    assert!(ctx
        .client
        .try_add_chain(&ctx.admin, &SWAP_SCOPE, &eth_config(&ctx.env))
        .is_err());
}

#[test]
fn test_chain_config_rejects_address_len_above_max() {
    let ctx = setup();
    let mut cfg = eth_config(&ctx.env);
    cfg.address_len = MAX_DEST_ADDRESS_LEN + 1;
    let res = ctx.client.try_add_chain(&ctx.admin, &2, &cfg);
    assert!(is_contract_error(res, ContractError::InvalidConfig));
}

#[test]
fn test_bridge_out_dest_address_max_length_boundary() {
    let ctx = setup();
    let mut cfg = eth_config(&ctx.env);
    cfg.address_len = MAX_DEST_ADDRESS_LEN;
    ctx.client.add_chain(&ctx.admin, &2, &cfg);
    ctx.client.set_token_allowed(&ctx.admin, &2, &ctx.token, &true);

    let user = Address::generate(&ctx.env);
    mint(&ctx, &user, 500);

    let at_max = Bytes::from_slice(&ctx.env, &[b'a'; MAX_DEST_ADDRESS_LEN as usize]);
    assert_eq!(
        ctx.client
            .bridge_out(&user, &ctx.token, &100, &2, &at_max),
        1
    );

    let over_max = Bytes::from_slice(&ctx.env, &[b'a'; (MAX_DEST_ADDRESS_LEN + 1) as usize]);
    let res = ctx
        .client
        .try_bridge_out(&user, &ctx.token, &100, &2, &over_max);
    assert!(is_contract_error(res, ContractError::InputTooLong));
    assert_eq!(balance(&ctx, &user), 400);
}

#[test]
fn test_claim_swap_preimage_max_length_boundary() {
    let ctx = setup();

    let at_max = Bytes::from_slice(&ctx.env, &[b's'; MAX_PREIMAGE_LEN as usize]);
    let (id_ok, _, recipient) = open_swap(&ctx, &at_max, 1_000);
    ctx.client.claim_swap(&id_ok, &at_max);
    assert_eq!(ctx.client.get_swap(&id_ok).status, SwapStatus::Claimed);
    assert_eq!(balance(&ctx, &recipient), 100);

    let over_max = Bytes::from_slice(&ctx.env, &[b's'; (MAX_PREIMAGE_LEN + 1) as usize]);
    let (id_over, _, _) = open_swap(&ctx, &over_max, 1_000);
    let res = ctx.client.try_claim_swap(&id_over, &over_max);
    assert!(is_contract_error(res, ContractError::InputTooLong));
    assert_eq!(ctx.client.get_swap(&id_over).status, SwapStatus::Open);
}
