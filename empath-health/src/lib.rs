//! Health check endpoints for Empath MTA
//!
//! This crate provides HTTP health check endpoints for Kubernetes liveness and readiness probes.
//! It enables production deployments with proper health monitoring and container orchestration.
//!
//! # Endpoints
//!
//! - **`/health/live`** - Liveness probe: Returns 200 if the application is running
//! - **`/health/ready`** - Readiness probe: Returns 200 if the application can accept traffic
//!
//! # Usage
//!
//! ```rust,no_run
//! use empath_health::{HealthServer, HealthConfig, HealthChecker};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HealthConfig {
//!     enabled: true,
//!     listen_address: "[::]:8080".to_string(),
//!     max_queue_size: 10000,
//! };
//!
//! let health_checker = Arc::new(HealthChecker::new(10000));
//! let server = HealthServer::new(config, health_checker).await?;
//!
//! // Run the health server
//! // server.serve(shutdown_receiver).await?;
//! # Ok(())
//! # }
//! ```

mod checker;
mod config;
mod error;
mod server;

pub use checker::HealthChecker;
pub use config::HealthConfig;
pub use error::HealthError;
pub use server::HealthServer;
