#![no_std]

use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Bytes, BytesN, Env, String, Symbol, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ContractError {
    NoMaintenanceHistory = 1,
    UnauthorizedEngineer = 2,
    UnauthorizedAdmin = 3,
    /// A prune was executed (or attempted) before the 48h timelock elapsed.
    PruneTimelockNotElapsed = 4,
    /// execute_prune_asset_history called with no matching proposal on file.
    NoPrunePending = 5,
}

/// Maintenance record with a tamper-evident hash chain.
///
/// `record_hash` is computed as:
///   sha256(XDR(asset_id) || XDR(task_type) || XDR(engineer) || XDR(timestamp) || XDR(nonce) || XDR(prev_hash))
///
/// `notes` is deliberately EXCLUDED from the hash input because it is a
/// free-form, user-controlled string. Including user-controlled bytes in a
/// hash that is meant to prove chain integrity would let a submitter craft
/// `notes` to try to influence/predict the resulting hash. `nonce` (the
/// record's position in the asset's history) and `prev_hash` (the hash of
/// the prior record, or 32 zero bytes for the first record) are included
/// instead, so every record's hash is bound to its position in the chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceRecord {
    pub asset_id: u64,
    pub task_type: Symbol,
    pub notes: String,
    pub engineer: Address,
    pub timestamp: u64,
    /// True if the engineer's credential was in its post-expiry grace
    /// period at the time this record was signed.
    pub signed_during_grace_period: bool,
    /// sha256 hash covering this record's chain-relevant fields (see above).
    pub record_hash: BytesN<32>,
    /// record_hash of the previous record in this asset's history.
    pub prev_hash: BytesN<32>,
}

/// A pending admin proposal to permanently delete an asset's maintenance
/// history. Must sit for PRUNE_TIMELOCK_SECS before it can be executed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PruneProposal {
    pub asset_id: u64,
    pub proposed_at: u64,
}

/// A point-in-time snapshot of the collateral score, recorded at each maintenance event.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoreEntry {
    pub timestamp: u64,
    pub score: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchRecord {
    pub task_type: Symbol,
    pub notes: String,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub admin: Address,
    pub max_history: u32,
    pub score_increment: u32,
}

const ASSET_REGISTRY: Symbol = symbol_short!("REGISTRY");
const ENG_REGISTRY: Symbol = symbol_short!("ENG_REG");
const CONFIG: Symbol = symbol_short!("CONFIG");
const DEFAULT_MAX_HISTORY: u32 = 200;
const DEFAULT_SCORE_INCREMENT: u32 = 5;
const DECAY_INTERVAL: u64 = 2592000; // 30 days in seconds
const DECAY_RATE: u32 = 5;
/// Timelock enforced between proposing and executing a history prune: 48 hours.
const PRUNE_TIMELOCK_SECS: u64 = 172800;
/// Records signed during an engineer's credential grace period score at half weight.
const GRACE_PENALTY_NUM: u32 = 1;
const GRACE_PENALTY_DEN: u32 = 2;

fn history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("HIST"), asset_id)
}

fn prune_proposal_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("PRUNE"), asset_id)
}

fn grace_period_key(engineer: &Address) -> (Symbol, Address) {
    (symbol_short!("GRACEEND"), engineer.clone())
}

fn chain_head_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("CHAINHD"), asset_id)
}

fn zero_hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[0u8; 32])
}

/// Computes a MaintenanceRecord's chain hash. See the doc comment on
/// `MaintenanceRecord` for exactly which fields are covered.
fn compute_record_hash(
    env: &Env,
    asset_id: u64,
    task_type: &Symbol,
    engineer: &Address,
    timestamp: u64,
    nonce: u32,
    prev_hash: &BytesN<32>,
) -> BytesN<32> {
    let mut bytes = Bytes::new(env);
    bytes.append(&Bytes::from(asset_id.to_xdr(env)));
    bytes.append(&Bytes::from(task_type.to_xdr(env)));
    bytes.append(&Bytes::from(engineer.to_xdr(env)));
    bytes.append(&Bytes::from(timestamp.to_xdr(env)));
    bytes.append(&Bytes::from(nonce.to_xdr(env)));
    bytes.append(&Bytes::from(prev_hash.to_xdr(env)));
    env.crypto().sha256(&bytes).into()
}

