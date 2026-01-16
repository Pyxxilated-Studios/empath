//! Delivery job abstraction for message delivery lifecycle
//!
//! This module provides the `DeliveryJob` struct which encapsulates the common
//! parameter group (`processor`, `message_id`, `spool`) that is passed through
//! most delivery functions. This reduces parameter passing and provides a
//! cleaner API for delivery operations.

use std::sync::Arc;

use empath_common::DeliveryStatus;
use empath_common::context::Context;
use empath_common::tracing::warn;
use empath_spool::{BackingStore, SpooledMessageId};

use crate::error::{DeliveryError, SystemError};
use crate::processor::DeliveryProcessor;
use crate::types::DeliveryInfo;

/// Represents a delivery job for a single message
///
/// This struct encapsulates the core trio of parameters that flow through
/// most delivery operations: the processor, message ID, and spool reference.
/// It provides methods for common operations like marking status, loading
/// context, and persisting state.
pub struct DeliveryJob<'a> {
    pub(crate) processor: &'a DeliveryProcessor,
    pub(crate) message_id: &'a SpooledMessageId,
    pub(crate) spool: &'a Arc<dyn BackingStore>,
}

impl<'a> DeliveryJob<'a> {
    /// Create a new delivery job
    #[must_use]
    pub const fn new(
        processor: &'a DeliveryProcessor,
        message_id: &'a SpooledMessageId,
        spool: &'a Arc<dyn BackingStore>,
    ) -> Self {
        Self {
            processor,
            message_id,
            spool,
        }
    }

    /// Mark the message as in-progress and persist the state
    pub async fn mark_in_progress(&self) {
        self.processor
            .queue
            .update_status(self.message_id, DeliveryStatus::InProgress);

        if let Err(e) = self.persist().await {
            warn!(
                message_id = %self.message_id,
                error = %e,
                "Failed to persist delivery state after status update to InProgress"
            );
        }
    }

    /// Load the context and delivery info from spool and queue
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be read from spool or is not in the queue
    pub async fn load(&self) -> Result<LoadedDeliveryJob, DeliveryError> {
        let mut context = self
            .spool
            .read_metadata(self.message_id)
            .await
            .map_err(|e| SystemError::SpoolRead(e.to_string()))?;
        context.audit_logger = Some(Arc::clone(&self.processor.audit_logger));

        let info = self.processor.queue.get(self.message_id).ok_or_else(|| {
            SystemError::MessageNotFound(format!("Message {:?} not in queue", self.message_id))
        })?;

        Ok(LoadedDeliveryJob { context, info })
    }

    /// Persist the current delivery queue state to the spool
    ///
    /// This method synchronizes the in-memory queue state (status, attempts, retry timing)
    /// to the spool's persistent storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the message is not in the queue or if spool update fails
    pub async fn persist(&self) -> Result<(), DeliveryError> {
        super::delivery::persist_delivery_state(self.processor, self.message_id, self.spool).await
    }
}

/// A delivery job with loaded context and info
///
/// This represents a delivery job after the context and delivery info have
/// been loaded from storage. It provides access to all the state needed
/// to process a delivery attempt.
pub struct LoadedDeliveryJob {
    /// The message context loaded from spool
    pub context: Context,
    /// The delivery info loaded from queue
    pub info: DeliveryInfo,
}
