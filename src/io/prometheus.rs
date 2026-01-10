//! Prometheus metrics HTTP endpoint
//!
//! Exposes gateway metrics in Prometheus text format at /metrics.
//! Uses hyper for the HTTP server.

use crate::domain::types::TrackId;
use crate::infra::metrics::{
    Metrics, MetricsSummary, METRICS_BUCKET_BOUNDS, METRICS_NUM_BUCKETS, METRICS_STITCH_DIST_BOUNDS,
};
use crate::services::gate::GateCommand;
use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::fmt::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{error, info};

/// Prometheus metric type
enum MetricType {
    Counter,
    Gauge,
}

impl MetricType {
    fn as_str(&self) -> &'static str {
        match self {
            MetricType::Counter => "counter",
            MetricType::Gauge => "gauge",
        }
    }
}

/// Write a simple metric (counter or gauge) with site label
fn write_metric(
    output: &mut String,
    name: &str,
    help: &str,
    typ: MetricType,
    site: &str,
    val: u64,
) {
    let _ = writeln!(output, "# HELP {name} {help}");
    let _ = writeln!(output, "# TYPE {name} {}", typ.as_str());
    let _ = writeln!(output, "{name}{{site=\"{site}\"}} {val}");
}

/// Write a gauge metric with f64 value
fn write_gauge_f64(output: &mut String, name: &str, help: &str, site: &str, val: f64) {
    let _ = writeln!(output, "# HELP {name} {help}");
    let _ = writeln!(output, "# TYPE {name} gauge");
    let _ = writeln!(output, "{name}{{site=\"{site}\"}} {val:.6}");
}

/// Write a histogram metric with buckets, sum, and count
fn write_histogram(
    output: &mut String,
    name: &str,
    help: &str,
    site: &str,
    buckets: &[u64; METRICS_NUM_BUCKETS],
    bounds: &[u64; 10],
    avg: u64,
) {
    let _ = writeln!(output, "# HELP {name} {help}");
    let _ = writeln!(output, "# TYPE {name} histogram");

    let mut cumulative = 0u64;
    for (i, &bound) in bounds.iter().enumerate() {
        cumulative += buckets[i];
        let _ = writeln!(output, "{name}_bucket{{site=\"{site}\",le=\"{bound}\"}} {cumulative}");
    }
    cumulative += buckets[METRICS_NUM_BUCKETS - 1];
    let _ = writeln!(output, "{name}_bucket{{site=\"{site}\",le=\"+Inf\"}} {cumulative}");

    let count: u64 = buckets.iter().sum();
    let sum = avg * count;
    let _ = writeln!(output, "{name}_sum{{site=\"{site}\"}} {sum}");
    let _ = writeln!(output, "{name}_count{{site=\"{site}\"}} {count}");
}

/// Format metrics in Prometheus text exposition format
fn format_prometheus_metrics(
    metrics: &Metrics,
    active_tracks: usize,
    authorized_tracks: usize,
    site_id: &str,
) -> String {
    let summary = metrics.report(active_tracks, authorized_tracks);
    let mut output = String::with_capacity(8192);

    write_core_metrics(&mut output, site_id, &summary);
    write_latency_metrics(&mut output, site_id, &summary);
    write_gate_metrics(&mut output, site_id, &summary);
    write_track_metrics(&mut output, site_id, &summary);
    write_pos_occupancy(&mut output, site_id, metrics);
    write_acc_metrics(&mut output, site_id, &summary);
    write_stitch_metrics(&mut output, site_id, &summary);
    write_drop_metrics(&mut output, site_id, &summary);
    write_queue_metrics(&mut output, site_id, &summary);

    output
}

fn write_core_metrics(output: &mut String, site: &str, summary: &MetricsSummary) {
    write_metric(
        output,
        "gateway_events_total",
        "Total events processed",
        MetricType::Counter,
        site,
        summary.events_total,
    );
    let _ = writeln!(output, "# HELP gateway_events_per_sec Events processed per second");
    let _ = writeln!(output, "# TYPE gateway_events_per_sec gauge");
    let _ =
        writeln!(output, "gateway_events_per_sec{{site=\"{site}\"}} {:.2}", summary.events_per_sec);
}

