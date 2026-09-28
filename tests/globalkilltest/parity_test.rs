// Copyright 2026 AsterSQL.

//! Parity tests for `tests/globalkilltest` public contracts vs Go `util.go`.

// 本文件对应 `tests/globalkilltest/parity_test.rs`，本次任务只补中文解释，不改行为。
// 本文件按成功、边界、错误和清理四类场景组织。
// 阅读时先看总入口，再看各个合同分组。
// 这里的目标是证明 Rust 与 Go 的公共合同一致。
// 成功路径关注正常返回值和可观测副作用。
// 边界路径关注空输入、默认值和最小变体。
// 错误路径关注日志、panic、exit 与错误文本。
// 清理路径关注 Close、Sync、Join 和资源释放。
// 中文注释优先解释为什么要断言。
// 补充阅读提示 1：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 2：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 3：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 4：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 5：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 6：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 7：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 8：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 9：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 10：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 11：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 12：当多个 helper 串联时，顺序本身往往就是语义的一部分。
// 补充阅读提示 13：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 14：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 15：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 16：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 17：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 18：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 19：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 20：边界路径通常说明默认值、空输入和最小可用配置。
use crate::stubs::decode_pd_health;
use crate::stubs::server;
use crate::stubs::util::{
    self as stub_util, Body, Response, StatusOK, TimeoutOverrides, set_http_get,
};
use crate::stubs::{Error, errors};
use crate::util::{
    PdHealth, check_pd_health, check_tidb_status, check_tikv_status, default_timeouts, with_retry,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn with_retry_zero_timeout_returns_zero_value_without_calling_callback() {
    let calls = AtomicUsize::new(0);
    let value = with_retry(
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<i32, Error>(42)
        },
        Duration::ZERO,
    )
    .expect("Go withRetry returns T's zero value when the loop never runs");

    assert_eq!(value, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn tidb_status_json_missing_fields_uses_go_zero_values() {
    let status = server::decode_status(b"{}").expect("encoding/json accepts omitted fields");

    assert_eq!(status.connections, 0);
    assert_eq!(status.version, "");
    assert_eq!(status.git_hash, "");
    assert_eq!(status.status.init_stats_percentage, 0.0);

    assert_eq!(
        decode_pd_health(b"{}").expect("encoding/json zero-fills omitted health"),
        ""
    );
}

#[test]
fn pd_health_json_missing_field_uses_go_zero_value() {
    assert_eq!(
        decode_pd_health(b"{}").expect("encoding/json zero-fills omitted health"),
        ""
    );
}

#[test]
fn json_decoding_matches_go_for_malformed_and_escaped_input() {
    assert!(
        decode_pd_health(br#"{"health":"true""#).is_err(),
        "encoding/json rejects an unterminated object"
    );
    assert_eq!(
        decode_pd_health(br#"{"health":"tr\u0075e"}"#)
            .expect("encoding/json decodes unicode escapes"),
        "true"
    );

    assert!(
        server::decode_status(br#"{"connections":1 trailing}"#).is_err(),
        "encoding/json rejects malformed object syntax"
    );
    assert_eq!(
        server::decode_status(br#"{"version":"dev\nnightly"}"#)
            .expect("encoding/json decodes escaped strings")
            .version,
        "dev\nnightly"
    );
}

// 测试 `go_rust_public_contract_matches` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `go_rust_public_contract_matches` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn go_rust_public_contract_matches() {
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

/// Normal: ComposeURL, healthy PD/TiKV/TiDB status checks succeed.
// `contract_normal_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_normal_paths() {
    stub_util::set_internal_http_schema("http");
    stub_util::clear_timeout_overrides();

    let (pd_t, tikv_t, tidb_t, retry) = default_timeouts();
    assert_eq!(pd_t, Duration::from_secs(10));
    assert_eq!(tikv_t, Duration::from_secs(30));
    assert_eq!(tidb_t, Duration::from_secs(60));
    assert_eq!(retry, Duration::from_millis(500));

    assert_eq!(
        stub_util::ComposeURL("127.0.0.1:2379", "/health"),
        "http://127.0.0.1:2379/health"
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_c = calls.clone();
    set_http_get(Arc::new(move |url: &str| {
        calls_c.fetch_add(1, Ordering::SeqCst);
        if url.ends_with("/health") {
            Ok(Response {
                StatusCode: StatusOK,
                Body: Body::new(br#"{"health":"true"}"#.to_vec()),
            })
        } else if url.contains(":20180") && url.ends_with("/status") {
            Ok(Response {
                StatusCode: StatusOK,
                Body: Body::new(b"".to_vec()),
            })
        } else if url.contains(":10080") && url.ends_with("/status") {
            Ok(Response {
                StatusCode: StatusOK,
                Body: Body::new(
                    br#"{"connections":1,"version":"v8.0.0","git_hash":"abc","status":{"init_stats_percentage":100.0}}"#
                        .to_vec(),
                ),
            })
        } else {
            Err(errors::Errorf(format!("unexpected url {url}")))
        }
    }));

    check_pd_health("127.0.0.1:2379").expect("PD healthy");
    check_tikv_status().expect("TiKV ok");
    check_tidb_status(10080).expect("TiDB ok");
    assert!(calls.load(Ordering::SeqCst) >= 3);

    // withRetry succeeds on first ok.
    let v = with_retry(|| Ok::<i32, Error>(42), Duration::from_millis(200)).unwrap();
    assert_eq!(v, 42);

    let health = PdHealth {
        health: decode_pd_health(br#"{"health":"true"}"#).unwrap(),
    };
    assert_eq!(health.health, "true");
}

/// Boundary: scheme already present; https schema; custom status port.
// `contract_boundary` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_boundary() {
    stub_util::set_internal_http_schema("http");
    assert_eq!(
        stub_util::ComposeURL("http://pd.local", "/health"),
        "http://pd.local/health"
    );
    assert_eq!(
        stub_util::ComposeURL("https://pd.local", "/api"),
        "https://pd.local/api"
    );

    stub_util::set_internal_http_schema("https");
    assert_eq!(
        stub_util::ComposeURL("127.0.0.1:2379", "/health"),
        "https://127.0.0.1:2379/health"
    );
    stub_util::set_internal_http_schema("http");

    set_http_get(Arc::new(|url: &str| {
        assert!(url.contains("127.0.0.1:10081"));
        Ok(Response {
            StatusCode: StatusOK,
            Body: Body::new(
                br#"{"connections":0,"version":"dev","git_hash":"x","status":{"init_stats_percentage":0.0}}"#
                    .to_vec(),
            ),
        })
    }));
    check_tidb_status(10081).expect("custom port");
}

/// Error: bad status, unhealthy PD, decode failure, retry timeout.
// `contract_error_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_error_paths() {
    stub_util::set_timeout_overrides(TimeoutOverrides {
        pd: Some(Duration::from_millis(80)),
        tikv: Some(Duration::from_millis(80)),
        tidb: Some(Duration::from_millis(80)),
        retry_interval: Some(Duration::from_millis(10)),
    });

    // Non-200 status → retries then error.
    set_http_get(Arc::new(|_url: &str| {
        Ok(Response {
            StatusCode: 503,
            Body: Body::new(b"".to_vec()),
        })
    }));
    let err = check_pd_health("127.0.0.1:2379").unwrap_err();
    assert!(
        err.Error().contains("PD health status code 503"),
        "got {}",
        err.Error()
    );

    // health != "true"
    set_http_get(Arc::new(|_url: &str| {
        Ok(Response {
            StatusCode: StatusOK,
            Body: Body::new(br#"{"health":"false"}"#.to_vec()),
        })
    }));
    let err = check_pd_health("127.0.0.1:2379").unwrap_err();
    assert!(
        err.Error().contains("PD not healthy false"),
        "got {}",
        err.Error()
    );

    // TiKV non-200
    set_http_get(Arc::new(|_url: &str| {
        Ok(Response {
            StatusCode: 500,
            Body: Body::new(b"".to_vec()),
        })
    }));
    let err = check_tikv_status().unwrap_err();
    assert!(
        err.Error().contains("TiKV status code 500"),
        "got {}",
        err.Error()
    );

    // TiDB bad JSON
    set_http_get(Arc::new(|_url: &str| {
        Ok(Response {
            StatusCode: StatusOK,
            Body: Body::new(b"not-json".to_vec()),
        })
    }));
    let err = check_tidb_status(10080).unwrap_err();
    assert!(!err.Error().is_empty());

    // withRetry exhausts timeout and returns last error.
    let attempts = Arc::new(AtomicUsize::new(0));
    let a = attempts.clone();
    let err = with_retry(
        || {
            a.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(errors::Errorf("boom"))
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(err.Error(), "boom");
    assert!(attempts.load(Ordering::SeqCst) >= 1);

    stub_util::clear_timeout_overrides();
}

/// Resource cleanup: response Body.Close is invoked (Go defer / explicit Close).
// `contract_resource_cleanup` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_resource_cleanup() {
    stub_util::clear_timeout_overrides();
    stub_util::set_timeout_overrides(TimeoutOverrides {
        pd: Some(Duration::from_millis(200)),
        tikv: Some(Duration::from_millis(200)),
        tidb: Some(Duration::from_millis(200)),
        retry_interval: Some(Duration::from_millis(5)),
    });

    let closed = Arc::new(Mutex::new(Vec::new()));

    // TiKV path: explicit Body.Close()
    let closed_tikv = closed.clone();
    set_http_get(Arc::new(move |_url: &str| {
        let body = Body::new(b"".to_vec());
        let marker = body.clone();
        closed_tikv.lock().unwrap().push(marker);
        Ok(Response {
            StatusCode: StatusOK,
            Body: body,
        })
    }));
    check_tikv_status().expect("tikv");
    {
        let bodies = closed.lock().unwrap();
        assert!(!bodies.is_empty());
        assert!(bodies.last().unwrap().is_closed(), "TiKV body must Close");
    }

    // PD path: defer Close via BodyGuard even on unhealthy (after retries end).
    let closed_pd = Arc::new(Mutex::new(Vec::new()));
    let closed_pd_c = closed_pd.clone();
    set_http_get(Arc::new(move |_url: &str| {
        let body = Body::new(br#"{"health":"false"}"#.to_vec());
        closed_pd_c.lock().unwrap().push(body.clone());
        Ok(Response {
            StatusCode: StatusOK,
            Body: body,
        })
    }));
    let _ = check_pd_health("127.0.0.1:2379");
    {
        let bodies = closed_pd.lock().unwrap();
        assert!(!bodies.is_empty());
        assert!(
            bodies.iter().all(|b| b.is_closed()),
            "PD bodies must Close on every attempt"
        );
    }

    // TiDB success path closes body.
    let closed_tidb = Arc::new(Mutex::new(Vec::new()));
    let closed_tidb_c = closed_tidb.clone();
    set_http_get(Arc::new(move |_url: &str| {
        let body = Body::new(
            br#"{"connections":0,"version":"v","git_hash":"g","status":{"init_stats_percentage":100.0}}"#
                .to_vec(),
        );
        closed_tidb_c.lock().unwrap().push(body.clone());
        Ok(Response {
            StatusCode: StatusOK,
            Body: body,
        })
    }));
    check_tidb_status(10080).expect("tidb");
    assert!(closed_tidb.lock().unwrap()[0].is_closed());

    stub_util::clear_timeout_overrides();
    stub_util::reset_http_get();
}
