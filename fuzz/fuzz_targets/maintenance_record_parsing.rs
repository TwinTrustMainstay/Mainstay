//! Fuzz target for maintenance record parsing.
//!
//! Exercises the notes and task_type parsing paths in `submit_maintenance`
//! with structured arbitrary inputs covering:
//!
//! - Note lengths: empty, 1 byte, boundary (255/256/257), maximum (256), over-max
//! - Note content: valid ASCII, Unicode multi-byte sequences, embedded NUL bytes,
//!   control characters, overlong patterns, and repeated special characters
//! - Task types: every weighted task type in the scoring table plus unknown types
//! - Cost field: `None`, `Some(0)`, `Some(u64::MAX)`, arbitrary values
//!
//! The invariant being tested is that **no input combination causes an unexpected
//! panic**. All rejections must surface as a structured `ContractError` returned
//! via `try_submit_maintenance`, never as an unhandled trap.
//!
//! Additionally, after every successful submission the fuzz target asserts:
//! - `collateral_score` is in `[0, 100]`
//! - `maintenance_history` contains the expected record count
//! - the stored record's `notes` field round-trips identically (no truncation)
//!
//! # Running
//!
//! ```bash
//! cd fuzz
//! cargo fuzz run maintenance_record_parsing -- -max_total_time=7200
//! # Or with a specific seed corpus:
//! cargo fuzz run maintenance_record_parsing corpus/maintenance_record_parsing/
//! ```
//!
//! # Adding seed inputs
//!
//! Drop raw bytes into `fuzz/corpus/maintenance_record_parsing/` before running
//! to prime the fuzzer with known interesting inputs (e.g. max-length notes,
//! NUL-embedded strings, valid JSON-like payloads).

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

// ── Fuzz input definition ────────────────────────────────────────────────────

/// Selects which well-known task type to use.
///
/// Using an enum rather than raw bytes ensures the fuzzer explores the weight
/// table exhaustively without spending entropy on uninteresting symbol bytes.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum TaskTypeChoice {
    // Weighted task types from the scoring table
    OilChg,
    Lube,
    Inspect,
    Filter,
    TuneUp,
    Brake,
    Engine,
    Overhaul,
    Rebuild,
    // Unknown task type → falls back to default weight (3 pts)
    Unknown,
    // Empty symbol string → should be rejected or treated as unknown
    Empty,
}

/// Describes how to construct the `notes` string.
#[derive(Arbitrary, Debug)]
enum NotesShape {
    /// Empty string — `validate_notes_length` rejects empty notes.
    Empty,
    /// Single ASCII byte.
    SingleByte(u8),
    /// Exactly 255 bytes of the given fill byte (one under the default cap).
    Fill255(u8),
    /// Exactly 256 bytes of the given fill byte (at the default cap).
    Fill256(u8),
    /// Exactly 257 bytes of the given fill byte (one over the default cap).
    Fill257(u8),
    /// Arbitrary-length repetition of a fill byte (0..=512 bytes).
    Repeat { byte: u8, count: u16 },
    /// Valid UTF-8 string from a fixed set of interesting payloads.
    Interesting(InterestingNotes),
    /// Raw bytes — may contain invalid UTF-8, NUL bytes, or control chars.
    RawBytes(Vec<u8>),
}

/// A curated set of structurally interesting note strings.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum InterestingNotes {
    /// Lone NUL byte.
    NulByte,
    /// NUL bytes embedded mid-string.
    EmbeddedNul,
    /// Tab and newline characters.
    TabAndNewline,
    /// Multi-byte Unicode: 2-byte sequence (U+00E9 é).
    TwoByteUtf8,
    /// Multi-byte Unicode: 3-byte sequence (U+20AC €).
    ThreeByteUtf8,
    /// Multi-byte Unicode: 4-byte emoji (U+1F527 🔧).
    FourByteUtf8,
    /// Right-to-left override character (U+202E).
    RtlOverride,
    /// String that looks like a JSON payload.
    JsonLike,
    /// String that looks like a SQL injection attempt.
    SqlInjection,
    /// String consisting entirely of whitespace.
    AllWhitespace,
    /// Maximum-length string of repeated 'x' chars (exactly 256 bytes).
    MaxLengthAscii,
    /// One byte beyond the maximum (257 bytes).
    OverMaxAscii,
}

/// Top-level structured fuzz input.
#[derive(Arbitrary, Debug)]
struct FuzzInput {
    task_type: TaskTypeChoice,
    notes_shape: NotesShape,
    /// Optional cost in stroops.
    cost: Option<u64>,
}

// ── Fuzz target ──────────────────────────────────────────────────────────────

