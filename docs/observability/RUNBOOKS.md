# Empath MTA - Alert Runbooks

This document provides remediation procedures for all Empath MTA alerts defined in `prometheus-alerts.yml`.

**Table of Contents:**
- [Critical Alerts](#critical-alerts)
  - [DeliverySuccessRateLow](#deliverysuccessratelow)
  - [QueueBacklogCritical](#queuebacklogcritical)
  - [SMTPListenerDown](#smtplistenerdown)
  - [CircuitBreakerStormDetected](#circuitbreakerstormdetected)
  - [SpoolDiskSpaceLow](#spooldiskspacelow)
- [Warning Alerts](#warning-alerts)
  - [DeliveryLatencyHigh](#deliverylatencyhigh)
  - [OldestMessageAgeHigh](#oldestmessageagehigh)
  - [DnsCacheHitRateLow](#dnscachehitratelow)
  - [RateLimitingExcessive](#ratelimitingexcessive)
  - [DeliveryErrorRateElevated](#deliveryerrorrateele vated)
  - [QueueSizeGrowing](#queuesizegrowing)
  - [CircuitBreakerOpen](#circuitbreakeropen)

---

## Critical Alerts

### DeliverySuccessRateLow

**Alert**: Delivery success rate below SLO (< 95% for 5 minutes)

**Severity**: Critical - Page immediately

**Impact**: Messages are failing to deliver at an unacceptable rate. User-facing impact.

#### Diagnostic Steps

1. **Check current success rate:**
   ```promql
   empath_delivery_success_rate
   ```

2. **Identify failing domains:**
   ```bash
   empathctl queue list --status failed | head -n 20
   ```

3. **Check recent delivery errors:**
   ```promql
   topk(10, rate(empath_delivery_attempts_total{status="failed"}[5m])) by (domain)
   ```

4. **Review delivery logs:**
   ```logql
   {service="empath"} | json | level="ERROR" | fields.domain!=""
   ```

5. **Check for DNS issues:**
   ```bash
   empathctl dns list-cache
   ```

6. **Check circuit breaker states:**
   ```promql
   empath_delivery_circuit_breaker_state == 1  # Open state
   ```

#### Remediation

1. **Identify root cause:**
   - If specific domain(s): DNS issue, recipient server down, or rate limiting
   - If widespread: Network issue, resource exhaustion, or Empath bug

2. **Immediate actions:**
   - **For DNS failures:** `empathctl dns clear-cache` and verify DNS resolver is responding
   - **For recipient server down:** Wait for recovery or contact recipient administrator
   - **For resource exhaustion:** Check CPU, memory, disk I/O on Empath host
   - **For rate limiting:** Review rate limit configuration in `empath.config.ron`

3. **Temporary mitigations:**
   ```bash
   # If specific domain is causing widespread failures, add temporary override
   # Edit empath.config.ron delivery.domains section, then reload config
   ```

4. **Monitor recovery:**
   ```promql
   empath_delivery_success_rate
   ```
   Success rate should recover above 95% within 5-10 minutes if root cause addressed.

#### Escalation

- If success rate remains below 95% after 15 minutes of investigation
- If root cause is unclear
- If widespread DNS or network issue suspected

---

### QueueBacklogCritical

**Alert**: Queue backlog critical (> 10,000 pending messages for 2 minutes)

**Severity**: Critical - Page immediately

**Impact**: Delivery system is overloaded or stalled. Messages will experience delays.

#### Diagnostic Steps

1. **Check queue sizes:**
   ```bash
   empathctl queue stats
   ```

2. **Check queue growth rate:**
   ```promql
   deriv(empath_delivery_queue_size{status="pending"}[10m])
   ```

3. **Check delivery processor health:**
   ```bash
   empathctl system status
   ```

4. **Check delivery rate:**
   ```promql
   rate(empath_delivery_attempts_total[5m])
   ```

5. **Check for stuck messages:**
   ```bash
   empathctl queue list --status pending --limit 100 | grep -o 'domain: [^,]*' | sort | uniq -c | sort -rn
   ```

6. **Review resource usage:**
   ```bash
   # CPU and memory
   top -p $(pgrep empath)

   # Disk I/O
   iostat -x 1 5
   ```

#### Remediation

1. **Identify bottleneck:**
   - **Delivery rate low:** Check for network issues, DNS problems, or recipient server slowness
   - **Ingress rate high:** Check SMTP connection rate, possible spam attack
   - **Delivery processor stalled:** Check for deadlocks, panics in logs

2. **Immediate actions:**

   a. **If delivery processor is stalled:**
   ```bash
   # Check for panics or errors in logs
   {service="empath"} | json | level="ERROR" | fields.component="delivery"

   # If confirmed stalled, restart Empath (graceful shutdown waits for in-flight deliveries)
   systemctl restart empath
   ```

   b. **If ingress rate is too high:**
   ```bash
   # Temporarily reduce SMTP connections (edit config, reload)
   # Or use firewall rate limiting
   ```

   c. **If delivery rate is low due to recipient issues:**
   ```bash
   # Identify problematic domains
   empathctl queue list --status retry | grep -o 'domain: [^,]*' | sort | uniq -c | sort -rn

   # Manually trigger delivery processing
   empathctl queue process-now
   ```

3. **Scale horizontally (if available):**
   ```bash
   # Increase max_concurrent_deliveries in config
   # Requires restart
   ```

4. **Monitor queue drain:**
   ```promql
   empath_delivery_queue_size{status="pending"}
   ```

#### Escalation

- If queue continues growing after 30 minutes
- If delivery processor appears stalled with no clear cause
- If resource exhaustion (CPU, memory, disk) is root cause

---

### SMTPListenerDown

**Alert**: SMTP listener appears to be down (no connections for 3 minutes)

**Severity**: Critical - Page immediately

**Impact**: Empath cannot receive new messages. Complete service outage for inbound mail.

#### Diagnostic Steps

1. **Check SMTP listener health:**
   ```bash
   empathctl system status
   ```

2. **Check active connections:**
   ```promql
   empath_smtp_connections_active
   ```

3. **Check connection attempts:**
   ```promql
   rate(empath_smtp_connections_total[5m])
   ```

4. **Verify listener is bound:**
   ```bash
   ss -tlnp | grep :1025  # Or configured SMTP port
   ```

5. **Check for crashes:**
   ```bash
   # Check systemd logs
   journalctl -u empath -n 100

   # Check for panics
   {service="empath"} | json | level="ERROR" | fields.component="smtp"
   ```

6. **Test connectivity:**
   ```bash
   telnet localhost 1025
   ```

#### Remediation

1. **Verify process is running:**
   ```bash
   systemctl status empath
   ```

2. **If process crashed:**
   ```bash
   # Check crash logs
   journalctl -u empath --since "10 minutes ago"

   # Restart service
   systemctl restart empath

   # Verify listeners started
   ss -tlnp | grep :1025
   ```

3. **If process is running but listener not accepting connections:**
   ```bash
   # Check for port conflicts
   lsof -i :1025

   # Check firewall rules
   iptables -L -n | grep 1025

   # Check Empath configuration
   grep -A 5 'smtp_controller' /path/to/empath.config.ron
   ```

4. **If no incoming traffic:**
   ```bash
   # This may be a load balancer or DNS issue, not Empath
   # Check upstream infrastructure
   ```

5. **Monitor recovery:**
   ```promql
   rate(empath_smtp_connections_total[1m])
   ```

#### Escalation

- If listener cannot be restarted after 3 attempts
- If crashes are recurring (indicates bug)
- If network/infrastructure issue beyond Empath control

---

### CircuitBreakerStormDetected

**Alert**: Circuit breaker storm detected (5+ domains tripped in 5 minutes)

**Severity**: Critical - Page immediately

**Impact**: Systemic delivery issue. Multiple recipient domains are failing simultaneously.

#### Diagnostic Steps

1. **List tripped circuit breakers:**
   ```bash
   # Check which domains have open circuit breakers
   empathctl queue list --status retry | grep -o 'domain: [^,]*' | sort | uniq -c | sort -rn | head -n 20
   ```

2. **Check circuit breaker trip rate:**
   ```promql
   sum by (domain) (increase(empath_delivery_circuit_breaker_trips_total[5m]))
   ```

3. **Identify common failure pattern:**
   ```bash
   # Check delivery error logs
   {service="empath"} | json | level="ERROR" | fields.message=~"Circuit breaker.*"
   ```

4. **Check for network issues:**
   ```bash
   # DNS resolution
   empathctl dns list-cache

   # Network connectivity (ping common mail servers)
   ping -c 3 gmail-smtp-in.l.google.com
   ```

5. **Check for resource exhaustion:**
   ```bash
   # Connection limits
   ss -s

   # File descriptors
   lsof -p $(pgrep empath) | wc -l
   ```

#### Remediation

1. **Identify root cause:**
   - **Network outage:** All domains failing with connection timeouts
   - **DNS failure:** All domains failing with DNS resolution errors
   - **Resource exhaustion:** Connection pool exhausted, file descriptor limit hit
   - **Recipient-side issue:** Multiple domains hosted by same provider (e.g., Gmail) all failing

2. **Immediate actions:**

   a. **For network outage:**
   ```bash
   # Verify network connectivity
   ping 8.8.8.8

   # Check routes
   ip route show
   ```

   b. **For DNS failure:**
   ```bash
   # Clear DNS cache
   empathctl dns clear-cache

   # Verify DNS resolver
   dig @8.8.8.8 gmail-smtp-in.l.google.com MX
   ```

   c. **For resource exhaustion:**
   ```bash
   # Check limits
   ulimit -n

   # Increase if needed (requires restart)
   # Edit systemd service file: LimitNOFILE=65536
   systemctl daemon-reload
   systemctl restart empath
   ```

   d. **For recipient provider issue:**
   ```bash
   # Wait for provider recovery
   # Monitor provider status pages
   ```

3. **Reset circuit breakers (after root cause fixed):**
   ```bash
   # Circuit breakers will auto-recover after timeout (default: 5 minutes)
   # Or manually retry failed messages
   for msg_id in $(empathctl queue list --status retry | grep -o 'id: [^ ]*' | cut -d' ' -f2 | head -n 100); do
       empathctl queue retry "$msg_id"
   done
   ```

4. **Monitor recovery:**
   ```promql
   sum(empath_delivery_circuit_breaker_state == 1)  # Should decrease to 0
   ```

#### Escalation

- If systemic issue persists > 15 minutes
- If root cause is infrastructure beyond Empath control
- If pattern suggests Empath bug (e.g., all domains failing with same Empath-related error)

---

### SpoolDiskSpaceLow

**Alert**: Spool disk space critically low (< 10% remaining)

**Severity**: Critical - Page immediately

**Impact**: Empath will fail to spool new messages when disk is full. Service outage imminent.

#### Diagnostic Steps

1. **Check spool disk usage:**
   ```bash
   df -h /var/spool/empath  # Or configured spool path
   ```

2. **Check spool directory size:**
   ```bash
   du -sh /var/spool/empath/*
   ```

3. **Count spooled messages:**
   ```bash
   find /var/spool/empath -type f | wc -l
   ```

4. **Check for large files:**
   ```bash
   find /var/spool/empath -type f -size +10M -exec ls -lh {} \;
   ```

5. **Check disk I/O:**
   ```bash
   iostat -x 1 5
   ```

#### Remediation

1. **Immediate triage:**
   ```bash
   # Check if disk is actually full or just metric lag
   df -h /var/spool/empath
   ```

2. **Free space quickly:**

   a. **Delete old completed/failed messages (if retention policy allows):**
   ```bash
   # BE CAREFUL: Only delete if you have backups or retention policy allows
   find /var/spool/empath -type f -mtime +30 -delete  # Files older than 30 days
   ```

   b. **Move messages to larger disk:**
   ```bash
   # If available, move spool to larger filesystem
   rsync -av /var/spool/empath/ /mnt/larger-disk/empath/
   # Update config, restart Empath
   ```

   c. **Delete messages permanently stuck in failed state:**
   ```bash
   # Identify permanently failed messages (5xx errors, max retries exceeded)
   empathctl queue list --status failed | head -n 100

   # Delete if appropriate (requires empathctl delete command)
   # empathctl queue delete <message-id>
   ```

3. **Long-term solutions:**
   - **Expand disk:** Add storage to spool filesystem
   - **Implement cleanup policy:** Configure automatic deletion of old messages
   - **Monitor disk usage:** Set up alerts at 20%, 30% thresholds
   - **Archive messages:** Move old messages to archival storage

4. **Monitor recovery:**
   ```bash
   watch -n 5 'df -h /var/spool/empath'
   ```

#### Escalation

- If disk space cannot be freed within 10 minutes
- If disk expansion required (infrastructure team)
- If issue is caused by message retention bug (development team)

**Note**: This alert requires `node_exporter` to be deployed alongside Empath. See [README.md](README.md) for integration details.

---

## Warning Alerts

### DeliveryLatencyHigh

**Alert**: Delivery latency high (p95 queue age > 10 minutes)

**Severity**: Warning - Create ticket

**Impact**: Messages are experiencing delays but still being delivered. User-facing latency.

#### Diagnostic Steps

1. **Check current p95 latency:**
   ```promql
   histogram_quantile(0.95, rate(empath_delivery_queue_age_seconds_bucket[5m]))
   ```

2. **Check queue age distribution:**
   ```bash
   empathctl queue stats
   ```

3. **Identify slow domains:**
   ```bash
   empathctl queue list --status pending | grep -o 'domain: [^,]*' | sort | uniq -c | sort -rn | head -n 10
   ```

4. **Check delivery rate:**
   ```promql
   rate(empath_delivery_attempts_total[5m])
   ```

5. **Check for rate limiting:**
   ```promql
   sum by (domain) (rate(empath_delivery_rate_limited_total[5m]))
   ```

#### Remediation

1. **Identify bottleneck:**
   - **Specific slow domains:** Recipient server slowness or rate limiting
   - **General slowness:** Delivery concurrency too low or resource constraints

2. **Optimize delivery:**

   a. **Increase delivery concurrency (if resources allow):**
   ```ron
   # In empath.config.ron
   delivery: (
       max_concurrent_deliveries: 16,  // Increase from default (num_cpus)
   )
   ```

   b. **Adjust rate limits (if too aggressive):**
   ```ron
   # In empath.config.ron
   delivery: (
       rate_limit: (
           messages_per_second: 15.0,  // Increase from 10.0
           domain_limits: {
               "slow-domain.com": (
                   messages_per_second: 5.0,  // Domain-specific limit
               ),
           },
       ),
   )
   ```

   c. **Tune retry intervals (if appropriate):**
   ```ron
   # Review retry backoff in config
   ```

3. **Monitor improvement:**
   ```promql
   histogram_quantile(0.95, rate(empath_delivery_queue_age_seconds_bucket[5m]))
   ```
   Should decrease below 300 seconds (5 minutes) once changes take effect.

#### Long-term Actions

- Review SLO: Is p95 < 5 minutes achievable for all workloads?
- Monitor specific slow domains: Consider domain-specific SLOs
- Capacity planning: May need more resources or horizontal scaling

---

### OldestMessageAgeHigh

**Alert**: Oldest message in queue is > 1 hour old

**Severity**: Warning - Create ticket

**Impact**: At least one message is stuck, possibly in retry loop. Isolated issue.

#### Diagnostic Steps

1. **Find oldest message:**
   ```bash
   empathctl queue list --status pending --limit 1
   ```

2. **Check message history:**
   ```bash
   # View message details
   empathctl queue view <message-id>
   ```

3. **Check delivery attempts:**
   ```promql
   empath_delivery_retry_count{message_id="<message-id>"}
   ```

4. **Review error logs for this message:**
   ```logql
   {service="empath"} | json | fields.message_id="<message-id>"
   ```

#### Remediation

1. **Analyze failure reason:**
   - **DNS failure:** `empathctl dns refresh <domain>`
   - **Recipient server issue:** Wait for next retry or contact recipient admin
   - **Message format issue:** May require manual intervention

2. **Manual retry (if appropriate):**
   ```bash
   empathctl queue retry <message-id>
   ```

3. **If message is permanently stuck:**
   ```bash
   # After verifying issue, may need to delete
   # Generate DSN (bounce) to sender first
   empathctl queue delete <message-id>
   ```

4. **Monitor:**
   ```promql
   empath_delivery_queue_oldest_seconds
   ```

#### Long-term Actions

- If recurring for specific domain: Review domain configuration
- If due to Empath bug: File bug report with message details

---

### DnsCacheHitRateLow

**Alert**: DNS cache hit rate low (< 70% for 15 minutes)

**Severity**: Warning - Create ticket

**Impact**: High DNS query load may impact delivery performance and increase DNS server load.

#### Diagnostic Steps

1. **Check cache hit rate:**
   ```promql
   rate(empath_dns_cache_hits_total[5m]) / (rate(empath_dns_cache_hits_total[5m]) + rate(empath_dns_cache_misses_total[5m]))
   ```

2. **Check cache size:**
   ```bash
   empathctl dns list-cache | wc -l
   ```

3. **Check cache eviction rate:**
   ```promql
   rate(empath_dns_cache_evictions_total[5m])
   ```

4. **Check query patterns:**
   ```bash
   empathctl queue list | grep -o 'domain: [^,]*' | sort | uniq -c | sort -rn | head -n 20
   ```

#### Remediation

1. **Identify cause:**
   - **High domain diversity:** Many unique domains, cache too small
   - **Cache eviction:** Cache size insufficient for workload
   - **TTL too short:** Upstream DNS records have short TTLs

2. **Increase cache size (if needed):**
   ```rust
   // In empath-delivery DNS resolver config
   // Currently no config option - requires code change
   // File enhancement request
   ```

3. **Monitor after DNS infrastructure changes:**
   - Check if DNS server changed or upstream TTLs modified

4. **Temporary workaround:**
   - Acceptable if temporary spike due to unusual traffic pattern
   - Monitor for return to normal (> 70%)

#### Long-term Actions

- Implement configurable DNS cache size (enhancement request)
- Monitor cache metrics regularly for capacity planning
- Consider DNS cache warming for high-volume domains

---

### RateLimitingExcessive

**Alert**: Excessive rate limiting for domain (> 100 delays/min)

**Severity**: Warning - Create ticket

**Impact**: Deliveries to specific domain are being excessively delayed. May indicate misconfiguration.

#### Diagnostic Steps

1. **Check affected domain:**
   ```promql
   sum by (domain) (rate(empath_delivery_rate_limited_total[1m]))
   ```

2. **Check configured rate limit:**
   ```bash
   grep -A 10 "rate_limit" /path/to/empath.config.ron | grep -A 5 "$DOMAIN"
   ```

3. **Check actual delivery rate needed:**
   ```bash
   empathctl queue list --status pending | grep "domain: $DOMAIN" | wc -l
   ```

4. **Check recipient server capabilities:**
   ```bash
   # Check if recipient has published rate limit preferences
   # Review SPF/DMARC records, consult recipient postmaster
   ```

#### Remediation

1. **Adjust rate limit configuration:**
   ```ron
   # In empath.config.ron
   delivery: (
       rate_limit: (
           domain_limits: {
               "affected-domain.com": (
                   messages_per_second: 20.0,  // Increase from current limit
                   burst_size: 50,               // Allow bursts
               ),
           },
       ),
   )
   ```

2. **Restart Empath to apply changes:**
   ```bash
   systemctl reload empath  # If hot reload supported, otherwise restart
   ```

3. **Monitor improvement:**
   ```promql
   rate(empath_delivery_rate_limited_total{domain="affected-domain.com"}[1m])
   ```

#### Long-term Actions

- Document domain-specific rate limits in runbook
- Coordinate with recipient administrators for optimal rate limits
- Implement configuration hot reload (if not available)

---

### DeliveryErrorRateElevated

**Alert**: Delivery error rate elevated (> 5% for 15 minutes)

**Severity**: Warning - Create ticket

**Impact**: Elevated errors but not yet critical (< 5% success rate threshold).

#### Diagnostic Steps

1. **Check current error rate:**
   ```promql
   empath_delivery_error_rate
   ```

2. **Identify error types:**
   ```bash
   {service="empath"} | json | level="ERROR" | fields.component="delivery" | fields.smtp_code!=""
   ```

3. **Check failing domains:**
   ```promql
   topk(10, rate(empath_delivery_attempts_total{status="failed"}[5m])) by (domain)
   ```

4. **Review error distribution:**
   ```promql
   sum by (smtp_code) (rate(empath_delivery_attempts_total{status="failed"}[5m]))
   ```

#### Remediation

1. **Categorize errors:**
   - **4xx errors (temporary):** Recipient server busy, retry will likely succeed
   - **5xx errors (permanent):** Invalid recipient, policy rejection, etc.
   - **Network errors:** DNS, connection failures

2. **Take action based on error type:**

   a. **For 4xx (temporary) errors:**
   - Monitor - retries should succeed
   - If persistent: Contact recipient administrator

   b. **For 5xx (permanent) errors:**
   - Review sender practices (SPF, DKIM, DMARC)
   - May indicate reputation issue or policy violation
   - Check if messages are spam

   c. **For network errors:**
   - Check DNS: `empathctl dns list-cache`
   - Check connectivity to common mail servers

3. **Monitor trend:**
   ```promql
   empath_delivery_error_rate
   ```
   Should decrease below 5% as transient issues resolve.

#### Long-term Actions

- Investigate if error rate correlates with specific sender or content type
- Review email authentication (SPF, DKIM, DMARC setup)
- Monitor for patterns indicating reputation issues

---

### QueueSizeGrowing

**Alert**: Queue size growing (> 10 msgs/min increase for 15 minutes)

**Severity**: Warning - Create ticket

**Impact**: Early warning of backlog. System load increasing faster than delivery rate.

#### Diagnostic Steps

1. **Check queue growth rate:**
   ```promql
   deriv(empath_delivery_queue_size{status="pending"}[10m])
   ```

2. **Check ingress rate:**
   ```promql
   rate(empath_smtp_messages_received_total[5m])
   ```

3. **Check delivery rate:**
   ```promql
   rate(empath_delivery_attempts_total{status="delivered"}[5m])
   ```

4. **Check queue size:**
   ```bash
   empathctl queue stats
   ```

#### Remediation

1. **Compare ingress vs. delivery rates:**
   ```bash
   # If ingress > delivery: System cannot keep up
   # If ingress < delivery but queue growing: Delivery rate declining
   ```

2. **Identify bottleneck:**

   a. **If delivery rate declining:**
   - Check for network issues
   - Check for DNS problems
   - Check for recipient server slowness

   b. **If ingress rate spiking:**
   - Normal traffic surge (e.g., newsletter)
   - Possible spam attack
   - Check SMTP connection rate

3. **Take action:**

   a. **For normal traffic surge:**
   - Monitor - queue should drain naturally
   - Consider increasing `max_concurrent_deliveries`

   b. **For delivery rate issues:**
   - Follow procedures in [DeliveryLatencyHigh](#deliverylatencyhigh)

   c. **For spam attack:**
   - Implement rate limiting on SMTP connections
   - Review sender authentication requirements

4. **Monitor stabilization:**
   ```promql
   deriv(empath_delivery_queue_size{status="pending"}[10m])
   ```
   Growth rate should decrease to near-zero or negative (queue draining).

#### Long-term Actions

- Capacity planning: Review if current resources sufficient for peak load
- Implement auto-scaling (if infrastructure supports)
- Set up predictive alerts based on traffic patterns

---

### CircuitBreakerOpen

**Alert**: Circuit breaker open for domain (> 10 minutes)

**Severity**: Warning - Create ticket

**Impact**: Deliveries to specific domain are being rejected. Messages accumulate in retry queue.

#### Diagnostic Steps

1. **Identify affected domain:**
   ```promql
   empath_delivery_circuit_breaker_state{state="Open"} == 1
   ```

2. **Check recent failures:**
   ```bash
   {service="empath"} | json | fields.domain="$DOMAIN" | level="ERROR"
   ```

3. **Check failure pattern:**
   ```promql
   rate(empath_delivery_circuit_breaker_trips_total{domain="$DOMAIN"}[30m])
   ```

4. **Test connectivity to domain:**
   ```bash
   # Resolve MX records
   dig $DOMAIN MX

   # Test SMTP connection
   telnet <mx-hostname> 25
   ```

#### Remediation

1. **Determine if recipient issue or Empath issue:**

   a. **If recipient server down:**
   - Wait for server recovery
   - Circuit breaker will auto-recover and retry
   - Monitor: `empath_delivery_circuit_breaker_state{domain="$DOMAIN"}`

   b. **If DNS issue:**
   ```bash
   empathctl dns refresh $DOMAIN
   ```

   c. **If Empath configuration issue:**
   - Review domain-specific settings in config
   - Check rate limits, timeout values

2. **Manual recovery (if needed):**
   ```bash
   # Circuit breaker will transition to Half-Open automatically
   # Manual retry can test recovery:
   empathctl queue retry <message-id-for-domain>
   ```

3. **Monitor state transitions:**
   ```promql
   empath_delivery_circuit_breaker_state{domain="$DOMAIN"}
   ```
   States: Open (1) → Half-Open (2) → Closed (0)

#### Long-term Actions

- If circuit breaker trips frequently for domain: Review configuration
- Document known-problematic domains and mitigation strategies
- Consider domain-specific circuit breaker thresholds

---

## General Troubleshooting Resources

### Useful Commands

**Queue Management:**
```bash
empathctl queue list [--status pending|retry|failed]
empathctl queue stats [--watch]
empathctl queue view <message-id>
empathctl queue retry <message-id>
empathctl queue process-now
```

**DNS Management:**
```bash
empathctl dns list-cache
empathctl dns clear-cache
empathctl dns refresh <domain>
```

**System Status:**
```bash
empathctl system status
empathctl system ping
```

### Useful Prometheus Queries

**Delivery Metrics:**
```promql
# Success rate
empath_delivery_success_rate

# Error rate
empath_delivery_error_rate

# Queue sizes by status
empath_delivery_queue_size

# Delivery latency (p50, p95, p99)
histogram_quantile(0.95, rate(empath_delivery_queue_age_seconds_bucket[5m]))

# Deliveries per second
rate(empath_delivery_attempts_total[5m])
```

**SMTP Metrics:**
```promql
# Connection rate
rate(empath_smtp_connections_total[5m])

# Active connections
empath_smtp_connections_active

# Error rate
empath_smtp_connection_error_rate
```

**DNS Metrics:**
```promql
# Cache hit rate
rate(empath_dns_cache_hits_total[5m]) / (rate(empath_dns_cache_hits_total[5m]) + rate(empath_dns_cache_misses_total[5m]))

# Lookup duration
rate(empath_dns_lookup_duration_seconds_sum[5m]) / rate(empath_dns_lookup_duration_seconds_count[5m])
```

### Log Queries (Loki/LogQL)

**Error Investigation:**
```logql
{service="empath"} | json | level="ERROR"
{service="empath"} | json | level="ERROR" | fields.component="delivery"
{service="empath"} | json | fields.message_id="01JCXYZ..."
```

**Delivery Tracking:**
```logql
{service="empath"} | json | fields.domain="example.com"
{service="empath"} | json | fields.smtp_code=~"5.."
```

### Dashboards

- **Empath MTA - Overview**: http://grafana:3000/d/empath-overview
- **Empath MTA - Delivery**: http://grafana:3000/d/empath-delivery
- **Empath MTA - SMTP**: http://grafana:3000/d/empath-smtp
- **Empath MTA - Queue**: http://grafana:3000/d/empath-queue
- **Empath MTA - DNS**: http://grafana:3000/d/empath-dns
- **Empath MTA - Log Exploration**: http://grafana:3000/d/empath-logs

### Escalation Contacts

- **On-call Engineer**: [Paging system]
- **Empath Development Team**: [Ticket system / Slack channel]
- **Infrastructure Team**: [For network, DNS, disk issues]
- **Security Team**: [For spam/abuse incidents]

### Additional Resources

- **Configuration Reference**: `empath.config.ron` with inline comments
- **Architecture Documentation**: `CLAUDE.md`
- **Metric Catalog**: [README.md](README.md#metrics-reference)
- **SLO Definitions**: [README.md](README.md#slo-definitions)