fn write_latency_metrics(output: &mut String, site: &str, summary: &MetricsSummary) {
    write_histogram(
        output,
        "gateway_event_latency_us",
        "Event processing latency in microseconds",
        site,
        &summary.lat_buckets,
        &METRICS_BUCKET_BOUNDS,
        summary.avg_process_latency_us,
    );

    write_metric(
        output,
        "gateway_event_latency_p50_us",
        "50th percentile event latency",
        MetricType::Gauge,
        site,
        summary.lat_p50_us,
    );
    write_metric(
        output,
        "gateway_event_latency_p95_us",
        "95th percentile event latency",
        MetricType::Gauge,
        site,
        summary.lat_p95_us,
    );
    write_metric(
        output,
        "gateway_event_latency_p99_us",
        "99th percentile event latency",
        MetricType::Gauge,
        site,
        summary.lat_p99_us,
    );
}

fn write_gate_metrics(output: &mut String, site: &str, summary: &MetricsSummary) {
    write_metric(
        output,
        "gateway_gate_commands_total",
        "Total gate commands sent",
        MetricType::Counter,
        site,
        summary.gate_commands_sent,
    );

    write_histogram(
        output,
        "gateway_gate_latency_us",
        "Gate command E2E latency in microseconds",
        site,
        &summary.gate_lat_buckets,
        &METRICS_BUCKET_BOUNDS,
        summary.gate_lat_avg_us,
    );

    write_metric(
        output,
        "gateway_gate_latency_p99_us",
        "99th percentile gate command latency",
        MetricType::Gauge,
        site,
        summary.gate_lat_p99_us,
    );
    write_metric(
        output,
        "gateway_gate_latency_max_us",
        "Maximum gate command latency",
        MetricType::Gauge,
        site,
        summary.gate_lat_max_us,
    );
    write_metric(
        output,
        "gateway_gate_state",
        "Current gate state (0=closed, 1=moving, 2=open)",
        MetricType::Gauge,
        site,
        summary.gate_state,
    );
    write_metric(
        output,
        "gateway_exits_total",
        "Total exits through gate",
        MetricType::Counter,
        site,
        summary.exits_total,
    );
}

fn write_track_metrics(output: &mut String, site: &str, summary: &MetricsSummary) {
    write_metric(
        output,
        "gateway_active_tracks",
        "Current active tracks",
        MetricType::Gauge,
        site,
        summary.active_tracks as u64,
    );
    write_metric(
        output,
        "gateway_authorized_tracks",
        "Current authorized tracks",
        MetricType::Gauge,
        site,
        summary.authorized_tracks as u64,
    );
}

fn write_pos_occupancy(output: &mut String, site: &str, metrics: &Metrics) {
    let _ = writeln!(output, "# HELP gateway_pos_occupancy Number of people in each POS zone");
    let _ = writeln!(output, "# TYPE gateway_pos_occupancy gauge");
    for (zone_id, count) in metrics.pos_occupancy() {
        let _ = writeln!(
            output,
            "gateway_pos_occupancy{{site=\"{site}\",zone_id=\"{zone_id}\"}} {count}"
        );
    }
}

fn write_acc_metrics(output: &mut String, site: &str, summary: &MetricsSummary) {
    write_metric(
        output,
        "gateway_acc_events_total",
        "Total ACC events received",
        MetricType::Counter,
        site,
        summary.acc_events_total,
    );
    write_metric(
        output,
        "gateway_acc_matched_total",
        "ACC events matched to tracks",
        MetricType::Counter,
        site,
        summary.acc_matched_total,
    );
    write_metric(
        output,
        "gateway_acc_late_total",
        "ACC events that arrived late (after gate entry)",
        MetricType::Counter,
        site,
        summary.acc_late_total,
    );
    write_metric(
        output,
        "gateway_acc_no_journey_total",
        "ACC events matched but no journey found",
        MetricType::Counter,
        site,
        summary.acc_no_journey_total,
    );
}

fn write_stitch_metrics(output: &mut String, site: &str, summary: &MetricsSummary) {
    write_metric(
        output,
        "gateway_stitch_matched_total",
        "Tracks successfully stitched",
        MetricType::Counter,
        site,
        summary.stitch_matched_total,
    );
    write_metric(
        output,
        "gateway_stitch_expired_total",
        "Tracks truly lost (expired without stitch)",
        MetricType::Counter,
        site,
        summary.stitch_expired_total,
    );

    write_histogram(
        output,
        "gateway_stitch_distance_cm",
        "Stitch distance in centimeters",
        site,
        &summary.stitch_distance_buckets,
        &METRICS_STITCH_DIST_BOUNDS,
        summary.stitch_distance_avg_cm,
    );
    write_metric(
        output,
        "gateway_stitch_distance_avg_cm",
        "Average stitch distance",
        MetricType::Gauge,
        site,
        summary.stitch_distance_avg_cm,
    );

    write_histogram(
        output,
        "gateway_stitch_time_ms",
        "Stitch time in milliseconds",
        site,
        &summary.stitch_time_buckets,
        &METRICS_BUCKET_BOUNDS,
        summary.stitch_time_avg_ms,
    );
    write_metric(
        output,
        "gateway_stitch_time_avg_ms",
        "Average stitch time",
        MetricType::Gauge,
        site,
        summary.stitch_time_avg_ms,
    );
}

