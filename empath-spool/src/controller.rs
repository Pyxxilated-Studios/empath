use std::{
    io::{Error, ErrorKind},
    path::{Path, PathBuf},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

use empath_common::{internal, Signal};
use tokio::sync::mpsc::{channel, Receiver};

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Serialize, Deserialize)]
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
                    format!("Expected {path:?} to be a Directory, but it is not"),
                )
                .into(),
            );
        }

        Ok(())
    }

    fn watcher() -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
        let (tx, rx) = channel(1);

        let watcher = notify::recommended_watcher(move |res| {
            let _ = tx.blocking_send(res);
        })?;

        Ok((watcher, rx))
    }

    async fn watch<P: AsRef<Path>>(path: P) -> notify::Result<()> {
        let (mut watcher, mut rx) = Self::watcher()?;

        watcher.watch(path.as_ref(), RecursiveMode::Recursive)?;

        while let Some(res) = rx.recv().await {
            match res {
                Ok(event) => println!("changed: {event:?}"),
                Err(e) => println!("watch error: {e:?}"),
            }
        }

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

            r = Self::watch(path) => {
                Ok(r?)
            }
        }
    }
}
