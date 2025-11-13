# empath-metrics

OpenTelemetry metrics and observability for the Empath Mail Transfer Agent.

## Features

- **Prometheus Export**: HTTP endpoint for metrics scraping
- **SMTP Metrics**: Connection tracking, error rates, session durations
- **Delivery Metrics**: Success/failure rates, queue sizes, retry counts
- **DNS Metrics**: Lookup durations, cache hit rates

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
empath-metrics = { path = "../empath-metrics" }
```

Initialize metrics at startup:

```rust
use empath_metrics::{init_metrics, MetricsConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = MetricsConfig {
        enabled: true,
        listen_addr: "0.0.0.0:9090".parse()?,
    };

    init_metrics(config).await?;

    // Metrics available at http://localhost:9090/metrics
    Ok(())
}
```

Record metrics:

```rust
use empath_metrics::metrics;

// SMTP metrics
metrics().smtp.record_connection();
metrics().smtp.record_error(550);

// Delivery metrics
metrics().delivery.record_delivery_success("example.com", 1.5, 0);
metrics().delivery.update_queue_size("pending", 1);

// DNS metrics
metrics().dns.record_lookup("MX", 0.05);
metrics().dns.record_cache_hit("MX");
```

## Metrics Reference

### SMTP Metrics

- `empath.smtp.connections.total` - Total connections established
- `empath.smtp.connections.active` - Currently active connections
- `empath.smtp.errors.total{code}` - Errors by SMTP response code
- `empath.smtp.session.duration.seconds` - Session duration histogram
- `empath.smtp.command.duration.seconds{command}` - Command processing time
- `empath.smtp.messages.received.total` - Messages received count
- `empath.smtp.message.size.bytes` - Message size histogram

### Delivery Metrics

- `empath.delivery.attempts.total{status,domain}` - Delivery attempts by outcome
- `empath.delivery.duration.seconds{domain}` - Delivery duration histogram
- `empath.delivery.queue.size{status}` - Queue size by status
- `empath.delivery.connections.active` - Active outbound connections
- `empath.delivery.messages.delivered.total` - Successful deliveries
- `empath.delivery.messages.failed.total{reason}` - Failed deliveries
- `empath.delivery.messages.retrying.total` - Messages in retry state
- `empath.delivery.retry.count` - Retry count histogram

### DNS Metrics

- `empath.dns.lookup.duration.seconds{query_type}` - DNS lookup duration
- `empath.dns.lookups.total{query_type}` - Total lookups by type
- `empath.dns.cache.hits.total{query_type}` - Cache hits
- `empath.dns.cache.misses.total{query_type}` - Cache misses
- `empath.dns.errors.total{error_type}` - DNS errors
- `empath.dns.cache.evictions.total` - Cache evictions

## Configuration

Metrics can be configured in `empath.config.ron`:

```ron
Empath (
    // ... other config ...
    metrics: (
        enabled: true,
        listen_addr: "0.0.0.0:9090",
    ),
)
```

## Grafana Dashboards

Example Prometheus queries for Grafana:

```promql
# SMTP connection rate
rate(empath_smtp_connections_total[5m])

# Delivery success rate
rate(empath_delivery_messages_delivered_total[5m]) /
rate(empath_delivery_attempts_total[5m])

# DNS cache hit rate
rate(empath_dns_cache_hits_total[5m]) /
(rate(empath_dns_cache_hits_total[5m]) + rate(empath_dns_cache_misses_total[5m]))

# Queue size by status
empath_delivery_queue_size
```

## License

Apache-2.0
