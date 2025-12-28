//! Integration tests for WASM module execution
//!
//! These tests use the compiled simple-validator.wasm example module to test:
//! - Validation callback execution (accept/reject)
//! - Event callback execution
//! - Context modifications (session + transaction)
//! - Error handling and edge cases

use std::{
    borrow::Cow,
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime},
};

use ahash::AHashMap;
use empath_common::{
    address::{Address, AddressList},
    address_parser::parse_forward_path,
    context::Context,
    envelope::Envelope,
    message_id::SpooledMessageId,
};
use empath_wasm::{DeterministicWasiConfig, WasmError, WasmModule};
use proptest::{prelude::*, test_runner::TestCaseError};
use tokio::runtime::Runtime;

type TestResult = Result<(), String>;

fn require_result<T, E: std::fmt::Debug>(res: Result<T, E>, context: &str) -> Result<T, String> {
    res.map_err(|err| format!("{context}: {err:?}"))
}

fn require_option<T>(opt: Option<T>, context: &str) -> Result<T, String> {
    opt.ok_or_else(|| context.to_string())
}

/// Compute the profile directory (debug/release) Cargo is building with
fn cargo_profile() -> String {
    env::var("PROFILE").unwrap_or_else(|_| "debug".to_string())
}

/// Location of the compiled test WASM module, reusing cached artifacts when available and only
/// invoking `cargo run -p xtask -- build-wasm` if nothing fresh is present.
fn workspace_root() -> Result<PathBuf, String> {
    require_option(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(Path::to_path_buf),
        "workspace root",
    )
}

fn resolved_module_path() -> Result<&'static Path, String> {
    static MODULE_PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();

    let result = MODULE_PATH.get_or_init(|| {
        let workspace_root = workspace_root()?;

        let profile = cargo_profile();

        let cached_path = workspace_root
            .join("target/modules")
            .join("simple_validator.wasm");

        let source_mtime = latest_source_mtime(&workspace_root);

        if module_fresh(&cached_path, source_mtime) {
            return Ok(cached_path);
        }

        require_result(
            build_wasm_example(&workspace_root, &profile),
            "failed to build wasm example via xtask",
        )?;

        if !cached_path.exists() {
            return Err(format!(
                "expected WASM test module at {} after xtask build, build may have failed",
                cached_path.display()
            ));
        }

        Ok(cached_path)
    });

    match result {
        Ok(path) => Ok(path.as_path()),
        Err(err) => Err(err.clone()),
    }
}

fn build_wasm_example(workspace_root: &Path, profile: &str) -> Result<(), String> {
    let status = Command::new("cargo")
        .current_dir(workspace_root)
        .args(["run", "-p", "xtask", "--", "build-wasm"])
        .env("PROFILE", profile)
        .status()
        .map_err(|e| format!("failed to run xtask build-wasm: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("xtask build-wasm exited with status {status}"))
    }
}

fn latest_source_mtime(workspace_root: &Path) -> Option<SystemTime> {
    let mut times = Vec::new();

    for path in [
        workspace_root.join("empath-examples/wasm/src/lib.rs"),
        workspace_root.join("empath-examples/wasm/Cargo.toml"),
        workspace_root.join("empath-wasm/wit/empath.wit"),
        workspace_root.join("empath-wasm-guest/wit/empath.wit"),
        workspace_root.join("empath-examples/wasm/wit/empath.wit"),
    ] {
        if let Ok(meta) = std::fs::metadata(path) {
            times.push(meta.modified().ok()?);
        }
    }

    times.into_iter().max()
}

fn module_fresh(wasm_path: &Path, source_mtime: Option<SystemTime>) -> bool {
    let Ok(wasm_meta) = std::fs::metadata(wasm_path) else {
        return false;
    };

    let Ok(wasm_mtime) = wasm_meta.modified() else {
        return false;
    };

    source_mtime.is_none_or(|src_mtime| wasm_mtime >= src_mtime)
}

fn load_test_module() -> Result<WasmModule, String> {
    let rt = runtime()?;
    let _guard = rt.enter();
    let path = resolved_module_path()?;
    let module = WasmModule::load(path)
        .map(|module| module.with_deterministic_wasi(DeterministicWasiConfig::default()));

    match module {
        Ok(module) => Ok(module),
        Err(err) => {
            let profile = cargo_profile();
            let root = workspace_root()?;
            require_result(
                build_wasm_example(&root, &profile),
                "failed to rebuild wasm example via xtask",
            )?;

            require_result(
                WasmModule::load(path).map(|module| {
                    module.with_deterministic_wasi(DeterministicWasiConfig::default())
                }),
                &format!("Failed to load test module after rebuild: {err:?}"),
            )
        }
    }
}

fn runtime() -> Result<&'static Runtime, String> {
    static RT: OnceLock<Result<Runtime, String>> = OnceLock::new();
    let result = RT.get_or_init(|| {
        require_result(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build(),
            "tokio runtime for wasm tests",
        )
    });

    match result {
        Ok(runtime) => Ok(runtime),
        Err(err) => Err(err.clone()),
    }
}

