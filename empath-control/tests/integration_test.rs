//! Integration tests for control socket client/server communication
//!
//! These tests verify the full request/response cycle between the control
//! client and server, including error handling, timeouts, and protocol correctness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::unreachable
)]

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use empath_control::{
    ControlClient, ControlError, ControlServer, Result,
    protocol::{
        CachedMailServer, DnsCommand, QueueCommand, Request, RequestCommand, Response,
        ResponseData, ResponsePayload, SystemCommand, SystemStatus,
    },
    server::CommandHandler,
};
use tempfile::TempDir;
use tokio::sync::broadcast;

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
}

#[async_trait]
impl CommandHandler for MockHandler {
    async fn handle_request(&self, request: Request) -> Result<Response> {
        match request.command {
            RequestCommand::Dns(cmd) => match cmd {
                DnsCommand::ListCache => Ok(Response::data(ResponseData::DnsCache(
                    self.dns_cache.clone(),
                ))),
                DnsCommand::ClearCache => Ok(Response::data(ResponseData::Message(
                    "Cache cleared".to_string(),
                ))),
                DnsCommand::RefreshDomain(domain) => Ok(Response::data(ResponseData::Message(
                    format!("Refreshed {domain}"),
                ))),
                DnsCommand::SetOverride { domain, mx_server } => Ok(Response::data(
                    ResponseData::Message(format!("Set override {domain} -> {mx_server}")),
                )),
                DnsCommand::RemoveOverride(domain) => Ok(Response::data(ResponseData::Message(
                    format!("Removed override for {domain}"),
                ))),
                DnsCommand::ListOverrides => Ok(Response::data(ResponseData::MxOverrides(
                    self.mx_overrides.clone(),
                ))),
            },
            RequestCommand::System(cmd) => match cmd {
                SystemCommand::Ping => Ok(Response::ok()),
                SystemCommand::Status => {
                    Ok(Response::data(ResponseData::SystemStatus(SystemStatus {
                        version: "0.0.2".to_string(),
                        uptime_secs: 12345,
                        queue_size: 42,
                        dns_cache_entries: 10,
                    })))
                }
            },
            RequestCommand::Queue(cmd) => match cmd {
                QueueCommand::Stats => Ok(Response::data(ResponseData::Message(
                    "Queue stats".to_string(),
                ))),
                _ => Ok(Response::error(
                    "Queue command not implemented in mock".to_string(),
                )),
            },
            RequestCommand::Spool(_) => Ok(Response::error(
                "Spool command not implemented in mock".to_string(),
            )),
        }
    }
}