fn write_drop_metrics(output: &mut String, site: &str, summary: &MetricsSummary) {
    write_metric(
        output,
        "gateway_mqtt_events_received_total",
        "MQTT events received (before try_send)",
        MetricType::Counter,
        site,
        summary.mqtt_events_received,
    );
    write_metric(
        output,
        "gateway_mqtt_events_dropped_total",
        "MQTT events dropped due to channel full",
        MetricType::Counter,
        site,
        summary.mqtt_events_dropped,
    );
    write_gauge_f64(
        output,
        "gateway_mqtt_drop_ratio",
        "MQTT drop ratio (dropped / received)",
        site,
        summary.mqtt_drop_ratio,
    );

    write_metric(
        output,
        "gateway_acc_events_received_total",
        "ACC events received (before try_send)",
        MetricType::Counter,
        site,
        summary.acc_events_received,
    );
    write_metric(
        output,
        "gateway_acc_events_dropped_total",
        "ACC events dropped due to channel full",
        MetricType::Counter,
        site,
        summary.acc_events_dropped,
    );
    write_gauge_f64(
        output,
        "gateway_acc_drop_ratio",
        "ACC drop ratio (dropped / received)",
        site,
        summary.acc_drop_ratio,
    );

    write_metric(
        output,
        "gateway_gate_cmds_dropped_total",
        "Gate commands dropped due to channel full",
        MetricType::Counter,
        site,
        summary.gate_cmds_dropped,
    );
    write_metric(
        output,
        "gateway_journey_egress_dropped_total",
        "Journey egress events dropped",
        MetricType::Counter,
        site,
        summary.journey_egress_dropped,
    );
    write_metric(
        output,
        "gateway_journey_egress_received_total",
        "Journey egress events attempted (before try_send)",
        MetricType::Counter,
        site,
        summary.journey_egress_received,
    );
    write_gauge_f64(
        output,
        "gateway_egress_drop_ratio",
        "Journey egress drop ratio (dropped / received)",
        site,
        summary.egress_drop_ratio,
    );
}

fn write_queue_metrics(output: &mut String, site: &str, summary: &MetricsSummary) {
    write_histogram(
        output,
        "gateway_gate_queue_delay_us",
        "Gate command queue delay in microseconds",
        site,
        &summary.gate_queue_delay_buckets,
        &METRICS_BUCKET_BOUNDS,
        summary.gate_queue_delay_avg_us,
    );
    write_metric(
        output,
        "gateway_gate_queue_delay_p99_us",
        "99th percentile gate queue delay",
        MetricType::Gauge,
        site,
        summary.gate_queue_delay_p99_us,
    );
    write_metric(
        output,
        "gateway_gate_queue_delay_max_us",
        "Maximum gate queue delay",
        MetricType::Gauge,
        site,
        summary.gate_queue_delay_max_us,
    );

    write_metric(
        output,
        "gateway_event_queue_depth",
        "Current event queue depth",
        MetricType::Gauge,
        site,
        summary.event_queue_depth,
    );
    write_metric(
        output,
        "gateway_gate_queue_depth",
        "Current gate queue depth (CloudPlus outbound)",
        MetricType::Gauge,
        site,
        summary.gate_queue_depth,
    );
    write_metric(
        output,
        "gateway_cloudplus_queue_depth",
        "Current CloudPlus outbound queue depth",
        MetricType::Gauge,
        site,
        summary.cloudplus_queue_depth,
    );

    write_metric(
        output,
        "gateway_event_queue_utilization_pct",
        "Event queue utilization percentage (0-100)",
        MetricType::Gauge,
        site,
        summary.event_queue_utilization_pct,
    );
    write_metric(
        output,
        "gateway_gate_queue_utilization_pct",
        "Gate queue utilization percentage (0-100)",
        MetricType::Gauge,
        site,
        summary.gate_queue_utilization_pct,
    );

    write_histogram(
        output,
        "gateway_gate_send_latency_us",
        "Gate command network send latency in microseconds",
        site,
        &summary.gate_send_latency_buckets,
        &METRICS_BUCKET_BOUNDS,
        summary.gate_send_latency_avg_us,
    );
    write_metric(
        output,
        "gateway_gate_send_latency_p99_us",
        "99th percentile gate send latency",
        MetricType::Gauge,
        site,
        summary.gate_send_latency_p99_us,
    );

    write_histogram(
        output,
        "gateway_gate_enqueue_to_send_us",
        "Gate command enqueue to send time in microseconds",
        site,
        &summary.gate_enqueue_to_send_buckets,
        &METRICS_BUCKET_BOUNDS,
        summary.gate_enqueue_to_send_avg_us,
    );
    write_metric(
        output,
        "gateway_gate_enqueue_to_send_p99_us",
        "99th percentile gate enqueue to send time",
        MetricType::Gauge,
        site,
        summary.gate_enqueue_to_send_p99_us,
    );
}