fn init_test_module() -> Result<WasmModule, String> {
    let module = load_test_module()?;
    require_result(
        module.initialize(),
        "Failed to initialize module during test setup",
    )?;
    Ok(module)
}

fn init_test_module_with_timeout(timeout: Duration) -> Result<WasmModule, String> {
    let module = load_test_module()?.with_timeout(timeout);
    require_result(
        module.initialize(),
        "Failed to initialize module during test setup",
    )?;
    Ok(module)
}

fn init_test_module_with_memory_limit(max_memory_bytes: usize) -> Result<WasmModule, String> {
    let module = load_test_module()?.with_max_memory(max_memory_bytes);
    require_result(
        module.initialize(),
        "Failed to initialize module during test setup",
    )?;
    Ok(module)
}

fn fuzz_module() -> Result<&'static WasmModule, String> {
    static MODULE: OnceLock<Result<WasmModule, String>> = OnceLock::new();
    let result = MODULE.get_or_init(init_test_module);
    match result {
        Ok(module) => Ok(module),
        Err(err) => Err(err.clone()),
    }
}

/// Create a test context with sensible defaults
fn create_test_context() -> Context {
    Context {
        envelope: Arc::new(Envelope::default()),
        data_len: None,
        data: None,
        id: "test-session-123".to_string(),
        extended: false,
        session: AHashMap::new(),
        transaction: AHashMap::new(),
        response: None,
        banner: Arc::from("test.example.com"),
        max_message_size: 10_000_000,
        capabilities: Vec::new(),
        delivery: None,
        tracking_id: None,
        message_parts: None,
        message_dirty: false,
        module_context: None,
    }
}

fn regex_strategy(pattern: &str, fallback: &str) -> BoxedStrategy<String> {
    prop::string::string_regex(pattern).map_or_else(
        |_| Just(fallback.to_string()).boxed(),
        proptest::strategy::Strategy::boxed,
    )
}

fn session_id_strategy() -> BoxedStrategy<String> {
    regex_strategy("[a-z0-9-]{1,24}", "session-1")
}

fn local_part_strategy() -> BoxedStrategy<String> {
    regex_strategy("[a-z0-9]{1,16}", "local")
}

fn domain_label_strategy() -> BoxedStrategy<String> {
    regex_strategy("[a-z]{3,10}", "example")
}

fn domain_strategy() -> BoxedStrategy<String> {
    (domain_label_strategy(), regex_strategy("[a-z]{2,6}", "com"))
        .prop_map(|(label, tld)| format!("{label}.{tld}"))
        .boxed()
}

fn mailbox_strategy() -> BoxedStrategy<String> {
    (local_part_strategy(), domain_strategy())
        .prop_map(|(local, domain)| format!("<{local}@{domain}>"))
        .boxed()
}

fn address_strategy() -> BoxedStrategy<Address> {
    mailbox_strategy()
        .prop_filter_map("valid mailbox", |path| {
            parse_forward_path(&path).ok().map(Address::from)
        })
        .boxed()
}

fn sender_strategy() -> BoxedStrategy<Option<Address>> {
    prop_oneof![Just(None), address_strategy().prop_map(Some)].boxed()
}

fn recipients_strategy() -> BoxedStrategy<Vec<Address>> {
    prop::collection::vec(address_strategy(), 0..=12).boxed()
}

fn transaction_key_strategy() -> BoxedStrategy<String> {
    regex_strategy("[a-z_]{1,12}", "key")
        .prop_filter("avoid panic_event", |key| key != "panic_event")
        .boxed()
}