fn score_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("SCORE"), asset_id)
}

fn score_history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("SCHIST"), asset_id)
}

fn last_update_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("LUPD"), asset_id)
}

// Task type weight mapping for collateral scoring
fn get_task_weight(_env: &Env, task_type: &Symbol) -> u32 {
    // Minor tasks: 2 points
    if task_type == &symbol_short!("OIL_CHG")
        || task_type == &symbol_short!("LUBE")
        || task_type == &symbol_short!("INSPECT")
    {
        return 2;
    }
    // Medium tasks: 5 points
    if task_type == &symbol_short!("FILTER")
        || task_type == &symbol_short!("TUNE_UP")
        || task_type == &symbol_short!("BRAKE")
    {
        return 5;
    }
    // Major tasks: 10 points
    if task_type == &symbol_short!("ENGINE")
        || task_type == &symbol_short!("OVERHAUL")
        || task_type == &symbol_short!("REBUILD")
    {
        return 10;
    }
    // Default for unknown task types: 3 points
    3
}

// Minimal client interface for cross-contract call to EngineerRegistry
mod engineer_registry {
    use soroban_sdk::{contractclient, Address, Env};

    #[allow(dead_code)]
    #[contractclient(name = "EngineerRegistryClient")]
    pub trait EngineerRegistry {
        fn verify_engineer(env: Env, engineer: Address) -> bool;
    }
}

#[contract]
pub struct Lifecycle;

#[contractimpl]
impl Lifecycle {
    /// Must be called once after deployment to bind dependent registries.
    /// Pass `0` for `max_history` to use the default of 200 records per asset.
    pub fn initialize(
        env: Env,
        asset_registry: Address,
        engineer_registry: Address,
        admin: Address,
        max_history: u32,
    ) {
        env.storage()
            .instance()
            .set(&ASSET_REGISTRY, &asset_registry);
        env.storage()
            .instance()
            .set(&ENG_REGISTRY, &engineer_registry);

        let config = Config {
            admin,
            max_history: if max_history == 0 {
                DEFAULT_MAX_HISTORY
            } else {
                max_history
            },
            score_increment: DEFAULT_SCORE_INCREMENT,
        };
        env.storage().instance().set(&CONFIG, &config);
    }

    pub fn update_score_increment(env: Env, admin: Address, score_increment: u32) {
        admin.require_auth();

        let mut config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .expect("config not set");
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        config.score_increment = score_increment;
        env.storage().instance().set(&CONFIG, &config);
    }

    pub fn submit_maintenance(
        env: Env,
        asset_id: u64,
        task_type: Symbol,
        notes: String,
        engineer: Address,
    ) {
        engineer.require_auth();

        // Verify asset exists
        let asset_registry: Address = env
            .storage()
            .instance()
            .get(&ASSET_REGISTRY)
            .expect("asset registry not set");
        let asset_registry_client =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        asset_registry_client.get_asset(&asset_id);

        // Cross-check engineer credential
        let registry_id: Address = env
            .storage()
            .instance()
            .get(&ENG_REGISTRY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::UnauthorizedEngineer));
        let registry = engineer_registry::EngineerRegistryClient::new(&env, &registry_id);
        if !registry.verify_engineer(&engineer) {
            panic_with_error!(&env, ContractError::UnauthorizedEngineer);
        }

        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .expect("config not set");

        let mut history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        if history.len() >= config.max_history {
            panic!("history cap reached");
        }

        let timestamp = env.ledger().timestamp();

        // A credential in its post-expiry grace period still lets an engineer
        // submit, but the record is flagged so lenders can weigh it differently.
        let grace_end: u64 = env
            .storage()
            .persistent()
            .get(&grace_period_key(&engineer))
            .unwrap_or(0);
        let signed_during_grace_period = grace_end > 0 && timestamp <= grace_end;

