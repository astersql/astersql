// Copyright 2026 AsterSQL.

use crate::pprof::{RegisterDefaultStatusHandlers, StatusRequest};
use prost::Message;

fn get(target: &str) -> StatusRequest {
    StatusRequest {
        method: "GET".to_owned(),
        target: target.to_owned(),
        body: Vec::new(),
    }
}

#[test]
fn default_metrics_handler_exports_registered_metrics() {
    let metric = prometheus::IntCounter::new(
        "astersql_br_pprof_parity_total",
        "pprof parity regression counter",
    )
    .unwrap();
    prometheus::default_registry()
        .register(Box::new(metric.clone()))
        .unwrap();
    metric.inc();

    let response =
        RegisterDefaultStatusHandlers()(&get("/metrics")).expect("the Go mux registers /metrics");
    let body = String::from_utf8(response.body).unwrap();
    assert!(body.contains("# HELP astersql_br_pprof_parity_total"));
    assert!(body.contains("astersql_br_pprof_parity_total 1"));

    prometheus::default_registry()
        .unregister(Box::new(metric))
        .unwrap();
}

#[test]
fn default_cmdline_handler_reports_the_real_process_command_line() {
    let response = RegisterDefaultStatusHandlers()(&get("/debug/pprof/cmdline"))
        .expect("the Go mux registers /debug/pprof/cmdline");
    let expected = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\0");
    assert_eq!(response.body, expected.into_bytes());
}

#[test]
fn unknown_routes_are_left_unmatched_for_the_http_server_to_return_404() {
    assert!(RegisterDefaultStatusHandlers()(&get("/not-registered")).is_none());
}

#[test]
fn profile_handler_returns_decodable_pprof_data() {
    let response = RegisterDefaultStatusHandlers()(&get("/debug/pprof/profile?seconds=1"))
        .expect("the Go mux registers /debug/pprof/profile");
    assert_eq!(response.status, "200 OK");
    assert_eq!(response.content_type, "application/octet-stream");
    let profile = pprof::protos::Profile::decode(response.body.as_slice()).unwrap();
    assert!(!profile.sample_type.is_empty());
}

#[test]
fn default_handlers_preserve_http_error_semantics() {
    let mut post_cmdline = get("/debug/pprof/cmdline");
    post_cmdline.method = "POST".to_owned();
    assert_eq!(
        RegisterDefaultStatusHandlers()(&post_cmdline)
            .unwrap()
            .status,
        "405 Method Not Allowed"
    );
    assert_eq!(
        RegisterDefaultStatusHandlers()(&get("/debug/pprof/trace?seconds=1"))
            .unwrap()
            .status,
        "501 Not Implemented"
    );
}
