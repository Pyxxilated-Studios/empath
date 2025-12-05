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
};
use empath_wasm::{WasmError, WasmModule};
use tokio::runtime::Runtime;

#[allow(clippy::panic, reason = "Test helpers should fail loudly")]
fn unwrap_result<T, E: std::fmt::Debug>(res: Result<T, E>, context: &str) -> T {
    res.unwrap_or_else(|e| panic!("{context}: {e:?}"))
}

#[allow(clippy::panic, reason = "Test helpers should fail loudly")]
fn unwrap_option<T>(opt: Option<T>, context: &str) -> T {
    opt.unwrap_or_else(|| panic!("{context}"))
}

/// Compute the profile directory (debug/release) Cargo is building with
fn cargo_profile() -> String {
    env::var("PROFILE").unwrap_or_else(|_| "debug".to_string())
}

/// Location of the compiled test WASM module, reusing cached artifacts when available and only
/// invoking `cargo run -p xtask -- build-wasm` if nothing fresh is present.
fn resolved_module_path() -> &'static Path {
    static MODULE_PATH: OnceLock<PathBuf> = OnceLock::new();

    MODULE_PATH
        .get_or_init(|| {
            let workspace_root = unwrap_option(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .map(Path::to_path_buf),
                "workspace root",
            );

            let profile = cargo_profile();
            let wasm_path = workspace_root
                .join("empath-examples/wasm/target/wasm32-wasip2")
                .join(&profile)
                .join("simple_validator.wasm");

            let cached_path = workspace_root
                .join("target/modules")
                .join("simple_validator.wasm");

            let source_mtime = latest_source_mtime(&workspace_root);

            if module_fresh(&wasm_path, source_mtime) {
                return wasm_path;
            }

            if module_fresh(&cached_path, source_mtime) {
                return cached_path;
            }

            unwrap_result(
                build_wasm_example(&workspace_root, &profile),
                "failed to build wasm example via xtask",
            );

            assert!(
                wasm_path.exists(),
                "expected WASM test module at {} after xtask build, build may have failed",
                wasm_path.display()
            );

            wasm_path
        })
        .as_path()
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

fn load_test_module() -> WasmModule {
    let rt = runtime();
    let _guard = rt.enter();
    unwrap_result(
        WasmModule::load(resolved_module_path()),
        "Failed to load test module",
    )
}

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        unwrap_result(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build(),
            "tokio runtime for wasm tests",
        )
    })
}

fn init_test_module() -> WasmModule {
    let module = load_test_module();
    unwrap_result(
        module.initialize(),
        "Failed to initialize module during test setup",
    );
    module
}

fn init_test_module_with_timeout(timeout: Duration) -> WasmModule {
    let module = load_test_module().with_timeout(timeout);
    unwrap_result(
        module.initialize(),
        "Failed to initialize module during test setup",
    );
    module
}

fn init_test_module_with_memory_limit(max_memory_bytes: usize) -> WasmModule {
    let module = load_test_module().with_max_memory(max_memory_bytes);
    unwrap_result(
        module.initialize(),
        "Failed to initialize module during test setup",
    );
    module
}

/// Create a test context with sensible defaults
fn create_test_context() -> Context {
    Context {
        envelope: Arc::new(Envelope::default()),
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
    }
}

#[test]
fn test_load_nonexistent_module() {
    let result = WasmModule::load("/nonexistent/path.wasm");
    assert!(result.is_err());
}

#[test]
fn test_load_valid_module() {
    // Ensure epoch task has a runtime by entering it during load
    let rt = runtime();
    let _guard = rt.enter();
    let result = WasmModule::load(resolved_module_path());
    assert!(
        result.is_ok(),
        "Failed to load test module: {:?}",
        result.err()
    );
}

#[test]
fn test_module_initialization() {
    let module = load_test_module();
    let init_result = module.initialize();
    assert!(
        init_result.is_ok(),
        "Module initialization failed: {:?}",
        init_result.err()
    );
}

// --- Validation Callback Tests ---

#[test]
fn test_validate_connect_accept() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let result = unwrap_result(
        module.execute_callback("validate_connect", &mut ctx),
        "Callback execution failed",
    );
    assert_eq!(result, 0, "Expected Accept (0)");

    // Check that session context was set
    assert_eq!(
        ctx.session.get(&Cow::Borrowed("connection_validated")),
        Some(&"true".to_string()),
        "Session context not set"
    );
}

