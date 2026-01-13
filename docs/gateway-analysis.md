# Gateway Analysis Logger

The `gateway-analysis` binary captures raw data from all gateway input sources for offline debugging and correlation analysis.

## Mosquitto Broker Setup

During analysis runs, a dedicated Mosquitto broker replaces the gateway's embedded broker. This allows capturing all MQTT traffic while the gateway connects as a client.

### Prerequisites

Install Mosquitto on the target system:

```bash
# Raspberry Pi / Debian
sudo apt update && sudo apt install -y mosquitto mosquitto-clients

# macOS
brew install mosquitto
```

### Configuration

Copy config files to the Mosquitto directory:

```bash
# On Raspberry Pi
sudo cp config/mosquitto.conf /etc/mosquitto/conf.d/gateway-analysis.conf
sudo cp config/mosquitto.passwd /etc/mosquitto/passwd

# Hash the password file (required before first use)
sudo mosquitto_passwd -U /etc/mosquitto/passwd
```

### Starting the Broker

```bash
# Stop any existing gateway service (uses embedded broker)
sudo systemctl stop gateway-poc

# Start Mosquitto
sudo systemctl start mosquitto

# Or run in foreground for debugging
mosquitto -c /etc/mosquitto/conf.d/gateway-analysis.conf -v
```

### Verifying Connectivity

Test that the broker accepts connections:

```bash
# Subscribe to all topics (run in one terminal)
mosquitto_sub -h localhost -u avero -P avero -t '#' -v

# Publish a test message (run in another terminal)
mosquitto_pub -h localhost -u avero -P avero -t 'test/topic' -m 'hello'
```

You should see `test/topic hello` in the subscriber terminal.

### Stopping the Broker

```bash
sudo systemctl stop mosquitto

# Restart gateway service
sudo systemctl start gateway-poc
```

### Broker Logs

Logs are written to `/var/log/mosquitto/mosquitto.log`. Monitor in real-time:

```bash
sudo tail -f /var/log/mosquitto/mosquitto.log
```

## Quick Reference

| Setting | Value |
|---------|-------|
| Broker host | localhost |
| Broker port | 1883 |
| Username | avero |
| Password | avero |
| Config file | `/etc/mosquitto/conf.d/gateway-analysis.conf` |
| Password file | `/etc/mosquitto/passwd` |
| Log file | `/var/log/mosquitto/mosquitto.log` |