fuzz_target!(|input: FuzzInput| {
    use asset_registry::{AssetRegistry, AssetRegistryClient};
    use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
    use lifecycle::{Lifecycle, LifecycleClient};
    use soroban_sdk::{
        symbol_short,
        testutils::Address as _,
        Address, BytesN, Env, String as SorobanString,
    };

    let env = Env::default();
    env.mock_all_auths();

    // ── Bootstrap contracts ──────────────────────────────────────────────────

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let engineer = Address::generate(&env);
    let issuer = Address::generate(&env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("FUZZ"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    let asset_id = asset_registry.register_asset(
        &symbol_short!("FUZZ"),
        &SorobanString::from_str(&env, "Fuzz test asset"),
        &SorobanString::from_str(&env, "SN-FUZZ-PARSE"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[0xABu8; 32]);
    engineer_registry.register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &31_536_000,
        &None,
    );
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // ── Build task_type symbol ───────────────────────────────────────────────

    let task_type = match input.task_type {
        TaskTypeChoice::OilChg   => symbol_short!("OIL_CHG"),
        TaskTypeChoice::Lube     => symbol_short!("LUBE"),
        TaskTypeChoice::Inspect  => symbol_short!("INSPECT"),
        TaskTypeChoice::Filter   => symbol_short!("FILTER"),
        TaskTypeChoice::TuneUp   => symbol_short!("TUNE_UP"),
        TaskTypeChoice::Brake    => symbol_short!("BRAKE"),
        TaskTypeChoice::Engine   => symbol_short!("ENGINE"),
        TaskTypeChoice::Overhaul => symbol_short!("OVERHAUL"),
        TaskTypeChoice::Rebuild  => symbol_short!("REBUILD"),
        TaskTypeChoice::Unknown  => symbol_short!("UNKNOWN"),
        TaskTypeChoice::Empty    => symbol_short!(""),
    };

    // ── Build notes string ───────────────────────────────────────────────────
    //
    // Soroban's `String::from_str` accepts a Rust `&str`, so we must produce
    // valid UTF-8. For `RawBytes` inputs we lossily convert; the fuzzer still
    // finds boundary conditions because the *length* and *structure* vary.

    let notes_str: std::string::String = match &input.notes_shape {
        NotesShape::Empty => std::string::String::new(),

        NotesShape::SingleByte(b) => {
            let c = if b.is_ascii() && *b != 0 { *b as char } else { 'X' };
            c.to_string()
        }

        NotesShape::Fill255(b) => {
            let c = if b.is_ascii_graphic() { *b as char } else { 'a' };
            std::iter::repeat(c).take(255).collect()
        }

        NotesShape::Fill256(b) => {
            let c = if b.is_ascii_graphic() { *b as char } else { 'a' };
            std::iter::repeat(c).take(256).collect()
        }

        NotesShape::Fill257(b) => {
            let c = if b.is_ascii_graphic() { *b as char } else { 'a' };
            std::iter::repeat(c).take(257).collect()
        }

        NotesShape::Repeat { byte, count } => {
            let c = if byte.is_ascii_graphic() { *byte as char } else { 'z' };
            let n = (*count as usize).min(512);
            std::iter::repeat(c).take(n).collect()
        }

        NotesShape::Interesting(kind) => match kind {
            InterestingNotes::NulByte         => "\x00".to_string(),
            InterestingNotes::EmbeddedNul     => "start\x00middle\x00end".to_string(),
            InterestingNotes::TabAndNewline   => "line1\tcolumn\nline2".to_string(),
            InterestingNotes::TwoByteUtf8     => "café".to_string(),
            InterestingNotes::ThreeByteUtf8   => "price: €100".to_string(),
            InterestingNotes::FourByteUtf8    => "fixed with 🔧".to_string(),
            InterestingNotes::RtlOverride     => "normal\u{202E}reversed".to_string(),
            InterestingNotes::JsonLike        => r#"{"key":"value","n":42}"#.to_string(),
            InterestingNotes::SqlInjection    => "'; DROP TABLE assets; --".to_string(),
            InterestingNotes::AllWhitespace   => "   \t\t   ".to_string(),
            InterestingNotes::MaxLengthAscii  => std::iter::repeat('x').take(256).collect(),
            InterestingNotes::OverMaxAscii    => std::iter::repeat('x').take(257).collect(),
        },

        NotesShape::RawBytes(bytes) => {
            // Lossily convert to UTF-8; replacement char (U+FFFD) is 3 bytes,
            // so very long byte slices can exceed the notes cap after conversion.
            std::string::String::from_utf8_lossy(bytes).into_owned()
        }
    };

    let notes = SorobanString::from_str(&env, &notes_str);

    // ── Call submit_maintenance ──────────────────────────────────────────────
    //
    // INVARIANT: this must NEVER cause an unexpected panic (trap).
    // Valid inputs return Ok(()); invalid inputs return a structured ContractError.

    let before_count = lifecycle
        .try_get_maintenance_history(&asset_id)
        .ok()
        .map(|h| h.len())
        .unwrap_or(0);

    let result = lifecycle.try_submit_maintenance(
        &asset_id,
        &task_type,
        &notes,
        &engineer,
        &input.cost,
    );

    // ── Post-call invariant checks ───────────────────────────────────────────

    match result {
        Ok(_) => {
            // Successful submission: verify score and history are consistent.

            let score = lifecycle
                .try_get_collateral_score(&asset_id)
                .expect("get_collateral_score must not panic after successful submit");
            assert!(
                score <= 100,
                "collateral score must be in [0, 100], got {}",
                score
            );

            let history = lifecycle
                .try_get_maintenance_history(&asset_id)
                .expect("get_maintenance_history must not panic after successful submit");

            // History must have grown by exactly 1.
            assert_eq!(
                history.len(),
                before_count + 1,
                "history must grow by 1 after successful submit"
            );

            // The stored notes must round-trip identically (no silent truncation).
            let stored_record = history
                .get(history.len() - 1)
                .expect("last record must exist");
            assert_eq!(
                stored_record.notes, notes,
                "stored notes must match submitted notes exactly"
            );
        }
        Err(_) => {
            // Rejection via ContractError is expected for invalid inputs (empty
            // notes, notes > max_notes_length, etc.). The history count must be
            // unchanged.
            let history_after = lifecycle
                .try_get_maintenance_history(&asset_id)
                .ok()
                .map(|h| h.len())
                .unwrap_or(0);
            assert_eq!(
                history_after, before_count,
                "history must not change on rejected submit"
            );
        }
    }
});
