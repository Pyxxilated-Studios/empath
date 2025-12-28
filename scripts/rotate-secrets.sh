#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Rotate Empath secrets and emit config snippets.

Usage:
  scripts/rotate-secrets.sh [options]

Options:
  --control-token [role]  Generate a control token (role: admin|read_only, default: admin)
  --metrics-api-key       Generate a metrics API key
  --tls-reload            Print TLS reload reminder
  --empathctl PATH        Path to empathctl (default: empathctl)
  -h, --help              Show this help

Examples:
  scripts/rotate-secrets.sh --control-token admin --metrics-api-key
  scripts/rotate-secrets.sh --control-token read_only --tls-reload
EOF
}

if [[ ${1:-} == "-h" || ${1:-} == "--help" ]]; then
    usage
    exit 0
fi

CONTROL_ROLES=()
METRICS=false
TLS_RELOAD=false
EMPATHCTL=${EMPATHCTL:-empathctl}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --control-token)
            role=${2:-admin}
            if [[ -z "${2:-}" || "${2:-}" == --* ]]; then
                role="admin"
                shift
            else
                shift 2
            fi
            case "$role" in
                admin) CONTROL_ROLES+=("admin") ;;
                read_only|readonly|read-only) CONTROL_ROLES+=("read_only") ;;
                *)
                    echo "Unknown control token role: $role" >&2
                    exit 1
                    ;;
            esac
            ;;
        --metrics-api-key)
            METRICS=true
            shift
            ;;
        --tls-reload)
            TLS_RELOAD=true
            shift
            ;;
        --empathctl)
            EMPATHCTL=${2:?Missing empathctl path}
            shift 2
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage
            exit 1
            ;;
    esac
done

if [[ ${#CONTROL_ROLES[@]} -eq 0 && "$METRICS" == "false" && "$TLS_RELOAD" == "false" ]]; then
    usage
    exit 1
fi

generate_pair() {
    python3 - <<'PY'
import hashlib
import secrets

token = secrets.token_hex(32)
digest = hashlib.sha256(token.encode()).hexdigest()
print(f"{token}|{digest}")
PY
}

if [[ ${#CONTROL_ROLES[@]} -gt 0 ]]; then
    echo "Control tokens:"
    CONTROL_ENTRIES=()
    for role in "${CONTROL_ROLES[@]}"; do
        pair=$(generate_pair)
        token=${pair%%|*}
        digest=${pair#*|}
        CONTROL_ENTRIES+=("        (hash: \"${digest}\", role: \"${role}\")")
        echo "  role=${role}"
        echo "    token: ${token}"
        echo "    hash:  ${digest}"
        echo "    use:   ${EMPATHCTL} --token \"${token}\" system status"
    done

    echo ""
    echo "control_auth snippet:"
    echo "control_auth: ("
    echo "    enabled: true,"
    echo "    tokens: ["
    printf "%s,\n" "${CONTROL_ENTRIES[@]}"
    echo "    ],"
    echo ")"
    echo ""
    echo "Reload with: ${EMPATHCTL} system reload-config"
    echo ""
fi

if [[ "$METRICS" == "true" ]]; then
    pair=$(generate_pair)
    metrics_token=${pair%%|*}
    echo "Metrics API key:"
    echo "  api_key: ${metrics_token}"
    echo ""
    echo "metrics snippet:"
    echo "metrics: ("
    echo "    api_key: \"${metrics_token}\","
    echo ")"
    echo ""
    echo "Reload with: ${EMPATHCTL} system reload-config"
    echo ""
fi

if [[ "$TLS_RELOAD" == "true" ]]; then
    echo "TLS reload:"
    echo "  Replace cert/key files, then run: ${EMPATHCTL} system reload-tls"
    echo ""
fi

echo "Store plaintext tokens securely and avoid committing them to config files."