fn transaction_entries_strategy() -> BoxedStrategy<Vec<(String, String)>> {
    prop::collection::vec(
        (
            transaction_key_strategy(),
            regex_strategy("[a-z0-9_-]{0,24}", "value"),
        ),
        0..=8,
    )
    .boxed()
}

fn message_data_strategy() -> BoxedStrategy<Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=2048).boxed()
}

fn sanitize_session_id(mut session_id: String) -> String {
    if session_id.starts_with("panic-event") {
        session_id.insert_str(0, "safe-");
    }
    session_id
}

fn sanitize_message_data(mut data: Vec<u8>) -> Vec<u8> {
    if data.starts_with(b"timeout") || data.starts_with(b"alloc:") {
        if data.is_empty() {
            data.push(b'x');
        } else {
            data[0] = b'x';
        }
    }
    data
}

struct FuzzContextInput {
    session_id: String,
    sender: Option<Address>,
    recipients: Vec<Address>,
    extended: bool,
    data: Vec<u8>,
    max_message_size: usize,
    transaction_entries: Vec<(String, String)>,
    include_tracking: bool,
}

fn build_fuzz_context(input: FuzzContextInput) -> Context {
    let mut envelope = Envelope::default();
    *envelope.sender_mut() = input.sender;
    if !input.recipients.is_empty() {
        *envelope.recipients_mut() = Some(AddressList::from(input.recipients));
    }

    let mut ctx = create_test_context();
    ctx.id = sanitize_session_id(input.session_id);
    ctx.extended = input.extended;
    ctx.envelope = Arc::new(envelope);
    let data = sanitize_message_data(input.data);
    ctx.data_len = Some(data.len());
    ctx.data = Some(Arc::from(data));
    ctx.max_message_size = input.max_message_size;
    ctx.transaction = input
        .transaction_entries
        .into_iter()
        .map(|(key, value)| (Cow::Owned(key), value))
        .collect();
    ctx.tracking_id = input.include_tracking.then(SpooledMessageId::generate);
    ctx
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_load_nonexistent_module() {
    let result = WasmModule::load("/nonexistent/path.wasm");
    assert!(result.is_err());
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_load_valid_module() -> TestResult {
    // Ensure epoch task has a runtime by entering it during load
    let rt = runtime()?;
    let _guard = rt.enter();
    let result = WasmModule::load(resolved_module_path()?);
    assert!(
        result.is_ok(),
        "Failed to load test module: {:?}",
        result.err()
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_module_initialization() -> TestResult {
    let module = load_test_module()?;
    let init_result = module.initialize();
    assert!(
        init_result.is_ok(),
        "Module initialization failed: {:?}",
        init_result.err()
    );
    Ok(())
}

// --- Validation Callback Tests ---

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_connect_accept() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let result = require_result(
        module.execute_callback("validate_connect", &mut ctx),
        "Callback execution failed",
    )?;
    assert_eq!(result, 0, "Expected Accept (0)");

    // Check that session context was set
    assert_eq!(
        ctx.session.get(&Cow::Borrowed("connection_validated")),
        Some(&"true".to_string()),
        "Session context not set"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_ehlo_accept() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    ctx.extended = true;

    let result = require_result(
        module.execute_callback("validate_ehlo", &mut ctx),
        "validate_ehlo failed",
    )?;
    assert_eq!(result, 0, "Expected Accept");
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_mail_from_accept() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    let sender_mailbox = require_result(
        parse_forward_path("<user@example.com>"),
        "Failed to parse sender",
    )?;
    *envelope.sender_mut() = Some(Address::from(sender_mailbox));
    ctx.envelope = Arc::new(envelope);

    let result = require_result(
        module.execute_callback("validate_mail_from", &mut ctx),
        "validate_mail_from failed",
    )?;
    assert_eq!(result, 0, "Expected Accept");

    // Check transaction context
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("validated_sender")),
        Some(&"user@example.com".to_string()),
        "Sender not stored in transaction context"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_mail_from_reject_spam_domain() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    let sender_mailbox = require_result(
        parse_forward_path("<spammer@spam.example.com>"),
        "Failed to parse sender",
    )?;
    *envelope.sender_mut() = Some(Address::from(sender_mailbox));
    ctx.envelope = Arc::new(envelope);

    let result = require_result(
        module.execute_callback("validate_mail_from", &mut ctx),
        "validate_mail_from failed",
    )?;
    assert_eq!(result, 550, "Expected Reject with 550");

    // Check rejection reason in transaction context
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("rejection_reason")),
        Some(&"sender_blocked_spam_domain".to_string()),
        "Rejection reason not stored"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_rcpt_to_accept() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    let recipients = vec![
        Address::from(require_result(
            parse_forward_path("<user1@example.com>"),
            "valid recipient",
        )?),
        Address::from(require_result(
            parse_forward_path("<user2@example.com>"),
            "valid recipient",
        )?),
    ];
    *envelope.recipients_mut() = Some(AddressList::from(recipients));
    ctx.envelope = Arc::new(envelope);

    let result = require_result(
        module.execute_callback("validate_rcpt_to", &mut ctx),
        "validate_rcpt_to failed",
    )?;
    assert_eq!(result, 0, "Expected Accept");

    // Check recipient count tracking
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("recipient_count")),
        Some(&"2".to_string()),
        "Recipient count not tracked"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_rcpt_to_reject_too_many() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    // Add 11 recipients (exceeds limit of 10)
    let mut recipients = Vec::new();
    for i in 1..=11 {
        let mailbox = require_result(
            parse_forward_path(&format!("<user{i}@example.com>")),
            "valid recipient",
        )?;
        recipients.push(Address::from(mailbox));
    }
    *envelope.recipients_mut() = Some(AddressList::from(recipients));
    ctx.envelope = Arc::new(envelope);

    let result = require_result(
        module.execute_callback("validate_rcpt_to", &mut ctx),
        "validate_rcpt_to failed",
    )?;
    assert_eq!(result, 452, "Expected Reject with 452");

    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("rejection_reason")),
        Some(&"too_many_recipients".to_string())
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_data_accept() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(b"Subject: Test\r\n\r\nHello world!".to_vec()));
    let expected_size = require_option(ctx.data.as_ref().map(|d| d.len()), "message data missing")?;

    let result = require_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    )?;
    assert_eq!(result, 0, "Expected Accept");

    // Check message size tracking
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("message_size")),
        Some(&expected_size.to_string()),
        "Message size not tracked"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_data_reject_too_large() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    // Create message larger than 1MB
    let large_data = vec![b'A'; 1_000_001];
    ctx.data = Some(Arc::from(large_data));

    let result = require_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    )?;
    assert_eq!(result, 552, "Expected Reject with 552");

    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("rejection_reason")),
        Some(&"message_too_large".to_string())
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_data_reject_spam_content() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(
        b"Subject: Special offer\r\n\r\nBuy cheap viagra now!".to_vec(),
    ));

    let result = require_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    )?;
    assert_eq!(result, 550, "Expected Reject with 550 for spam");

    // Check spam keyword was recorded
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("spam_keyword")),
        Some(&"viagra".to_string())
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_data_timeout() -> TestResult {
    let module = init_test_module_with_timeout(Duration::from_millis(10))?;

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(b"timeout-trigger".to_vec()));

    let result = module.execute_callback("validate_data", &mut ctx);

    let err = match result {
        Ok(value) => return Err(format!("Expected validate_data to time out, got {value}")),
        Err(err) => err,
    };

    match err {
        WasmError::Timeout(callback, _) => assert_eq!(callback, "validate_data"),
        other => return Err(format!("Expected timeout error, got {other:?}")),
    }
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_data_memory_limit() -> TestResult {
    // Set a low memory limit (just above module minimum) to force failure on growth
    let module = init_test_module_with_memory_limit(1_200_000)?;

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(b"alloc:3000000".to_vec())); // ~3 MB allocation request

    let result = module.execute_callback("validate_data", &mut ctx);
    let err = match result {
        Ok(value) => {
            return Err(format!(
                "Expected validate_data to hit memory limit, got {value}"
            ));
        }
        Err(err) => err,
    };

    match err {
        WasmError::Wasmtime(_) => {}
        other => {
            return Err(format!(
                "Expected Wasmtime error for memory limit, got {other:?}"
            ));
        }
    }
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_event_error_path_returns_err() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    ctx.id = "panic-event-123".to_string();

    let result = module.execute_callback("on_delivery_failure", &mut ctx);
    assert!(result.is_err(), "Expected event callback to error");
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_context_survives_event_error() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    ctx.id = "panic-event-ctx".to_string();
    ctx.transaction
        .insert(Cow::Borrowed("panic_event"), "true".to_string());
    ctx.session
        .insert(Cow::Borrowed("existing_session"), "keep".to_string());

    let result = module.execute_callback("on_connection_opened", &mut ctx);
    assert!(result.is_err(), "Expected event callback to error");

    // Context maps should remain accessible after error
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("panic_event")),
        Some(&"true".to_string())
    );
    assert_eq!(
        ctx.session.get(&Cow::Borrowed("existing_session")),
        Some(&"keep".to_string())
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_validate_starttls_accept() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let result = module.execute_callback("validate_starttls", &mut ctx);
    let value = require_result(result, "validate_starttls failed")?;
    assert_eq!(value, 0, "Expected Accept");
    Ok(())
}

