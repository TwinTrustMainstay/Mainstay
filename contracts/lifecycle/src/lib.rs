#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, panic_with_error, symbol_short, Address, Env, String, Symbol, Vec};

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ContractError {
    NoMaintenanceHistory  = 1,
    UnauthorizedEngineer  = 2,
}

#[contracttype]
#[derive(Clone)]
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

fn last_update_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("LAST_UPD"), asset_id)
}

// Time-decay constants
const DECAY_INTERVAL: u64 = 2592000; // 30 days in seconds
const DECAY_RATE: u32 = 5; // Points to decay per interval

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

        let score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);
        let new_score = (score + 5).min(100);
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
    use soroban_sdk::{symbol_short, testutils::{Address as _, Events, Ledger}, BytesN, Env, String};

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

        for _ in 0..10 {
            client.submit_maintenance(
                &1u64,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "Routine oil change"),
                &engineer,
            );
        }

        assert_eq!(client.get_collateral_score(&1u64), 50);
        assert!(client.is_collateral_eligible(&1u64));
        assert_eq!(client.get_maintenance_history(&1u64).len(), 10);
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
        assert!(result.is_err());
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
    fn test_score_decay_after_30_days() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Submit maintenance to build up score
        for _ in 0..10 {
            client.submit_maintenance(
                &1u64,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "Maintenance"),
                &engineer,
            );
        }
        
        assert_eq!(client.get_collateral_score(&1u64), 50);
        
        // Advance time by 30 days (2592000 seconds)
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp + 2592000;
        });
        
        // Apply decay
        let new_score = client.decay_score(&1u64);
        assert_eq!(new_score, 45); // 50 - 5 = 45
        assert_eq!(client.get_collateral_score(&1u64), 45);
    }

    #[test]
    fn test_score_decay_multiple_intervals() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Build score to 100
        for _ in 0..20 {
            client.submit_maintenance(
                &1u64,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "Maintenance"),
                &engineer,
            );
        }
        
        assert_eq!(client.get_collateral_score(&1u64), 100);
        
        // Advance time by 90 days (3 intervals)
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp + (2592000 * 3);
        });
        
        // Apply decay: 3 intervals * 5 points = 15 points
        let new_score = client.decay_score(&1u64);
        assert_eq!(new_score, 85); // 100 - 15 = 85
    }

    #[test]
    fn test_score_decay_does_not_go_negative() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Build small score
        client.submit_maintenance(
            &1u64,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Maintenance"),
            &engineer,
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
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Build score
        for _ in 0..10 {
            client.submit_maintenance(
                &1u64,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "Maintenance"),
                &engineer,
            );
        }
        
        // Advance time
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp + 2592000;
        });
        
        // Anyone can call decay_score (no auth required)
        let new_score = client.decay_score(&1u64);
        assert_eq!(new_score, 45);
    }

    #[test]
    fn test_maintenance_resets_decay_timer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, eng_client) = setup(&env);
        
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        eng_client.register_engineer(&engineer, &hash, &issuer);

        // Initial maintenance
        client.submit_maintenance(
            &1u64,
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
}
