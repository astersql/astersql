// Copyright 2026 AsterSQL.

use crate::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

fn get(addr: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("status service must accept connections");
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn status_service_binds_and_exposes_go_routes() {
    let tctx = tcontext::Background();
    let handle = startDumplingService(&tctx, "127.0.0.1:0").unwrap();

    for path in [
        "/metrics",
        "/debug/pprof/",
        "/debug/pprof/cmdline",
        "/debug/pprof/profile",
        "/debug/pprof/symbol",
        "/debug/pprof/trace",
    ] {
        let response = get(&handle.addr, path);
        assert!(response.starts_with("HTTP/1.1 200"), "{path}: {response}");
    }

    handle.stop();
}

#[test]
fn status_service_reports_listen_failure_with_context() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = occupied.local_addr().unwrap().to_string();
    let err = startDumplingService(&tcontext::Background(), &addr).unwrap_err();
    assert!(err.msg.contains("start listening"), "{}", err.msg);
}

#[test]
fn net_closing_detection_matches_go_nil_and_substring_cases() {
    assert!(!isErrNetClosing_pub(None));
    assert!(isErrNetClosing_pub(Some(&errors_new(format!(
        "wrapped: {useOfClosedErrMsg}"
    )))));
    assert!(!isErrNetClosing_pub(Some(&errors_new("other error"))));
}

#[test]
fn status_service_exposes_cached_status() {
    let handle = startDumplingService(&tcontext::Background(), "127.0.0.1:0").unwrap();
    let response = get(&handle.addr, "/status");
    handle.stop();
    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
}

fn http_dumper() -> Dumper {
    let conf = crate::main_test::default_config_for_test();
    Dumper {
        tctx: tcontext::Background(),
        metrics: Arc::new(newMetrics(conf.PromFactory.as_ref(), &conf.Labels)),
        conf: Arc::new(conf),
        db: None,
        ext_storage: None,
        speedRecorder: Arc::new(Mutex::new(NewSpeedRecorder())),
        status: Arc::new(Mutex::new(DumpStatus::default())),
        totalTables: Arc::new(AtomicI64::new(0)),
        cancel: None,
        http: None,
        pd_client: None,
    }
}

#[test]
fn http_status_reports_progress_and_polling_preserves_speed() {
    let d = http_dumper();
    let handle = startDumplingServiceWithDumper(&d.tctx, "127.0.0.1:0", Some(&d)).unwrap();
    let empty = get(&handle.addr, "/status");
    assert!(empty.starts_with("HTTP/1.1 200"));
    assert!(!empty.contains("progress"));
    AddCounter(Some(&d.metrics.finishedTablesCounter), 3.0);
    AddGauge(Some(&d.metrics.finishedSizeGauge), 4096.0);
    AddGauge(Some(&d.metrics.finishedRowsGauge), 250.0);
    AddCounter(Some(&d.metrics.estimateTotalRowsCounter), 1000.0);
    d.metrics.totalChunks.store(8, Ordering::SeqCst);
    d.metrics.completedChunks.store(2, Ordering::SeqCst);
    d.metrics.progressReady.store(true, Ordering::SeqCst);
    d.RefreshStatus();
    let first = get(&handle.addr, "/status");
    assert!(first.contains("Content-Type: application/json"));
    for field in [
        "\"completedTables\":3",
        "\"finishedBytes\":4096",
        "\"finishedRows\":250",
        "\"estimateTotalRows\":1000",
        "\"progressPercent\":25",
    ] {
        assert!(first.contains(field), "{first}");
    }
    let checkpoint = d.speedRecorder.lock().unwrap().last_update_time;
    AddGauge(Some(&d.metrics.finishedSizeGauge), 4096.0);
    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| assert_eq!(get(&handle.addr, "/status"), first));
        }
    });
    assert_eq!(checkpoint, d.speedRecorder.lock().unwrap().last_update_time);
    handle.stop();
}

#[test]
fn http_metrics_serve_live_configured_registry_and_preserve_custom_families() {
    let mut d = http_dumper();
    let registry = Arc::new(DefaultRegistry::default());
    let mut conf = d.conf.clone_for_mutate();
    conf.PromRegistry = registry.clone();
    d.conf = Arc::new(conf);
    d.metrics.registerTo(registry.as_ref());
    registry.RegisterSample(
        "go_goroutines",
        Arc::new(|| "# TYPE go_goroutines gauge\ngo_goroutines 123\n".to_owned()),
    );
    registry.RegisterSample("custom_histogram", Arc::new(|| "# TYPE custom_histogram histogram\ncustom_histogram_bucket{le=\"1\"} 2\ncustom_histogram_sum 1.5\ncustom_histogram_count 2\n".to_owned()));
    AddGauge(Some(&d.metrics.finishedRowsGauge), 42.0);
    let handle = startDumplingServiceWithDumper(&d.tctx, "127.0.0.1:0", Some(&d)).unwrap();
    let body = get(&handle.addr, "/metrics");
    assert!(body.contains("dumpling_dump_finished_rows 42\n"));
    assert!(body.contains("go_goroutines 123\n"));
    assert!(body.contains("custom_histogram_count 2\n"));
    assert!(!body.contains("process_cpu_seconds_total"));
    assert!(!body.contains("promhttp_metric_handler_requests_total"));
    AddGauge(Some(&d.metrics.finishedRowsGauge), 1.0);
    assert!(get(&handle.addr, "/metrics").contains("dumpling_dump_finished_rows 43\n"));
    handle.stop();
    d.metrics.unregisterFrom(registry.as_ref());
    assert!(
        !registry
            .Gather()
            .unwrap()
            .contains("dumpling_dump_finished_rows")
    );
}

#[test]
fn metrics_register_only_falls_back_and_shared_default_is_preserved() {
    struct RegisterOnly;
    impl Registry for RegisterOnly {
        fn MustRegister(&self, _: &str) {}
        fn Unregister(&self, _: &str) -> bool {
            true
        }
    }
    let default = DefaultGatherer();
    default.RegisterSample(
        "task36_default",
        Arc::new(|| "task36_default 42\n".to_owned()),
    );
    assert!(metricsHandler(&RegisterOnly).contains("task36_default 42\n"));
    assert_eq!(metricsHandler(default.as_ref()), default.Gather().unwrap());
    assert!(!metricsHandler(&DefaultRegistry::default()).contains("task36_default"));
    default.Unregister("task36_default");
}
