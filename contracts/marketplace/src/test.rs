use super::*;
use soroban_sdk::{
    contracttype, symbol_short, testutils::Address as _, testutils::Ledger, Address, BytesN, Env,
    String,
};

/// Stand-in for the engineer registry: per-address credential status and reputation.
#[contracttype]
#[derive(Clone)]
enum MockKey {
    Status(Address),
    Rep(Address),
}

#[contract]
pub struct MockRegistry;

#[contractimpl]
impl MockRegistry {
    pub fn set_engineer(env: Env, engineer: Address, status: CredentialStatus, reputation: u32) {
        env.storage()
            .instance()
            .set(&MockKey::Status(engineer.clone()), &status);
        env.storage()
            .instance()
            .set(&MockKey::Rep(engineer), &reputation);
    }

    pub fn verify_engineer(
        env: Env,
        engineer: Address,
        _required: Option<Symbol>,
    ) -> CredentialStatus {
        env.storage()
            .instance()
            .get(&MockKey::Status(engineer))
            .unwrap_or(CredentialStatus::NotFound)
    }

    pub fn get_reputation(env: Env, engineer: Address) -> u32 {
        env.storage()
            .instance()
            .get(&MockKey::Rep(engineer))
            .unwrap_or(0)
    }
}

fn pricing() -> PricingConfig {
    PricingConfig {
        base_price: 1_000,
        min_multiplier_bps: 5_000,
        max_multiplier_bps: 20_000,
        max_step_bps: 1_000,
        sensitivity_bps: 10_000,
        min_update_interval_secs: 60,
    }
}

const T0: u64 = 1_000;
const BID_END: u64 = T0 + 3_600;
const REVEAL_END: u64 = BID_END + 3_600;

struct Ctx<'a> {
    env: &'a Env,
    client: MarketplaceClient<'a>,
    registry: MockRegistryClient<'a>,
    admin: Address,
}

fn setup(env: &Env) -> Ctx<'_> {
    env.mock_all_auths();
    env.ledger().with_mut(|li| li.timestamp = T0);
    let admin = Address::generate(env);
    let registry_id = env.register(MockRegistry, ());
    let registry = MockRegistryClient::new(env, &registry_id);
    let id = env.register(Marketplace, ());
    let client = MarketplaceClient::new(env, &id);
    client.initialize(&admin, &registry_id, &pricing());
    Ctx {
        env,
        client,
        registry,
        admin,
    }
}

fn engineer(ctx: &Ctx, status: CredentialStatus, reputation: u32) -> Address {
    let addr = Address::generate(ctx.env);
    ctx.registry.set_engineer(&addr, &status, &reputation);
    addr
}

fn open_params(ctx: &Ctx, criterion: WinnerCriterion) -> AuctionParams {
    let _ = ctx;
    AuctionParams {
        asset_id: 7,
        task_type: symbol_short!("OILCHG"),
        ceiling_price: Some(5_000),
        sealed: false,
        criterion,
        bid_end: BID_END,
        reveal_end: 0,
    }
}

fn sealed_params(criterion: WinnerCriterion) -> AuctionParams {
    AuctionParams {
        asset_id: 7,
        task_type: symbol_short!("OILCHG"),
        ceiling_price: Some(5_000),
        sealed: true,
        criterion,
        bid_end: BID_END,
        reveal_end: REVEAL_END,
    }
}

fn at(env: &Env, ts: u64) {
    env.ledger().with_mut(|li| li.timestamp = ts);
}

fn contract_err(code: ContractError) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(code as u32)
}

// ---------- Dynamic pricing (#1738) ----------

#[test]
fn price_is_base_before_any_market_state() {
    let env = Env::default();
    let ctx = setup(&env);
    assert_eq!(ctx.client.get_price(), 1_000);
    assert_eq!(ctx.client.get_price_history().len(), 0);
}

#[test]
fn excess_demand_raises_price_by_at_most_one_step() {
    let env = Env::default();
    let ctx = setup(&env);
    // demand = 2x supply targets the 2.0x cap, but one step is only +10%.
    let price = ctx.client.record_market_state(&ctx.admin, &10, &20);
    assert_eq!(price, 1_100);
    assert_eq!(ctx.client.get_price(), 1_100);
}

#[test]
fn repeated_updates_converge_to_the_surge_cap() {
    let env = Env::default();
    let ctx = setup(&env);
    for i in 0..15u64 {
        at(&env, T0 + i * 61);
        ctx.client.record_market_state(&ctx.admin, &10, &100);
    }
    // 10_000 + 15 * 1_000 would exceed the 20_000 cap, so it must stop there.
    assert_eq!(ctx.client.get_price(), 2_000);
    assert_eq!(
        ctx.client
            .get_price_history()
            .last()
            .unwrap()
            .multiplier_bps,
        20_000
    );
}