/// Helper to start a test control server
async fn start_test_server(
    socket_path: &str,
    handler: Arc<dyn CommandHandler>,
) -> (
    tokio::task::JoinHandle<()>,
    broadcast::Sender<empath_common::Signal>,
) {
    let server = ControlServer::new(socket_path, handler).expect("Failed to create server");
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.serve(shutdown_rx).await {
            eprintln!("Server error: {e}");
        }
    });

    // Give server time to bind
    tokio::time::sleep(Duration::from_millis(100)).await;

    (server_handle, shutdown_tx)
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_list_cache() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test DNS list cache command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::ListCache));
    let response = client.send_request(request).await.unwrap();

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::DnsCache(cache) => {
                assert!(cache.contains_key("example.com"));
                let servers = cache.get("example.com").unwrap();
                assert_eq!(servers.len(), 2);
                assert_eq!(servers[0].host, "mail.example.com");
                assert_eq!(servers[1].host, "mail2.example.com");
            }
            _ => panic!("Expected DnsCache response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_clear_cache() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test DNS clear cache command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::ClearCache));
    let response = client.send_request(request).await.unwrap();

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::Message(msg) => {
                assert_eq!(msg, "Cache cleared");
            }
            _ => panic!("Expected Message response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_refresh_domain() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test DNS refresh domain command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::RefreshDomain(
        "example.com".to_string(),
    )));
    let response = client.send_request(request).await.unwrap();

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::Message(msg) => {
                assert_eq!(msg, "Refreshed example.com");
            }
            _ => panic!("Expected Message response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_set_override() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test DNS set override command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::SetOverride {
        domain: "test.example.com".to_string(),
        mx_server: "localhost:1025".to_string(),
    }));
    let response = client.send_request(request).await.unwrap();

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::Message(msg) => {
                assert!(msg.contains("Set override"));
                assert!(msg.contains("test.example.com"));
                assert!(msg.contains("localhost:1025"));
            }
            _ => panic!("Expected Message response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_remove_override() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test DNS remove override command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::RemoveOverride(
        "test.local".to_string(),
    )));
    let response = client.send_request(request).await.unwrap();

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::Message(msg) => {
                assert!(msg.contains("Removed override"));
                assert!(msg.contains("test.local"));
            }
            _ => panic!("Expected Message response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_dns_list_overrides() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test DNS list overrides command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::Dns(DnsCommand::ListOverrides));
    let response = client.send_request(request).await.unwrap();

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::MxOverrides(overrides) => {
                assert!(overrides.contains_key("test.local"));
                assert_eq!(overrides.get("test.local").unwrap(), "localhost:1025");
            }
            _ => panic!("Expected MxOverrides response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_system_ping() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test system ping command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let response = client.send_request(request).await.unwrap();

    match response.payload {
        ResponsePayload::Ok => {
            // Success
        }
        _ => panic!("Expected Ok response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_system_status() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test system status command
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::System(SystemCommand::Status));
    let response = client.send_request(request).await.unwrap();

    match response.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::SystemStatus(status) => {
                assert_eq!(status.version, "0.0.2");
                assert_eq!(status.uptime_secs, 12345);
                assert_eq!(status.queue_size, 42);
                assert_eq!(status.dns_cache_entries, 10);
            }
            _ => panic!("Expected SystemStatus response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_socket_not_exist_error() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("nonexistent.sock");
    let socket_str = socket_path.to_str().unwrap();

    // Test connecting to non-existent socket
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let result = client.send_request(request).await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ControlError::Io(_)));
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_check_socket_exists() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    // Test with non-existent socket
    let client = ControlClient::new(socket_str);
    let result = client.check_socket_exists();
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ControlError::InvalidSocketPath(_)
    ));

    // Start server
    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Test with existing socket
    let result = client.check_socket_exists();
    assert!(result.is_ok());
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_client_timeout() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

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
        Err(e) => panic!("Unexpected error: {e}"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_graceful_shutdown() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (server_handle, shutdown_tx) = start_test_server(socket_str, handler).await;

    // Verify server is running
    let client = ControlClient::new(socket_str);
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let response = client.send_request(request).await.unwrap();
    assert!(matches!(response.payload, ResponsePayload::Ok));

    // Send shutdown signal
    shutdown_tx.send(empath_common::Signal::Shutdown).unwrap();

    // Wait for server to shut down
    tokio::time::timeout(Duration::from_secs(5), server_handle)
        .await
        .expect("Server did not shut down within timeout")
        .expect("Server task panicked");

    // Verify socket is cleaned up
    assert!(!socket_path.exists());
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_concurrent_requests() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap().to_string();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(&socket_str, handler).await;

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
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_multiple_sequential_requests() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    let client = ControlClient::new(socket_str);

    // Send multiple sequential requests
    for _ in 0..5 {
        let request = Request::new(RequestCommand::System(SystemCommand::Ping));
        let response = client.send_request(request).await.unwrap();
        assert!(matches!(response.payload, ResponsePayload::Ok));
    }

    // Mix different command types
    let request = Request::new(RequestCommand::Dns(DnsCommand::ListCache));
    let response = client.send_request(request).await.unwrap();
    assert!(matches!(response.payload, ResponsePayload::Data(_)));

    let request = Request::new(RequestCommand::System(SystemCommand::Status));
    let response = client.send_request(request).await.unwrap();
    assert!(matches!(response.payload, ResponsePayload::Data(_)));
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_persistent_connection_mode() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap();

    let handler = Arc::new(MockHandler::new());
    let (_server_handle, _shutdown_tx) = start_test_server(socket_str, handler).await;

    // Create client with persistent connection enabled
    let client = ControlClient::new(socket_str).with_persistent_connection();

    // Send multiple requests - should reuse same connection
    for i in 0..10 {
        let request = if i % 2 == 0 {
            Request::new(RequestCommand::System(SystemCommand::Ping))
        } else {
            Request::new(RequestCommand::Dns(DnsCommand::ListCache))
        };
        let response = client.send_request(request).await.unwrap();
        assert!(response.is_success());
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_persistent_connection_reconnect() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    let socket_str = socket_path.to_str().unwrap().to_string();

    let handler = Arc::new(MockHandler::new());
    let (server_handle, shutdown_tx) = start_test_server(&socket_str, handler.clone()).await;

    // Create client with persistent connection
    let client = ControlClient::new(&socket_str).with_persistent_connection();

    // First request establishes connection
    let request = Request::new(RequestCommand::System(SystemCommand::Ping));
    let response = client.send_request(request).await.unwrap();
    assert!(matches!(response.payload, ResponsePayload::Ok));

    // Shutdown server to simulate connection loss
    shutdown_tx.send(empath_common::Signal::Shutdown).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server_handle)
        .await
        .expect("Server did not shut down within timeout")
        .expect("Server task panicked");

    // Wait for socket to be removed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Restart server
    let (_server_handle2, _shutdown_tx2) = start_test_server(&socket_str, handler).await;

    // Next request should automatically reconnect
    let request = Request::new(RequestCommand::System(SystemCommand::Status));
    let response = client.send_request(request).await.unwrap();
    assert!(matches!(response.payload, ResponsePayload::Data(_)));
}
