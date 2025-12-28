# Disaster Recovery

This runbook documents Empath disaster recovery procedures for single-node
deployments with a file-backed spool. Adapt the paths and service manager
commands to your environment.

## Scope

- File-backed spool (`/var/spool/empath`)
- Local config (`/etc/empath/empath.config.ron`)
- TLS certificates (`/etc/empath/tls`)
- Optional queue state (`queue_state.bin`)

## Recovery Objectives

- **RPO (Recovery Point Objective):** equal to your backup interval
  (e.g., daily backups → up to 24h data loss).
- **RTO (Recovery Time Objective):** time to restore spool + config and restart
  service (typically 15–60 minutes).

## Backup

The `empathctl` helper backs up spool and queue state by default. Use explicit
paths for config and TLS if you want them included.

```bash
empathctl \
  --spool-path /var/spool/empath \
  --queue-state /var/spool/empath/queue_state.bin \
  backup \
  --output-dir /backup/empath/backup-$(date +%Y%m%d-%H%M%S) \
  --config-path /etc/empath/empath.config.ron \
  --tls-path /etc/empath/tls
```

## Restore

```bash
# Stop service
sudo systemctl stop empath

# Restore from backup directory
empathctl \
  --spool-path /var/spool/empath \
  --queue-state /var/spool/empath/queue_state.bin \
  restore \
  --input-dir /backup/empath/backup-20250115-120000 \
  --config-path /etc/empath/empath.config.ron \
  --tls-path /etc/empath/tls \
  --force

# Fix permissions
sudo chown -R empath:empath /var/spool/empath /etc/empath/tls /etc/empath/empath.config.ron

# Start service
sudo systemctl start empath

# Verify queue health
empathctl queue stats
```

## Validation Scenarios

Use these steps to validate DR readiness after changes:

- **Spool restore:** Back up, delete `/var/spool/empath`, restore, and confirm
  `empathctl queue stats` reflects expected counts.
- **Queue state restore:** Restore `queue_state.bin`, then confirm retries are
  scheduled (no immediate retry storms).
- **Config/TLS restore:** Restore config and TLS, then run `empathctl system reload-config`
  and `empathctl system reload-tls`, verify `empathctl system tls`.

## Notes

- Restore operations are destructive when `--force` is used. Ensure you are
  pointing at the correct target directories.
- For multi-node or shared-spool deployments, coordinate restores to avoid
  concurrent writes during recovery.
