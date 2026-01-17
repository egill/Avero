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

use std::sync::Arc;

use clap::Parser;
use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};
use tracing::info;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::EnvFilter;

use gateway::infra::{Config, Metrics};
use gateway::io::analysis_logger::{AnalysisLogger, RotationStrategy};
use gateway::io::{
    create_egress_channel, create_egress_writer, start_acc_listener, AccListenerConfig,
    MqttPublisher, Rs485Monitor,
};
use gateway::services::{create_gate_worker, GateController};

/// Gateway PoC - Automated retail gate control system
#[derive(Parser, Debug)]
#[command(name = "gateway-poc", version, about)]
struct Args {
    /// Path to TOML configuration file
    #[arg(short, long, default_value = "config/dev.toml")]
    config: String,
}

/// Calculate queue utilization as a percentage (0-100).
#[inline]
fn utilization_pct(used: u64, capacity: u64) -> u64 {
    if capacity > 0 {
        used * 100 / capacity
    } else {
        0
    }
}

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

    info!(
        version = env!("CARGO_PKG_VERSION"),
        git_hash = option_env!("GIT_HASH").unwrap_or("unknown"),
        "gateway_starting"
    );

    // Parse command line arguments using clap
    let args = Args::parse();

    // Load configuration from TOML file (needed for broker config)
    let config = Config::load_from_path(&args.config);

    // Start embedded MQTT broker with config
    gateway::infra::broker::start_embedded_broker(&config);

    info!(
        config_file = %config.config_file(),
        mqtt_host = %config.mqtt_host(),
        mqtt_port = %config.mqtt_port(),
        mqtt_topic = %config.mqtt_topic(),
        mqtt_egress_host = %config.mqtt_egress_host(),
        mqtt_egress_port = %config.mqtt_egress_port(),
        gate_mode = ?config.gate_mode(),
        gate_tcp_addr = %config.gate_tcp_addr(),
        min_dwell_ms = %config.min_dwell_ms(),
        pos_zones = ?config.pos_zones(),
        gate_zone = %config.gate_zone(),
        prometheus_port = %config.prometheus_port(),
        "config_loaded"
    );

    // Initialize analysis logger if enabled (for offline position data analysis)
    let analysis_logger = if config.analysis_log_enabled() {
        let rotation = match config.analysis_log_rotation() {
            s if s.starts_with("size:") => {
                let mb: u64 = s[5..].parse().unwrap_or(100);
                RotationStrategy::Size(mb * 1024 * 1024)
            }
            _ => RotationStrategy::Daily,
        };
        Some(Arc::new(Mutex::new(AnalysisLogger::with_rotation(
            config.analysis_log_dir(),
            config.site_id(),
            rotation,
        ))))
    } else {
        None
    };

    // Create shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create shared components
    let metrics = Arc::new(Metrics::new());
    let gate = Arc::new(GateController::new(config.clone(), Some(metrics.clone())));

    // Initialize POS zone tracking
    metrics.set_pos_zones(config.pos_zones());

    // Create MQTT egress channel early (needed by gate worker for timing events)
    // The publisher will be started later if mqtt_egress is enabled
    let (egress_sender, egress_rx) = if config.mqtt_egress_enabled() {
        let (sender, rx) = create_egress_channel(1000, config.site_id().to_string());
        (Some(sender), Some(rx))
    } else {
        (None, None)
    };

    // Start CloudPlus TCP client if in TCP mode
    if let Some(tcp_client) = gate.tcp_client() {
        let tcp_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            tcp_client.run(tcp_shutdown).await;
        });
    }

    // Create gate command worker (decouples gate I/O from tracker loop)
    let (gate_cmd_tx, gate_worker) =
        create_gate_worker(gate.clone(), metrics.clone(), 64, egress_sender.clone());
    tokio::spawn(async move {
        gate_worker.run().await;
    });

    // Create egress writer (decouples file I/O from tracker loop)
    let (journey_tx, egress_writer) = create_egress_writer(config.egress_file().to_string(), 100);
    tokio::spawn(async move {
        egress_writer.run().await;
    });

    // Create event channel (bounded for backpressure)
    // Keep a clone of the sender for queue depth sampling
    let (event_tx, event_rx) = mpsc::channel(1000);
    let event_tx_sampler = event_tx.clone();

    // Create watch channel for door state (lossless - latest value always available)
    let (door_tx, door_rx) = watch::channel(gateway::domain::types::DoorStatus::Unknown);

    // Start RS485 monitor (with watch channel for door state changes)
    // Clone door_tx since we also need it for the HTTP /door/simulate endpoint
    let rs485_monitor = Rs485Monitor::new(&config).with_door_tx(door_tx.clone());
    let rs485_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        rs485_monitor.run(rs485_shutdown).await;
    });

    // Start MQTT client (with metrics for drop tracking and position streaming)
    let mqtt_config = config.clone();
    let mqtt_tx = event_tx.clone();
    let mqtt_metrics = metrics.clone();
    let mqtt_shutdown = shutdown_rx.clone();
    let mqtt_analysis_logger = analysis_logger.clone();
    let mqtt_egress_sender = egress_sender.clone();
    tokio::spawn(async move {
        if let Err(e) = gateway::io::mqtt::start_mqtt_client(
            &mqtt_config,
            mqtt_tx,
            mqtt_metrics,
            mqtt_shutdown,
            mqtt_analysis_logger,
            mqtt_egress_sender,
        )
        .await
        {
            tracing::error!(error = %e, "MQTT client error");
        }
    });

    // Clone event_tx for the HTTP server's ACC simulation endpoint
    let prom_event_tx = event_tx.clone();

    // Start ACC TCP listener (with metrics for drop tracking)
    let acc_config = AccListenerConfig {
        port: config.acc_listener_port(),
        enabled: config.acc_listener_enabled(),
    };
    let acc_tx = event_tx;
    let acc_metrics = metrics.clone();
    let acc_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        if let Err(e) = start_acc_listener(acc_config, acc_tx, acc_metrics, acc_shutdown).await {
            tracing::error!(error = %e, "ACC listener error");
        }
    });

    // Start Prometheus metrics HTTP server (if port > 0)
    let prometheus_port = config.prometheus_port();
    if prometheus_port > 0 {
        let prom_metrics = metrics.clone();
        let prom_site_id = config.site_id().to_string();
        let prom_gate = gate.clone();
        let prom_door_tx = door_tx.clone(); // For /door/simulate endpoint
        let prom_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = gateway::io::prometheus::start_metrics_server(
                prometheus_port,
                prom_metrics,
                prom_site_id,
                Some(prom_gate),
                Some(prom_event_tx),
                Some(prom_door_tx),
                prom_shutdown,
            )
            .await
            {
                tracing::error!(error = %e, "Prometheus metrics server error");
            }
        });
    }

    // Start metrics reporter (lock-free reads with full summary)
    // Also samples queue depths for diagnosability
    let metrics_clone = metrics.clone();
    let gate_for_metrics = gate.clone();
    let metrics_interval = config.metrics_interval_secs();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(metrics_interval));
        loop {
            interval.tick().await;

            // Sample queue depths (max_capacity - capacity = current depth)
            let event_max_capacity = event_tx_sampler.max_capacity() as u64;
            let event_depth =
                (event_tx_sampler.max_capacity() - event_tx_sampler.capacity()) as u64;
            let cloudplus_depth = gate_for_metrics.cloudplus_queue_depth() as u64;
            let gate_max_capacity = gate_for_metrics.cloudplus_max_capacity() as u64;
            metrics_clone.set_event_queue_depth(event_depth);
            // gate_queue_depth = CloudPlus outbound per PRD US-005
            metrics_clone.set_gate_queue_depth(cloudplus_depth);
            metrics_clone.set_cloudplus_queue_depth(cloudplus_depth);

            metrics_clone
                .set_event_queue_utilization_pct(utilization_pct(event_depth, event_max_capacity));
            metrics_clone.set_gate_queue_utilization_pct(utilization_pct(
                cloudplus_depth,
                gate_max_capacity,
            ));

            // Use full report with placeholder track counts (actual counts are in tracker)
            let summary = metrics_clone.report(0, 0);
            summary.log();
        }
    });

    // Start MQTT egress publisher (if enabled, channel was created earlier)
    if let (Some(ref sender), Some(rx)) = (&egress_sender, egress_rx) {
        // Start MQTT egress publisher
        let publisher = MqttPublisher::new(&config, rx);
        let publisher_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            publisher.run(publisher_shutdown).await;
        });

        // Start metrics egress publisher (separate from logging)
        // Clone door_rx to get current gate state for heartbeat
        let metrics_egress = sender.clone();
        let metrics_for_egress = metrics.clone();
        let metrics_door_rx = door_rx.clone();
        let egress_interval = config.mqtt_egress_metrics_interval_secs();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(egress_interval));
            loop {
                interval.tick().await;
                let summary = metrics_for_egress.report(0, 0);
                let door_status = *metrics_door_rx.borrow();
                metrics_egress.send_metrics(summary, door_status.as_str());
            }
        });
    }

    // Start tracker (main event processing loop)
    let mut tracker = gateway::services::Tracker::new(
        config,
        gate_cmd_tx,
        journey_tx,
        metrics,
        egress_sender,
        door_rx,
    );
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