#[test]
fn test_validate_ehlo_accept() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    ctx.extended = true;

    let result = unwrap_result(
        module.execute_callback("validate_ehlo", &mut ctx),
        "validate_ehlo failed",
    );
    assert_eq!(result, 0, "Expected Accept");
}

#[test]
fn test_validate_mail_from_accept() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    let sender_mailbox = unwrap_result(
        parse_forward_path("<user@example.com>"),
        "Failed to parse sender",
    );
    *envelope.sender_mut() = Some(Address::from(sender_mailbox));
    ctx.envelope = Arc::new(envelope);

    let result = unwrap_result(
        module.execute_callback("validate_mail_from", &mut ctx),
        "validate_mail_from failed",
    );
    assert_eq!(result, 0, "Expected Accept");

    // Check transaction context
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("validated_sender")),
        Some(&"user@example.com".to_string()),
        "Sender not stored in transaction context"
    );
}

#[test]
fn test_validate_mail_from_reject_spam_domain() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    let sender_mailbox = unwrap_result(
        parse_forward_path("<spammer@spam.example.com>"),
        "Failed to parse sender",
    );
    *envelope.sender_mut() = Some(Address::from(sender_mailbox));
    ctx.envelope = Arc::new(envelope);

    let result = unwrap_result(
        module.execute_callback("validate_mail_from", &mut ctx),
        "validate_mail_from failed",
    );
    assert_eq!(result, 550, "Expected Reject with 550");

    // Check rejection reason in transaction context
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("rejection_reason")),
        Some(&"sender_blocked_spam_domain".to_string()),
        "Rejection reason not stored"
    );
}

#[test]
fn test_validate_rcpt_to_accept() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    let recipients = vec![
        Address::from(unwrap_result(
            parse_forward_path("<user1@example.com>"),
            "valid recipient",
        )),
        Address::from(unwrap_result(
            parse_forward_path("<user2@example.com>"),
            "valid recipient",
        )),
    ];
    *envelope.recipients_mut() = Some(AddressList::from(recipients));
    ctx.envelope = Arc::new(envelope);

    let result = unwrap_result(
        module.execute_callback("validate_rcpt_to", &mut ctx),
        "validate_rcpt_to failed",
    );
    assert_eq!(result, 0, "Expected Accept");

    // Check recipient count tracking
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("recipient_count")),
        Some(&"2".to_string()),
        "Recipient count not tracked"
    );
}

#[test]
fn test_validate_rcpt_to_reject_too_many() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    // Add 11 recipients (exceeds limit of 10)
    let recipients: Vec<_> = (1..=11)
        .map(|i| {
            Address::from(unwrap_result(
                parse_forward_path(&format!("<user{i}@example.com>")),
                "valid recipient",
            ))
        })
        .collect();
    *envelope.recipients_mut() = Some(AddressList::from(recipients));
    ctx.envelope = Arc::new(envelope);

    let result = unwrap_result(
        module.execute_callback("validate_rcpt_to", &mut ctx),
        "validate_rcpt_to failed",
    );
    assert_eq!(result, 452, "Expected Reject with 452");

    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("rejection_reason")),
        Some(&"too_many_recipients".to_string())
    );
}

#[test]
fn test_validate_data_accept() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(b"Subject: Test\r\n\r\nHello world!".to_vec()));
    let expected_size = unwrap_option(ctx.data.as_ref().map(|d| d.len()), "message data missing");

    let result = unwrap_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    );
    assert_eq!(result, 0, "Expected Accept");

    // Check message size tracking
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("message_size")),
        Some(&expected_size.to_string()),
        "Message size not tracked"
    );
}

#[test]
fn test_validate_data_reject_too_large() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    // Create message larger than 1MB
    let large_data = vec![b'A'; 1_000_001];
    ctx.data = Some(Arc::from(large_data));

    let result = unwrap_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    );
    assert_eq!(result, 552, "Expected Reject with 552");

    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("rejection_reason")),
        Some(&"message_too_large".to_string())
    );
}

#[test]
fn test_validate_data_reject_spam_content() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(
        b"Subject: Special offer\r\n\r\nBuy cheap viagra now!".to_vec(),
    ));

    let result = unwrap_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    );
    assert_eq!(result, 550, "Expected Reject with 550 for spam");

    // Check spam keyword was recorded
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("spam_keyword")),
        Some(&"viagra".to_string())
    );
}

