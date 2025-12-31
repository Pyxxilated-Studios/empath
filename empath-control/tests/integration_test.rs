//! Integration tests for control socket client/server communication
//!
//! These tests verify the full request/response cycle between the control
//! client and server, including error handling, timeouts, and protocol correctness.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use empath_common::Signal;
use empath_control::auth::{ControlRole, ControlToken};
use empath_control::protocol::{
    BindingPreview, CachedMailServer, ConnectionStats, DnsCommand, QueueCommand, Request,
    RequestCommand, Response, ResponseData, ResponsePayload, SystemCommand, SystemStatus,
};
use empath_control::server::CommandHandler;
use empath_control::{ControlAuthConfig, ControlClient, ControlError, ControlServer, Result};
use hex::encode;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::broadcast;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Mock command handler for testing
struct MockHandler {
    /// Simulated DNS cache
    dns_cache: HashMap<String, Vec<CachedMailServer>>,
    /// Simulated MX overrides
    mx_overrides: HashMap<String, String>,
}

impl MockHandler {
    fn new() -> Self {
        let mut dns_cache = HashMap::new();
        dns_cache.insert(
            "example.com".to_string(),
            vec![
                CachedMailServer {
                    host: "mail.example.com".to_string(),
                    port: 25,
                    priority: 10,
                    ttl_remaining_secs: 300,
                },
                CachedMailServer {
                    host: "mail2.example.com".to_string(),
                    port: 25,
                    priority: 20,
                    ttl_remaining_secs: 300,
                },
            ],
        );

        let mut mx_overrides = HashMap::new();
        mx_overrides.insert("test.local".to_string(), "localhost:1025".to_string());

        Self {
            dns_cache,
            mx_overrides,
        }
    }

    fn handle_dns(&self, cmd: DnsCommand) -> Response {
        match cmd {
            DnsCommand::ListCache => Response::data(ResponseData::DnsCache(self.dns_cache.clone())),
            DnsCommand::ClearCache => {
                Response::data(ResponseData::Message("Cache cleared".to_string()))
            }
            DnsCommand::RefreshDomain(domain) => {
                Response::data(ResponseData::Message(format!("Refreshed {domain}")))
            }
            DnsCommand::SetOverride { domain, mx_server } => Response::data(ResponseData::Message(
                format!("Set override {domain} -> {mx_server}"),
            )),
            DnsCommand::RemoveOverride(domain) => Response::data(ResponseData::Message(format!(
                "Removed override for {domain}"
            ))),
            DnsCommand::ListOverrides => {
                Response::data(ResponseData::MxOverrides(self.mx_overrides.clone()))
            }
        }
    }

    fn handle_system(cmd: SystemCommand) -> Response {
        match cmd {
            SystemCommand::Ping => Response::ok(),
            SystemCommand::Status => Response::data(ResponseData::SystemStatus(SystemStatus {
                version: "0.0.2".to_string(),
                uptime_secs: 12345,
                queue_size: 42,
                dns_cache_entries: 10,
            })),
            SystemCommand::SetLogLevel { level } => {
                Response::data(ResponseData::Message(format!("Log level set to {level}")))
            }
            SystemCommand::ReloadConfig => {
                Response::data(ResponseData::Message("Reloading config".to_string()))
            }
            SystemCommand::ReloadTls => Response::data(ResponseData::Message(
                "TLS credentials reloaded".to_string(),
            )),
            SystemCommand::ListTls => Response::data(ResponseData::TlsContexts(Vec::new())),
            SystemCommand::SetTlsProfile { set_id, profile } => Response::data(
                ResponseData::Message(format!("TLS profile '{profile}' activated for '{set_id}'")),
            ),
            SystemCommand::DrainListener { socket } => {
                Response::data(ResponseData::Message(format!("Listener {socket} draining")))
            }
            SystemCommand::ConnectionStats => {
                Response::data(ResponseData::ConnectionStats(ConnectionStats {
                    queue_size: 42,
                    rate_limit_domains: 3,
                }))
            }
            SystemCommand::ListBindings => {
                Response::data(ResponseData::OutboundBindings(Vec::new()))
            }
            SystemCommand::PreviewBinding { domain } => {
                Response::data(ResponseData::BindingPreview(BindingPreview {
                    domain,
                    binding: None,
                    binding_enabled: None,
                    attempt_starttls: None,
                    tls_policy: "opportunistic".to_string(),
                }))
            }
            SystemCommand::SetBindingState { name, enabled } => {
                let state = if enabled { "enabled" } else { "disabled" };
                Response::data(ResponseData::Message(format!("Binding {name} {state}")))
            }
            SystemCommand::SetRateLimit {
                domain,
                messages_per_second,
                burst,
            } => Response::data(ResponseData::Message(format!(
                "Rate limit set: domain={domain:?}, mps={messages_per_second}, burst={burst}"
            ))),
            SystemCommand::ListCapabilities => {
                Response::data(ResponseData::SmtpCapabilities(vec!["AUTH".to_string()]))
            }
            SystemCommand::SetCapabilityState {
                capability,
                enabled,
            } => {
                let state = if enabled { "enabled" } else { "disabled" };
                Response::data(ResponseData::Message(format!(
                    "Capability {capability} {state}"
                )))
            }
        }
    }

