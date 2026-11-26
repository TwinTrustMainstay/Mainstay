//! Fuzz target for marketplace commit and reveal.
//!
//! Exercises commitment hashing, bid commits, and bid reveals with
//! arbitrary prices, salt values, and auction IDs to ensure no panics or invariant violations.
//!
//! # Usage
//!
//! ```bash
//! cd fuzz && cargo fuzz run marketplace_commit_reveal_fuzz -- -max_total_time=7200
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;
use soroban_sdk::{
    testutils::Address as _,
    Address, BytesN, Env,
};
use marketplace::auction::commitment;

#[derive(Arbitrary, Debug)]
struct CommitRevealFuzzInput {
    auction_id: u64,
    price: i128,
    salt_bytes: [u8; 32],
    wrong_salt_bytes: [u8; 32],
    tampered_price: i128,
}

fuzz_target!(|input: CommitRevealFuzzInput| {
    let env = Env::default();
    env.mock_all_auths();

    let engineer = Address::generate(&env);
    let salt = BytesN::from_array(&env, &input.salt_bytes);
    let wrong_salt = BytesN::from_array(&env, &input.wrong_salt_bytes);

    // Compute original commitment
    let c = commitment(&env, input.auction_id, &engineer, input.price, &salt);

    // Verifying same parameters reconstructs the exact same commitment
    let c_verify = commitment(&env, input.auction_id, &engineer, input.price, &salt);
    assert_eq!(c, c_verify);

    // If price or salt differs, commitment should not match (collision resistance)
    if input.tampered_price != input.price {
        let c_tampered = commitment(&env, input.auction_id, &engineer, input.tampered_price, &salt);
        assert_ne!(c, c_tampered);
    }
    if input.wrong_salt_bytes != input.salt_bytes {
        let c_diff_salt = commitment(&env, input.auction_id, &engineer, input.price, &wrong_salt);
        assert_ne!(c, c_diff_salt);
    }
});