// --- Event Callback Tests ---

#[test]
#[cfg_attr(miri, ignore)]
fn test_on_connection_opened() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let result = require_result(
        module.execute_callback("on_connection_opened", &mut ctx),
        "Event callback failed",
    )?;
    assert_eq!(result, 0, "Event callbacks should return 0");

    // Check session context was set
    assert!(
        ctx.session
            .contains_key(&Cow::Borrowed("connection_opened_at")),
        "Event callback did not set session context"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_on_connection_closed() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    // Simulate previous connection_opened event
    ctx.session.insert(
        Cow::Borrowed("connection_opened_at"),
        "test-session-123".to_string(),
    );

    let result = require_result(
        module.execute_callback("on_connection_closed", &mut ctx),
        "Event callback failed",
    )?;
    assert_eq!(result, 0);
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_on_delivery_attempt() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    // Add spam keyword from previous validation
    ctx.transaction
        .insert(Cow::Borrowed("spam_keyword"), "test".to_string());

    let result = require_result(
        module.execute_callback("on_delivery_attempt", &mut ctx),
        "Event callback failed",
    )?;
    assert_eq!(result, 0);
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_on_delivery_success() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let result = require_result(
        module.execute_callback("on_delivery_success", &mut ctx),
        "Event callback failed",
    )?;
    assert_eq!(result, 0);
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_on_delivery_failure() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let result = require_result(
        module.execute_callback("on_delivery_failure", &mut ctx),
        "Event callback failed",
    )?;
    assert_eq!(result, 0);
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_on_smtp_error() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let result = require_result(
        module.execute_callback("on_smtp_error", &mut ctx),
        "Event callback failed",
    )?;
    assert_eq!(result, 0);
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_on_smtp_message_received() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(b"Test message".to_vec()));
    let mut envelope = Envelope::default();
    *envelope.sender_mut() = Some(Address::from(require_result(
        parse_forward_path("<sender@example.com>"),
        "valid sender",
    )?));
    ctx.envelope = Arc::new(envelope);

    let result = require_result(
        module.execute_callback("on_smtp_message_received", &mut ctx),
        "Event callback failed",
    )?;
    assert_eq!(result, 0);
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_on_dns_lookup() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let result = require_result(
        module.execute_callback("on_dns_lookup", &mut ctx),
        "Event callback failed",
    )?;
    assert_eq!(result, 0);
    Ok(())
}