#[allow(clippy::expect_used, clippy::panic)]
#[test]
fn test_validate_data_timeout() {
    let module = init_test_module_with_timeout(Duration::from_millis(10));

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(b"timeout-trigger".to_vec()));

    let result = module.execute_callback("validate_data", &mut ctx);

    let err = result.expect_err("Expected validate_data to time out");
    match err {
        WasmError::Timeout(callback, _) => assert_eq!(callback, "validate_data"),
        other => panic!("Expected timeout error, got {other:?}"),
    }
}

#[allow(clippy::panic, clippy::expect_used)]
#[test]
fn test_validate_data_memory_limit() {
    // Set a low memory limit (just above module minimum) to force failure on growth
    let module = init_test_module_with_memory_limit(1_200_000);

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(b"alloc:3000000".to_vec())); // ~3 MB allocation request

    let result = module.execute_callback("validate_data", &mut ctx);
    let err = result.expect_err("Expected validate_data to hit memory limit");
    match err {
        WasmError::Wasmtime(_) => {}
        other => panic!("Expected Wasmtime error for memory limit, got {other:?}"),
    }
}

#[allow(clippy::panic, clippy::expect_used)]
#[test]
fn test_event_error_path_returns_err() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    ctx.id = "panic-event-123".to_string();

    let result = module.execute_callback("on_delivery_failure", &mut ctx);
    assert!(result.is_err(), "Expected event callback to error");
}

#[allow(clippy::panic, clippy::expect_used)]
#[test]
fn test_context_survives_event_error() {
    let module = init_test_module();

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
}

#[test]
fn test_validate_starttls_accept() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let result = module.execute_callback("validate_starttls", &mut ctx);
    assert!(result.is_ok());
    assert_eq!(
        unwrap_result(result, "validate_starttls failed"),
        0,
        "Expected Accept"
    );
}

// --- Event Callback Tests ---

#[test]
fn test_on_connection_opened() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let result = unwrap_result(
        module.execute_callback("on_connection_opened", &mut ctx),
        "Event callback failed",
    );
    assert_eq!(result, 0, "Event callbacks should return 0");

    // Check session context was set
    assert!(
        ctx.session
            .contains_key(&Cow::Borrowed("connection_opened_at")),
        "Event callback did not set session context"
    );
}

#[test]
fn test_on_connection_closed() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    // Simulate previous connection_opened event
    ctx.session.insert(
        Cow::Borrowed("connection_opened_at"),
        "test-session-123".to_string(),
    );

    let result = unwrap_result(
        module.execute_callback("on_connection_closed", &mut ctx),
        "Event callback failed",
    );
    assert_eq!(result, 0);
}

#[test]
fn test_on_delivery_attempt() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    // Add spam keyword from previous validation
    ctx.transaction
        .insert(Cow::Borrowed("spam_keyword"), "test".to_string());

    let result = unwrap_result(
        module.execute_callback("on_delivery_attempt", &mut ctx),
        "Event callback failed",
    );
    assert_eq!(result, 0);
}

#[test]
fn test_on_delivery_success() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let result = unwrap_result(
        module.execute_callback("on_delivery_success", &mut ctx),
        "Event callback failed",
    );
    assert_eq!(result, 0);
}

#[test]
fn test_on_delivery_failure() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let result = unwrap_result(
        module.execute_callback("on_delivery_failure", &mut ctx),
        "Event callback failed",
    );
    assert_eq!(result, 0);
}

#[test]
fn test_on_smtp_error() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let result = unwrap_result(
        module.execute_callback("on_smtp_error", &mut ctx),
        "Event callback failed",
    );
    assert_eq!(result, 0);
}

#[test]
fn test_on_smtp_message_received() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    ctx.data = Some(Arc::from(b"Test message".to_vec()));
    let mut envelope = Envelope::default();
    *envelope.sender_mut() = Some(Address::from(unwrap_result(
        parse_forward_path("<sender@example.com>"),
        "valid sender",
    )));
    ctx.envelope = Arc::new(envelope);

    let result = unwrap_result(
        module.execute_callback("on_smtp_message_received", &mut ctx),
        "Event callback failed",
    );
    assert_eq!(result, 0);
}

