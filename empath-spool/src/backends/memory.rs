use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use empath_common::context::Context;

use crate::{SpoolError, r#trait::BackingStore, types::SpooledMessageId};

/// In-memory backing store implementation
///
/// This implementation stores messages in a `HashMap` protected by an `RwLock`.
/// It's primarily intended for testing, but can also be used for transient
/// message handling.
///
/// # Capacity Management
/// The store can be configured with a maximum capacity to prevent unbounded
/// memory growth. When capacity is reached, write operations will fail with
/// an error. This is useful for:
/// - Testing capacity-related error handling
/// - Preventing memory exhaustion if accidentally used in production
/// - Simulating resource constraints
///
/// # Performance
/// - Write: O(1) -`HashMap` insert + Arc clone (plus capacity check)
/// - List: O(n log n) - Must clone all keys and sort
/// - Read: O(1) - `HashMap` lookup + Context clone
/// - Delete: O(1) - `HashMap` remove
///
/// # Concurrency
/// Uses an `RwLock` for interior mutability. Production workloads should use
/// file-backed or database-backed stores with better concurrency primitives.
#[derive(Debug, Clone)]
pub struct MemoryBackingStore {
    pub(crate) messages: Arc<RwLock<HashMap<SpooledMessageId, Context>>>,
    /// Maximum number of messages to store (None = unlimited)
    capacity: Option<usize>,
}

impl MemoryBackingStore {
    /// Create a new empty memory-backed store with unlimited capacity
    #[must_use]
    pub fn new() -> Self {
        Self {
            messages: Arc::new(RwLock::new(HashMap::new())),
            capacity: None,
        }
    }

    /// Create a new memory-backed store with a capacity limit
    ///
    /// # Examples
    /// ```ignore
    /// // Limit to 1000 messages
    /// let store = MemoryBackingStore::with_capacity(1000);
    /// ```
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            messages: Arc::new(RwLock::new(HashMap::new())),
            capacity: Some(capacity),
        }
    }

    /// Get the current number of messages in the store
    ///
    /// Recovers gracefully if the lock is poisoned by accessing the underlying data.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Check if the store is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the configured capacity (None = unlimited)
    #[must_use]
    pub const fn capacity(&self) -> Option<usize> {
        self.capacity
    }
}

impl Default for MemoryBackingStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BackingStore for MemoryBackingStore {
    async fn write(&self, context: &mut Context) -> crate::Result<SpooledMessageId> {
        // Generate unique ULID
        let id = SpooledMessageId::generate();

        // Set the tracking ID in the context
        context.tracking_id = Some(id.to_string());

        // Check capacity before inserting (don't count if overwriting existing)
        if let Some(cap) = self.capacity
            && !self.messages.read()?.contains_key(&id)
            && self.len() >= cap
        {
            return Err(SpoolError::Internal(format!(
                "Memory spool capacity exceeded: {}/{} messages",
                self.len(),
                cap
            )));
        }

        self.messages.write()?.insert(id.clone(), context.clone());

        Ok(id)
    }

    async fn list(&self) -> crate::Result<Vec<SpooledMessageId>> {
        let mut ids: Vec<_> = self.messages.read()?.keys().cloned().collect();

        // ULIDs are lexicographically sortable by creation time
        ids.sort();

        Ok(ids)
    }

    async fn read(&self, id: &SpooledMessageId) -> crate::Result<Context> {
        self.messages
            .read()?
            .get(id)
            .cloned()
            .ok_or_else(|| SpoolError::NotFound(id.clone()))
    }

    async fn update(&self, id: &SpooledMessageId, context: &Context) -> crate::Result<()> {
        if self.messages.read()?.contains_key(id) {
            self.messages.write()?.insert(id.clone(), context.clone());
            Ok(())
        } else {
            Err(SpoolError::NotFound(id.clone()))
        }
    }

