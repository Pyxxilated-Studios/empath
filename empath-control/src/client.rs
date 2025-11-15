//! Client for connecting to the control socket

use std::{path::Path, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Mutex,
};
use tracing::{debug, trace, warn};

use crate::{ControlError, Request, Response, Result};

/// Maximum response size to prevent `DoS` attacks (10MB)
/// This is generous enough for large DNS cache responses while preventing memory exhaustion
const MAX_RESPONSE_SIZE: u32 = 10_000_000;

/// Client for communicating with the Empath control server
pub struct ControlClient {
    socket_path: String,
    timeout: Duration,
    /// Optional persistent connection for watch mode to avoid reconnection overhead
    persistent_connection: Option<Arc<Mutex<Option<UnixStream>>>>,
}

impl ControlClient {
    /// Create a new control client with the given socket path
    #[must_use]
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            timeout: Duration::from_secs(10),
            persistent_connection: None,
        }
    }

    /// Set the request timeout
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Enable persistent connection mode for reduced overhead in watch mode
    ///
    /// When enabled, the client will maintain a single connection across multiple
    /// requests instead of creating a new connection for each request. This is
    /// particularly useful for `--watch` mode to avoid socket connection overhead.
    ///
    /// The connection will automatically reconnect if lost.
    #[must_use]
    pub fn with_persistent_connection(mut self) -> Self {
        self.persistent_connection = Some(Arc::new(Mutex::new(None)));
        self
    }

    /// Connect to the control server
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails
    async fn connect(&self) -> Result<UnixStream> {
        debug!("Connecting to control socket: {}", self.socket_path);
        let stream = UnixStream::connect(&self.socket_path).await?;
        Ok(stream)
    }

    /// Send a request and receive a response
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Connection fails
    /// - Protocol error occurs
    /// - Request times out
    /// - Server returns an error
    pub async fn send_request(&self, request: Request) -> Result<Response> {
        // Apply timeout to the entire request/response cycle
        tokio::time::timeout(self.timeout, self.send_request_internal(request))
            .await
            .map_err(|_| ControlError::Timeout)?
    }

    async fn send_request_internal(&self, request: Request) -> Result<Response> {
        // Check if we're in persistent connection mode
        if let Some(persistent) = &self.persistent_connection {
            self.send_request_persistent(request, persistent).await
        } else {
            self.send_request_oneshot(request).await
        }
    }

    /// Send a request using a one-shot connection (traditional mode)
    async fn send_request_oneshot(&self, request: Request) -> Result<Response> {
        let mut stream = self.connect().await?;
        self.send_and_receive(&mut stream, request).await
    }

    /// Send a request using persistent connection with automatic reconnection
    async fn send_request_persistent(
        &self,
        request: Request,
        persistent: &Arc<Mutex<Option<UnixStream>>>,
    ) -> Result<Response> {
        let mut guard = persistent.lock().await;

        // Try to use existing connection, or create new one
        let result = if let Some(stream) = guard.as_mut() {
            self.send_and_receive(stream, request.clone()).await
        } else {
            // No connection exists, create new one
            let mut stream = self.connect().await?;
            let result = self.send_and_receive(&mut stream, request.clone()).await;
            if result.is_ok() {
                *guard = Some(stream);
            }
            result
        };

        // If connection failed, try to reconnect once
        if result.is_err() {
            warn!(
                "Persistent connection failed, reconnecting to {}",
                self.socket_path
            );
            *guard = None;
            drop(guard); // Release lock before reconnecting

            // Reconnect and retry
            let mut stream = self.connect().await?;
            let result = self.send_and_receive(&mut stream, request).await;

            // Store new connection if successful
            if result.is_ok() {
                let mut guard = persistent.lock().await;
                *guard = Some(stream);
            }

            result
        } else {
            result
        }
    }

    /// Send request and receive response on an existing stream
    ///
    /// # Errors
    ///
    /// Returns an error if I/O fails or protocol error occurs
    async fn send_and_receive(
        &self,
        stream: &mut UnixStream,
        request: Request,
    ) -> Result<Response> {
        // Serialize request
        let request_bytes = bincode::serialize(&request)?;
        let request_len = u32::try_from(request_bytes.len())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        trace!("Sending request: {request_len} bytes");

        // Send length prefix (4 bytes) + request
        stream.write_all(&request_len.to_be_bytes()).await?;
        stream.write_all(&request_bytes).await?;
        stream.flush().await?;

        // Read response length prefix
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let response_len = u32::from_be_bytes(len_buf);

        // Validate response size to prevent DoS attacks
        if response_len > MAX_RESPONSE_SIZE {
            return Err(ControlError::Protocol(Box::new(
                bincode::ErrorKind::Custom(format!(
                    "Response too large: {response_len} bytes (max {MAX_RESPONSE_SIZE})"
                )),
            )));
        }

        trace!("Receiving response: {response_len} bytes");

        // Read response
        let mut response_bytes = vec![0u8; response_len as usize];
        stream.read_exact(&mut response_bytes).await?;

        // Deserialize response
        let response: Response = bincode::deserialize(&response_bytes)?;

        // Validate protocol version
        if !response.is_version_compatible() {
            return Err(ControlError::Protocol(Box::new(
                bincode::ErrorKind::Custom(format!(
                    "Incompatible protocol version: server={}, client={}",
                    response.version,
                    crate::PROTOCOL_VERSION
                )),
            )));
        }

        // Check for server error
        if let crate::ResponsePayload::Error(ref err) = response.payload {
            return Err(ControlError::ServerError(err.clone()));
        }

        Ok(response)
    }

    /// Check if the control server is reachable
    ///
    /// # Errors
    ///
    /// Returns an error if the socket doesn't exist or connection fails
    pub fn check_socket_exists(&self) -> Result<()> {
        let path = Path::new(&self.socket_path);
        if !path.exists() {
            return Err(ControlError::InvalidSocketPath(format!(
                "Socket does not exist: {}",
                self.socket_path
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = ControlClient::new("/tmp/test.sock");
        assert_eq!(client.socket_path, "/tmp/test.sock");
        assert_eq!(client.timeout, Duration::from_secs(10));
        assert!(client.persistent_connection.is_none());
    }

    #[test]
    fn test_client_with_timeout() {
        let client = ControlClient::new("/tmp/test.sock").with_timeout(Duration::from_secs(5));
        assert_eq!(client.timeout, Duration::from_secs(5));
        assert!(client.persistent_connection.is_none());
    }

    #[test]
    fn test_client_with_persistent_connection() {
        let client = ControlClient::new("/tmp/test.sock").with_persistent_connection();
        assert!(client.persistent_connection.is_some());
    }

    #[test]
    fn test_client_builder_chain() {
        let client = ControlClient::new("/tmp/test.sock")
            .with_timeout(Duration::from_secs(5))
            .with_persistent_connection();
        assert_eq!(client.timeout, Duration::from_secs(5));
        assert!(client.persistent_connection.is_some());
    }
}