// --- Context Modification Tests ---

#[test]
#[cfg_attr(miri, ignore)]
fn test_transaction_context_persistence() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    *envelope.sender_mut() = Some(Address::from(require_result(
        parse_forward_path("<user@example.com>"),
        "valid sender",
    )?));
    ctx.envelope = Arc::new(envelope);

    // First callback sets transaction context
    require_result(
        module.execute_callback("validate_mail_from", &mut ctx),
        "validate_mail_from failed",
    )?;

    // Verify it's accessible in subsequent callback
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("validated_sender")),
        Some(&"user@example.com".to_string())
    );

    // Second callback can read it
    ctx.data = Some(Arc::from(b"Test".to_vec()));
    require_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    )?;

    // Transaction context should still be there
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("validated_sender")),
        Some(&"user@example.com".to_string()),
        "Transaction context not persisted across callbacks"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_session_context_persistence() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();

    // First event sets session context
    require_result(
        module.execute_callback("on_connection_opened", &mut ctx),
        "on_connection_opened failed",
    )?;

    let session_value = ctx
        .session
        .get(&Cow::Borrowed("connection_opened_at"))
        .cloned();
    assert!(session_value.is_some(), "Session context not set");

    // Second event can read it
    require_result(
        module.execute_callback("on_connection_closed", &mut ctx),
        "on_connection_closed failed",
    )?;

    // Session context should still be there
    assert_eq!(
        ctx.session.get(&Cow::Borrowed("connection_opened_at")),
        session_value.as_ref(),
        "Session context not persisted"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_transaction_context_isolation_between_sessions() -> TestResult {
    let module = init_test_module()?;

    // First context writes transaction data
    let mut ctx1 = create_test_context();
    require_result(
        module.execute_callback("validate_mail_from", &mut ctx1),
        "validate_mail_from failed",
    )?;
    assert!(
        ctx1.transaction
            .contains_key(&Cow::Borrowed("validated_sender")),
        "Expected validated_sender in first transaction context"
    );

    // Second context should not see prior transaction data
    let mut ctx2 = create_test_context();
    require_result(
        module.execute_callback("validate_data", &mut ctx2),
        "validate_data failed",
    )?;
    assert!(
        !ctx2
            .transaction
            .contains_key(&Cow::Borrowed("validated_sender")),
        "Transaction data leaked between contexts"
    );
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_transaction_overwrite() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    ctx.transaction
        .insert(Cow::Borrowed("rejection_reason"), "old".to_string());
    ctx.data = Some(Arc::from(vec![0_u8; 1_000_001]));

    require_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    )?;

    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("rejection_reason")),
        Some(&"message_too_large".to_string()),
        "Rejection reason not updated by callback"
    );
    Ok(())
}