    async fn delete(&self, id: &SpooledMessageId) -> crate::Result<()> {
        self.messages
            .write()?
            .remove(id)
            .ok_or_else(|| SpoolError::NotFound(id.clone()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ahash::AHashMap;
    use empath_common::envelope::Envelope;

    use super::*;

    fn create_test_context(data: &str) -> Context {
        Context {
            envelope: Envelope::default(),
            data: Some(Arc::from(data.as_bytes())),
            id: "test.example.com".to_string(),
            extended: false,
            metadata: AHashMap::new(),
            ..Default::default()
        }
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    async fn test_memory_store_basic_operations() {
        let store = MemoryBackingStore::new();
        let mut context = create_test_context("test message");
        let expected_data = context.data.clone();

        // Write context and get tracking ID
        let id = store.write(&mut context).await.expect("Failed to write");

        // List messages
        let ids = store.list().await.expect("Failed to list");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], id);

        // Read context
        let read_ctx = store.read(&id).await.expect("Failed to read");
        assert_eq!(read_ctx.data.as_deref(), expected_data.as_deref());
        assert_eq!(read_ctx.tracking_id, Some(id.to_string()));

        // Delete message
        store.delete(&id).await.expect("Failed to delete");
        let ids_after = store.list().await.expect("Failed to list");
        assert_eq!(ids_after.len(), 0);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    async fn test_memory_store_capacity_limit() {
        let store = MemoryBackingStore::with_capacity(2);

        // Write up to capacity
        let mut ctx1 = create_test_context("message 1");
        let mut ctx2 = create_test_context("message 2");
        store
            .write(&mut ctx1)
            .await
            .expect("First write should succeed");
        store
            .write(&mut ctx2)
            .await
            .expect("Second write should succeed");

        // Third write should fail
        let mut ctx3 = create_test_context("message 3");
        let result = store.write(&mut ctx3).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("capacity exceeded")
        );

        // After deleting one, we should be able to write again
        let ids = store.list().await.expect("Failed to list");
        store.delete(&ids[0]).await.expect("Failed to delete");

        let result = store.write(&mut ctx3).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    async fn test_unique_id_generation() {
        let store = MemoryBackingStore::new();

        // Write multiple messages concurrently
        let mut handles = vec![];
        for i in 0..100 {
            let store_clone = store.clone();
            let handle = tokio::spawn(async move {
                let mut msg = create_test_context(&format!("message {i}"));
                store_clone.write(&mut msg).await
            });
            handles.push(handle);
        }

        // Wait for all writes
        for handle in handles {
            handle.await.expect("Task panicked").expect("Write failed");
        }

        // All IDs should be unique
        let ids = store.list().await.expect("Failed to list");
        assert_eq!(ids.len(), 100);

        // Check for uniqueness
        let mut id_set = std::collections::HashSet::new();
        for id in &ids {
            assert!(id_set.insert(id.clone()), "Found duplicate ID: {id}");
        }
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    async fn test_message_ordering() {
        let store = MemoryBackingStore::new();

        // Write messages and collect IDs in generation order
        let mut generated_ids = Vec::new();
        for i in 0..10 {
            let mut msg = create_test_context(&format!("message {i}"));
            let id = store.write(&mut msg).await.expect("Failed to write");
            generated_ids.push(id);
        }

        // List should return sorted lexicographically
        let listed_ids = store.list().await.expect("Failed to list");
        assert_eq!(listed_ids.len(), 10);

        // Sort the generated IDs and verify they match the listed order
        // This tests that list() properly sorts ULIDs without relying on timing
        generated_ids.sort();
        assert_eq!(
            generated_ids, listed_ids,
            "Listed IDs should match sorted generation order"
        );

        // Verify all IDs are unique (additional sanity check)
        let unique_count = generated_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(unique_count, 10, "All ULIDs should be unique");
    }

    #[test]
    fn test_capacity_methods() {
        let unlimited = MemoryBackingStore::new();
        assert_eq!(unlimited.capacity(), None);

        let limited = MemoryBackingStore::with_capacity(100);
        assert_eq!(limited.capacity(), Some(100));
    }
}
