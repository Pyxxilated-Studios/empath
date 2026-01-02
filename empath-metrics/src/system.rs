//! System resource metrics for capacity planning
//!
//! This module provides metrics for monitoring system resources to enable
//! capacity planning and early detection of resource exhaustion.
//!
//! # Metrics Categories
//!
//! - **Process Metrics**: CPU, memory, threads, file descriptors
//! - **Spool Disk Metrics**: Disk usage, inodes, I/O operations
//! - **Network Metrics**: Connections, bandwidth usage

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Gauge, Histogram, Meter};
use sysinfo::{Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::Mutex;

use crate::util::u64_to_f64;

/// System resource metrics
#[derive(Debug, Clone)]
pub struct SystemMetrics {
    // Process metrics
    process_cpu_usage: Gauge<f64>,
    process_memory_bytes: Gauge<u64>,
    process_thread_count: Gauge<u64>,
    process_fd_count: Gauge<u64>,
    process_uptime_seconds: Gauge<u64>,

    // Spool disk metrics
    spool_disk_used_bytes: Gauge<u64>,
    spool_disk_available_bytes: Gauge<u64>,
    spool_disk_total_bytes: Gauge<u64>,
    spool_disk_usage_percent: Gauge<f64>,
    spool_file_count: Gauge<u64>,

    // Network metrics
    network_connections_total: Gauge<u64>,
    network_bytes_received: Gauge<u64>,
    network_bytes_sent: Gauge<u64>,

    // I/O metrics
    spool_io_reads_total: Gauge<u64>,
    spool_io_writes_total: Gauge<u64>,
    spool_io_read_bytes: Gauge<u64>,
    spool_io_write_bytes: Gauge<u64>,
    spool_io_duration: Histogram<f64>,

    // Internal state for periodic updates
    sys_info: Arc<Mutex<System>>,
    networks: Arc<Mutex<Networks>>,
    process_start_time: SystemTime,
}

struct SystemInstruments {
    process_cpu_usage: Gauge<f64>,
    process_memory_bytes: Gauge<u64>,
    process_thread_count: Gauge<u64>,
    process_fd_count: Gauge<u64>,
    process_uptime_seconds: Gauge<u64>,
    spool_disk_used_bytes: Gauge<u64>,
    spool_disk_available_bytes: Gauge<u64>,
    spool_disk_total_bytes: Gauge<u64>,
    spool_disk_usage_percent: Gauge<f64>,
    spool_file_count: Gauge<u64>,
    network_connections_total: Gauge<u64>,
    network_bytes_received: Gauge<u64>,
    network_bytes_sent: Gauge<u64>,
    spool_io_reads_total: Gauge<u64>,
    spool_io_writes_total: Gauge<u64>,
    spool_io_read_bytes: Gauge<u64>,
    spool_io_write_bytes: Gauge<u64>,
    spool_io_duration: Histogram<f64>,
}

struct ProcessInstruments {
    cpu_usage: Gauge<f64>,
    memory_bytes: Gauge<u64>,
    thread_count: Gauge<u64>,
    fd_count: Gauge<u64>,
    uptime_seconds: Gauge<u64>,
}

struct SpoolDiskInstruments {
    used_bytes: Gauge<u64>,
    available_bytes: Gauge<u64>,
    total_bytes: Gauge<u64>,
    usage_percent: Gauge<f64>,
    file_count: Gauge<u64>,
}

struct NetworkInstruments {
    connections_total: Gauge<u64>,
    bytes_received: Gauge<u64>,
    bytes_sent: Gauge<u64>,
}

struct SpoolIoInstruments {
    reads_total: Gauge<u64>,
    writes_total: Gauge<u64>,
    read_bytes: Gauge<u64>,
    write_bytes: Gauge<u64>,
    duration: Histogram<f64>,
}

impl SystemMetrics {
    /// Create a new `SystemMetrics` instance
    pub fn new() -> Self {
        let meter: Meter = opentelemetry::global::meter("empath.system");
        let instruments = Self::build_instruments(&meter);

        let sys_info = Arc::new(Mutex::new(System::new_all()));
        let networks = Arc::new(Mutex::new(Networks::new_with_refreshed_list()));
        let process_start_time = SystemTime::now();

        Self {
            process_cpu_usage: instruments.process_cpu_usage,
            process_memory_bytes: instruments.process_memory_bytes,
            process_thread_count: instruments.process_thread_count,
            process_fd_count: instruments.process_fd_count,
            process_uptime_seconds: instruments.process_uptime_seconds,
            spool_disk_used_bytes: instruments.spool_disk_used_bytes,
            spool_disk_available_bytes: instruments.spool_disk_available_bytes,
            spool_disk_total_bytes: instruments.spool_disk_total_bytes,
            spool_disk_usage_percent: instruments.spool_disk_usage_percent,
            spool_file_count: instruments.spool_file_count,
            network_connections_total: instruments.network_connections_total,
            network_bytes_received: instruments.network_bytes_received,
            network_bytes_sent: instruments.network_bytes_sent,
            spool_io_reads_total: instruments.spool_io_reads_total,
            spool_io_writes_total: instruments.spool_io_writes_total,
            spool_io_read_bytes: instruments.spool_io_read_bytes,
            spool_io_write_bytes: instruments.spool_io_write_bytes,
            spool_io_duration: instruments.spool_io_duration,
            sys_info,
            networks,
            process_start_time,
        }
    }

    fn build_instruments(meter: &Meter) -> SystemInstruments {
        let process = Self::build_process_instruments(meter);
        let spool_disk = Self::build_spool_disk_instruments(meter);
        let network = Self::build_network_instruments(meter);
        let spool_io = Self::build_spool_io_instruments(meter);

        SystemInstruments {
            process_cpu_usage: process.cpu_usage,
            process_memory_bytes: process.memory_bytes,
            process_thread_count: process.thread_count,
            process_fd_count: process.fd_count,
            process_uptime_seconds: process.uptime_seconds,
            spool_disk_used_bytes: spool_disk.used_bytes,
            spool_disk_available_bytes: spool_disk.available_bytes,
            spool_disk_total_bytes: spool_disk.total_bytes,
            spool_disk_usage_percent: spool_disk.usage_percent,
            spool_file_count: spool_disk.file_count,
            network_connections_total: network.connections_total,
            network_bytes_received: network.bytes_received,
            network_bytes_sent: network.bytes_sent,
            spool_io_reads_total: spool_io.reads_total,
            spool_io_writes_total: spool_io.writes_total,
            spool_io_read_bytes: spool_io.read_bytes,
            spool_io_write_bytes: spool_io.write_bytes,
            spool_io_duration: spool_io.duration,
        }
    }

    fn build_process_instruments(meter: &Meter) -> ProcessInstruments {
        let process_cpu_usage = meter
            .f64_gauge("empath.system.process.cpu.usage")
            .with_description("Process CPU usage percentage (0-100)")
            .with_unit("{percent}")
            .build();

        let process_memory_bytes = meter
            .u64_gauge("empath.system.process.memory.bytes")
            .with_description("Process memory usage in bytes")
            .with_unit("{bytes}")
            .build();

        let process_thread_count = meter
            .u64_gauge("empath.system.process.threads.count")
            .with_description("Number of threads in the process")
            .with_unit("{threads}")
            .build();

        let process_fd_count = meter
            .u64_gauge("empath.system.process.fd.count")
            .with_description("Number of open file descriptors")
            .with_unit("{fds}")
            .build();

        let process_uptime_seconds = meter
            .u64_gauge("empath.system.process.uptime.seconds")
            .with_description("Process uptime in seconds")
            .with_unit("{seconds}")
            .build();

        ProcessInstruments {
            cpu_usage: process_cpu_usage,
            memory_bytes: process_memory_bytes,
            thread_count: process_thread_count,
            fd_count: process_fd_count,
            uptime_seconds: process_uptime_seconds,
        }
    }

    fn build_spool_disk_instruments(meter: &Meter) -> SpoolDiskInstruments {
        let spool_disk_used_bytes = meter
            .u64_gauge("empath.system.spool.disk.used.bytes")
            .with_description("Spool disk space used in bytes")
            .with_unit("{bytes}")
            .build();

        let spool_disk_available_bytes = meter
            .u64_gauge("empath.system.spool.disk.available.bytes")
            .with_description("Spool disk space available in bytes")
            .with_unit("{bytes}")
            .build();

        let spool_disk_total_bytes = meter
            .u64_gauge("empath.system.spool.disk.total.bytes")
            .with_description("Total spool disk space in bytes")
            .with_unit("{bytes}")
            .build();

        let spool_disk_usage_percent = meter
            .f64_gauge("empath.system.spool.disk.usage.percent")
            .with_description("Spool disk usage percentage (0-100)")
            .with_unit("{percent}")
            .build();

        let spool_file_count = meter
            .u64_gauge("empath.system.spool.files.count")
            .with_description("Number of files in the spool directory")
            .with_unit("{files}")
            .build();

        SpoolDiskInstruments {
            used_bytes: spool_disk_used_bytes,
            available_bytes: spool_disk_available_bytes,
            total_bytes: spool_disk_total_bytes,
            usage_percent: spool_disk_usage_percent,
            file_count: spool_file_count,
        }
    }

    fn build_network_instruments(meter: &Meter) -> NetworkInstruments {
        let network_connections_total = meter
            .u64_gauge("empath.system.network.connections.total")
            .with_description("Total number of network connections")
            .with_unit("{connections}")
            .build();

        let network_bytes_received = meter
            .u64_gauge("empath.system.network.bytes.received")
            .with_description("Total bytes received")
            .with_unit("{bytes}")
            .build();

        let network_bytes_sent = meter
            .u64_gauge("empath.system.network.bytes.sent")
            .with_description("Total bytes sent")
            .with_unit("{bytes}")
            .build();

        NetworkInstruments {
            connections_total: network_connections_total,
            bytes_received: network_bytes_received,
            bytes_sent: network_bytes_sent,
        }
    }

    fn build_spool_io_instruments(meter: &Meter) -> SpoolIoInstruments {
        let spool_io_reads_total = meter
            .u64_gauge("empath.system.spool.io.reads.total")
            .with_description("Total number of spool read operations")
            .with_unit("{operations}")
            .build();

        let spool_io_writes_total = meter
            .u64_gauge("empath.system.spool.io.writes.total")
            .with_description("Total number of spool write operations")
            .with_unit("{operations}")
            .build();

        let spool_io_read_bytes = meter
            .u64_gauge("empath.system.spool.io.read.bytes")
            .with_description("Total bytes read from spool")
            .with_unit("{bytes}")
            .build();

        let spool_io_write_bytes = meter
            .u64_gauge("empath.system.spool.io.write.bytes")
            .with_description("Total bytes written to spool")
            .with_unit("{bytes}")
            .build();

        let spool_io_duration = meter
            .f64_histogram("empath.system.spool.io.duration.seconds")
            .with_description("Spool I/O operation duration")
            .with_unit("{seconds}")
            .build();

        SpoolIoInstruments {
            reads_total: spool_io_reads_total,
            writes_total: spool_io_writes_total,
            read_bytes: spool_io_read_bytes,
            write_bytes: spool_io_write_bytes,
            duration: spool_io_duration,
        }
    }

    /// Update process metrics
    ///
    /// This should be called periodically (e.g., every 10-30 seconds) to refresh
    /// process resource metrics.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tokio::time::{interval, Duration};
    ///
    /// let mut interval = interval(Duration::from_secs(15));
    /// loop {
    ///     interval.tick().await;
    ///     if let Some(metrics) = metrics() {
    ///         metrics.system.update_process_metrics().await;
    ///     }
    /// }
    /// ```
    pub async fn update_process_metrics(&self) {
        let mut sys = self.sys_info.lock().await;
        let pid = Pid::from_u32(std::process::id());

        // Refresh process information
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            ProcessRefreshKind::everything(),
        );

        if let Some(process) = sys.process(pid) {
            // CPU usage (percentage)
            self.process_cpu_usage
                .record(f64::from(process.cpu_usage()), &[]);

            // Memory usage (bytes)
            self.process_memory_bytes.record(process.memory(), &[]);

            // File descriptor count (Unix-like systems only)
            #[cfg(target_os = "linux")]
            {
                if let Ok(fd_count) = count_file_descriptors(pid) {
                    self.process_fd_count.record(fd_count, &[]);
                }
            }

            // On non-Linux systems, record 0 as placeholder
            #[cfg(not(target_os = "linux"))]
            {
                self.process_fd_count.record(0, &[]);
            }

            // Uptime
            if let Ok(uptime) = self.process_start_time.elapsed() {
                self.process_uptime_seconds.record(uptime.as_secs(), &[]);
            }
        }

        drop(sys);

        // Note: sysinfo 0.33 doesn't have a direct thread count API,
        // so we'll skip this metric for now or implement via platform-specific code.
        self.process_thread_count.record(0, &[]);
    }

    /// Update spool disk metrics
    ///
    /// # Arguments
    ///
    /// * `spool_path` - Path to the spool directory
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(metrics) = metrics() {
    ///     metrics.system.update_spool_metrics(Path::new("./spool"));
    /// }
    /// ```
    pub fn update_spool_metrics(&self, spool_path: &Path) {
        if let Ok(usage) = fs2::statvfs(spool_path) {
            let total = usage.total_space();
            let available = usage.available_space();
            let used = total.saturating_sub(available);
            let usage_percent = if total > 0 {
                (u64_to_f64(used) / u64_to_f64(total)) * 100.0
            } else {
                0.0
            };

            self.spool_disk_total_bytes.record(total, &[]);
            self.spool_disk_available_bytes.record(available, &[]);
            self.spool_disk_used_bytes.record(used, &[]);
            self.spool_disk_usage_percent.record(usage_percent, &[]);
        }

        // Count files in spool
        if let Ok(file_count) = count_files_in_directory(spool_path) {
            self.spool_file_count.record(file_count, &[]);
        }
    }

    /// Update network byte metrics
    ///
    /// Uses system-wide network interface totals to record cumulative bytes in/out.
    pub async fn update_network_metrics(&self) {
        let (received, sent) = {
            let mut networks = self.networks.lock().await;
            networks.refresh(true);

            let mut received: u64 = 0;
            let mut sent: u64 = 0;
            for (_name, data) in networks.iter() {
                received = received.saturating_add(data.total_received());
                sent = sent.saturating_add(data.total_transmitted());
            }
            drop(networks);
            (received, sent)
        };

        self.record_network_bytes(received, sent);
    }

    /// Record network connection count
    ///
    /// This should be called when connections are established/closed to track
    /// the total number of active connections.
    ///
    /// # Arguments
    ///
    /// * `count` - Total number of active connections
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(metrics) = metrics() {
    ///     metrics.system.record_network_connections(42);
    /// }
    /// ```
    pub fn record_network_connections(&self, count: u64) {
        self.network_connections_total.record(count, &[]);
    }

    /// Record network bytes received/sent
    ///
    /// # Arguments
    ///
    /// * `received` - Cumulative bytes received
    /// * `sent` - Cumulative bytes sent
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(metrics) = metrics() {
    ///     metrics.system.record_network_bytes(1024000, 512000);
    /// }
    /// ```
    pub fn record_network_bytes(&self, received: u64, sent: u64) {
        self.network_bytes_received.record(received, &[]);
        self.network_bytes_sent.record(sent, &[]);
    }

    /// Record spool I/O operation
    ///
    /// # Arguments
    ///
    /// * `operation` - Type of I/O operation ("read" or "write")
    /// * `bytes` - Number of bytes transferred
    /// * `duration` - Duration of the operation
    ///
    /// # Example
    ///
    /// ```ignore
    /// let start = Instant::now();
    /// // ... perform I/O ...
    /// if let Some(metrics) = metrics() {
    ///     metrics.system.record_spool_io("write", 4096, start.elapsed());
    /// }
    /// ```
    pub fn record_spool_io(&self, operation: &str, bytes: u64, duration: Duration) {
        let attrs = [KeyValue::new("operation", operation.to_string())];

        match operation {
            "read" => {
                self.spool_io_reads_total.record(1, &attrs);
                self.spool_io_read_bytes.record(bytes, &attrs);
            }
            "write" => {
                self.spool_io_writes_total.record(1, &attrs);
                self.spool_io_write_bytes.record(bytes, &attrs);
            }
            _ => {}
        }

        self.spool_io_duration
            .record(duration.as_secs_f64(), &attrs);
    }
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Count file descriptors for a process (Linux only)
#[cfg(target_os = "linux")]
fn count_file_descriptors(pid: Pid) -> Result<u64, std::io::Error> {
    let fd_dir = format!("/proc/{}/fd", pid.as_u32());
    let count = std::fs::read_dir(fd_dir)?.count() as u64;
    Ok(count)
}

/// Count files in a directory (non-recursive)
fn count_files_in_directory(path: &Path) -> Result<u64, std::io::Error> {
    let count = std::fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        .count() as u64;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::{fs, io};

    use tempfile::tempdir;

    use super::count_files_in_directory;

    #[test]
    fn count_files_in_directory_counts_only_files() -> io::Result<()> {
        let dir = tempdir()?;
        fs::write(dir.path().join("one.txt"), b"one")?;
        fs::write(dir.path().join("two.txt"), b"two")?;
        fs::create_dir(dir.path().join("subdir"))?;
        fs::write(dir.path().join("subdir").join("nested.txt"), b"nested")?;

        let count = count_files_in_directory(dir.path())?;

        assert_eq!(count, 2);
        Ok(())
    }
}