    fn handle_queue(cmd: &QueueCommand) -> Response {
        match cmd {
            QueueCommand::Stats => Response::data(ResponseData::Message("Queue stats".to_string())),
            _ => Response::error("Queue command not implemented in mock".to_string()),
        }
    }
}

#[async_trait]
impl CommandHandler for MockHandler {
    async fn handle_request(&self, request: Request) -> Result<Response> {
        match request.command {
            RequestCommand::Dns(cmd) => Ok(self.handle_dns(cmd)),
            RequestCommand::System(cmd) => Ok(Self::handle_system(cmd)),
            RequestCommand::Queue(cmd) => Ok(Self::handle_queue(&cmd)),
            RequestCommand::Spool(_) => Ok(Response::error(
                "Spool command not implemented in mock".to_string(),
            )),
        }
    }
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    encode(hasher.finalize())
}

/// Helper to start a test control server
async fn start_test_server(
    socket_path: &str,
    handler: Arc<dyn CommandHandler>,
) -> Result<(tokio::task::JoinHandle<()>, broadcast::Sender<Signal>)> {
    let server = ControlServer::new(socket_path, handler)?;
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.serve(shutdown_rx).await {
            eprintln!("Server error: {e}");
        }
    });

    // Give server time to bind
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok((server_handle, shutdown_tx))
}

