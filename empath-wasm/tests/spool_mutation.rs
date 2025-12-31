//! End-to-end: WASM message helpers mutate headers/body and persist to spool.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use empath_common::traits::protocol::{Protocol, SessionHandler};
use empath_ffi::modules::wasm::Wasm;
use empath_ffi::modules::{Module, ModuleRegistryBuilder};
use empath_smtp::client::{QuitAfter, SmtpClientBuilder};
use empath_smtp::extensions::Extension;
use empath_smtp::{Smtp, SmtpArgs};
use empath_spool::{BackingStore, MemoryBackingStore, SpooledMessageId};
use tokio::net::TcpListener;
use tokio::time::{Duration, timeout};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn start_server_with_module(
    module_path: PathBuf,
    spool: Arc<MemoryBackingStore>,
) -> TestResult<(u16, tokio::task::JoinHandle<()>)> {
    let wasm_module = Wasm::from_path(module_path);
    ModuleRegistryBuilder::new()
        .with_modules(vec![Module::Wasm(wasm_module)])
        .build()?
        .install()?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);

    let handle = tokio::spawn(async move {
        let smtp = Smtp;
        let args = SmtpArgs::builder()
            .with_extensions(vec![Extension::Size(10_000_000)])
            .with_spool(spool);

        while let Ok((stream, peer)) = listener.accept().await {
            let shutdown_rx = shutdown_tx.subscribe();
            let session = smtp.handle(stream, peer, HashMap::default(), args.clone());
            tokio::spawn(async move {
                let _ = timeout(Duration::from_secs(30), async {
                    SessionHandler::run(session, shutdown_rx).await
                })
                .await;
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok((port, handle))
}

fn find_wasm_artifact(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries = std::fs::read_dir(&path).ok()?;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("wasm") {
                return Some(p);
            }
        }
    }
    None
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn wasm_message_helpers_mutate_spool() -> TestResult {
    // Build guest module to WASI target
    let guest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/guest");
    // Build the guest module
    let status = std::process::Command::new("cargo")
        .args(["build", "--target", "wasm32-wasip2"])
        .env("CARGO_TARGET_DIR", guest_dir.join("target"))
        .current_dir(&guest_dir)
        .status()?;
    if !status.success() {
        return Err("guest build failed".into());
    }

    let target_dir = guest_dir.join("target/wasm32-wasip2");
    let output_wasm = find_wasm_artifact(&target_dir).ok_or("guest wasm not built")?;

    let spool = Arc::new(MemoryBackingStore::new());
    let (port, _handle) = start_server_with_module(output_wasm, Arc::clone(&spool)).await?;

    let responses = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .data_with_content("Subject: Original\r\n\r\nBody")
        .quit_after(QuitAfter::DataEnd)
        .execute()
        .await?;
    assert!(responses.iter().any(|r| r.code == 250));

    // Fetch the single message from memory spool
    let messages: Vec<SpooledMessageId> = spool.list().await?;
    assert_eq!(messages.len(), 1);
    let id = &messages[0];
    let ctx = spool.read(id).await?;
    let data = ctx.data.as_ref().ok_or("message data missing")?;
    let body_str = String::from_utf8_lossy(data);
    assert!(
        body_str.contains("X-Wasm-Test: ok"),
        "missing mutated header: {body_str}"
    );
    assert!(
        body_str.contains(" appended"),
        "missing mutated body: {body_str}"
    );
    Ok(())
}
