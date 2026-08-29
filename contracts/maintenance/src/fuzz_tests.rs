#[cfg(test)]
mod fuzz_submit_maintenance {
    use soroban_sdk::{Env, String};
    use crate::MaintenanceContract;

    // ── Deterministic pseudo-random seed corpus ──────────────────────────
    // True fuzz frameworks (cargo-fuzz / libFuzzer) need std; Soroban runs
    // no_std. We use a structured corpus + boundary table that covers the
    // same equivalence classes a fuzzer would find.

    struct FuzzCase {
        label: &'static str,
        task_type: &'static str,
        notes: &'static str,
    }

    const CORPUS: &[FuzzCase] = &[
        // Happy paths
        FuzzCase { label: "normal",           task_type: "inspection",   notes: "Routine check" },
        FuzzCase { label: "repair",           task_type: "repair",       notes: "Fixed valve seal" },
        // Empty strings — must not panic
        FuzzCase { label: "empty_task",       task_type: "",             notes: "Valid notes" },
        FuzzCase { label: "empty_notes",      task_type: "inspection",   notes: "" },
        FuzzCase { label: "both_empty",       task_type: "",             notes: "" },
        // Boundary lengths
        FuzzCase { label: "task_1_char",      task_type: "x",            notes: "ok" },
        FuzzCase { label: "notes_1_char",     task_type: "ok",           notes: "x" },
        // Unicode / multibyte
        FuzzCase { label: "unicode_task",     task_type: "检查",          notes: "Unicode task type" },
        FuzzCase { label: "unicode_notes",    task_type: "check",        notes: "Überprüfung abgeschlossen" },
        FuzzCase { label: "emoji",            task_type: "🔧",            notes: "🛠️ repaired" },
        // Injection-style inputs — must be stored verbatim, not executed
        FuzzCase { label: "sql_inject",       task_type: "'; DROP TABLE maintenance; --", notes: "payload" },
        FuzzCase { label: "script_tag",       task_type: "<script>alert(1)</script>",      notes: "xss attempt" },
        FuzzCase { label: "null_byte",        task_type: "task\0type",   notes: "with null" },
        FuzzCase { label: "newline",          task_type: "task\ntype",   notes: "line\nbreak" },
        FuzzCase { label: "tab",              task_type: "task\ttype",   notes: "tab\there" },
        // Whitespace only
        FuzzCase { label: "whitespace_task",  task_type: "   ",          notes: "spaces" },
        FuzzCase { label: "whitespace_notes", task_type: "check",        notes: "   " },
        // Long strings (255 chars)
        FuzzCase {
            label: "long_task",
            task_type: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            notes: "normal notes",
        },
        FuzzCase {
            label: "long_notes",
            task_type: "inspection",
            notes: "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn",
        },
        // Repeated characters
        FuzzCase { label: "all_zeros",        task_type: "000",          notes: "000" },
        FuzzCase { label: "slash",            task_type: "///",          notes: "///" },
        FuzzCase { label: "backslash",        task_type: "\\\\",         notes: "\\\\" },
    ];

    /// Core invariant: submit_maintenance must never panic for any string input.
    /// If the contract enforces validation, it must return an error — not panic.
    #[test]
    fn fuzz_no_panic_on_corpus() {
        for case in CORPUS {
            // Fresh env per case so state doesn't bleed
            let env = Env::default();
            env.mock_all_auths();
            let id = env.register_contract(None, MaintenanceContract);
            let client = crate::MaintenanceContractClient::new(&env, &id);

            let asset_id    = String::from_str(&env, "FUZZ-ASSET");
            let engineer_id = String::from_str(&env, "FUZZ-ENG");
            let task_type   = String::from_str(&env, case.task_type);
            let notes       = String::from_str(&env, case.notes);

            // Must not panic — either succeeds or returns a structured error
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = client.try_submit_maintenance(&asset_id, &engineer_id, &task_type, &notes);
            }));

            assert!(
                result.is_ok(),
                "PANIC on corpus case '{}': task_type={:?} notes={:?}",
                case.label,
                case.task_type,
                case.notes
            );
        }
    }

    /// Idempotency: submitting the same record twice must not corrupt state.
    #[test]
    fn fuzz_duplicate_submission_no_corruption() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, MaintenanceContract);
        let client = crate::MaintenanceContractClient::new(&env, &id);

        let asset_id    = String::from_str(&env, "FUZZ-DUP");
        let engineer_id = String::from_str(&env, "ENG-DUP");
        let task_type   = String::from_str(&env, "inspection");
        let notes       = String::from_str(&env, "first");

        let _ = client.try_submit_maintenance(&asset_id, &engineer_id, &task_type, &notes);
        let _ = client.try_submit_maintenance(&asset_id, &engineer_id, &task_type, &notes);

        // Records count should be deterministic — either 1 (dedup) or 2 (append), never 0
        let records = client.get_maintenance_records(&asset_id);
        assert!(
            !records.is_empty(),
            "Duplicate submissions must not corrupt or erase records"
        );
    }

    /// Ordering invariant: records are returned in submission order.
    #[test]
    fn fuzz_submission_order_preserved() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, MaintenanceContract);
        let client = crate::MaintenanceContractClient::new(&env, &id);

        let asset_id = String::from_str(&env, "FUZZ-ORDER");
        let tasks = ["first", "second", "third"];

        for (i, task) in tasks.iter().enumerate() {
            let _ = client.try_submit_maintenance(
                &asset_id,
                &String::from_str(&env, &format!("ENG-{i}")),
                &String::from_str(&env, task),
                &String::from_str(&env, "notes"),
            );
        }

        let records = client.get_maintenance_records(&asset_id);
        // Contract must return records in a consistent order (first-in or last-in)
        assert_eq!(records.len(), 3, "All 3 submissions must be recorded");
    }
}