#[test]
fn excess_supply_lowers_price_but_not_below_floor() {
    let env = Env::default();
    let ctx = setup(&env);
    for i in 0..10u64 {
        at(&env, T0 + i * 61);
        ctx.client.record_market_state(&ctx.admin, &10, &0);
    }
    assert_eq!(ctx.client.get_price(), 500);
}

#[test]
fn updates_closer_than_interval_are_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.record_market_state(&ctx.admin, &10, &10);
    at(&env, T0 + 30);
    let res = ctx.client.try_record_market_state(&ctx.admin, &10, &20);
    assert_eq!(
        res,
        Err(Ok(contract_err(ContractError::PriceUpdateTooSoon)))
    );
}

#[test]
fn non_admin_cannot_record_market_state() {
    let env = Env::default();
    let ctx = setup(&env);
    let other = Address::generate(&env);
    let res = ctx.client.try_record_market_state(&other, &10, &20);
    assert_eq!(res, Err(Ok(contract_err(ContractError::UnauthorizedAdmin))));
}

#[test]
fn price_history_is_bounded() {
    let env = Env::default();
    let ctx = setup(&env);
    let mut cfg = pricing();
    cfg.min_update_interval_secs = 0;
    ctx.client.update_pricing_config(&ctx.admin, &cfg);
    for _ in 0..(MAX_PRICE_HISTORY + 5) {
        ctx.client.record_market_state(&ctx.admin, &10, &10);
    }
    assert_eq!(ctx.client.get_price_history().len(), MAX_PRICE_HISTORY);
}

#[test]
fn elasticity_reflects_demand_response_to_price() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.record_market_state(&ctx.admin, &10, &10);
    at(&env, T0 + 61);
    // Demand doubles and price rises 10%: elasticity = +100% / +10% = 10x.
    ctx.client.record_market_state(&ctx.admin, &10, &20);
    assert_eq!(ctx.client.get_price_elasticity(), 100_000);
}

#[test]
fn invalid_pricing_config_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let registry = env.register(MockRegistry, ());
    let id = env.register(Marketplace, ());
    let client = MarketplaceClient::new(&env, &id);
    let mut bad = pricing();
    bad.max_step_bps = 0;
    let res = client.try_initialize(&admin, &registry, &bad);
    assert_eq!(
        res,
        Err(Ok(contract_err(ContractError::InvalidPricingConfig)))
    );
}

// ---------- Auctions (#1739) ----------

#[test]
fn lowest_price_wins_open_auction() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    let a = engineer(&ctx, CredentialStatus::Valid, 500);
    let b = engineer(&ctx, CredentialStatus::GracePeriod, 900);
    let c = engineer(&ctx, CredentialStatus::Valid, 100);
    ctx.client.place_bid(&a, &id, &3_000);
    ctx.client.place_bid(&b, &id, &2_000);
    ctx.client.place_bid(&c, &id, &2_500);

    at(&env, BID_END + 1);
    ctx.client.finalize_auction(&id);

    let auction = ctx.client.get_auction(&id);
    assert_eq!(auction.status, AuctionStatus::Awarded);
    assert_eq!(auction.winner, Some(b));
    assert_eq!(auction.winning_price, 2_000);
    assert_eq!(ctx.client.get_bids(&id).len(), 3);
}

#[test]
fn highest_reputation_wins_open_auction_regardless_of_price() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx.client.create_auction(
        &ctx.admin,
        &open_params(&ctx, WinnerCriterion::HighestReputation),
    );
    let cheap = engineer(&ctx, CredentialStatus::Valid, 300);
    let trusted = engineer(&ctx, CredentialStatus::Valid, 800);
    ctx.client.place_bid(&cheap, &id, &1_000);
    ctx.client.place_bid(&trusted, &id, &4_000);

    at(&env, BID_END + 1);
    ctx.client.finalize_auction(&id);
    assert_eq!(ctx.client.get_auction(&id).winner, Some(trusted));
}

#[test]
fn bid_above_ceiling_or_zero_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    let a = engineer(&ctx, CredentialStatus::Valid, 0);
    assert_eq!(
        ctx.client.try_place_bid(&a, &id, &5_001),
        Err(Ok(contract_err(ContractError::InvalidBid)))
    );
    assert_eq!(
        ctx.client.try_place_bid(&a, &id, &0),
        Err(Ok(contract_err(ContractError::InvalidBid)))
    );
}

