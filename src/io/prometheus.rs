//! Prometheus metrics HTTP endpoint
//!
//! Exposes gateway metrics in Prometheus text format at /metrics.
//! Uses hyper for the HTTP server.

use crate::domain::types::TrackId;
use crate::infra::metrics::{
    Metrics, METRICS_BUCKET_BOUNDS, METRICS_NUM_BUCKETS, METRICS_STITCH_DIST_BOUNDS,
};
use crate::services::gate::GateCommand;
use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{error, info};

/// Format metrics in Prometheus text exposition format
fn format_prometheus_metrics(
    metrics: &Metrics,
    active_tracks: usize,
    authorized_tracks: usize,
    site_id: &str,
) -> String {
    let summary = metrics.report(active_tracks, authorized_tracks);
    let mut output = String::with_capacity(8192);

    // Event processing metrics
    output.push_str("# HELP gateway_events_total Total events processed\n");
    output.push_str("# TYPE gateway_events_total counter\n");
    output.push_str(&format!(
        "gateway_events_total{{site=\"{}\"}} {}\n",
        site_id, summary.events_total
    ));

    output.push_str("# HELP gateway_events_per_sec Events processed per second\n");
    output.push_str("# TYPE gateway_events_per_sec gauge\n");
    output.push_str(&format!(
        "gateway_events_per_sec{{site=\"{}\"}} {:.2}\n",
        site_id, summary.events_per_sec
    ));

    // Event latency histogram
    output.push_str("# HELP gateway_event_latency_us Event processing latency in microseconds\n");
    output.push_str("# TYPE gateway_event_latency_us histogram\n");

    let mut cumulative = 0u64;
    for (i, &bound) in METRICS_BUCKET_BOUNDS.iter().enumerate() {
        cumulative += summary.lat_buckets[i];
        output.push_str(&format!(
            "gateway_event_latency_us_bucket{{site=\"{}\",le=\"{}\"}} {}\n",
            site_id, bound, cumulative
        ));
    }
    // Add overflow bucket and +Inf
    cumulative += summary.lat_buckets[METRICS_NUM_BUCKETS - 1];
    output.push_str(&format!(
        "gateway_event_latency_us_bucket{{site=\"{}\",le=\"+Inf\"}} {}\n",
        site_id, cumulative
    ));

    // Sum and count (approximate sum from avg * count)
    let count = summary.lat_buckets.iter().sum::<u64>();
    let sum = summary.avg_process_latency_us * count;
    output.push_str(&format!("gateway_event_latency_us_sum{{site=\"{}\"}} {}\n", site_id, sum));
    output.push_str(&format!("gateway_event_latency_us_count{{site=\"{}\"}} {}\n", site_id, count));

    // Percentiles as gauges (easier to graph)
    output.push_str("# HELP gateway_event_latency_p50_us 50th percentile event latency\n");
    output.push_str("# TYPE gateway_event_latency_p50_us gauge\n");
    output.push_str(&format!(
        "gateway_event_latency_p50_us{{site=\"{}\"}} {}\n",
        site_id, summary.lat_p50_us
    ));

    output.push_str("# HELP gateway_event_latency_p95_us 95th percentile event latency\n");
    output.push_str("# TYPE gateway_event_latency_p95_us gauge\n");
    output.push_str(&format!(
        "gateway_event_latency_p95_us{{site=\"{}\"}} {}\n",
        site_id, summary.lat_p95_us
    ));

    output.push_str("# HELP gateway_event_latency_p99_us 99th percentile event latency\n");
    output.push_str("# TYPE gateway_event_latency_p99_us gauge\n");
    output.push_str(&format!(
        "gateway_event_latency_p99_us{{site=\"{}\"}} {}\n",
        site_id, summary.lat_p99_us
    ));

    // Gate command metrics
    output.push_str("# HELP gateway_gate_commands_total Total gate commands sent\n");
    output.push_str("# TYPE gateway_gate_commands_total counter\n");
    output.push_str(&format!(
        "gateway_gate_commands_total{{site=\"{}\"}} {}\n",
        site_id, summary.gate_commands_sent
    ));

    // Gate latency histogram
    output.push_str("# HELP gateway_gate_latency_us Gate command E2E latency in microseconds\n");
    output.push_str("# TYPE gateway_gate_latency_us histogram\n");

    let mut gate_cumulative = 0u64;
    for (i, &bound) in METRICS_BUCKET_BOUNDS.iter().enumerate() {
        gate_cumulative += summary.gate_lat_buckets[i];
        output.push_str(&format!(
            "gateway_gate_latency_us_bucket{{site=\"{}\",le=\"{}\"}} {}\n",
            site_id, bound, gate_cumulative
        ));
    }
    gate_cumulative += summary.gate_lat_buckets[METRICS_NUM_BUCKETS - 1];
    output.push_str(&format!(
        "gateway_gate_latency_us_bucket{{site=\"{}\",le=\"+Inf\"}} {}\n",
        site_id, gate_cumulative
    ));

    let gate_count = summary.gate_lat_buckets.iter().sum::<u64>();
    let gate_sum = summary.gate_lat_avg_us * gate_count;
    output.push_str(&format!("gateway_gate_latency_us_sum{{site=\"{}\"}} {}\n", site_id, gate_sum));
    output.push_str(&format!(
        "gateway_gate_latency_us_count{{site=\"{}\"}} {}\n",
        site_id, gate_count
    ));

    output.push_str("# HELP gateway_gate_latency_p99_us 99th percentile gate command latency\n");
    output.push_str("# TYPE gateway_gate_latency_p99_us gauge\n");
    output.push_str(&format!(
        "gateway_gate_latency_p99_us{{site=\"{}\"}} {}\n",
        site_id, summary.gate_lat_p99_us
    ));

    output.push_str("# HELP gateway_gate_latency_max_us Maximum gate command latency\n");
    output.push_str("# TYPE gateway_gate_latency_max_us gauge\n");
    output.push_str(&format!(
        "gateway_gate_latency_max_us{{site=\"{}\"}} {}\n",
        site_id, summary.gate_lat_max_us
    ));

    // Track counts
    output.push_str("# HELP gateway_active_tracks Current active tracks\n");
    output.push_str("# TYPE gateway_active_tracks gauge\n");
    output.push_str(&format!(
        "gateway_active_tracks{{site=\"{}\"}} {}\n",
        site_id, summary.active_tracks
    ));

    output.push_str("# HELP gateway_authorized_tracks Current authorized tracks\n");
    output.push_str("# TYPE gateway_authorized_tracks gauge\n");
    output.push_str(&format!(
        "gateway_authorized_tracks{{site=\"{}\"}} {}\n",
        site_id, summary.authorized_tracks
    ));

    // Gate state (0=closed, 1=moving, 2=open)
    output.push_str("# HELP gateway_gate_state Current gate state (0=closed, 1=moving, 2=open)\n");
    output.push_str("# TYPE gateway_gate_state gauge\n");
    output
        .push_str(&format!("gateway_gate_state{{site=\"{}\"}} {}\n", site_id, summary.gate_state));

    // Exits counter
    output.push_str("# HELP gateway_exits_total Total exits through gate\n");
    output.push_str("# TYPE gateway_exits_total counter\n");
    output.push_str(&format!(
        "gateway_exits_total{{site=\"{}\"}} {}\n",
        site_id, summary.exits_total
    ));

    // POS zone occupancy
    output.push_str("# HELP gateway_pos_occupancy Number of people in each POS zone\n");
    output.push_str("# TYPE gateway_pos_occupancy gauge\n");
    for (zone_id, count) in metrics.pos_occupancy() {
        output.push_str(&format!(
            "gateway_pos_occupancy{{site=\"{}\",zone_id=\"{}\"}} {}\n",
            site_id, zone_id, count
        ));
    }

    // ACC metrics
    output.push_str("# HELP gateway_acc_events_total Total ACC events received\n");
    output.push_str("# TYPE gateway_acc_events_total counter\n");
    output.push_str(&format!(
        "gateway_acc_events_total{{site=\"{}\"}} {}\n",
        site_id, summary.acc_events_total
    ));

    output.push_str("# HELP gateway_acc_matched_total ACC events matched to tracks\n");
    output.push_str("# TYPE gateway_acc_matched_total counter\n");
    output.push_str(&format!(
        "gateway_acc_matched_total{{site=\"{}\"}} {}\n",
        site_id, summary.acc_matched_total
    ));

    // Stitch metrics
    output.push_str("# HELP gateway_stitch_matched_total Tracks successfully stitched\n");
    output.push_str("# TYPE gateway_stitch_matched_total counter\n");
    output.push_str(&format!(
        "gateway_stitch_matched_total{{site=\"{}\"}} {}\n",
        site_id, summary.stitch_matched_total
    ));

    output.push_str(
        "# HELP gateway_stitch_expired_total Tracks truly lost (expired without stitch)\n",
    );
    output.push_str("# TYPE gateway_stitch_expired_total counter\n");
    output.push_str(&format!(
        "gateway_stitch_expired_total{{site=\"{}\"}} {}\n",
        site_id, summary.stitch_expired_total
    ));

    // Stitch distance histogram (cm)
    output.push_str("# HELP gateway_stitch_distance_cm Stitch distance in centimeters\n");
    output.push_str("# TYPE gateway_stitch_distance_cm histogram\n");

    let mut stitch_dist_cumulative = 0u64;
    for (i, &bound) in METRICS_STITCH_DIST_BOUNDS.iter().enumerate() {
        stitch_dist_cumulative += summary.stitch_distance_buckets[i];
        output.push_str(&format!(
            "gateway_stitch_distance_cm_bucket{{site=\"{}\",le=\"{}\"}} {}\n",
            site_id, bound, stitch_dist_cumulative
        ));
    }
    stitch_dist_cumulative += summary.stitch_distance_buckets[METRICS_NUM_BUCKETS - 1];
    output.push_str(&format!(
        "gateway_stitch_distance_cm_bucket{{site=\"{}\",le=\"+Inf\"}} {}\n",
        site_id, stitch_dist_cumulative
    ));

    let stitch_dist_count = summary.stitch_distance_buckets.iter().sum::<u64>();
    let stitch_dist_sum = summary.stitch_distance_avg_cm * stitch_dist_count;
    output.push_str(&format!(
        "gateway_stitch_distance_cm_sum{{site=\"{}\"}} {}\n",
        site_id, stitch_dist_sum
    ));
    output.push_str(&format!(
        "gateway_stitch_distance_cm_count{{site=\"{}\"}} {}\n",
        site_id, stitch_dist_count
    ));

    output.push_str("# HELP gateway_stitch_distance_avg_cm Average stitch distance\n");
    output.push_str("# TYPE gateway_stitch_distance_avg_cm gauge\n");
    output.push_str(&format!(
        "gateway_stitch_distance_avg_cm{{site=\"{}\"}} {}\n",
        site_id, summary.stitch_distance_avg_cm
    ));

    // Stitch time histogram (ms)
    output.push_str("# HELP gateway_stitch_time_ms Stitch time in milliseconds\n");
    output.push_str("# TYPE gateway_stitch_time_ms histogram\n");

    let mut stitch_time_cumulative = 0u64;
    for (i, &bound) in METRICS_BUCKET_BOUNDS.iter().enumerate() {
        stitch_time_cumulative += summary.stitch_time_buckets[i];
        output.push_str(&format!(
            "gateway_stitch_time_ms_bucket{{site=\"{}\",le=\"{}\"}} {}\n",
            site_id, bound, stitch_time_cumulative
        ));
    }
    stitch_time_cumulative += summary.stitch_time_buckets[METRICS_NUM_BUCKETS - 1];
    output.push_str(&format!(
        "gateway_stitch_time_ms_bucket{{site=\"{}\",le=\"+Inf\"}} {}\n",
        site_id, stitch_time_cumulative
    ));

    let stitch_time_count = summary.stitch_time_buckets.iter().sum::<u64>();
    let stitch_time_sum = summary.stitch_time_avg_ms * stitch_time_count;
    output.push_str(&format!(
        "gateway_stitch_time_ms_sum{{site=\"{}\"}} {}\n",
        site_id, stitch_time_sum
    ));
    output.push_str(&format!(
        "gateway_stitch_time_ms_count{{site=\"{}\"}} {}\n",
        site_id, stitch_time_count
    ));

    output.push_str("# HELP gateway_stitch_time_avg_ms Average stitch time\n");
    output.push_str("# TYPE gateway_stitch_time_avg_ms gauge\n");
    output.push_str(&format!(
        "gateway_stitch_time_avg_ms{{site=\"{}\"}} {}\n",
        site_id, summary.stitch_time_avg_ms
    ));

    // ACC late and no_journey counters
    output.push_str(
        "# HELP gateway_acc_late_total ACC events that arrived late (after gate entry)\n",
    );
    output.push_str("# TYPE gateway_acc_late_total counter\n");
    output.push_str(&format!(
        "gateway_acc_late_total{{site=\"{}\"}} {}\n",
        site_id, summary.acc_late_total
    ));

    output
        .push_str("# HELP gateway_acc_no_journey_total ACC events matched but no journey found\n");
    output.push_str("# TYPE gateway_acc_no_journey_total counter\n");
    output.push_str(&format!(
        "gateway_acc_no_journey_total{{site=\"{}\"}} {}\n",
        site_id, summary.acc_no_journey_total
    ));

    // Gate queue delay histogram (time from enqueue to worker pickup)
    output
        .push_str("# HELP gateway_gate_queue_delay_us Gate command queue delay in microseconds\n");
    output.push_str("# TYPE gateway_gate_queue_delay_us histogram\n");

    let mut gate_queue_delay_cumulative = 0u64;
    for (i, &bound) in METRICS_BUCKET_BOUNDS.iter().enumerate() {
        gate_queue_delay_cumulative += summary.gate_queue_delay_buckets[i];
        output.push_str(&format!(
            "gateway_gate_queue_delay_us_bucket{{site=\"{}\",le=\"{}\"}} {}\n",
            site_id, bound, gate_queue_delay_cumulative
        ));
    }
    gate_queue_delay_cumulative += summary.gate_queue_delay_buckets[METRICS_NUM_BUCKETS - 1];
    output.push_str(&format!(
        "gateway_gate_queue_delay_us_bucket{{site=\"{}\",le=\"+Inf\"}} {}\n",
        site_id, gate_queue_delay_cumulative
    ));

    let gate_queue_delay_count = summary.gate_queue_delay_buckets.iter().sum::<u64>();
    let gate_queue_delay_sum = summary.gate_queue_delay_avg_us * gate_queue_delay_count;
    output.push_str(&format!(
        "gateway_gate_queue_delay_us_sum{{site=\"{}\"}} {}\n",
        site_id, gate_queue_delay_sum
    ));
    output.push_str(&format!(
        "gateway_gate_queue_delay_us_count{{site=\"{}\"}} {}\n",
        site_id, gate_queue_delay_count
    ));

    output.push_str("# HELP gateway_gate_queue_delay_p99_us 99th percentile gate queue delay\n");
    output.push_str("# TYPE gateway_gate_queue_delay_p99_us gauge\n");
    output.push_str(&format!(
        "gateway_gate_queue_delay_p99_us{{site=\"{}\"}} {}\n",
        site_id, summary.gate_queue_delay_p99_us
    ));

    output.push_str("# HELP gateway_gate_queue_delay_max_us Maximum gate queue delay\n");
    output.push_str("# TYPE gateway_gate_queue_delay_max_us gauge\n");
    output.push_str(&format!(
        "gateway_gate_queue_delay_max_us{{site=\"{}\"}} {}\n",
        site_id, summary.gate_queue_delay_max_us
    ));

    // Queue depths
    output.push_str("# HELP gateway_event_queue_depth Current event queue depth\n");
    output.push_str("# TYPE gateway_event_queue_depth gauge\n");
    output.push_str(&format!(
        "gateway_event_queue_depth{{site=\"{}\"}} {}\n",
        site_id, summary.event_queue_depth
    ));

    output.push_str("# HELP gateway_gate_queue_depth Current gate command queue depth\n");
    output.push_str("# TYPE gateway_gate_queue_depth gauge\n");
    output.push_str(&format!(
        "gateway_gate_queue_depth{{site=\"{}\"}} {}\n",
        site_id, summary.gate_queue_depth
    ));

    output
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
