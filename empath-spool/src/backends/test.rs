use std::sync::Arc;

use async_trait::async_trait;
use empath_common::context::Context;
use tokio::sync::Notify;

use super::memory::MemoryBackingStore;
use crate::{SpoolError, r#trait::BackingStore, types::SpooledMessageId};

/// Testing utilities for memory-backed spool
///
/// This wrapper adds test-specific functionality like waiting for operations
/// to complete and clearing the store.
#[derive(Debug, Clone)]
pub struct TestBackingStore {
    pub(crate) inner: MemoryBackingStore,
    notify: Arc<Notify>,
}

impl Default for TestBackingStore {
    fn default() -> Self {
        Self {
            inner: MemoryBackingStore::new(),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl TestBackingStore {
    /// Create a new test backing store
    pub fn new() -> Self {
        Self::default()
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
    ) -> crate::Result<()> {
        tokio::time::timeout(timeout, async {
            loop {
                let count = self.inner.list().await.unwrap_or_default().len();
                if count >= expected {
                    return;
                }
                self.notify.notified().await;
            }
        })
        .await
        .map_err(|e| SpoolError::Internal(format!("Timeout waiting for messages: {e}")))?;
        Ok(())
    }

    /// Clear all messages from the store
    pub fn clear(&self) {
        self.inner.messages.clear();
    }

    /// Get the number of spooled messages
    pub fn message_count(&self) -> usize {
        self.inner.messages.len()
    }

    /// Get all messages (for test assertions)
    ///
    /// # Errors
    /// If there is an issue with listing the messages inside this store
    pub async fn messages(&self) -> crate::Result<Vec<Context>> {
        let ids = self.inner.list().await?;
        let mut messages = Vec::new();
        for id in ids {
            messages.push(self.inner.read(&id).await?);
        }
        Ok(messages)
    }
}

#[async_trait]
impl BackingStore for TestBackingStore {
    async fn write(&self, context: &mut Context) -> crate::Result<SpooledMessageId> {
        let id = self.inner.write(context).await?;
        self.notify.notify_waiters();
        Ok(id)
    }

    async fn list(&self) -> crate::Result<Vec<SpooledMessageId>> {
        self.inner.list().await
    }

    async fn read(&self, id: &SpooledMessageId) -> crate::Result<Context> {
        self.inner.read(id).await
    }

    async fn update(&self, id: &SpooledMessageId, context: &Context) -> crate::Result<()> {
        self.inner.update(id, context).await
    }

    async fn delete(&self, id: &SpooledMessageId) -> crate::Result<()> {
        self.inner.delete(id).await
    }
}
