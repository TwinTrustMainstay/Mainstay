#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, String, Symbol, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ContractError {
    NoMaintenanceHistory = 1,
    UnauthorizedEngineer = 2,
    UnauthorizedAdmin = 3,
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
const COLLATERAL_THRESHOLD: u32 = 50;

fn history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("HIST"), asset_id)
}

fn score_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("SCORE"), asset_id)
}

// Minimal client interface for cross-contract call to EngineerRegistry.
mod engineer_registry_client {
const DEFAULT_MAX_HISTORY: u32 = 200;

// Task type weight mapping for collateral scoring
fn get_task_weight(_env: &Env, task_type: &Symbol) -> u32 {
    // Minor tasks: 2 points
    if task_type == &symbol_short!("OIL_CHG") 
        || task_type == &symbol_short!("LUBE") 
        || task_type == &symbol_short!("INSPECT") {
        return 2;
    }
    // Medium tasks: 5 points
    if task_type == &symbol_short!("FILTER") 
        || task_type == &symbol_short!("TUNE_UP") 
        || task_type == &symbol_short!("BRAKE") {
        return 5;
    }
    // Major tasks: 10 points
    if task_type == &symbol_short!("ENGINE") 
        || task_type == &symbol_short!("OVERHAUL") 
        || task_type == &symbol_short!("REBUILD") {
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

        let asset_registry: Address = env
            .storage()
            .instance()
            .get(&ASSET_REGISTRY)
            .expect("asset registry not set");
        let asset_registry_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        asset_registry_client.get_asset(&asset_id);

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
        if history.len() >= config.max_history {
            panic!("history cap reached");
        // Cross-check engineer credential
        let registry_id: Address = env.storage().instance().get(&ENG_REGISTRY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::UnauthorizedEngineer));
        let registry = engineer_registry::EngineerRegistryClient::new(&env, &registry_id);
        if !registry.verify_engineer(&engineer) {
            panic_with_error!(&env, ContractError::UnauthorizedEngineer);
        }

        let mut history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        let record = MaintenanceRecord {
            asset_id,
            task_type: task_type.clone(),
            notes,
            engineer: engineer.clone(),
            timestamp: env.ledger().timestamp(),
        };

        history.push_back(record);
        env.storage()
            .persistent()
            .set(&history_key(asset_id), &history);

        let score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);
        let weight = get_task_weight(&env, &task_type);
        let new_score = (score + weight).min(100);
        env.storage().persistent().set(&score_key(asset_id), &new_score);
        
        // Update last maintenance timestamp for decay tracking
        let current_time = env.ledger().timestamp();
        env.storage().persistent().set(&last_update_key(asset_id), &current_time);
        
        // Emit maintenance submission event
        env.events().publish(
            (symbol_short!("MAINT"), asset_id),
            (task_type, engineer, env.ledger().timestamp())
        );
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
        
        // Update score and last update timestamp
        env.storage().persistent().set(&score_key(asset_id), &new_score);
        env.storage().persistent().set(&last_update_key(asset_id), &current_time);
        
        // Emit decay event
        env.events().publish(
            (symbol_short!("DECAY"), asset_id),
            (current_score, new_score, current_time)
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

    pub fn is_collateral_eligible(env: Env, asset_id: u64) -> bool {
        let threshold = 50u32; // Default threshold
        Self::get_collateral_score(env, asset_id) >= threshold
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

    fn register_asset<'a>(env: &Env, registry_client: &AssetRegistryClient<'a>) -> u64 {
        let owner = Address::generate(env);
        registry_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Caterpillar 3516"),
            &owner,
        )
    }

    fn register_engineer<'a>(env: &Env, registry_client: &EngineerRegistryClient<'a>) -> Address {
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

        let (client, asset_registry_client, engineer_registry_client, eng_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // 10 oil changes at 2 points each = 20 points
        for _ in 0..10 {
            client.submit_maintenance(
                &1u64,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "Routine oil change"),
                &engineer,
            );
        }

        assert_eq!(client.get_collateral_score(&1u64), 20);
        assert_eq!(client.get_maintenance_history(&1u64).len(), 10);
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
    fn test_weighted_scoring_minor_tasks() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Minor tasks: OIL_CHG, LUBE, INSPECT = 2 points each
        client.submit_maintenance(&1u64, &symbol_short!("OIL_CHG"), &String::from_str(&env, "Oil change"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 2);

        client.submit_maintenance(&1u64, &symbol_short!("LUBE"), &String::from_str(&env, "Lubrication"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 4);

        client.submit_maintenance(&1u64, &symbol_short!("INSPECT"), &String::from_str(&env, "Inspection"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 6);
    }

    #[test]
    fn test_weighted_scoring_medium_tasks() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Medium tasks: FILTER, TUNE_UP, BRAKE = 5 points each
        client.submit_maintenance(&1u64, &symbol_short!("FILTER"), &String::from_str(&env, "Filter replacement"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 5);

        client.submit_maintenance(&1u64, &symbol_short!("TUNE_UP"), &String::from_str(&env, "Tune up"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 10);

        client.submit_maintenance(&1u64, &symbol_short!("BRAKE"), &String::from_str(&env, "Brake service"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 15);
    }

    #[test]
    fn test_weighted_scoring_major_tasks() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Major tasks: ENGINE, OVERHAUL, REBUILD = 10 points each
        client.submit_maintenance(&1u64, &symbol_short!("ENGINE"), &String::from_str(&env, "Engine repair"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 10);

        client.submit_maintenance(&1u64, &symbol_short!("OVERHAUL"), &String::from_str(&env, "Full overhaul"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 20);

        client.submit_maintenance(&1u64, &symbol_short!("REBUILD"), &String::from_str(&env, "Complete rebuild"), &engineer);
        assert_eq!(client.get_collateral_score(&1u64), 30);
    }

    #[test]
    fn test_weighted_scoring_mixed_tasks() {
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

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "over cap"),
            &engineer,
        );
    }

    #[test]
    fn test_score_decay_does_not_go_negative() {
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
        
        assert_eq!(client.get_collateral_score(&1u64), 5);
        
        // Advance time by 365 days (12 intervals)
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp + (2592000 * 12);
        });
        
        // Apply decay: should go to 0, not negative
        let new_score = client.decay_score(&1u64);
        assert_eq!(new_score, 0);
    }

    #[test]
    fn test_decay_score_callable_by_anyone() {
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
    fn test_maintenance_resets_decay_timer() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let asset_id = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Initial maintenance
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Maintenance"),
            &engineer,
        );
        
        assert_eq!(client.get_collateral_score(&1u64), 5);
        
        // Advance time by 15 days (half interval)
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp + 1296000;
        });
        
        // Do maintenance again - this resets the decay timer
        client.submit_maintenance(
            &1u64,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Maintenance"),
            &engineer,
        );
        
        assert_eq!(client.get_collateral_score(&1u64), 10);
        
        // Advance another 15 days (total 30 from first, but only 15 from second)
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp + 1296000;
        });
        
        // Apply decay - should not decay because only 15 days since last maintenance
        let new_score = client.decay_score(&1u64);
        assert_eq!(new_score, 10); // No decay yet
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
}
