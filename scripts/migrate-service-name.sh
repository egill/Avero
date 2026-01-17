#!/bin/bash
set -e

# Migration script: Rename gateway-poc service to gateway
# Run this ONCE on each server before deploying with the new scripts

if [ -z "$1" ]; then
    echo "Usage: $0 <host>"
    echo "  host: netto or avero"
    echo ""
    echo "Example: $0 netto"
    exit 1
fi

case "$1" in
    netto)
        HOST="avero@100.80.187.3"
        SITE="netto"
        ;;
    avero)
        HOST="avero@100.65.110.63"
        SITE="avero"
        ;;
    *)
        echo "Unknown host: $1"
        echo "Valid options: netto, avero"
        exit 1
        ;;
esac

echo "Migrating service name on $HOST (site: $SITE)..."
echo ""

ssh "$HOST" "SITE=$SITE bash -s" << 'REMOTE_SCRIPT'
set -e

echo "=== Checking current state ==="

# Check if old service exists
if systemctl list-unit-files | grep -q gateway-poc; then
    echo "Found gateway-poc service"
    OLD_SERVICE_EXISTS=true
else
    echo "No gateway-poc service found"
    OLD_SERVICE_EXISTS=false
fi

# Check if new service already exists
if systemctl list-unit-files | grep -q "gateway.service"; then
    echo "WARNING: gateway.service already exists"
    echo "Migration may have already been done."
    exit 0
fi

if [ "$OLD_SERVICE_EXISTS" = false ]; then
    echo "No migration needed - gateway-poc service doesn't exist"
    echo "Creating fresh gateway service..."
fi

echo ""
echo "=== Creating new service file ==="

# Create new systemd service file
sudo tee /etc/systemd/system/gateway.service > /dev/null << 'EOF'
[Unit]
Description=Avero Gateway
After=network.target

[Service]
Type=simple
User=avero
ExecStart=/opt/avero/gateway-bin --config /opt/avero/config/SITE_PLACEHOLDER.toml
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
EOF

sudo sed -i "s/SITE_PLACEHOLDER/$SITE/g" /etc/systemd/system/gateway.service
echo "Created /etc/systemd/system/gateway.service"

if [ "$OLD_SERVICE_EXISTS" = true ]; then
    echo ""
    echo "=== Stopping old service ==="
    sudo systemctl stop gateway-poc || true

    echo ""
    echo "=== Migrating binary ==="
    # Copy binary to new location if old one exists
    if [ -f /opt/avero/gateway-poc-bin ]; then
        sudo cp /opt/avero/gateway-poc-bin /opt/avero/gateway-bin
        echo "Copied gateway-poc-bin to gateway-bin"
    fi
fi

echo ""
echo "=== Enabling new service ==="
sudo systemctl daemon-reload
sudo systemctl enable gateway

if [ "$OLD_SERVICE_EXISTS" = true ]; then
    echo ""
    echo "=== Starting new service ==="
    sudo systemctl start gateway

    echo ""
    echo "=== Disabling old service ==="
    sudo systemctl disable gateway-poc || true

    echo ""
    echo "=== Verifying ==="
    sudo systemctl status gateway --no-pager || true
fi

echo ""
echo "=== Migration complete ==="
echo ""
echo "Old service: gateway-poc (disabled)"
echo "New service: gateway (enabled)"
echo ""
echo "To clean up old files later:"
echo "  sudo rm /etc/systemd/system/gateway-poc.service"
echo "  sudo rm /opt/avero/gateway-poc-bin"

REMOTE_SCRIPT

echo ""
echo "Done. You can now deploy using: ./scripts/deploy-$1.sh"
