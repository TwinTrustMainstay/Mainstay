#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, String, Symbol, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ContractError {
    NoMaintenanceHistory = 1,
    UnauthorizedEngineer = 2,
    UnauthorizedAdmin = 3,
    HistoryCapReached = 4,
    AssetNotFound = 5,
    NotInitialized = 6,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceRecord {
    pub asset_id: u64,
    pub task_type: Symbol,
    pub notes: String,
    pub engineer: Address,
    pub timestamp: u64,
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
    pub decay_rate: u32,
    pub decay_interval: u64,
}

const ASSET_REGISTRY: Symbol = symbol_short!("REGISTRY");
const ENG_REGISTRY: Symbol = symbol_short!("ENG_REG");
const CONFIG: Symbol = symbol_short!("CONFIG");
const DEFAULT_MAX_HISTORY: u32 = 200;
const DEFAULT_SCORE_INCREMENT: u32 = 5;
const DEFAULT_DECAY_RATE: u32 = 5;
const DEFAULT_DECAY_INTERVAL: u64 = 2592000; // 30 days in seconds

fn history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("HIST"), asset_id)
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
            admin: admin.clone(),
            max_history: if max_history == 0 {
                DEFAULT_MAX_HISTORY
            } else {
                max_history
            },
            score_increment: DEFAULT_SCORE_INCREMENT,
            decay_rate: DEFAULT_DECAY_RATE,
            decay_interval: DEFAULT_DECAY_INTERVAL,
        };
        env.storage().instance().set(&CONFIG, &config);

        env.events().publish(
            (symbol_short!("INIT"),),
            (asset_registry, engineer_registry, admin),
        );
    }

    pub fn update_score_increment(env: Env, admin: Address, score_increment: u32) {
        admin.require_auth();

        let mut config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        config.score_increment = score_increment;
        env.storage().instance().set(&CONFIG, &config);
    }

    /// Admin-only: update the decay rate and interval for collateral score decay.
    /// decay_rate: points to deduct per interval
    /// decay_interval: time interval in seconds for each decay step
    pub fn update_decay_config(
        env: Env,
        admin: Address,
        decay_rate: u32,
        decay_interval: u64,
    ) {
        admin.require_auth();

        let mut config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        config.decay_rate = decay_rate;
        config.decay_interval = decay_interval;
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
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
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
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        let mut history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        if history.len() >= config.max_history {
            panic_with_error!(&env, ContractError::HistoryCapReached);
        }

        let timestamp = env.ledger().timestamp();

        let record = MaintenanceRecord {
            asset_id,
            task_type: task_type.clone(),
            notes,
            engineer: engineer.clone(),
            timestamp,
        };

        history.push_back(record);
        env.storage()
            .persistent()
            .set(&history_key(asset_id), &history);

        // Update collateral score
        let score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);
        let weight = get_task_weight(&env, &task_type);
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
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        let asset_registry_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        asset_registry_client.get_asset(&asset_id);

        // Validate engineer credential
        let engineer_registry: Address = env
            .storage()
            .instance()
            .get(&ENG_REGISTRY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::UnauthorizedEngineer));
        let engineer_registry_client =
            engineer_registry::EngineerRegistryClient::new(&env, &engineer_registry);
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
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        // Validate all records fit before writing any
        if history.len() + records.len() > config.max_history {
            panic_with_error!(&env, ContractError::HistoryCapReached);
        }

        // Write all records
        let timestamp = env.ledger().timestamp();
        let mut score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);
        let mut score_history: Vec<ScoreEntry> = env
            .storage()
            .persistent()
            .get(&score_history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        for record in records.iter() {
            let weight = get_task_weight(&env, &record.task_type);
            score = (score + weight).min(100);
            history.push_back(MaintenanceRecord {
                asset_id,
                task_type: record.task_type.clone(),
                notes: record.notes.clone(),
                engineer: engineer.clone(),
                timestamp,
            });
            score_history.push_back(ScoreEntry { timestamp, score });
        }

        env.storage().persistent().set(&history_key(asset_id), &history);
        env.storage().persistent().set(&score_key(asset_id), &score);
        env.storage().persistent().set(&score_history_key(asset_id), &score_history);
        env.storage().persistent().set(&last_update_key(asset_id), &timestamp);
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

        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        let current_time = env.ledger().timestamp();
        let time_elapsed = current_time.saturating_sub(last_update);

        // Calculate decay using configured rate and interval
        let decay_intervals = time_elapsed / config.decay_interval;
        let total_decay = (decay_intervals as u32) * config.decay_rate;

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

    /// Returns a paginated slice of the maintenance history.
    /// `offset` is the zero-based start index; `limit` is the max number of records to return.
    pub fn get_maintenance_history_page(
        env: Env,
        asset_id: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<MaintenanceRecord> {
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        let len = history.len();
        if offset >= len || limit == 0 {
            return Vec::new(&env);
        }

        let end = (offset + limit).min(len);
        let mut page = Vec::new(&env);
        for i in offset..end {
            page.push_back(history.get(i).unwrap());
        }
        page
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
        // Verify asset exists before returning score
        let asset_registry: Address = env
            .storage()
            .instance()
            .get(&ASSET_REGISTRY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        let asset_registry_client =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        asset_registry_client.get_asset(&asset_id);

        env.storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0)
    }

pub fn is_collateral_eligible(env: Env, asset_id: u64) -> bool {
        Self::get_collateral_score(env, asset_id) >= 50
    /// Returns the full score trend: one (timestamp, score) entry per maintenance event.
    pub fn get_score_history(env: Env, asset_id: u64) -> Vec<ScoreEntry> {
        env.storage()
            .persistent()
            .get(&score_history_key(asset_id))
            .unwrap_or(Vec::new(&env))
    }

    /// Returns the last `n` ScoreEntry items from the score history.
    /// If `n` is 0 or the history is empty, returns an empty vec.
    /// If `n` exceeds the history length, returns all entries.
    pub fn get_score_trend(env: Env, asset_id: u64, n: u32) -> Vec<ScoreEntry> {
        if n == 0 {
            return Vec::new(&env);
        }
        let history: Vec<ScoreEntry> = env
            .storage()
            .persistent()
            .get(&score_history_key(asset_id))
            .unwrap_or(Vec::new(&env));
        let len = history.len();
        if len == 0 {
            return Vec::new(&env);
        }
        let start = if n >= len { 0u32 } else { len - n };
        let mut result = Vec::new(&env);
        for i in start..len {
            result.push_back(history.get(i).unwrap());
        }
        result
    }

    pub fn is_collateral_eligible(env: Env, asset_id: u64) -> bool {
        // Verify asset exists before checking eligibility
        let asset_registry: Address = env
            .storage()
            .instance()
            .get(&ASSET_REGISTRY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        let asset_registry_client =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        asset_registry_client.get_asset(&asset_id);

        let threshold = 50u32;
        Self::get_collateral_score(env, asset_id) >= threshold
    }

    pub fn get_asset_registry(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&ASSET_REGISTRY)
            .expect("asset registry not set")
    }

    pub fn update_asset_registry(env: Env, admin: Address, new_registry: Address) {
        admin.require_auth();

        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .expect("config not set");
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        env.storage().instance().set(&ASSET_REGISTRY, &new_registry);

        env.events().publish(
            (symbol_short!("UPD_AST"),),
            (admin, new_registry),
        );
    }

    pub fn get_engineer_registry(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&ENG_REGISTRY)
            .expect("engineer registry not set")
    }

    pub fn update_engineer_registry(env: Env, admin: Address, new_registry: Address) {
        admin.require_auth();

        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .expect("config not set");
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        env.storage().instance().set(&ENG_REGISTRY, &new_registry);

        env.events().publish(
            (symbol_short!("UPD_ENG"),),
            (admin, new_registry),
        );
    }

    pub fn get_config(env: Env) -> Config {
        env.storage()
            .instance()
            .get(&CONFIG)
            .expect("config not set")
    }

    /// Admin-only: upgrade the contract WASM to a new hash.
    pub fn upgrade(env: Env, admin: Address, _new_wasm_hash: BytesN<32>) {
        admin.require_auth();

        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        #[cfg(not(test))]
        {
            env.deployer().update_current_contract_wasm(_new_wasm_hash);
        }
    }

    /// Admin-only: reset an asset's collateral score to zero.
    /// Use in cases of fraud or after an asset transfer.
    pub fn reset_score(env: Env, admin: Address, asset_id: u64) {
        admin.require_auth();

        let config: Config = env
            .storage()
            .instance()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        env.storage().persistent().set(&score_key(asset_id), &0u32);

        env.events().publish(
            (symbol_short!("RST_SCR"), asset_id),
            (admin, env.ledger().timestamp()),
        );
    }

    pub fn batch_is_collateral_eligible(env: Env, asset_ids: Vec<u64>) -> Vec<bool> {
        asset_ids.iter().map(|&id| Self::is_collateral_eligible(env.clone(), id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::engineer_registry::{EngineerRegistry, EngineerRegistryClient};
    use asset_registry::{AssetRegistry, AssetRegistryClient};
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events, Ledger},
        BytesN, Env, String, TryIntoVal,
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
        let admin = Address::generate(env);
        let hash = BytesN::from_array(env, &[1u8; 32]);
        registry_client.initialize_admin(&admin);
        registry_client.add_trusted_issuer(&admin, &issuer);
        registry_client.register_engineer(&engineer, &hash, &issuer, &31_536_000);
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
    fn test_history_cap_enforced() {
        let env = Env::default();
        env.mock_all_auths();

        let engineer = Address::generate(&env);
        // Fill to cap
for _ in 0..3 {
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

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "over cap"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::HistoryCapReached as u32,
            ))),
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

        // score_increment config is stored but task weight (2 for OIL_CHG) governs scoring
        assert_eq!(client.get_collateral_score(&asset_id), 2);
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
    fn test_admin_can_update_decay_config() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Build up a score first
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );

        // Update decay config: 10 points per 60 seconds (for testing)
        client.update_decay_config(&admin, &10, &60);

        // Advance ledger time by 120 seconds (2 intervals)
        env.ledger().with_mut(|li| li.timestamp = li.timestamp + 120);

        // Apply decay: should lose 20 points (10 * 2 intervals)
        let initial_score = client.get_collateral_score(&asset_id);
        client.decay_score(&asset_id);
        let new_score = client.get_collateral_score(&asset_id);

        assert_eq!(new_score, initial_score.saturating_sub(20));
    }

    #[test]
    fn test_non_admin_cannot_update_decay_config() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let result = client.try_update_decay_config(&outsider, &10, &60);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_decay_score_uses_configured_values() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Build up a score
        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "Major work"),
                &engineer,
            );
        }

        // Set custom decay: 2 points per 100 seconds
        client.update_decay_config(&admin, &2, &100);

        // Advance time by 250 seconds (2 full intervals)
        env.ledger().with_mut(|li| li.timestamp = li.timestamp + 250);

        // Apply decay: should lose 4 points (2 * 2 intervals)
        let initial_score = client.get_collateral_score(&asset_id);
        client.decay_score(&asset_id);
        let new_score = client.get_collateral_score(&asset_id);

        assert_eq!(new_score, initial_score.saturating_sub(4));
    }

    #[test]
    fn test_decay_score_five_points_per_thirty_day_interval() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "Build score to 50"),
                &engineer,
            );
        }
        assert_eq!(client.get_collateral_score(&asset_id), 50);

        env.ledger().with_mut(|li| li.timestamp = li.timestamp + 2 * DEFAULT_DECAY_INTERVAL);

        let decayed = client.decay_score(&asset_id);
        assert_eq!(decayed, 40);
        assert_eq!(client.get_collateral_score(&asset_id), 40);
    }

    #[test]
    fn test_decay_score_clamps_at_zero_after_long_elapsed_time() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Single major service"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 10);

        const SECONDS_PER_DAY: u64 = 86_400;
        const DAYS_PER_YEAR: u64 = 365;
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp + DAYS_PER_YEAR * SECONDS_PER_DAY;
        });

        let decayed = client.decay_score(&asset_id);
        assert_eq!(decayed, 0);
        assert_eq!(client.get_collateral_score(&asset_id), 0);
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

    #[test]
    fn test_initialize_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(&env);

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        lifecycle.initialize(
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &0u32,
        );

        let events = env.events().all();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_get_collateral_score_unregistered_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);

        // Query score for non-existent asset ID
        let result = client.try_get_collateral_score(&999u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                asset_registry::ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_is_collateral_eligible_unregistered_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);

        // Check eligibility for non-existent asset ID
        let result = client.try_is_collateral_eligible(&999u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                asset_registry::ContractError::AssetNotFound as u32,
            ))),
        );
    }

    // --- Upgrade tests ---

    #[test]
    fn test_admin_can_upgrade() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        // In test env WASM won't exist; verify no UnauthorizedAdmin error is returned
        let result = client.try_upgrade(&admin, &new_wasm_hash);
        assert!(result != Err(Ok(soroban_sdk::Error::from_contract_error(ContractError::UnauthorizedAdmin as u32))));
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
    fn test_score_trend_returns_last_n() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "entry"),
                &engineer,
            );
        }

        let full = client.get_score_history(&asset_id);
        let trend = client.get_score_trend(&asset_id, &3);
        assert_eq!(trend.len(), 3);
        // Should be the last 3 entries
        assert_eq!(trend.get(0).unwrap().score, full.get(2).unwrap().score);
        assert_eq!(trend.get(1).unwrap().score, full.get(3).unwrap().score);
        assert_eq!(trend.get(2).unwrap().score, full.get(4).unwrap().score);
    }

    #[test]
    fn test_score_trend_n_exceeds_history_length() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "only one"),
            &engineer,
        );

        // n=10 but only 1 entry exists — should return all 1
        let trend = client.get_score_trend(&asset_id, &10);
        assert_eq!(trend.len(), 1);
    }

    #[test]
    fn test_score_trend_n_zero_returns_empty() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "entry"),
            &engineer,
        );

        let trend = client.get_score_trend(&asset_id, &0);
        assert_eq!(trend.len(), 0);
    }

    #[test]
    fn test_score_trend_empty_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);

        let trend = client.get_score_trend(&asset_id, &5);
        assert_eq!(trend.len(), 0);
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
    fn test_batch_submit_fails_atomically_on_history_cap() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 3);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Fill to max_history - 1 = 2
        for _ in 0..2 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, ""),
                &engineer,
            );
        }
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 2);

        // Batch of 2 would push total to 4, exceeding cap of 3
        let mut records = Vec::new(&env);
        records.push_back(BatchRecord { task_type: symbol_short!("OIL_CHG"), notes: String::from_str(&env, "") });
        records.push_back(BatchRecord { task_type: symbol_short!("OIL_CHG"), notes: String::from_str(&env, "") });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::HistoryCapReached as u32,
            ))),
        );

        // No records written — history still at 2
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 2);
    }

    #[test]
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

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::HistoryCapReached as u32,
            ))),
        );
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

    #[test]
    fn test_full_lifecycle_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        // 1. Register asset
        let owner = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "GE LM2500 Turbine Unit 7"),
            &owner,
        );
        let asset = asset_registry.get_asset(&asset_id);
        assert_eq!(asset.owner, owner);

        // 2. Register and verify engineer
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let admin = Address::generate(&env);
        engineer_registry.initialize_admin(&admin);
        engineer_registry.add_trusted_issuer(&admin, &issuer);
        engineer_registry.register_engineer(&engineer, &BytesN::from_array(&env, &[2u8; 32]), &issuer, &31_536_000);
        assert!(engineer_registry.verify_engineer(&engineer));

        // 3. Submit 10 maintenance records (ENGINE = 10pts each, capped at 100)
        for i in 0..10u32 {
            lifecycle.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "Full engine service"),
                &engineer,
            );
            // advance ledger timestamp so records are distinct
            env.ledger().set_timestamp(env.ledger().timestamp() + 1);
            let _ = i;
        }

        // 4. Assert collateral eligible (score >= 50)
        assert!(lifecycle.is_collateral_eligible(&asset_id));

        // 5. Assert get_last_service returns the correct record
        let last = lifecycle.get_last_service(&asset_id);
        assert_eq!(last.asset_id, asset_id);
        assert_eq!(last.engineer, engineer);
        assert_eq!(last.task_type, symbol_short!("ENGINE"));
    }

    #[test]
    fn test_decay_score_emits_correct_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // ENGINE = 10 pts
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, ""),
            &engineer,
        );
        let initial_score: u32 = 10;

        // Use fast decay: 3 pts per 60s, advance 60s (1 interval)
        client.update_decay_config(&admin, &3, &60);
        env.ledger().with_mut(|li| li.timestamp += 60);
        let decay_time = env.ledger().timestamp();

        client.decay_score(&asset_id);

        let events = env.events().all();
        assert_eq!(events.len(), 1);

        let (_, topics, data) = events.get(0).unwrap();

        // Topics: (symbol("DECAY"), asset_id)
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("DECAY"));
        assert_eq!(t1, asset_id);

        // Data: (old_score, new_score, timestamp)
        let expected_new_score: u32 = initial_score - 3;
        let (ev_old, ev_new, ev_ts): (u32, u32, u64) = data.try_into_val(&env).unwrap();
        assert_eq!(ev_old, initial_score);
        assert_eq!(ev_new, expected_new_score);
        assert_eq!(ev_ts, decay_time);
    }

    #[test]
    fn test_admin_can_reset_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Build up a non-zero score
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );
        assert!(client.get_collateral_score(&asset_id) > 0);

        // Admin resets the score
        client.reset_score(&admin, &asset_id);
        assert_eq!(client.get_collateral_score(&asset_id), 0);
    }

    #[test]
    fn test_task_weight_tiers() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Minor: OIL_CHG = 2
        client.submit_maintenance(&asset_id, &symbol_short!("OIL_CHG"), &String::from_str(&env, ""), &engineer);
        assert_eq!(client.get_collateral_score(&asset_id), 2);

        client.reset_score(&admin, &asset_id);

        // Medium: FILTER = 5
        client.submit_maintenance(&asset_id, &symbol_short!("FILTER"), &String::from_str(&env, ""), &engineer);
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        client.reset_score(&admin, &asset_id);

        // Major: ENGINE = 10
        client.submit_maintenance(&asset_id, &symbol_short!("ENGINE"), &String::from_str(&env, ""), &engineer);
        assert_eq!(client.get_collateral_score(&asset_id), 10);

        client.reset_score(&admin, &asset_id);

        // Unknown type = 3
        client.submit_maintenance(&asset_id, &symbol_short!("UNKNOWN"), &String::from_str(&env, ""), &engineer);
        assert_eq!(client.get_collateral_score(&asset_id), 3);
    }

    #[test]
    fn test_non_admin_cannot_reset_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );

        let outsider = Address::generate(&env);
        let result = client.try_reset_score(&outsider, &asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    // --- Issue #142: NotInitialized structured error ---

    #[test]
    fn test_get_collateral_score_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();

        // Deploy lifecycle without calling initialize
        let lifecycle_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &lifecycle_id);

        let result = client.try_get_collateral_score(&1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    // --- Issue #144: batch_submit_maintenance updates score_history_key ---

    #[test]
    fn test_batch_submit_score_history_length_matches_records() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "First"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            notes: String::from_str(&env, "Second"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("ENGINE"),
            notes: String::from_str(&env, "Third"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        let score_history = client.get_score_history(&asset_id);
        assert_eq!(score_history.len(), 3, "score_history length must match batch record count");
    }

    #[test]
    fn test_get_maintenance_history_page() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "oil change"),
                &engineer,
            );
        }

        // First page: offset=0, limit=2 → 2 records
        assert_eq!(client.get_maintenance_history_page(&asset_id, &0, &2).len(), 2);
        // Second page: offset=2, limit=2 → 2 records
        assert_eq!(client.get_maintenance_history_page(&asset_id, &2, &2).len(), 2);
        // Third page: offset=4, limit=2 → 1 record (only one left)
        assert_eq!(client.get_maintenance_history_page(&asset_id, &4, &2).len(), 1);
        // Out-of-bounds offset → empty
        assert_eq!(client.get_maintenance_history_page(&asset_id, &10, &2).len(), 0);
        // limit=0 → empty
        assert_eq!(client.get_maintenance_history_page(&asset_id, &0, &0).len(), 0);
    }

    #[test]
    fn test_batch_is_collateral_eligible() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, registry_client) = setup(&env, 0);

        // Eligible asset: 10 maintenances -> score 50
        let asset1 = register_asset(&env, &registry_client);
        let engineer1 = Address::generate(&env);
        for _ in 0..10 {
            client.submit_maintenance(
                &asset1,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "eligible"),
                &engineer1,
            );
        }

        // Eligible asset2
        let asset2 = register_asset(&env, &registry_client);
        let engineer2 = Address::generate(&env);
        for _ in 0..11 {
            client.submit_maintenance(
                &asset2,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "eligible"),
                &engineer2,
            );
        }

        // Ineligible: 0 maintenances -> score 0
        let asset3 = register_asset(&env, &registry_client);

let results = client.batch_is_collateral_eligible(&vec![asset1, asset2, asset3]);
        assert_eq!(results, vec![true, true, false]);
    }
}
