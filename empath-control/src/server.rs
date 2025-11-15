//! Control server implementation

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::broadcast,
};
use tracing::{debug, error, info, trace, warn};

use crate::{ControlError, Request, Response, Result};

/// Handler trait for processing control requests
///
/// Implement this trait to handle specific command types
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Handle a request and return a response
    ///
    /// # Errors
    ///
    /// Returns an error if the command cannot be processed
    async fn handle_request(&self, request: Request) -> Result<Response>;
}

/// Control server for managing the Empath MTA via Unix domain socket
pub struct ControlServer {
    socket_path: String,
    handler: Arc<dyn CommandHandler>,
}

impl ControlServer {
    /// Create a new control server
    ///
    /// # Errors
    ///
    /// Returns an error if the socket path is invalid
    pub fn new(socket_path: impl Into<String>, handler: Arc<dyn CommandHandler>) -> Result<Self> {
        Ok(Self {
            socket_path: socket_path.into(),
            handler,
        })
    }

    /// Start the control server
    ///
    /// This function runs until a shutdown signal is received.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The socket cannot be bound
    /// - A fatal I/O error occurs
    pub async fn serve(
        &self,
        mut shutdown: broadcast::Receiver<empath_common::Signal>,
    ) -> Result<()> {
        // Check for existing socket file
        let socket_path = Path::new(&self.socket_path);
        if socket_path.exists() {
            // Test if socket is active by attempting connection
            if UnixStream::connect(socket_path).await.is_ok() {
                // Active socket - another instance is running
                return Err(ControlError::Io(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!(
                        "Socket already in use by running instance: {}",
                        self.socket_path
                    ),
                )));
            }
            // Stale socket from crashed process, safe to remove
            info!("Removing stale socket file: {}", self.socket_path);
            tokio::fs::remove_file(socket_path).await?;
        }

        // Bind the Unix socket
        let listener = UnixListener::bind(&self.socket_path)?;

        // Set restrictive permissions (owner only: rw-------)
        #[cfg(unix)]
        {
            let metadata = tokio::fs::metadata(&self.socket_path).await?;
            let mut perms = metadata.permissions();
            perms.set_mode(0o600); // Owner read/write only
            tokio::fs::set_permissions(&self.socket_path, perms).await?;
            info!(
                "Control socket created with mode 0600 (owner only): {}",
                self.socket_path
            );
        }
        #[cfg(not(unix))]
        {
            info!("Control server listening on: {}", self.socket_path);
        }

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            let handler = Arc::clone(&self.handler);
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection(stream, handler).await {
                                    error!("Error handling control connection: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            error!("Error accepting control connection: {e}");
                        }
                    }
                }
                sig = shutdown.recv() => {
                    match sig {
                        Ok(empath_common::Signal::Shutdown | empath_common::Signal::Finalised) => {
                            info!("Control server shutting down");
                            break;
                        }
                        Err(e) => {
                            error!("Control server shutdown channel error: {e}");
                            break;
                        }
                    }
                }
            }
        }

        // Clean up socket file
        if socket_path.exists() {
            debug!("Removing socket file: {}", self.socket_path);
            let _ = tokio::fs::remove_file(socket_path).await;
        }

        Ok(())
    }

    /// Handle a single client connection
    async fn handle_connection(
        mut stream: UnixStream,
        handler: Arc<dyn CommandHandler>,
    ) -> Result<()> {
        // Set a read timeout to prevent hanging on malicious/broken clients
        let timeout = Duration::from_secs(30);

        // Read request with timeout
        let request = tokio::time::timeout(timeout, Self::read_request(&mut stream))
            .await
            .map_err(|_| ControlError::Timeout)??;

        trace!("Received request: {request:?}");

        // Process request
        let response = match handler.handle_request(request).await {
            Ok(response) => response,
            Err(e) => {
                warn!("Error handling request: {e}");
                Response::error(e.to_string())
            }
        };

        trace!("Sending response: {response:?}");

        // Send response with timeout
        tokio::time::timeout(timeout, Self::write_response(&mut stream, &response))
            .await
            .map_err(|_| ControlError::Timeout)??;

        Ok(())
    }

    /// Read a request from the stream
    async fn read_request(stream: &mut UnixStream) -> Result<Request> {
        // Read length prefix (4 bytes)
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                ControlError::ConnectionClosed
            } else {
                ControlError::Io(e)
            }
        })?;

        let request_len = u32::from_be_bytes(len_buf);

        // Sanity check: reject unreasonably large requests (> 1MB)
        if request_len > 1_000_000 {
            return Err(ControlError::ProtocolDeserialization(
                bincode::error::DecodeError::OtherString(format!(
                    "Request too large: {request_len} bytes"
                )),
            ));
        }

        // Read request bytes
        let mut request_bytes = vec![0u8; request_len as usize];
        stream.read_exact(&mut request_bytes).await?;

        // Deserialize request
        let (request, _): (Request, _) =
            bincode::serde::decode_from_slice(request_bytes.as_slice(), bincode::config::legacy())?;
        Ok(request)
    }

    /// Write a response to the stream
    async fn write_response(stream: &mut UnixStream, response: &Response) -> Result<()> {
        // Serialize response
        let response_bytes = bincode::serde::encode_to_vec(response, bincode::config::legacy())?;
        let response_len = u32::try_from(response_bytes.len())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Write length prefix + response
        stream.write_all(&response_len.to_be_bytes()).await?;
        stream.write_all(&response_bytes).await?;
        stream.flush().await?;

        Ok(())
    }
}
