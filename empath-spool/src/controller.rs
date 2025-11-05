use std::{
    io::{Error, ErrorKind},
    path::{Path, PathBuf},
};

use empath_common::{Signal, internal};
use empath_tracing::traced;
use serde::Deserialize;
use tokio::fs;

use crate::spool::Spool;

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Debug, Deserialize)]
pub struct Controller {
    path: std::path::PathBuf,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/spool/empath"),
        }
    }
}

impl Controller {
    ///
    ///
    /// # Errors
    /// If the spool path cannot be created, or is not a directory
    ///
    pub fn init(&mut self) -> anyhow::Result<()> {
        internal!("Initialising Spool ...");

        let path = Path::new(&self.path);
        if !path.try_exists()? {
            internal!("{:#?} does not exist, creating...", self.path);
            std::fs::create_dir_all(path)?;
        } else if !path.is_dir() {
            return anyhow::Result::Err(
                Error::new(
                    ErrorKind::NotADirectory,
                    format!(
                        "Expected {} to be a Directory, but it is not",
                        path.display()
                    ),
                )
                .into(),
            );
        }

        Ok(())
    }

    /// Write a message to the spool directory
    ///
    /// # Errors
    /// If the message data or metadata cannot be written to disk
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self, message), fields(id = message.id)), timing(precision = "ms"))]
    pub async fn spool_message(&self, message: &crate::message::Message) -> anyhow::Result<()> {
        let data_path = self.path.join(message.data_filename());
        let meta_path = self.path.join(message.meta_filename());

        // Write to temporary files first, then atomically rename
        let temp_data_path = self.path.join(format!(".tmp_{}", message.data_filename()));
        let temp_meta_path = self.path.join(format!(".tmp_{}", message.meta_filename()));

        // Write the email data
        fs::write(&temp_data_path, message.data.as_ref()).await?;

        // Write the metadata as JSON
        let metadata = serde_json::to_string_pretty(&message)?;
        fs::write(&temp_meta_path, metadata).await?;

        // Atomically rename both files
        fs::rename(&temp_data_path, &data_path).await?;
        fs::rename(&temp_meta_path, &meta_path).await?;

        internal!(
            level = DEBUG,
            "Spooled message {} to {}",
            message.id,
            data_path.display()
        );

        Ok(())
    }

    ///
    ///
    /// # Errors
    /// If there are any issues watching the spool directory
    ///
    pub async fn serve(
        &self,
        mut shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> anyhow::Result<()> {
        let path = self.path.clone();
        internal!("Serving spool at {path:?}");

        tokio::select! {
            _r = shutdown.recv() => {
                internal!(level = INFO, "Received Shutdown signal, shutting down");
                Ok(())
            }
        }
    }
}

impl Spool for Controller {
    fn spool_message(
        &self,
        message: &crate::message::Message,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        // Clone the necessary data to avoid lifetime issues
        let data_path = self.path.join(message.data_filename());
        let meta_path = self.path.join(message.meta_filename());
        let temp_data_path = self.path.join(format!(".tmp_{}", message.data_filename()));
        let temp_meta_path = self.path.join(format!(".tmp_{}", message.meta_filename()));
        let message_data = message.data.clone();
        let message_id = message.id;
        let message_clone = message.clone();

        Box::pin(async move {
            use tokio::fs;

            // Write the email data
            fs::write(&temp_data_path, message_data.as_ref()).await?;

            // Write the metadata as JSON
            let metadata = serde_json::to_string_pretty(&message_clone)?;
            fs::write(&temp_meta_path, metadata).await?;

            // Atomically rename both files
            fs::rename(&temp_data_path, &data_path).await?;
            fs::rename(&temp_meta_path, &meta_path).await?;

            internal!(
                level = DEBUG,
                "Spooled message {} to {}",
                message_id,
                data_path.display()
            );

            Ok(())
        })
    }
}