// --- Error Handling Tests ---

#[test]
#[cfg_attr(miri, ignore)]
fn test_unknown_callback_returns_zero() -> TestResult {
    let module = init_test_module()?;

    let mut ctx = create_test_context();
    let result = module.execute_callback("nonexistent_callback", &mut ctx);

    // Unknown validation callbacks return -1 internally, which gets converted to 0
    let value = require_result(result, "unknown callback failed")?;
    assert_eq!(value, 0, "Unknown callbacks should be ignored");
    Ok(())
}

// --- Module Configuration Tests ---

#[test]
#[cfg_attr(miri, ignore)]
fn test_with_timeout() -> TestResult {
    use std::time::Duration;

    let module = load_test_module()?.with_timeout(Duration::from_secs(10));

    assert_eq!(module.timeout(), Duration::from_secs(10));
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_with_max_memory() -> TestResult {
    let module = load_test_module()?.with_max_memory(5_000_000);

    assert_eq!(module.max_memory_bytes(), 5_000_000);
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_module_path() -> TestResult {
    let module = load_test_module()?;
    assert!(
        module.path().ends_with("simple_validator.wasm"),
        "unexpected module path: {}",
        module.path()
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        max_shrink_iters: 0,
        ..ProptestConfig::default()
    })]

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_wasm_context_fuzz(
        session_id in session_id_strategy(),
        sender in sender_strategy(),
        recipients in recipients_strategy(),
        extended in any::<bool>(),
        data in message_data_strategy(),
        max_message_size in 1_024usize..=2_000_000,
        transaction_entries in transaction_entries_strategy(),
        include_tracking in any::<bool>(),
    ) {
        let module = match fuzz_module() {
            Ok(module) => module,
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "failed to initialize wasm module for fuzz test: {err}"
                )));
            }
        };
        let mut ctx = build_fuzz_context(FuzzContextInput {
            session_id,
            sender,
            recipients,
            extended,
            data,
            max_message_size,
            transaction_entries,
            include_tracking,
        });

        let callbacks = [
            "on_connection_opened",
            "validate_connect",
            "validate_ehlo",
            "validate_mail_from",
            "validate_rcpt_to",
            "validate_data",
            "validate_starttls",
            "on_smtp_message_received",
            "on_delivery_attempt",
            "on_delivery_success",
            "on_delivery_failure",
            "on_smtp_error",
            "on_dns_lookup",
            "on_connection_closed",
        ];

        for callback in callbacks {
            let result = module.execute_callback(callback, &mut ctx);
            prop_assert!(result.is_ok(), "callback {callback} failed: {:?}", result.err());
        }
    }
}
