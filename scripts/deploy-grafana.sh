#!/bin/bash
# Deploy Grafana dashboards to e18n.net

set -e

HOST="root@e18n.net"
REMOTE_DIR="/opt/avero/grafana/dashboards"
LOCAL_DIR="$(dirname "$0")/../grafana"

# Default to netto-grandi if no argument
DASHBOARD="${1:-netto-grandi}"
DASHBOARD_FILE="$LOCAL_DIR/$DASHBOARD.json"

if [ ! -f "$DASHBOARD_FILE" ]; then
    echo "Dashboard file not found: $DASHBOARD_FILE"
    echo "Usage: $0 [dashboard-name]"
    echo "Available dashboards:"
    ls -1 "$LOCAL_DIR"/*.json 2>/dev/null | xargs -n1 basename | sed 's/.json$//'
    exit 1
fi

echo "Deploying $DASHBOARD to Grafana..."

# Copy file to server
scp "$DASHBOARD_FILE" "$HOST:$REMOTE_DIR/"

# Update via API (database version takes precedence over file provisioning)
ssh "$HOST" "jq '{dashboard: ., overwrite: true}' $REMOTE_DIR/$DASHBOARD.json | curl -s -X POST -H 'Content-Type: application/json' -u 'admin:avero' -d @- 'http://localhost:3000/api/dashboards/db'" | jq -r '.status // .message'

echo "Done: https://grafana.e18n.net/d/$DASHBOARD"