#[test]
fn revoked_engineer_cannot_bid() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    let revoked = engineer(&ctx, CredentialStatus::Revoked, 0);
    assert_eq!(
        ctx.client.try_place_bid(&revoked, &id, &1_000),
        Err(Ok(contract_err(ContractError::NotCredentialed)))
    );
}

#[test]
fn engineer_can_bid_only_once() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    let a = engineer(&ctx, CredentialStatus::Valid, 0);
    ctx.client.place_bid(&a, &id, &1_000);
    assert_eq!(
        ctx.client.try_place_bid(&a, &id, &900),
        Err(Ok(contract_err(ContractError::AlreadyBid)))
    );
}

#[test]
fn bidding_closes_at_bid_end_and_finalize_waits_for_it() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    let a = engineer(&ctx, CredentialStatus::Valid, 0);

    assert_eq!(
        ctx.client.try_finalize_auction(&id),
        Err(Ok(contract_err(ContractError::AuctionStillOpen)))
    );
    at(&env, BID_END + 1);
    assert_eq!(
        ctx.client.try_place_bid(&a, &id, &1_000),
        Err(Ok(contract_err(ContractError::BiddingClosed)))
    );
}

#[test]
fn auction_with_no_bids_is_unawarded() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    at(&env, BID_END + 1);
    ctx.client.finalize_auction(&id);
    let auction = ctx.client.get_auction(&id);
    assert_eq!(auction.status, AuctionStatus::Unawarded);
    assert_eq!(auction.winner, None);
    assert_eq!(
        ctx.client.try_finalize_auction(&id),
        Err(Ok(contract_err(ContractError::AuctionNotOpen)))
    );
}

#[test]
fn ceiling_defaults_to_current_dynamic_price() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.record_market_state(&ctx.admin, &10, &20); // price -> 1_100
    let mut params = open_params(&ctx, WinnerCriterion::LowestPrice);
    params.ceiling_price = None;
    let id = ctx.client.create_auction(&ctx.admin, &params);
    assert_eq!(ctx.client.get_auction(&id).ceiling_price, 1_100);
}

#[test]
fn invalid_auction_configs_are_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let mut past = open_params(&ctx, WinnerCriterion::LowestPrice);
    past.bid_end = T0 - 1;
    assert_eq!(
        ctx.client.try_create_auction(&ctx.admin, &past),
        Err(Ok(contract_err(ContractError::InvalidAuctionConfig)))
    );
    let mut no_reveal = sealed_params(WinnerCriterion::LowestPrice);
    no_reveal.reveal_end = BID_END;
    assert_eq!(
        ctx.client.try_create_auction(&ctx.admin, &no_reveal),
        Err(Ok(contract_err(ContractError::InvalidAuctionConfig)))
    );
}

#[test]
fn sealed_auction_commit_then_reveal_selects_winner() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &sealed_params(WinnerCriterion::LowestPrice));
    let a = engineer(&ctx, CredentialStatus::Valid, 0);
    let b = engineer(&ctx, CredentialStatus::Valid, 0);
    let salt_a = BytesN::from_array(&env, &[1u8; 32]);
    let salt_b = BytesN::from_array(&env, &[2u8; 32]);

    ctx.client
        .commit_bid(&a, &id, &commitment(&env, id, &a, 2_500, &salt_a));
    ctx.client
        .commit_bid(&b, &id, &commitment(&env, id, &b, 1_800, &salt_b));

    at(&env, BID_END + 1);
    ctx.client.reveal_bid(&a, &id, &2_500, &salt_a);
    ctx.client.reveal_bid(&b, &id, &1_800, &salt_b);

    at(&env, REVEAL_END + 1);
    ctx.client.finalize_auction(&id);
    let auction = ctx.client.get_auction(&id);
    assert_eq!(auction.winner, Some(b));
    assert_eq!(auction.winning_price, 1_800);
}

#[test]
fn sealed_reveal_rejects_wrong_price_and_wrong_window() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &sealed_params(WinnerCriterion::LowestPrice));
    let a = engineer(&ctx, CredentialStatus::Valid, 0);
    let salt = BytesN::from_array(&env, &[9u8; 32]);
    ctx.client
        .commit_bid(&a, &id, &commitment(&env, id, &a, 2_000, &salt));

    // Reveal during the commit window is not allowed.
    assert_eq!(
        ctx.client.try_reveal_bid(&a, &id, &2_000, &salt),
        Err(Ok(contract_err(ContractError::RevealWindowClosed)))
    );

    at(&env, BID_END + 1);
    assert_eq!(
        ctx.client.try_reveal_bid(&a, &id, &1_999, &salt),
        Err(Ok(contract_err(ContractError::RevealMismatch)))
    );
    ctx.client.reveal_bid(&a, &id, &2_000, &salt);
    // A second reveal of the same commitment has nothing left to match.
    assert_eq!(
        ctx.client.try_reveal_bid(&a, &id, &2_000, &salt),
        Err(Ok(contract_err(ContractError::NoCommitment)))
    );
}