/// Handle HTTP requests
async fn handle_request<G: GateCommand>(
    req: Request<hyper::body::Incoming>,
    metrics: Arc<Metrics>,
    site_id: Arc<String>,
    gate: Option<Arc<G>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/metrics") => {
            // TODO: Get actual track counts from tracker
            // For now, pass 0s - the histogram data is the important part
            let body = format_prometheus_metrics(&metrics, 0, 0, &site_id);
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
                .body(Full::new(Bytes::from(body)))
                .expect("static response should not fail"))
        }
        (&Method::GET, "/health") => Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::from("ok")))
            .expect("static response should not fail")),
        // Manual gate open endpoint - POST /gate/open
        (&Method::POST, "/gate/open") => {
            if let Some(gate) = gate {
                let latency_us = gate.send_open_command(TrackId(0)).await;
                info!(latency_us = %latency_us, "manual_gate_open");
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(Full::new(Bytes::from(format!(
                        r#"{{"ok":true,"latency_us":{}}}"#,
                        latency_us
                    ))))
                    .expect("static response should not fail"))
            } else {
                Ok(Response::builder()
                    .status(StatusCode::SERVICE_UNAVAILABLE)
                    .header("Content-Type", "application/json")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(Full::new(Bytes::from(r#"{"ok":false,"error":"gate_not_configured"}"#)))
                    .expect("static response should not fail"))
            }
        }
        // CORS preflight for gate/open
        (&Method::OPTIONS, "/gate/open") => Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "POST, OPTIONS")
            .header("Access-Control-Allow-Headers", "Content-Type")
            .body(Full::new(Bytes::from("")))
            .expect("static response should not fail")),
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("Not Found")))
            .expect("static response should not fail")),
    }
}

/// Start the Prometheus metrics HTTP server
pub async fn start_metrics_server<G: GateCommand + 'static>(
    port: u16,
    metrics: Arc<Metrics>,
    site_id: String,
    gate: Option<Arc<G>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    let site_id = Arc::new(site_id);

    info!(port = %port, site = %site_id, "prometheus_metrics_server_started");

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let io = TokioIo::new(stream);
                        let metrics = metrics.clone();
                        let site_id = site_id.clone();
                        let gate = gate.clone();

                        tokio::spawn(async move {
                            let service = service_fn(move |req| {
                                let metrics = metrics.clone();
                                let site_id = site_id.clone();
                                let gate = gate.clone();
                                async move { handle_request(req, metrics, site_id, gate).await }
                            });

                            if let Err(e) = http1::Builder::new()
                                .serve_connection(io, service)
                                .await
                            {
                                error!(error = %e, "prometheus_http_error");
                            }
                        });
                    }
                    Err(e) => {
                        error!(error = %e, "prometheus_accept_error");
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("prometheus_metrics_server_shutdown");
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_prometheus_metrics() {
        let metrics = Metrics::new();

        // Record some events
        metrics.record_event_processed(150);
        metrics.record_event_processed(250);
        metrics.record_gate_latency(100);
        metrics.record_gate_command();

        let output = format_prometheus_metrics(&metrics, 5, 2, "netto");

        assert!(output.contains("gateway_events_total{site=\"netto\"}"));
        assert!(output.contains("gateway_event_latency_us_bucket{site=\"netto\""));
        assert!(output.contains("gateway_gate_commands_total{site=\"netto\"}"));
        assert!(output.contains("gateway_active_tracks{site=\"netto\"} 5"));
        assert!(output.contains("gateway_authorized_tracks{site=\"netto\"} 2"));
    }
}
