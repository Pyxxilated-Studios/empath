//! Module trait definitions for the Empath MTA module system.
//!
//! This module provides the core trait abstractions that all module implementations
//! (Python, WASM, `SharedLibrary`, Core, Metrics, Test) must implement.

use std::any::Any;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

use empath_common::context::{Context, ModuleContext};

use super::error::ModuleError;
use super::{Error, Event};

/// Result type for module operations.
pub type ModuleResult<T> = Result<T, ModuleError>;

/// Initialization context passed to modules during registry build.
#[derive(Debug, Default, Clone)]
pub struct ModuleInitContext {
    /// Outbound binding names for WASM module helpers.
    pub outbound_bindings: Vec<String>,
}

/// Async trait for all module implementations.
///
/// This trait defines the core interface that all modules (Python, WASM,
/// `SharedLibrary`, Core, Metrics, Test) must implement.
///
/// # Design Decisions
///
/// - Uses native async traits (RPITIT) instead of `async_trait` crate for:
///   - No heap allocation overhead from boxing futures
///   - Better compiler optimizations
///   - Cleaner error messages
///
/// - `emit` is async to support:
///   - WASM modules using epoch-based timeouts with tokio
///   - Python modules potentially using async execution in future
///   - Uniform async dispatch in `ModuleRegistry`
///
/// - `Send + Sync` bounds required for `Arc<dyn DynModule>` usage in registry
pub trait ModuleInterface: Debug + Send + Sync {
    /// Initialize the module with runtime context.
    ///
    /// Called once during `ModuleRegistry::build()` before any events are dispatched.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails (missing files, invalid config, etc.)
    fn init(&mut self, context: &ModuleInitContext) -> Result<(), Error>;

    /// Emit an event to this module.
    ///
    /// # Arguments
    ///
    /// * `event` - The validation or lifecycle event to handle
    /// * `context` - Mutable SMTP context for reading/writing envelope, message, etc.
    ///
    /// # Returns
    ///
    /// * `0` - Accept / success
    /// * Non-zero - Reject code (SMTP status) for validation events
    fn emit(&self, event: Event, context: &mut Context) -> impl Future<Output = i32> + Send;

    /// Get module metadata for metrics and logging.
    fn module_context(&self) -> ModuleContext;

    /// Human-readable module name for display/logging.
    fn display_name(&self) -> String;
}

/// Boxed future type for dynamic dispatch.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Object-safe wrapper trait for dynamic dispatch.
///
/// This trait provides the same interface as [`ModuleInterface`] but with boxed futures,
/// enabling `Arc<dyn Module>` usage in the registry.
///
/// The [`Module`] trait uses RPITIT (Return Position Impl Trait in Trait) which
/// is not object-safe. This wrapper trait boxes the futures to enable dynamic
/// dispatch via trait objects.
#[typetag::serde(tag = "type")]
pub trait Module: Debug + Send + Sync {
    /// Initialize the module with runtime context.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails (missing files, invalid config, etc.)
    fn init(&mut self, context: &ModuleInitContext) -> Result<(), Error>;

    /// Emit an event to this module (async, boxed for object safety).
    fn emit<'a>(&'a self, event: Event, context: &'a mut Context) -> BoxFuture<'a, i32>;

    /// Get module metadata for metrics and logging.
    fn module_context(&self) -> ModuleContext;

    /// Human-readable module name for display/logging.
    fn display_name(&self) -> String;

    fn as_any(&self) -> &dyn Any;
}
