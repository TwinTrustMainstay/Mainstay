#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, panic_with_error, symbol_short, Address, Env, String, Symbol, Vec};

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ContractError {
    NoMaintenanceHistory  = 1,
    UnauthorizedEngineer  = 2,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct MaintenanceRecord {
    pub asset_id: u64,
    pub task_type: Symbol,
    pub notes: String,
    pub engineer: Address,
    pub timestamp: u64,
}

const ENG_REGISTRY: Symbol = symbol_short!("ENG_REG");

fn history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("HIST"), asset_id)
}

fn score_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("SCORE"), asset_id)
}

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
    #[contractclient(name = "EngineerRegistryClient")]
    pub trait EngineerRegistry {
        fn verify_engineer(env: Env, engineer: Address) -> bool;
    }
}

#[contract]
pub struct Lifecycle;

#[contractimpl]
impl Lifecycle {
    /// Must be called once after deployment to bind the engineer registry.
    pub fn initialize(env: Env, engineer_registry: Address) {
        env.storage().instance().set(&ENG_REGISTRY, &engineer_registry);
    }

    pub fn submit_maintenance(
        env: Env,
        asset_id: u64,
        task_type: Symbol,
        notes: String,
        engineer: Address,
    ) {
        engineer.require_auth();

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
        env.storage().persistent().set(&history_key(asset_id), &history);
        
        // Extend TTL for persistent storage entries to prevent data loss
        env.storage().persistent().extend_ttl(&history_key(asset_id), 518400, 518400); // 30 days

        let score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);
        let weight = get_task_weight(&env, &task_type);
        let new_score = (score + weight).min(100);
        env.storage().persistent().set(&score_key(asset_id), &new_score);
        
        // Extend TTL for persistent storage entries to prevent data loss
        env.storage().persistent().extend_ttl(&score_key(asset_id), 518400, 518400); // 30 days
        
        // Emit maintenance submission event
        env.events().publish(
            (symbol_short!("MAINT"), asset_id),
            (task_type, engineer, env.ledger().timestamp())
        );
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
        history.last().unwrap_or_else(|| panic_with_error!(&env, ContractError::NoMaintenanceHistory))
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
    use soroban_sdk::{symbol_short, testutils::{Address as _, Events}, BytesN, Env, String};

    mod engineer_registry_contract {
        soroban_sdk::contractimport!(
            file = "../../target/wasm32-unknown-unknown/release/engineer_registry.wasm"
        );
        pub type EngineerRegistryClient<'a> = Client<'a>;
    }

    fn setup(env: &Env) -> (LifecycleClient, engineer_registry_contract::EngineerRegistryClient) {
        let eng_reg_id = env.register(engineer_registry_contract::WASM, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let lifecycle = LifecycleClient::new(env, &lifecycle_id);
        lifecycle.initialize(&eng_reg_id);
        (lifecycle, engineer_registry_contract::EngineerRegistryClient::new(env, &eng_reg_id))
    }

    #[test]
    fn test_submit_and_score() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
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
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Mix of different task types
        client.submit_maintenance(&1u64, &symbol_short!("OIL_CHG"), &String::from_str(&env, "Oil change"), &engineer); // +2 = 2
        client.submit_maintenance(&1u64, &symbol_short!("FILTER"), &String::from_str(&env, "Filter"), &engineer); // +5 = 7
        client.submit_maintenance(&1u64, &symbol_short!("ENGINE"), &String::from_str(&env, "Engine work"), &engineer); // +10 = 17
        client.submit_maintenance(&1u64, &symbol_short!("LUBE"), &String::from_str(&env, "Lubrication"), &engineer); // +2 = 19

        assert_eq!(client.get_collateral_score(&1u64), 19);
    }

    #[test]
    fn test_score_cap_at_100() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Submit enough major tasks to exceed 100
        for _ in 0..12 {
            client.submit_maintenance(&1u64, &symbol_short!("ENGINE"), &String::from_str(&env, "Engine work"), &engineer);
        }

        // Score should be capped at 100
        assert_eq!(client.get_collateral_score(&1u64), 100);
    }

    #[test]
    fn test_unregistered_engineer_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let unregistered = Address::generate(&env);
        let result = client.try_submit_maintenance(
            &1u64,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Should fail"),
            &unregistered,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32
            )))
        );
    }

    #[test]
    fn test_get_last_service_no_history() {
        let env = Env::default();
        let contract_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &contract_id);
        let result = client.try_get_last_service(&999u64);
        assert!(result.is_err());
    }

    #[test]
    fn test_submit_maintenance_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        client.submit_maintenance(
            &1u64,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Routine maintenance"),
            &engineer,
        );

        // Verify maintenance event was emitted
        let events = env.events().all();
        assert!(events.len() > 0);
    }

    #[test]
    fn test_ttl_extended_on_maintenance_submission() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        let asset_id = 1u64;
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Routine maintenance"),
            &engineer,
        );

        // Verify TTL is set for history storage entry
        let history_ttl = env.storage().persistent().get_ttl(&history_key(asset_id));
        assert!(history_ttl > 0, "History TTL should be extended");

        // Verify TTL is set for score storage entry
        let score_ttl = env.storage().persistent().get_ttl(&score_key(asset_id));
        assert!(score_ttl > 0, "Score TTL should be extended");
    }
}
