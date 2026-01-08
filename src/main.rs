//! Gateway PoC - Rust MVP for gate control system
//!
//! Validates performance characteristics (latency, predictability) for gate control
//! running on Raspberry Pi 5.
//!
//! Module structure:
//! - `domain/` - Core business types (Journey, Person, Events)
//! - `io/` - External interfaces (MQTT, RS485, CloudPlus, Egress)
//! - `services/` - Business logic (Tracker, JourneyManager, Gate)
//! - `infra/` - Infrastructure (Config, Metrics, Broker)

use clap::Parser;
use gateway_poc::infra::{Config, GateMode, Metrics};

/// Gateway PoC - Automated retail gate control system
#[derive(Parser, Debug)]
#[command(name = "gateway-poc", version, about)]
struct Args {
    /// Path to TOML configuration file
    #[arg(short, long, default_value = "config/dev.toml")]
    config: String,
}
use gateway_poc::io::{
    create_egress_channel, start_acc_listener, AccListenerConfig, MqttPublisher, Rs485Monitor,
};
use gateway_poc::services::GateController;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::info;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize structured logging with configurable level via RUST_LOG env var
    // Default: INFO, use RUST_LOG=debug for full event visibility
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(UtcTime::rfc_3339())
        .with_target(false)
        .init();

    info!("gateway-poc starting");

    // Parse command line arguments using clap
    let args = Args::parse();

    // Load configuration from TOML file (needed for broker config)
    let config = Config::load_from_path(&args.config);

    // Start embedded MQTT broker with config
    gateway_poc::infra::broker::start_embedded_broker(&config);

    // Log configuration
    let gate_mode_str = match config.gate_mode() {
        GateMode::Tcp => "tcp",
        GateMode::Http => "http",
    };
    info!(
        config_file = %config.config_file(),
        mqtt_host = %config.mqtt_host(),
        mqtt_port = %config.mqtt_port(),
        mqtt_topic = %config.mqtt_topic(),
        gate_mode = %gate_mode_str,
        gate_tcp_addr = %config.gate_tcp_addr(),
        min_dwell_ms = %config.min_dwell_ms(),
        pos_zones = ?config.pos_zones(),
        gate_zone = %config.gate_zone(),
        prometheus_port = %config.prometheus_port(),
        "config_loaded"
    );

    // Create shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create shared components
    let gate = Arc::new(GateController::new(config.clone()));
    let metrics = Arc::new(Metrics::new());

    // Initialize POS zone tracking
    metrics.set_pos_zones(config.pos_zones());

    // Start CloudPlus TCP client if in TCP mode
    if let Some(tcp_client) = gate.tcp_client() {
        let tcp_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            tcp_client.run(tcp_shutdown).await;
        });
    }

    // Create event channel (bounded for backpressure)
    let (event_tx, event_rx) = mpsc::channel(1000);

    // Start RS485 monitor (with event channel for door state changes)
    let rs485_tx = event_tx.clone();
    let rs485_monitor = Rs485Monitor::new(&config).with_event_tx(rs485_tx);
    let rs485_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        rs485_monitor.run(rs485_shutdown).await;
    });

    // Start MQTT client
    let mqtt_config = config.clone();
    let mqtt_tx = event_tx.clone();
    let mqtt_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        if let Err(e) = gateway_poc::io::mqtt::start_mqtt_client(&mqtt_config, mqtt_tx, mqtt_shutdown).await {
            tracing::error!(error = %e, "MQTT client error");
        }
    });

    // Start ACC TCP listener
    let acc_config = AccListenerConfig {
        port: config.acc_listener_port(),
        enabled: config.acc_listener_enabled(),
    };
    let acc_tx = event_tx;
    let acc_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        if let Err(e) = start_acc_listener(acc_config, acc_tx, acc_shutdown).await {
            tracing::error!(error = %e, "ACC listener error");
        }
    });

    // Start Prometheus metrics HTTP server (if port > 0)
    let prometheus_port = config.prometheus_port();
    if prometheus_port > 0 {
        let prom_metrics = metrics.clone();
        let prom_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) =
                gateway_poc::io::prometheus::start_metrics_server(prometheus_port, prom_metrics, prom_shutdown)
                    .await
            {
                tracing::error!(error = %e, "Prometheus metrics server error");
            }
        });
    }

    // Start metrics reporter (lock-free reads with full summary)
    let metrics_clone = metrics.clone();
    let metrics_interval = config.metrics_interval_secs();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(metrics_interval));
        loop {
            interval.tick().await;
            // Use full report with placeholder track counts (actual counts are in tracker)
            let summary = metrics_clone.report(0, 0);
            summary.log();
        }
    });

    // Create MQTT egress channel and publisher (if enabled)
    let egress_sender = if config.mqtt_egress_enabled() {
        let (egress_sender, egress_rx) = create_egress_channel(1000, config.site_id().to_string());

        // Start MQTT egress publisher
        let publisher = MqttPublisher::new(&config, egress_rx);
        let publisher_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            publisher.run(publisher_shutdown).await;
        });

        // Start metrics egress publisher (separate from logging)
        let metrics_egress = egress_sender.clone();
        let metrics_for_egress = metrics.clone();
        let egress_interval = config.mqtt_egress_metrics_interval_secs();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(egress_interval));
            loop {
                interval.tick().await;
                let summary = metrics_for_egress.report(0, 0);
                metrics_egress.send_metrics(summary);
            }
        });

        Some(egress_sender)
    } else {
        None
    };

    // Start tracker (main event processing loop)
    let mut tracker = gateway_poc::services::Tracker::new(config, gate, metrics, egress_sender);
    info!("tracker_started");

    // Handle shutdown on Ctrl+C
    let shutdown_signal = shutdown_tx;
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("shutdown_signal_received");
        let _ = shutdown_signal.send(true);
    });

    // Run tracker - consumes events until channel closes
    tracker.run(event_rx).await;

    info!("gateway-poc shutdown complete");
    Ok(())
}
