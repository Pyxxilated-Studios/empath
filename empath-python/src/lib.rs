//! Python module support for Empath
//!
//! This crate enables writing Empath validation modules in Python using `PyO3`.
//! It provides:
//! - A Pythonic API for accessing Context data
//! - Module loading from .py files
//! - Safety rails (timeouts, exception handling)
//! - Integration with the existing module dispatch system
//!
//! # Architecture
//!
//! The Python module system uses an embedded Python interpreter (`PyO3`) running
//! in-process. Python modules are loaded from .py files and callbacks are
//! discovered by introspection.
//!
//! # Safety
//!
//! - Timeouts prevent runaway Python code
//! - Python exceptions are caught and converted to Rust errors
//! - Resource cleanup is automatic via `PyO3`'s reference counting
//! - Signal handlers are preserved across Python calls

mod context;
mod error;
mod loader;
mod safety;

pub use context::PyContext;
pub use error::{PythonError, Result};
pub use loader::PythonModule;
