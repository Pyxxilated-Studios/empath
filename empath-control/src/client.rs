//! Client for connecting to the control socket

use std::{path::Path, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
use tracing::{debug, trace};

use crate::{ControlError, Request, Response, Result};

/// Client for communicating with the Empath control server
pub struct ControlClient {
    socket_path: String,
    timeout: Duration,
}

impl ControlClient {
    /// Create a new control client with the given socket path
    #[must_use]
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            timeout: Duration::from_secs(10),
        }
    }

    /// Set the request timeout
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
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
        let mut stream = self.connect().await?;

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

        trace!("Receiving response: {response_len} bytes");

        // Read response
        let mut response_bytes = vec![0u8; response_len as usize];
        stream.read_exact(&mut response_bytes).await?;

        // Deserialize response
        let response: Response = bincode::deserialize(&response_bytes)?;

        // Check for server error
        if let Response::Error(ref err) = response {
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
    }

    #[test]
    fn test_client_with_timeout() {
        let client = ControlClient::new("/tmp/test.sock").with_timeout(Duration::from_secs(5));
        assert_eq!(client.timeout, Duration::from_secs(5));
    }
}