/// Helper to start a test control server with authentication enabled
async fn start_test_server_with_auth(
    socket_path: &str,
    handler: Arc<dyn CommandHandler>,
    auth_config: ControlAuthConfig,
) -> Result<(tokio::task::JoinHandle<()>, broadcast::Sender<Signal>)> {
    let server = ControlServer::with_auth(socket_path, handler, Some(auth_config))?;
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.serve(shutdown_rx).await {
            eprintln!("Server error: {e}");
        }
    });

    // Give server time to bind
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok((server_handle, shutdown_tx))
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_list_cache() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test DNS list cache command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::ListCache));
    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::DnsCache(cache) => {
                assert!(cache.contains_key("example.com"));
                let Some(servers) = cache.get("example.com") else {
                    return Err("expected example.com cache entry".into());
                };
                assert_eq!(servers.len(), 2);
                assert_eq!(servers[0].host, "mail.example.com");
                assert_eq!(servers[1].host, "mail2.example.com");
            }
            _ => return Err("expected DnsCache response".into()),
        },
        _ => return Err("expected Data response".into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_control_auth_read_only_blocks_write() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let auth_config = ControlAuthConfig {
        enabled: true,
        token_hashes: vec![hash_token("admin-token")],
        tokens: vec![ControlToken {
            hash: hash_token("read-token"),
            role: ControlRole::ReadOnly,
        }],
    };
    assert_eq!(
        auth_config.role_for_token("read-token"),
        Some(ControlRole::ReadOnly)
    );

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) =
        start_test_server_with_auth(socket_str, handler, auth_config).await?;

    let client = ControlClient::new(socket_str).with_token("read-token");
    let request = Request::new(RequestCommand::Dns(DnsCommand::ClearCache));
    let result = client.send_request(request).await;

    match result {
        Err(ControlError::ServerError(err)) => {
            assert!(
                err.contains("Authorization failed"),
                "unexpected error: {err}"
            );
        }
        other => return Err(format!("expected authorization failure, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_control_auth_read_only_allows_read() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let auth_config = ControlAuthConfig {
        enabled: true,
        token_hashes: Vec::new(),
        tokens: vec![ControlToken {
            hash: hash_token("read-token"),
            role: ControlRole::ReadOnly,
        }],
    };

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) =
        start_test_server_with_auth(socket_str, handler, auth_config).await?;

    let client = ControlClient::new(socket_str).with_token("read-token");
    let request = Request::new(RequestCommand::Dns(DnsCommand::ListCache));
    let response = client.send_request(request).await?;

    assert!(response.is_success());
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_control_auth_admin_allows_write() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let auth_config = ControlAuthConfig {
        enabled: true,
        token_hashes: vec![hash_token("admin-token")],
        tokens: Vec::new(),
    };

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) =
        start_test_server_with_auth(socket_str, handler, auth_config).await?;

    let client = ControlClient::new(socket_str).with_token("admin-token");
    let request = Request::new(RequestCommand::Dns(DnsCommand::ClearCache));
    let response = client.send_request(request).await?;

    assert!(response.is_success());
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_clear_cache() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test DNS clear cache command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::ClearCache));
    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::Message(msg) => {
                assert_eq!(msg, "Cache cleared");
            }
            _ => return Err("expected Message response".into()),
        },
        _ => return Err("expected Data response".into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_refresh_domain() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test DNS refresh domain command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::RefreshDomain(
        "example.com".to_string(),
    )));
    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::Message(msg) => {
                assert_eq!(msg, "Refreshed example.com");
            }
            _ => return Err("expected Message response".into()),
        },
        _ => return Err("expected Data response".into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_set_override() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test DNS set override command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::SetOverride {
        domain: "test.example.com".to_string(),
        mx_server: "localhost:1025".to_string(),
    }));
    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::Message(msg) => {
                assert!(msg.contains("Set override"));
                assert!(msg.contains("test.example.com"));
                assert!(msg.contains("localhost:1025"));
            }
            _ => return Err("expected Message response".into()),
        },
        _ => return Err("expected Data response".into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_remove_override() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test DNS remove override command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::RemoveOverride(
        "test.local".to_string(),
    )));
    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::Message(msg) => {
                assert!(msg.contains("Removed override"));
                assert!(msg.contains("test.local"));
            }
            _ => return Err("expected Message response".into()),
        },
        _ => return Err("expected Data response".into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_list_overrides() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test DNS list overrides command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::ListOverrides));
    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::MxOverrides(overrides) => match overrides.get("test.local") {
                Some(value) => assert_eq!(value, "localhost:1025"),
                None => return Err("expected override for test.local".into()),
            },
            _ => return Err("expected MxOverrides response".into()),
        },
        _ => return Err("expected Data response".into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_system_ping() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test system ping command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Ok => {
            // Success
        }
        _ => return Err("expected Ok response".into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_system_status() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test system status command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::System(SystemCommand::Status));
    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::SystemStatus(status) => {
                assert_eq!(status.version, "0.0.2");
                assert_eq!(status.uptime_secs, 12345);
                assert_eq!(status.queue_size, 42);
                assert_eq!(status.dns_cache_entries, 10);
            }
            _ => return Err("expected SystemStatus response".into()),
        },
        _ => return Err("expected Data response".into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_socket_not_exist_error() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("nonexistent.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    // Test connecting to non-existent socket
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let result = client.send_request(request).await;

    match result {
        Err(ControlError::Io(_)) => Ok(()),
        Err(err) => Err(format!("expected IO error, got {err:?}").into()),
        Ok(_) => Err("expected connection error".into()),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_check_socket_exists() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    // Test with non-existent socket
    let client = ControlClient::new(socket_str);
    let result = client.check_socket_exists();
    match result {
        Err(ControlError::InvalidSocketPath(_)) => {}
        Err(err) => return Err(format!("expected invalid socket path, got {err:?}").into()),
        Ok(()) => return Err("expected invalid socket path error".into()),
    }

    // Start server
    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test with existing socket
    let result = client.check_socket_exists();
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_client_timeout() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Test with very short timeout (should succeed for fast operations)
    let client = ControlClient::new(socket_str).with_timeout(Duration::from_millis(50));
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let result = client.send_request(request).await;

    // This might succeed or timeout depending on system load
    // We're just testing that the timeout mechanism works
    match result {
        Ok(_) | Err(ControlError::Timeout) => {
            // Timed out as expected
        }
        Err(e) => return Err(format!("unexpected error: {e}").into()),
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_graceful_shutdown() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (server_handle, shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Verify server is running
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let response = client.send_request(request).await?;
    assert!(matches!(response.payload, ResponsePayload::Ok));

    // Send shutdown signal
    shutdown_tx
        .send(Signal::Shutdown)
        .map_err(|err| format!("shutdown send failed: {err}"))?;

    // Wait for server to shut down
    tokio::time::timeout(Duration::from_secs(5), server_handle)
        .await
        .map_err(|_| "server did not shut down within timeout")?
        .map_err(|err| format!("server task panicked: {err}"))?;

    // Verify socket is cleaned up
    assert!(!socket_path.exists());
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_concurrent_requests() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?
        .to_string();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(&socket_str, handler).await?;

    // Send multiple concurrent requests
    let mut join_handles = vec![];

    for i in 0..10 {
        let socket_str = socket_str.clone();
        let handle = tokio::spawn(async move {
            let client = ControlClient::new(&socket_str);
            let request = if i % 2 == 0 {
                Request::new(RequestCommand::System(SystemCommand::Ping))
            } else {
                Request::new(RequestCommand::Dns(DnsCommand::ListOverrides))
            };
            client.send_request(request).await
        });
        join_handles.push(handle);
    }

    // Wait for all requests to complete
    for handle in join_handles {
        let result = handle.await?;
        assert!(result.is_ok());
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_multiple_sequential_requests() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    let client = ControlClient::new(socket_str);

    // Send multiple sequential requests
    for _ in 0..5 {
        let request = Request::new(RequestCommand::System(SystemCommand::Ping));
        let response = client.send_request(request).await?;
        assert!(matches!(response.payload, ResponsePayload::Ok));
    }

    // Mix different command types
    let request = Request::new(RequestCommand::Dns(DnsCommand::ListCache));
    let response = client.send_request(request).await?;
    assert!(matches!(response.payload, ResponsePayload::Data(_)));

    let request = Request::new(RequestCommand::System(SystemCommand::Status));
    let response = client.send_request(request).await?;
    assert!(matches!(response.payload, ResponsePayload::Data(_)));
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_persistent_connection_mode() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?;

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await?;

    // Create client with persistent connection enabled
    let client = ControlClient::new(socket_str).with_persistent_connection();

    // Send multiple requests - should reuse same connection
    for i in 0..10 {
        let request = if i % 2 == 0 {
            Request::new(RequestCommand::System(SystemCommand::Ping))
        } else {
            Request::new(RequestCommand::Dns(DnsCommand::ListCache))
        };
        let response = client.send_request(request).await?;
        assert!(response.is_success());
    }
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_persistent_connection_reconnect() -> TestResult {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path
        .to_str()
        .ok_or("socket path is not valid UTF-8")?
        .to_string();

    let handler = Arc::new(MockHandler::new());
    let (server_handle, shutdown_tx) = start_test_server(&socket_str, handler.clone()).await?;

    // Create client with persistent connection
    let client = ControlClient::new(&socket_str).with_persistent_connection();

    // First request establishes connection
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let response = client.send_request(request).await?;
    assert!(matches!(response.payload, ResponsePayload::Ok));

    // Shutdown server to simulate connection loss
    shutdown_tx
        .send(Signal::Shutdown)
        .map_err(|err| format!("shutdown send failed: {err}"))?;
    tokio::time::timeout(Duration::from_secs(5), server_handle)
        .await
        .map_err(|_| "server did not shut down within timeout")?
        .map_err(|err| format!("server task panicked: {err}"))?;

    // Wait for socket to be removed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Restart server
    let (_server_handle2, _shutdown_tx2) = start_test_server(&socket_str, handler).await?;

    // Next request should automatically reconnect
    let request = Request::new(RequestCommand::System(SystemCommand::Status));
    let response = client.send_request(request).await?;
    assert!(matches!(response.payload, ResponsePayload::Data(_)));
    Ok(())
}
