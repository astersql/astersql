// Copyright 2026 AsterSQL.
use crate::paging_resource_control::{PagingRUInterceptor, PagingTokenConfig};
use crate::resourcegroup::{CopRPCRequestInfo, CopRPCResponseInfo, CopRUInterceptor};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn config(tokens: f64) -> PagingTokenConfig {
    PagingTokenConfig {
        fill_rate: 0.0,
        burst: 0,
        tokens,
        max_wait: Duration::from_millis(100),
        retry_times: 1,
        retry_interval: Duration::from_millis(10),
    }
}
fn request(bytes: u64) -> CopRPCRequestInfo {
    CopRPCRequestInfo {
        resource_group_name: "bounded".into(),
        request_type: "Dag".into(),
        predicted_read_bytes: bytes,
        ..Default::default()
    }
}
#[test]
fn go_commit_ab7d93b603_precharge_refunds_and_debits_settlement_immediately() {
    let controller = PagingRUInterceptor::new("bounded", config(100.0));
    let req = request(4 * 65536);
    let delta = controller.OnRequestWait(&req).unwrap();
    assert!((delta.read_ru - 4.475).abs() < 1e-9);
    assert!((controller.available_tokens() - 95.525).abs() < 1e-9);
    let refund = controller
        .OnResponseWait(
            &req,
            &CopRPCResponseInfo {
                read_bytes: 65536,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(refund.read_ru, -3.0);
    assert!((controller.available_tokens() - 98.525).abs() < 1e-9);
    // Settlement may create debt even when no refill can pay it: completed work must not wait again.
    let delta = controller
        .OnResponseWait(
            &req,
            &CopRPCResponseInfo {
                read_bytes: 200 * 65536,
                kv_cpu_ms: 3.0,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(delta.read_ru, 197.0);
    assert!((controller.available_tokens() + 98.475).abs() < 1e-9);
}
#[test]
fn go_commit_ab7d93b603_precharge_throttled_reservation_keeps_tokens() {
    let controller = PagingRUInterceptor::new("bounded", config(0.25));
    assert!(
        controller
            .OnRequestWait(&request(65536))
            .unwrap_err()
            .contains("throttled")
    );
    assert_eq!(controller.available_tokens(), 0.25);
    assert!(
        controller
            .OnRequestWait(&CopRPCRequestInfo {
                resource_group_name: "missing".into(),
                ..Default::default()
            })
            .unwrap_err()
            .contains("not configured")
    );
}
#[test]
fn go_commit_ab7d93b603_precharge_cancelled_wait_refunds_reservation() {
    let mut cfg = config(0.0);
    cfg.fill_rate = 1.0;
    cfg.max_wait = Duration::from_secs(3);
    let controller = Arc::new(PagingRUInterceptor::new("bounded", cfg));
    let cancelled = Arc::new(AtomicBool::new(false));
    let c = controller.clone();
    let flag = cancelled.clone();
    let waiter =
        std::thread::spawn(move || c.OnRequestWaitCancellable(&request(65536), Some(&flag)));
    let deadline = Instant::now() + Duration::from_secs(2);
    while controller.available_tokens() >= 0.0 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    cancelled.store(true, Ordering::Release);
    assert_eq!(
        waiter.join().unwrap().unwrap_err(),
        "resource control cancelled"
    );
    assert!(controller.available_tokens() >= 0.0);
}
#[test]
fn go_commit_ab7d93b603_precharge_grant_wakes_failed_reservation_retry() {
    let mut cfg = config(0.0);
    cfg.retry_times = 3;
    cfg.retry_interval = Duration::from_secs(1);
    let controller = Arc::new(PagingRUInterceptor::new("bounded", cfg.clone()));
    let c = controller.clone();
    let started = Instant::now();
    let waiter = std::thread::spawn(move || c.OnRequestWait(&request(65536)));
    std::thread::sleep(Duration::from_millis(20));
    cfg.tokens = 10.0;
    controller.reconfigure(cfg);
    assert!((waiter.join().unwrap().unwrap().read_ru - 1.475).abs() < 1e-9);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!((controller.available_tokens() - 8.525).abs() < 1e-9);
}
#[test]
fn go_commit_ab7d93b603_no_hint_settlement_preserves_allow_debt_threshold() {
    let controller = PagingRUInterceptor::new("bounded", config(0.0));
    controller.set_throttled(true);
    let req = request(0);
    controller
        .OnResponseWait(
            &req,
            &CopRPCResponseInfo {
                read_bytes: 4 * 1024 * 1024 - 1,
                ..Default::default()
            },
        )
        .unwrap();
    let debt = controller.available_tokens();
    assert!(debt < 0.0);
    assert!(
        controller
            .OnResponseWait(
                &req,
                &CopRPCResponseInfo {
                    read_bytes: 4 * 1024 * 1024,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .contains("throttled")
    );
    assert_eq!(controller.available_tokens(), debt);
    controller.set_throttled(false);
    controller
        .OnResponseWait(
            &req,
            &CopRPCResponseInfo {
                read_bytes: 4 * 1024 * 1024,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(controller.available_tokens(), debt - 64.0);
}

#[test]
fn go_commit_ab7d93b603_precharge_unlimited_and_burst_cap() {
    let mut cfg = config(0.0);
    cfg.burst = -1;
    let unlimited = PagingRUInterceptor::new("bounded", cfg);
    unlimited.OnRequestWait(&request(4 * 1024 * 1024)).unwrap();
    assert_eq!(unlimited.available_tokens(), 0.0);
    let mut cfg = config(100.0);
    cfg.burst = 10;
    let capped = PagingRUInterceptor::new("bounded", cfg);
    assert_eq!(capped.available_tokens(), 10.0);
    capped.OnRequestWait(&request(65536)).unwrap();
    assert!((capped.available_tokens() - 8.525).abs() < 1e-9);
    capped
        .OnResponseWait(&request(10 * 65536), &Default::default())
        .unwrap();
    assert_eq!(capped.available_tokens(), 10.0);
}

#[test]
fn go_commit_ab7d93b603_paging_metrics_export_exact_dashboard_series() {
    let controller = PagingRUInterceptor::new("go_commit_ab7d93b603_metrics", config(100.0));
    let mut req = request(65536);
    req.resource_group_name = "go_commit_ab7d93b603_metrics".into();
    controller.OnRequestWait(&req).unwrap();
    controller
        .OnResponseWait(
            &req,
            &CopRPCResponseInfo {
                read_bytes: 32768,
                ..Default::default()
            },
        )
        .unwrap();
    req.predicted_read_bytes = 0;
    controller.OnRequestWait(&req).unwrap();
    let metrics = prometheus::gather();
    let series = |name: &str| {
        metrics
            .iter()
            .find(|metric| metric.name() == name)
            .unwrap()
            .get_metric()
            .iter()
            .find(|metric| {
                metric.get_label().iter().any(|label| {
                    label.name() == "resource_group"
                        && label.value() == "go_commit_ab7d93b603_metrics"
                })
            })
            .unwrap()
    };
    assert_eq!(
        series("resource_manager_client_request_cop_read_precharge_total")
            .get_counter()
            .value(),
        1.0
    );
    assert_eq!(
        series("resource_manager_client_request_cop_read_no_precharge_total")
            .get_counter()
            .value(),
        1.0
    );
    assert_eq!(
        series("resource_manager_client_request_paging_precharge_bytes_total")
            .get_counter()
            .value(),
        65536.0
    );
    assert_eq!(
        series("resource_manager_client_request_paging_actual_bytes_total")
            .get_counter()
            .value(),
        32768.0
    );
    let residual =
        series("resource_manager_client_request_paging_prediction_residual_bytes").get_histogram();
    assert_eq!(residual.sample_count(), 1);
    assert_eq!(residual.sample_sum(), -32768.0);
    assert!(
        residual
            .get_bucket()
            .iter()
            .any(|bucket| bucket.upper_bound() == -65536.0)
    );
    assert!(
        residual
            .get_bucket()
            .iter()
            .any(|bucket| bucket.upper_bound() == 65536.0)
    );
}
