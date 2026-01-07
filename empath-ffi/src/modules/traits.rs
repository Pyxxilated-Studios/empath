//! Module trait definitions for the Empath MTA module system.
//!
//! This module provides the core trait abstractions that all module implementations
//! (Python, WASM, `SharedLibrary`, Core, Metrics, Test) must implement.

use std::any::Any;
use std::fmt::Debug;

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
    fn emit(&self, event: Event, context: &mut Context) -> i32;

    /// Get module metadata for metrics and logging.
    fn module_context(&self) -> ModuleContext;

    /// Human-readable module name for display/logging.
    fn display_name(&self) -> String;

    fn as_any(&self) -> &dyn Any;
}