#[test]
fn copied_commitment_cannot_be_revealed_by_another_engineer() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &sealed_params(WinnerCriterion::LowestPrice));
    let victim = engineer(&ctx, CredentialStatus::Valid, 0);
    let copier = engineer(&ctx, CredentialStatus::Valid, 0);
    let salt = BytesN::from_array(&env, &[5u8; 32]);
    let victim_commitment = commitment(&env, id, &victim, 2_000, &salt);

    ctx.client.commit_bid(&victim, &id, &victim_commitment);
    ctx.client.commit_bid(&copier, &id, &victim_commitment);

    at(&env, BID_END + 1);
    assert_eq!(
        ctx.client.try_reveal_bid(&copier, &id, &2_000, &salt),
        Err(Ok(contract_err(ContractError::RevealMismatch)))
    );
}

#[test]
fn mode_mismatches_are_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let open = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    let sealed = ctx
        .client
        .create_auction(&ctx.admin, &sealed_params(WinnerCriterion::LowestPrice));
    let a = engineer(&ctx, CredentialStatus::Valid, 0);
    let commit = BytesN::from_array(&env, &[0u8; 32]);
    assert_eq!(
        ctx.client.try_commit_bid(&a, &open, &commit),
        Err(Ok(contract_err(ContractError::AuctionModeMismatch)))
    );
    assert_eq!(
        ctx.client.try_place_bid(&a, &sealed, &1_000),
        Err(Ok(contract_err(ContractError::AuctionModeMismatch)))
    );
}

#[test]
fn awarded_auction_can_be_disputed_and_admin_can_resolve() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    let cheap = engineer(&ctx, CredentialStatus::Valid, 0);
    let other = engineer(&ctx, CredentialStatus::Valid, 0);
    let outsider = engineer(&ctx, CredentialStatus::Valid, 0);
    ctx.client.place_bid(&cheap, &id, &1_000);
    ctx.client.place_bid(&other, &id, &1_500);

    at(&env, BID_END + 1);
    ctx.client.finalize_auction(&id);

    let reason = String::from_str(&env, "bid was mis-scored");
    assert_eq!(
        ctx.client.try_open_dispute(&outsider, &id, &reason),
        Err(Ok(contract_err(ContractError::NotABidder)))
    );
    ctx.client.open_dispute(&other, &id, &reason);
    assert_eq!(ctx.client.get_auction(&id).status, AuctionStatus::Disputed);

    ctx.client.resolve_dispute(&ctx.admin, &id, &other);
    let auction = ctx.client.get_auction(&id);
    assert_eq!(auction.status, AuctionStatus::Resolved);
    assert_eq!(auction.winner, Some(other));
    assert_eq!(auction.winning_price, 1_500);
    assert_eq!(
        ctx.client.try_resolve_dispute(&ctx.admin, &id, &cheap),
        Err(Ok(contract_err(ContractError::NotDisputed)))
    );
}

#[test]
fn dispute_window_closes() {
    let env = Env::default();
    let ctx = setup(&env);
    let id = ctx
        .client
        .create_auction(&ctx.admin, &open_params(&ctx, WinnerCriterion::LowestPrice));
    let a = engineer(&ctx, CredentialStatus::Valid, 0);
    ctx.client.place_bid(&a, &id, &1_000);
    at(&env, BID_END + 1);
    ctx.client.finalize_auction(&id);
    at(&env, BID_END + 1 + DISPUTE_WINDOW_SECS + 1);
    assert_eq!(
        ctx.client
            .try_open_dispute(&a, &id, &String::from_str(&env, "late")),
        Err(Ok(contract_err(ContractError::DisputeWindowClosed)))
    );
}

#[test]
fn only_admin_can_create_auctions() {
    let env = Env::default();
    let ctx = setup(&env);
    let other = Address::generate(&env);
    assert_eq!(
        ctx.client
            .try_create_auction(&other, &open_params(&ctx, WinnerCriterion::LowestPrice)),
        Err(Ok(contract_err(ContractError::UnauthorizedAdmin)))
    );
}
