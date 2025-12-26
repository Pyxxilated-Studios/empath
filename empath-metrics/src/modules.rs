//! Module metrics
//!
//! Tracks metrics for FFI modules (WASM, Python, `SharedLibrary`) including:
//! - Execution errors by module type and path
//! - Execution duration by module type and callback
//! - Memory usage by module path
//! - Timeout events by module path

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use dashmap::DashMap;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, Meter},
};

/// Module metrics collector
#[derive(Debug)]
pub struct ModuleMetrics {
    /// Total number of module errors by type and path
    errors_total: Counter<u64>,

    /// Distribution of module execution durations by type and callback
    execution_duration_seconds: Histogram<f64>,

    /// Total number of module timeouts by path
    timeout_total: Counter<u64>,

    /// Total number of module message mutations
    mutations_total: Counter<u64>,

    // Per-module memory tracking (module_path -> memory_bytes)
    // Lock-free concurrent map prevents panic on lock poisoning
    memory_bytes: Arc<DashMap<String, Arc<AtomicU64>>>,
}

impl ModuleMetrics {
    /// Create a new module metrics collector
    pub fn new() -> Self {
        let meter = meter();

        let errors_total = meter
            .u64_counter("empath.modules.errors.total")
            .with_description("Total number of module errors by type and path")
            .build();

        let execution_duration_seconds = meter
            .f64_histogram("empath.modules.execution.duration.seconds")
            .with_description("Distribution of module execution durations by type and callback")
            .build();

        let timeout_total = meter
            .u64_counter("empath.modules.timeout.total")
            .with_description("Total number of module timeouts by path")
            .build();

        let mutations_total = meter
            .u64_counter("empath.modules.mutations.total")
            .with_description("Total number of module message mutations")
            .build();

        // Create atomic map for per-module memory tracking
        let memory_ref = Arc::new(DashMap::<String, Arc<AtomicU64>>::new());

        // Observable gauge for module memory usage (reads from atomic map)
        let memory_clone = memory_ref.clone();
        meter
            .u64_observable_gauge("empath.modules.memory.bytes")
            .with_description("Memory usage in bytes by module path")
            .with_callback(move |observer| {
                for entry in memory_clone.iter() {
                    let module_path = entry.key();
                    let memory_bytes = entry.value().load(Ordering::Relaxed);
                    observer.observe(
                        memory_bytes,
                        &[KeyValue::new("module_path", module_path.clone())],
                    );
                }
            })
            .build();

        Self {
            errors_total,
            execution_duration_seconds,
            timeout_total,
            mutations_total,
            memory_bytes: memory_ref,
        }
    }

    /// Record a module error
    ///
    /// # Arguments
    ///
    /// * `module_type` - Type of module (e.g., "wasm", "python", "`shared_library`")
    /// * `module_path` - Path to the module file
    pub fn record_error(&self, module_type: &str, module_path: &str) {
        self.record_error_with_kind(module_type, module_path, "generic");
    }

    /// Record a module error with an error kind
    ///
    /// # Arguments
    ///
    /// * `module_type` - Type of module (e.g., "wasm", "python", "`shared_library`")
    /// * `module_path` - Path to the module file
    /// * `error_kind` - Optional classification (e.g., "timeout", "error")
    pub fn record_error_with_kind(&self, module_type: &str, module_path: &str, error_kind: &str) {
        let attributes = [
            KeyValue::new("module_type", module_type.to_string()),
            KeyValue::new("module_path", module_path.to_string()),
            KeyValue::new("error_kind", error_kind.to_string()),
        ];
        self.errors_total.add(1, &attributes);
    }

    /// Record module execution duration
    ///
    /// # Arguments
    ///
    /// * `module_type` - Type of module (e.g., "wasm", "python", "`shared_library`")
    /// * `callback` - Name of the callback function executed
    /// * `duration_secs` - Execution duration in seconds
    pub fn record_execution(&self, module_type: &str, callback: &str, duration_secs: f64) {
        let attributes = [
            KeyValue::new("module_type", module_type.to_string()),
            KeyValue::new("callback", callback.to_string()),
        ];
        self.execution_duration_seconds
            .record(duration_secs, &attributes);
    }

    /// Record a module timeout
    ///
    /// # Arguments
    ///
    /// * `module_path` - Path to the module file that timed out
    pub fn record_timeout(&self, module_path: &str) {
        let attributes = [KeyValue::new("module_path", module_path.to_string())];
        self.timeout_total.add(1, &attributes);
    }

    /// Record a message mutation performed by a module
    ///
    /// # Arguments
    ///
    /// * `module_type` - Module runtime type (e.g., "wasm", "python", "`shared_library`")
    /// * `module_name` - Module name or path
    /// * `mutation_kind` - "header" or "body"
    /// * `action` - Mutation action (set/append/remove/etc.)
    /// * `result` - "success" or "failure"
    /// * `error_kind` - Optional error classification for failures
    pub fn record_mutation(
        &self,
        module_type: &str,
        module_name: &str,
        mutation_kind: &str,
        action: &str,
        result: &str,
        error_kind: Option<&str>,
    ) {
        let attributes = [
            KeyValue::new("module_type", module_type.to_string()),
            KeyValue::new("module_name", module_name.to_string()),
            KeyValue::new("mutation_kind", mutation_kind.to_string()),
            KeyValue::new("action", action.to_string()),
            KeyValue::new("result", result.to_string()),
            KeyValue::new("error_kind", error_kind.unwrap_or_default().to_string()),
        ];
        self.mutations_total.add(1, &attributes);
    }

    /// Update module memory usage
    ///
    /// # Arguments
    ///
    /// * `module_path` - Path to the module file
    /// * `memory_bytes` - Current memory usage in bytes
    pub fn update_memory(&self, module_path: &str, memory_bytes: u64) {
        self.memory_bytes
            .entry(module_path.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .store(memory_bytes, Ordering::Relaxed);
    }

    /// Get current memory usage for a module
    ///
    /// # Arguments
    ///
    /// * `module_path` - Path to the module file
    ///
    /// # Returns
    ///
    /// Current memory usage in bytes, or 0 if module not tracked
    #[must_use]
    pub fn get_memory(&self, module_path: &str) -> u64 {
        self.memory_bytes
            .get(module_path)
            .map_or(0, |entry| entry.load(Ordering::Relaxed))
    }
}

impl Default for ModuleMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the OpenTelemetry meter for module metrics
fn meter() -> Meter {
    opentelemetry::global::meter("empath.modules")
}