#[test]
fn test_on_dns_lookup() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let result = unwrap_result(
        module.execute_callback("on_dns_lookup", &mut ctx),
        "Event callback failed",
    );
    assert_eq!(result, 0);
}

// --- Context Modification Tests ---

#[test]
fn test_transaction_context_persistence() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let mut envelope = Envelope::default();
    *envelope.sender_mut() = Some(Address::from(unwrap_result(
        parse_forward_path("<user@example.com>"),
        "valid sender",
    )));
    ctx.envelope = Arc::new(envelope);

    // First callback sets transaction context
    unwrap_result(
        module.execute_callback("validate_mail_from", &mut ctx),
        "validate_mail_from failed",
    );

    // Verify it's accessible in subsequent callback
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("validated_sender")),
        Some(&"user@example.com".to_string())
    );

    // Second callback can read it
    ctx.data = Some(Arc::from(b"Test".to_vec()));
    unwrap_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    );

    // Transaction context should still be there
    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("validated_sender")),
        Some(&"user@example.com".to_string()),
        "Transaction context not persisted across callbacks"
    );
}

#[test]
fn test_session_context_persistence() {
    let module = init_test_module();

    let mut ctx = create_test_context();

    // First event sets session context
    unwrap_result(
        module.execute_callback("on_connection_opened", &mut ctx),
        "on_connection_opened failed",
    );

    let session_value = ctx
        .session
        .get(&Cow::Borrowed("connection_opened_at"))
        .cloned();
    assert!(session_value.is_some(), "Session context not set");

    // Second event can read it
    unwrap_result(
        module.execute_callback("on_connection_closed", &mut ctx),
        "on_connection_closed failed",
    );

    // Session context should still be there
    assert_eq!(
        ctx.session.get(&Cow::Borrowed("connection_opened_at")),
        session_value.as_ref(),
        "Session context not persisted"
    );
}

#[test]
fn test_transaction_context_isolation_between_sessions() {
    let module = init_test_module();

    // First context writes transaction data
    let mut ctx1 = create_test_context();
    unwrap_result(
        module.execute_callback("validate_mail_from", &mut ctx1),
        "validate_mail_from failed",
    );
    assert!(
        ctx1.transaction
            .contains_key(&Cow::Borrowed("validated_sender")),
        "Expected validated_sender in first transaction context"
    );

    // Second context should not see prior transaction data
    let mut ctx2 = create_test_context();
    unwrap_result(
        module.execute_callback("validate_data", &mut ctx2),
        "validate_data failed",
    );
    assert!(
        !ctx2
            .transaction
            .contains_key(&Cow::Borrowed("validated_sender")),
        "Transaction data leaked between contexts"
    );
}

#[test]
fn test_transaction_overwrite() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    ctx.transaction
        .insert(Cow::Borrowed("rejection_reason"), "old".to_string());
    ctx.data = Some(Arc::from(vec![0_u8; 1_000_001]));

    unwrap_result(
        module.execute_callback("validate_data", &mut ctx),
        "validate_data failed",
    );

    assert_eq!(
        ctx.transaction.get(&Cow::Borrowed("rejection_reason")),
        Some(&"message_too_large".to_string()),
        "Rejection reason not updated by callback"
    );
}

// --- Error Handling Tests ---

#[test]
fn test_unknown_callback_returns_zero() {
    let module = init_test_module();

    let mut ctx = create_test_context();
    let result = module.execute_callback("nonexistent_callback", &mut ctx);

    // Unknown validation callbacks return -1 internally, which gets converted to 0
    assert!(result.is_ok());
    assert_eq!(
        unwrap_result(result, "unknown callback failed"),
        0,
        "Unknown callbacks should be ignored"
    );
}

// --- Module Configuration Tests ---

#[test]
fn test_with_timeout() {
    use std::time::Duration;

    let module = load_test_module().with_timeout(Duration::from_secs(10));

    assert_eq!(module.timeout(), Duration::from_secs(10));
}

#[test]
fn test_with_max_memory() {
    let module = load_test_module().with_max_memory(5_000_000);

    assert_eq!(module.max_memory_bytes(), 5_000_000);
}

#[test]
fn test_module_path() {
    let module = load_test_module();
    assert!(
        module.path().ends_with("simple_validator.wasm"),
        "unexpected module path: {}",
        module.path()
    );
}