        let nonce = history.len();
        let prev_hash = env
            .storage()
            .persistent()
            .get(&chain_head_key(asset_id))
            .unwrap_or_else(|| zero_hash(&env));
        let record_hash =
            compute_record_hash(&env, asset_id, &task_type, &engineer, timestamp, nonce, &prev_hash);

        let record = MaintenanceRecord {
            asset_id,
            task_type: task_type.clone(),
            notes,
            engineer: engineer.clone(),
            timestamp,
            signed_during_grace_period,
            record_hash: record_hash.clone(),
            prev_hash,
        };

        history.push_back(record);
        env.storage()
            .persistent()
            .set(&history_key(asset_id), &history);
        env.storage()
            .persistent()
            .set(&chain_head_key(asset_id), &record_hash);

        // Update collateral score
        let score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);
        let mut weight = get_task_weight(&env, &task_type);
        if signed_during_grace_period {
            weight = weight * GRACE_PENALTY_NUM / GRACE_PENALTY_DEN;
        }
        let new_score = (score + weight).min(100);
        env.storage()
            .persistent()
            .set(&score_key(asset_id), &new_score);

        // Append (timestamp, score) snapshot to score history
        let mut score_history: Vec<ScoreEntry> = env
            .storage()
            .persistent()
            .get(&score_history_key(asset_id))
            .unwrap_or(Vec::new(&env));
        score_history.push_back(ScoreEntry {
            timestamp,
            score: new_score,
        });
        env.storage()
            .persistent()
            .set(&score_history_key(asset_id), &score_history);

        // Update last maintenance timestamp for decay tracking
        env.storage()
            .persistent()
            .set(&last_update_key(asset_id), &timestamp);

        // Emit maintenance submission event
        env.events().publish(
            (symbol_short!("MAINT"), asset_id),
            (task_type, engineer, timestamp),
        );
    }

    /// Submit multiple maintenance records for the same asset in a single transaction.
    /// All records are validated before any are written.
    pub fn batch_submit_maintenance(
        env: Env,
        asset_id: u64,
        records: Vec<BatchRecord>,
        engineer: Address,
    ) {
        engineer.require_auth();

        // Validate asset exists
        let asset_registry: Address = env
            .storage()
            .instance()
            .get(&ASSET_REGISTRY)
            .expect("asset registry not set");
        let asset_registry_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        asset_registry_client.get_asset(&asset_id);

        // Validate engineer credential
        let engineer_registry: Address = env
            .storage()
            .instance()
            .get(&ENG_REGISTRY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::UnauthorizedEngineer));
        let engineer_registry_client =
            engineer_registry_client::EngineerRegistryClient::new(&env, &engineer_registry);
        if !engineer_registry_client.verify_engineer(&engineer) {
            panic_with_error!(&env, ContractError::UnauthorizedEngineer);
        }

        let mut history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .expect("config not set");

        // Validate all records fit before writing any
        if history.len() + records.len() > config.max_history {
            panic!("history cap reached");
        }

        // Write all records
        let timestamp = env.ledger().timestamp();
        let mut score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);

        let grace_end: u64 = env
            .storage()
            .persistent()
            .get(&grace_period_key(&engineer))
            .unwrap_or(0);
        let signed_during_grace_period = grace_end > 0 && timestamp <= grace_end;

        let mut prev_hash = env
            .storage()
            .persistent()
            .get(&chain_head_key(asset_id))
            .unwrap_or_else(|| zero_hash(&env));

        for record in records.iter() {
            let mut weight = get_task_weight(&env, &record.task_type);
            if signed_during_grace_period {
                weight = weight * GRACE_PENALTY_NUM / GRACE_PENALTY_DEN;
            }
            score = (score + weight).min(100);

            let nonce = history.len();
            let record_hash = compute_record_hash(
                &env,
                asset_id,
                &record.task_type,
                &engineer,
                timestamp,
                nonce,
                &prev_hash,
            );

            history.push_back(MaintenanceRecord {
                asset_id,
                task_type: record.task_type.clone(),
                notes: record.notes.clone(),
                engineer: engineer.clone(),
                timestamp,
                signed_during_grace_period,
                record_hash: record_hash.clone(),
                prev_hash: prev_hash.clone(),
            });
            prev_hash = record_hash;
        }

        env.storage().persistent().set(&history_key(asset_id), &history);
        env.storage().persistent().set(&score_key(asset_id), &score);
        env.storage().persistent().set(&last_update_key(asset_id), &timestamp);
        env.storage().persistent().set(&chain_head_key(asset_id), &prev_hash);
    }

    /// Apply time-based decay to an asset's collateral score.
    /// Can be called by anyone to ensure scores reflect current maintenance status.
    /// Decay rate: 5 points per 30 days of no maintenance.
    pub fn decay_score(env: Env, asset_id: u64) -> u32 {
        let current_score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);

        if current_score == 0 {
            return 0;
        }

        let last_update: u64 = env
            .storage()
            .persistent()
            .get(&last_update_key(asset_id))
            .unwrap_or(0u64);

        let current_time = env.ledger().timestamp();
        let time_elapsed = current_time.saturating_sub(last_update);

        // Calculate decay: 5 points per 30-day interval
        let decay_intervals = time_elapsed / DECAY_INTERVAL;
        let total_decay = (decay_intervals as u32) * DECAY_RATE;

        let new_score = current_score.saturating_sub(total_decay);

        env.storage()
            .persistent()
            .set(&score_key(asset_id), &new_score);
        env.storage()
            .persistent()
            .set(&last_update_key(asset_id), &current_time);

        env.events().publish(
            (symbol_short!("DECAY"), asset_id),
            (current_score, new_score, current_time),
        );

        new_score
    }

    pub fn get_maintenance_history(env: Env, asset_id: u64) -> Vec<MaintenanceRecord> {
        env.storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env))
    }

    pub fn get_last_service(env: Env, asset_id: u64) -> MaintenanceRecord {
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoMaintenanceHistory));

        history
            .last()
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoMaintenanceHistory))
    }

    pub fn get_collateral_score(env: Env, asset_id: u64) -> u32 {
        env.storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0)
    }

    /// Returns the full score trend: one (timestamp, score) entry per maintenance event.
    pub fn get_score_history(env: Env, asset_id: u64) -> Vec<ScoreEntry> {
        env.storage()
            .persistent()
            .get(&score_history_key(asset_id))
            .unwrap_or(Vec::new(&env))
    }

    pub fn is_collateral_eligible(env: Env, asset_id: u64) -> bool {
        let threshold = 50u32;
        Self::get_collateral_score(env, asset_id) >= threshold
    }

    /// Admin-only: set the timestamp until which an engineer's expired
    /// credential is still accepted (in a "grace period"). Records submitted
    /// while `env.ledger().timestamp() <= grace_period_end` are flagged via
    /// `MaintenanceRecord::signed_during_grace_period` and score at a
    /// reduced weight.
    pub fn set_credential_grace_period(
        env: Env,
        admin: Address,
        engineer: Address,
        grace_period_end: u64,
    ) {
        admin.require_auth();
        Self::require_admin(&env, &admin);
        env.storage()
            .persistent()
            .set(&grace_period_key(&engineer), &grace_period_end);
    }

    /// Admin-only: propose permanently deleting an asset's maintenance
    /// history. Cannot be executed until PRUNE_TIMELOCK_SECS (48h) have
    /// elapsed, giving lenders time to react to a compromised admin key.
    pub fn propose_prune_asset_history(env: Env, admin: Address, asset_id: u64) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let proposed_at = env.ledger().timestamp();
        env.storage().temporary().set(
            &prune_proposal_key(asset_id),
            &PruneProposal {
                asset_id,
                proposed_at,
            },
        );
        // PRUNE_PROP (shortened to fit Soroban's 9-char small-symbol limit).
        env.events().publish(
            (symbol_short!("PRUNEPROP"), asset_id),
            (admin, proposed_at),
        );
    }

    /// Admin-only: cancel a pending prune proposal before it can be executed.
    pub fn cancel_prune_asset_history(env: Env, admin: Address, asset_id: u64) {
        admin.require_auth();
        Self::require_admin(&env, &admin);
        env.storage()
            .temporary()
            .remove(&prune_proposal_key(asset_id));
    }

    /// Admin-only: execute a previously proposed prune once the 48h timelock
    /// has elapsed. Permanently deletes the asset's maintenance history.
    pub fn execute_prune_asset_history(env: Env, admin: Address, asset_id: u64) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let proposal: PruneProposal = env
            .storage()
            .temporary()
            .get(&prune_proposal_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoPrunePending));

        let now = env.ledger().timestamp();
        if now < proposal.proposed_at + PRUNE_TIMELOCK_SECS {
            panic_with_error!(&env, ContractError::PruneTimelockNotElapsed);
        }

        env.storage().persistent().remove(&history_key(asset_id));
        env.storage().persistent().remove(&chain_head_key(asset_id));
        env.storage()
            .temporary()
            .remove(&prune_proposal_key(asset_id));

        env.events()
            .publish((symbol_short!("PRUNEXEC"), asset_id), (admin, now));
    }

    fn require_admin(env: &Env, admin: &Address) {
        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .expect("config not set");
        if &config.admin != admin {
            panic_with_error!(env, ContractError::UnauthorizedAdmin);
        }
    }

    /// Admin-only: upgrade the contract WASM to a new hash.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        admin.require_auth();

        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .expect("config not set");
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::engineer_registry::{EngineerRegistry, EngineerRegistryClient};
    use asset_registry::{AssetRegistry, AssetRegistryClient};
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events},
        BytesN, Env, String,
    };

    fn setup<'a>(
        env: &'a Env,
        max_history: u32,
    ) -> (
        LifecycleClient<'a>,
        AssetRegistryClient<'a>,
        EngineerRegistryClient<'a>,
        Address,
    ) {
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(env);

        let lifecycle = LifecycleClient::new(env, &lifecycle_id);
        lifecycle.initialize(
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &max_history,
        );

        (
            lifecycle,
            AssetRegistryClient::new(env, &asset_registry_id),
            EngineerRegistryClient::new(env, &engineer_registry_id),
            admin,
        )
    }

    fn register_asset(env: &Env, registry_client: &AssetRegistryClient) -> u64 {
        let owner = Address::generate(env);
        registry_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Caterpillar 3516"),
            &owner,
        )
    }

    fn register_engineer(env: &Env, registry_client: &EngineerRegistryClient) -> Address {
        let engineer = Address::generate(env);
        let issuer = Address::generate(env);
        let hash = BytesN::from_array(env, &[1u8; 32]);
        registry_client.register_engineer(&engineer, &hash, &issuer);
        engineer
    }

    #[test]
    fn test_submit_and_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // 10 oil changes at 2 points each = 20 points
        for _ in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "Routine oil change"),
                &engineer,
            );
        }

        assert_eq!(client.get_collateral_score(&asset_id), 20);
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 10);
    }

    #[test]
    #[should_panic]
    fn test_submit_maintenance_nonexistent_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &999u64,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Should fail"),
            &engineer,
        );
    }

    #[test]
    #[should_panic]
    fn test_history_cap_enforced() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 3);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        for _ in 0..3 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "ok"),
                &engineer,
            );
        }

        // This 4th submission should panic (cap = 3)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "over cap"),
            &engineer,
        );
    }

    #[test]
    fn test_unregistered_engineer_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let unregistered = Address::generate(&env);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Should fail"),
            &unregistered,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_get_last_service_no_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let result = client.try_get_last_service(&asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NoMaintenanceHistory as u32,
            ))),
        );
    }

    #[test]
    fn test_admin_can_update_score_increment() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.update_score_increment(&admin, &12);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Configured increment"),
            &engineer,
        );

        assert_eq!(client.get_collateral_score(&asset_id), 12);
    }

    #[test]
    fn test_non_admin_cannot_update_score_increment() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let result = client.try_update_score_increment(&outsider, &12);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Routine"),
            &engineer,
        );

        let events = env.events().all();
        assert!(events.len() > 0);
    }

    // --- Upgrade tests ---

    #[test]
    fn test_admin_can_upgrade() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        // Should not panic — admin is authorized
        client.upgrade(&admin, &new_wasm_hash);
    }

    #[test]
    fn test_non_admin_cannot_upgrade() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        let result = client.try_upgrade(&outsider, &new_wasm_hash);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    // --- Score history tests ---

    #[test]
    fn test_score_history_empty_before_any_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);

        let history = client.get_score_history(&asset_id);
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_score_history_records_entry_per_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "First"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Second"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &String::from_str(&env, "Third"),
            &engineer,
        );

        let history = client.get_score_history(&asset_id);
        // One entry per maintenance event
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_score_history_scores_are_cumulative() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // OIL_CHG = 2 pts, ENGINE = 10 pts, FILTER = 5 pts
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "a"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "b"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &String::from_str(&env, "c"),
            &engineer,
        );

        let history = client.get_score_history(&asset_id);
        assert_eq!(history.get(0).unwrap().score, 2);   // 0 + 2
        assert_eq!(history.get(1).unwrap().score, 12);  // 2 + 10
        assert_eq!(history.get(2).unwrap().score, 17);  // 12 + 5
    }

    #[test]
    fn test_score_history_timestamps_match_ledger() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let t0 = env.ledger().timestamp();
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "at t0"),
            &engineer,
        );

        env.ledger().with_mut(|li| li.timestamp = li.timestamp + 1000);
        let t1 = env.ledger().timestamp();
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("LUBE"),
            &String::from_str(&env, "at t1"),
            &engineer,
        );

        let history = client.get_score_history(&asset_id);
        assert_eq!(history.get(0).unwrap().timestamp, t0);
        assert_eq!(history.get(1).unwrap().timestamp, t1);
    }

    #[test]
    fn test_score_history_capped_at_100() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // 10 REBUILD tasks at 10 pts each would be 100, then more should stay at 100
        for _ in 0..12 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("REBUILD"),
                &String::from_str(&env, "major"),
                &engineer,
            );
        }

        let history = client.get_score_history(&asset_id);
        // Score should never exceed 100
        for i in 0..history.len() {
            assert!(history.get(i).unwrap().score <= 100);
        }
        // After 10 REBUILD tasks the score is already 100; subsequent entries stay at 100
        assert_eq!(history.get(10).unwrap().score, 100);
        assert_eq!(history.get(11).unwrap().score, 100);
    }

    #[test]
    fn test_batch_submit_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Oil change"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            notes: String::from_str(&env, "Inspection"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("ENGINE"),
            notes: String::from_str(&env, "Engine repair"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        // OIL_CHG=2, INSPECT=2, ENGINE=10 => 14
        assert_eq!(client.get_collateral_score(&asset_id), 14);
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 3);
    }

    #[test]
    #[should_panic(expected = "history cap reached")]
    fn test_batch_submit_exceeds_history_cap() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 2);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "First"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Second"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Third - over cap"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);
    }

    #[test]
    fn test_batch_submit_unauthorized_engineer() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let unregistered = Address::generate(&env);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Should fail"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &unregistered);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_unregistered_engineer_should_panic() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let unregistered = Address::generate(&env);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Should fail"),
            &unregistered,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_collateral_score_caps_at_100() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // FILTER = 5 points each; 25 submissions would be 125 without a cap
        for _ in 0..25 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("FILTER"),
                &String::from_str(&env, "Filter replacement"),
                &engineer,
            );
        }

        assert_eq!(client.get_collateral_score(&asset_id), 100);
    }

    #[test]
    fn test_submit_maintenance_revoked_engineer_should_panic() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        engineer_registry_client.revoke_credential(&engineer);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Post-revocation attempt"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    // --- Issue 1: prune_asset_history timelock ---

    #[test]
    fn test_prune_asset_history_requires_timelock() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "before prune"),
            &engineer,
        );

        client.propose_prune_asset_history(&admin, &asset_id);

        // Executing immediately, before the 48h timelock elapses, must fail.
        let result = client.try_execute_prune_asset_history(&admin, &asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::PruneTimelockNotElapsed as u32,
            ))),
        );
        // History must still be intact.
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 1);
    }

    #[test]
    fn test_prune_asset_history_succeeds_after_timelock() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "before prune"),
            &engineer,
        );

        client.propose_prune_asset_history(&admin, &asset_id);
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + PRUNE_TIMELOCK_SECS + 1);
        client.execute_prune_asset_history(&admin, &asset_id);

        assert_eq!(client.get_maintenance_history(&asset_id).len(), 0);
    }

    #[test]
    fn test_prune_asset_history_no_pending_proposal() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);

        let result = client.try_execute_prune_asset_history(&admin, &asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NoPrunePending as u32,
            ))),
        );
    }

    #[test]
    fn test_prune_asset_history_non_admin_cannot_propose() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let outsider = Address::generate(&env);

        let result = client.try_propose_prune_asset_history(&outsider, &asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_prune_asset_history_emits_prune_prop_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);

        client.propose_prune_asset_history(&admin, &asset_id);
        let events = env.events().all();
        assert!(events.len() > 0);
    }

    // --- Issue 3: grace-period record flagging ---

    #[test]
    fn test_grace_period_record_is_flagged_and_penalized() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let now = env.ledger().timestamp();
        client.set_credential_grace_period(&admin, &engineer, &(now + 1000));

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"), // full weight = 10
            &String::from_str(&env, "grace period service"),
            &engineer,
        );

        let history = client.get_maintenance_history(&asset_id);
        let record = history.get(0).unwrap();
        assert!(record.signed_during_grace_period);
        // Halved weight: 10 -> 5
        assert_eq!(client.get_collateral_score(&asset_id), 5);
    }

    #[test]
    fn test_non_grace_period_record_is_not_flagged() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "normal service"),
            &engineer,
        );

        let history = client.get_maintenance_history(&asset_id);
        let record = history.get(0).unwrap();
        assert!(!record.signed_during_grace_period);
        assert_eq!(client.get_collateral_score(&asset_id), 10);
    }

    #[test]
    fn test_grace_period_expired_does_not_flag_record() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let now = env.ledger().timestamp();
        client.set_credential_grace_period(&admin, &engineer, &(now + 10));
        env.ledger().with_mut(|li| li.timestamp = li.timestamp + 1000);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "after grace period expired"),
            &engineer,
        );

        let history = client.get_maintenance_history(&asset_id);
        assert!(!history.get(0).unwrap().signed_during_grace_period);
        assert_eq!(client.get_collateral_score(&asset_id), 10);
    }

    // --- Issue 4: hash chain integrity ---

    #[test]
    fn test_record_hash_excludes_notes_and_is_deterministic() {
        let env = Env::default();
        let asset_id = 1u64;
        let task_type = symbol_short!("OIL_CHG");
        let engineer = Address::generate(&env);
        let timestamp = 1000u64;
        let prev = zero_hash(&env);

        // Identical inputs (notes is not part of the hash function's
        // signature at all) always produce the same hash.
        let hash_a = compute_record_hash(&env, asset_id, &task_type, &engineer, timestamp, 0, &prev);
        let hash_b = compute_record_hash(&env, asset_id, &task_type, &engineer, timestamp, 0, &prev);
        assert_eq!(hash_a, hash_b);

        // Changing the chain position (nonce) changes the hash even though
        // every other field is identical, which is what prevents an
        // attacker from crafting `notes` to predict or collide hashes.
        let hash_c = compute_record_hash(&env, asset_id, &task_type, &engineer, timestamp, 1, &prev);
        assert_ne!(hash_a, hash_c);
    }

    #[test]
    fn test_maintenance_history_hash_chain_links_records() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "first, arbitrary notes A"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "second, wildly different notes!!"),
            &engineer,
        );

        let history = client.get_maintenance_history(&asset_id);
        let first = history.get(0).unwrap();
        let second = history.get(1).unwrap();

        assert_eq!(first.prev_hash, zero_hash(&env));
        assert_eq!(second.prev_hash, first.record_hash);
        assert_ne!(first.record_hash, second.record_hash);
    }
}
