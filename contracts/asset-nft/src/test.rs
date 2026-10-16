#![cfg(test)]

use crate::{AssetNft, AssetNftClient, TradeKind};
use soroban_sdk::{
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, String,
};

struct Ctx<'a> {
    client: AssetNftClient<'a>,
    payments: TokenClient<'a>,
    payment_minter: StellarAssetClient<'a>,
    admin: Address,
}

fn setup<'a>(env: &'a Env) -> Ctx<'a> {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let payment_id = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(AssetNft, ());
    let client = AssetNftClient::new(env, &contract_id);
    client.initialize(&admin, &payment_id);

    Ctx {
        client,
        payments: TokenClient::new(env, &payment_id),
        payment_minter: StellarAssetClient::new(env, &payment_id),
        admin,
    }
}

fn uri(env: &Env) -> String {
    String::from_str(env, "ipfs://asset/1")
}

#[test]
fn mint_creates_one_nft_per_asset() {
    let env = Env::default();
    let ctx = setup(&env);
    let owner = Address::generate(&env);

    let token_id = ctx.client.mint(&ctx.admin, &42, &owner, &uri(&env));
    assert_eq!(token_id, 1);
    assert_eq!(ctx.client.get_asset_owner(&42), Some(owner.clone()));
    assert_eq!(ctx.client.get_token_for_asset(&42), Some(1));

    let token = ctx.client.get_token(&token_id);
    assert_eq!(token.asset_id, 42);
    assert_eq!(token.owner, owner);
    assert_eq!(
        ctx.client.get_history(&token_id).get(0).unwrap().kind,
        TradeKind::Mint
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn asset_cannot_be_tokenized_twice() {
    let env = Env::default();
    let ctx = setup(&env);
    let owner = Address::generate(&env);
    ctx.client.mint(&ctx.admin, &42, &owner, &uri(&env));
    ctx.client.mint(&ctx.admin, &42, &owner, &uri(&env));
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn non_admin_cannot_mint() {
    let env = Env::default();
    let ctx = setup(&env);
    let owner = Address::generate(&env);
    let intruder = Address::generate(&env);
    ctx.client.mint(&intruder, &42, &owner, &uri(&env));
}

#[test]
fn transfer_moves_nft_and_asset_ownership() {
    let env = Env::default();
    let ctx = setup(&env);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);
    let token_id = ctx.client.mint(&ctx.admin, &7, &alice, &uri(&env));

    ctx.client.transfer(&alice, &bob, &token_id);

    assert_eq!(ctx.client.get_asset_owner(&7), Some(bob.clone()));
    assert_eq!(ctx.client.get_token(&token_id).owner, bob);
    let history = ctx.client.get_history(&token_id);
    assert_eq!(history.len(), 2);
    assert_eq!(history.get(1).unwrap().kind, TradeKind::Transfer);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn transfer_by_non_owner_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let alice = Address::generate(&env);
    let mallory = Address::generate(&env);
    let token_id = ctx.client.mint(&ctx.admin, &7, &alice, &uri(&env));
    ctx.client.transfer(&mallory, &mallory, &token_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn listed_nft_cannot_be_transferred() {
    let env = Env::default();
    let ctx = setup(&env);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);
    let token_id = ctx.client.mint(&ctx.admin, &7, &alice, &uri(&env));
    ctx.client.list_for_sale(&alice, &token_id, &500);
    ctx.client.transfer(&alice, &bob, &token_id);
}

#[test]
fn metadata_sync_updates_fields() {
    let env = Env::default();
    let ctx = setup(&env);
    let alice = Address::generate(&env);
    let token_id = ctx.client.mint(&ctx.admin, &7, &alice, &uri(&env));

    let new_uri = String::from_str(&env, "ipfs://asset/1/v2");
    ctx.client
        .sync_metadata(&ctx.admin, &token_id, &new_uri, &85, &12);

    let token = ctx.client.get_token(&token_id);
    assert_eq!(token.uri, new_uri);
    assert_eq!(token.collateral_score, 85);
    assert_eq!(token.maintenance_count, 12);
}

#[test]
fn sale_pays_seller_and_transfers_nft() {
    let env = Env::default();
    let ctx = setup(&env);
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);
    ctx.payment_minter.mint(&buyer, &1_000);
    let token_id = ctx.client.mint(&ctx.admin, &7, &seller, &uri(&env));

    ctx.client.list_for_sale(&seller, &token_id, &600);
    assert!(ctx.client.get_listing(&token_id).is_some());
    ctx.client.buy(&buyer, &token_id);

    assert_eq!(ctx.payments.balance(&buyer), 400);
    assert_eq!(ctx.payments.balance(&seller), 600);
    assert_eq!(ctx.client.get_asset_owner(&7), Some(buyer.clone()));
    assert!(ctx.client.get_listing(&token_id).is_none());

    let history = ctx.client.get_history(&token_id);
    let last = history.get(history.len() - 1).unwrap();
    assert_eq!(last.kind, TradeKind::Sale);
    assert_eq!(last.price, 600);
    assert_eq!(last.from, Some(seller));
    assert_eq!(last.to, buyer);
}

#[test]
fn cancelled_listing_allows_transfer() {
    let env = Env::default();
    let ctx = setup(&env);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);
    let token_id = ctx.client.mint(&ctx.admin, &7, &alice, &uri(&env));

    ctx.client.list_for_sale(&alice, &token_id, &500);
    ctx.client.cancel_listing(&alice, &token_id);
    ctx.client.transfer(&alice, &bob, &token_id);
    assert_eq!(ctx.client.get_asset_owner(&7), Some(bob));
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn owner_cannot_buy_own_listing() {
    let env = Env::default();
    let ctx = setup(&env);
    let alice = Address::generate(&env);
    ctx.payment_minter.mint(&alice, &1_000);
    let token_id = ctx.client.mint(&ctx.admin, &7, &alice, &uri(&env));
    ctx.client.list_for_sale(&alice, &token_id, &100);
    ctx.client.buy(&alice, &token_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn buying_unlisted_nft_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let alice = Address::generate(&env);
    let buyer = Address::generate(&env);
    let token_id = ctx.client.mint(&ctx.admin, &7, &alice, &uri(&env));
    ctx.client.buy(&buyer, &token_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn zero_price_listing_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let alice = Address::generate(&env);
    let token_id = ctx.client.mint(&ctx.admin, &7, &alice, &uri(&env));
    ctx.client.list_for_sale(&alice, &token_id, &0);
}
