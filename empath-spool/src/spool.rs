use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use tokio::sync::Notify;

use crate::message::Message;

/// Trait for spooling messages
pub trait Spool: Send + Sync + std::fmt::Debug {
    /// Spool a message
    ///
    /// # Errors
    /// If the message cannot be spooled
    fn spool_message(
        &self,
        message: &Message,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

/// Mock implementation of Spool for testing
#[derive(Debug, Clone, Default)]
pub struct MockController {
    messages: Arc<Mutex<Vec<Message>>>,
    notify: Arc<Notify>,
}

impl MockController {
    /// Create a new mock controller
    pub fn new() -> Self {
        Self {
            messages: Arc::new(Mutex::new(Vec::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Get all spooled messages
    ///
    /// # Panics
    /// Panics if the mutex is poisoned
    pub fn messages(&self) -> Vec<Message> {
        self.messages
            .lock()
            .expect("MockController messages mutex poisoned")
            .clone()
    }

    /// Get the number of spooled messages
    ///
    /// # Panics
    /// Panics if the mutex is poisoned
    pub fn message_count(&self) -> usize {
        self.messages
            .lock()
            .expect("MockController messages mutex poisoned")
            .len()
    }

    /// Clear all spooled messages
    ///
    /// # Panics
    /// Panics if the mutex is poisoned
    pub fn clear(&self) {
        self.messages
            .lock()
            .expect("MockController messages mutex poisoned")
            .clear();
    }

    /// Get a specific message by index
    ///
    /// # Panics
    /// Panics if the mutex is poisoned
    pub fn get_message(&self, index: usize) -> Option<Message> {
        self.messages
            .lock()
            .expect("MockController messages mutex poisoned")
            .get(index)
            .cloned()
    }

    /// Wait for the next message to be spooled
    ///
    /// This is useful in tests to ensure spool operations complete before assertions
    pub async fn wait_for_spool(&self) {
        self.notify.notified().await;
    }

    /// Wait for a specific number of messages to be spooled, with timeout
    ///
    /// # Errors
    /// Returns an error if the timeout is reached before the expected count
    pub async fn wait_for_count(
        &self,
        expected: usize,
        timeout: std::time::Duration,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(timeout, async {
            loop {
                if self.message_count() >= expected {
                    return;
                }
                self.notify.notified().await;
            }
        })
        .await?;
        Ok(())
    }
}

impl Spool for MockController {
    fn spool_message(
        &self,
        message: &Message,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        let message = message.clone();
        let messages = self.messages.clone();
        let notify = self.notify.clone();
        Box::pin(async move {
            messages
                .lock()
                .expect("MockController messages mutex poisoned")
                .push(message);
            notify.notify_waiters();
            Ok(())
        })
    }
}
