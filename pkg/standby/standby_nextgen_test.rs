// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Corresponds to Go `//go:build nextgen` standby tests. Uses controller
// `set_starter_mode(true)` so starter paths are exercised on classic builds too.
//
// 对应 Go nextgen 构建标签下的 standby 测试。通过 `set_starter_mode(true)`
// 在经典构建上也能覆盖 starter：Manager free、export_id、优雅/强制 exit 与 wait 解析。

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use astersql_config as config;
use astersql_server::http_status::{Method, Request};
use astersql_server::standby::{StandbyController, StandbyShutdownServer};

use crate::{
    ActivateRequest, DEFAULT_CLOSE_CONNECTION_WAIT, ExitSignal, ExitSignaler,
    LoadKeyspaceController, MANAGER_FREE_MAX_ATTEMPTS, ManagerClient, RecordingExitSignaler, State,
    parse_exit_wait,
};
#[derive(Default)]
/// Manager 替身：可注入失败次数并记录 free 原因。
struct MockManager {
    called: AtomicBool,
    calls: Mutex<usize>,
    failures: usize,
    got_reason: Mutex<String>,
}

/// 前 failures 次返回临时错误。
impl ManagerClient for MockManager {
    fn free(&self, exit_reason: &str) -> Result<(), String> {
        self.called.store(true, Ordering::Release);
        let mut calls = self.calls.lock().expect("calls");
        *calls += 1;
        *self.got_reason.lock().expect("reason") = exit_reason.to_string();
        if *calls <= self.failures {
            return Err("temporary error".into());
        }
        Ok(())
    }
}

/// 可阻塞的关闭 Server：用于测试等待连接清零的并发路径。
struct BlockingShutdownServer {
    /// 是否强制关闭。
    force_shutdown: AtomicBool,
    /// 是否需要在清零后请求 Manager free。
    need_request_mgr_free: AtomicBool,
    /// wait_zero_connections 已进入的同步点。
    wait_started: Arc<(Mutex<bool>, Condvar)>,
    /// 测试侧通知“连接已清零”。
    wait_done: Arc<(Mutex<bool>, Condvar)>,
}

impl BlockingShutdownServer {
    /// 默认需要 Manager free，且等待未完成。
    fn new() -> Self {
        Self {
            force_shutdown: AtomicBool::new(false),
            need_request_mgr_free: AtomicBool::new(true),
            wait_started: Arc::new((Mutex::new(false), Condvar::new())),
            wait_done: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    /// 等待对端进入 wait_zero_connections。
    fn wait_until_started(&self, timeout: Duration) -> bool {
        let (lock, cv) = &*self.wait_started;
        let guard = lock.lock().expect("wait started");
        let (guard, _) = cv
            .wait_timeout_while(guard, timeout, |started| !*started)
            .expect("wait");
        *guard
    }

    /// 标记连接已清零并唤醒等待方。
    fn signal_done(&self) {
        let (lock, cv) = &*self.wait_done;
        *lock.lock().expect("wait done") = true;
        cv.notify_all();
    }
}

/// wait_zero_connections_timeout 先发 wait_started，再等 wait_done。
impl StandbyShutdownServer for BlockingShutdownServer {
    fn auto_id_service_close(&self) {}
    fn force_shutdown(&self) -> bool {
        self.force_shutdown.load(Ordering::Acquire)
    }
    fn need_request_manager_free(&self) -> bool {
        self.need_request_mgr_free.load(Ordering::Acquire)
    }
    fn is_auto_id_owner(&self) -> bool {
        false
    }
    fn set_force_shutdown(&self) {
        self.force_shutdown.store(true, Ordering::Release);
    }
    fn set_need_request_manager_free(&self) {
        self.need_request_mgr_free.store(true, Ordering::Release);
    }
    fn wait_zero_connections(&self) {
        let _ = self.wait_zero_connections_timeout(Duration::from_secs(3600));
    }
    fn wait_zero_connections_timeout(&self, timeout: Duration) -> bool {
        {
            let (lock, cv) = &*self.wait_started;
            *lock.lock().expect("wait started") = true;
            cv.notify_all();
        }
        let (lock, cv) = &*self.wait_done;
        let guard = lock.lock().expect("wait done");
        let (guard, result) = cv
            .wait_timeout_while(guard, timeout, |done| !*done)
            .expect("wait done");
        *guard && !result.timed_out()
    }
}

#[derive(Default)]
/// 立即成功的关闭 Server 替身。
struct SimpleShutdownServer {
    need_request_mgr_free: AtomicBool,
    force_shutdown: AtomicBool,
}

/// wait_zero_connections* 立即返回成功。
impl StandbyShutdownServer for SimpleShutdownServer {
    fn auto_id_service_close(&self) {}
    fn force_shutdown(&self) -> bool {
        self.force_shutdown.load(Ordering::Acquire)
    }
    fn need_request_manager_free(&self) -> bool {
        self.need_request_mgr_free.load(Ordering::Acquire)
    }
    fn is_auto_id_owner(&self) -> bool {
        false
    }
    fn set_force_shutdown(&self) {
        self.force_shutdown.store(true, Ordering::Release);
    }
    fn set_need_request_manager_free(&self) {
        self.need_request_mgr_free.store(true, Ordering::Release);
    }
    fn wait_zero_connections(&self) {}
    fn wait_zero_connections_timeout(&self, _timeout: Duration) -> bool {
        true
    }
}

/// 构造带 query 的 HTTP 请求。
fn http_request(method: Method, path: &str, query: HashMap<String, String>, body: &str) -> Request {
    Request {
        method,
        path: path.into(),
        query,
        raw_query: String::new(),
        headers: HashMap::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// JSON 语义相等断言。
fn assert_json_eq(actual: &[u8], expected: &str) {
    let actual_value: serde_json::Value =
        serde_json::from_slice(actual).expect("response body must be json");
    let expected_value: serde_json::Value =
        serde_json::from_str(expected).expect("expected json must parse");
    assert_eq!(actual_value, expected_value);
}

/// 将响应体解码为 UTF-8 字符串。
fn body_string(resp: &astersql_server::http_status::Response) -> String {
    String::from_utf8_lossy(&resp.body).into_owned()
}

#[test]
/// starter 关闭：从重启日志读原因并调用 Manager.free；覆盖等待成功与超时。
fn test_on_server_shutdown_starter_reports_free() {
    let log_path = std::env::temp_dir().join(format!(
        "astersql-standby-restart-{}.log",
        std::process::id()
    ));
    let _ = fs::remove_file(&log_path);
    fs::write(&log_path, b"keyspace-1:idle").expect("write restart log");

    let mgr = Arc::new(MockManager::default());
    let controller =
        LoadKeyspaceController::new(Some(mgr.clone())).with_restart_log_path(&log_path);
    controller.set_starter_mode(true);
    let svr = SimpleShutdownServer::default();
    svr.set_need_request_manager_free();

    controller.on_server_shutdown(&svr);

    assert!(mgr.called.load(Ordering::Acquire));
    assert_eq!(
        mgr.got_reason.lock().expect("reason").as_str(),
        "keyspace-1:idle"
    );
    assert_eq!(controller.state(), State::Terminating);
    let _ = fs::remove_file(&log_path);

    // reports after zero connection wait succeeds
    // 连接清零等待成功后才 report free。

    {
        let mgr = Arc::new(MockManager::default());
        let controller = LoadKeyspaceController::new(Some(mgr.clone()));
        controller.set_starter_mode(true);
        controller.set_close_connection_wait(Duration::from_secs(1));
        let svr = Arc::new(BlockingShutdownServer::new());
        let svr_thread = Arc::clone(&svr);
        let controller_thread = controller.clone();
        let done = Arc::new(AtomicBool::new(false));
        let done_flag = Arc::clone(&done);

        let join = thread::spawn(move || {
            controller_thread.on_server_shutdown(svr_thread.as_ref());
            done_flag.store(true, Ordering::Release);
        });

        assert!(
            svr.wait_until_started(Duration::from_secs(1)),
            "wait zero connection was not called"
        );
        assert!(!mgr.called.load(Ordering::Acquire));

        svr.signal_done();
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline && !done.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(done.load(Ordering::Acquire), "shutdown did not finish");
        join.join().expect("shutdown thread");
        assert!(mgr.called.load(Ordering::Acquire));
    }

    // does not report free after zero connection wait timeout
    // 等待超时则不调用 free。

    {
        let mgr = Arc::new(MockManager::default());
        let controller = LoadKeyspaceController::new(Some(mgr.clone()));
        controller.set_starter_mode(true);
        controller.set_close_connection_wait(Duration::from_millis(10));
        let svr = BlockingShutdownServer::new();
        controller.on_server_shutdown(&svr);
        assert!(!mgr.called.load(Ordering::Acquire));
        svr.signal_done();
    }
}

#[test]
/// Manager.free 失败时重试至上限；耗尽后 report_manager_free 返回 false。
fn test_on_server_shutdown_starter_retries_manager_free() {
    let mgr = Arc::new(MockManager {
        failures: 2,
        ..Default::default()
    });
    let controller = LoadKeyspaceController::new(Some(mgr.clone()));
    controller.set_starter_mode(true);
    let svr = SimpleShutdownServer::default();
    svr.set_need_request_manager_free();

    controller.on_server_shutdown(&svr);

    assert!(mgr.called.load(Ordering::Acquire));
    assert_eq!(*mgr.calls.lock().expect("calls"), 3);

    // returns false after exhausting retries
    // 重试耗尽返回 false。

    let mgr = Arc::new(MockManager {
        failures: MANAGER_FREE_MAX_ATTEMPTS,
        ..Default::default()
    });
    let controller = LoadKeyspaceController::new(Some(mgr.clone()));
    assert!(!controller.report_manager_free("manager free failed"));
    assert_eq!(*mgr.calls.lock().expect("calls"), MANAGER_FREE_MAX_ATTEMPTS);
}

#[test]
/// starter status 回传并正确转义 export_id。
fn test_status_returns_export_id_for_starter() {
    let controller = LoadKeyspaceController::new(None);
    controller.set_starter_mode(true);
    controller.set_activation_request(ActivateRequest {
        keyspace_name: "ks1".into(),
        export_id: "export\"\\1".into(),
        ..Default::default()
    });

    let resp = controller.handler(None).handle(&http_request(
        Method::Get,
        "/tidb-pool/status",
        HashMap::new(),
        "",
    ));
    assert_json_eq(
        &resp.body,
        r#"{"state": "standby", "keyspace_name": "ks1", "export_id": "export\"\\1"}"#,
    );
}

#[test]
/// exit 查询参数非法时返回 400；并验证 close_connection_wait 存取。
fn test_exit_rejects_invalid_query_params() {
    let restore = config::restore_func();
    config::update_global(|conf| {
        conf.keyspace_name = "ks1".into();
    });

    let controller = LoadKeyspaceController::new(None);
    controller.set_local_keyspace("ks1");
    let svr: Arc<dyn StandbyShutdownServer> = Arc::new(SimpleShutdownServer::default());
    let mux = controller.handler(Some(svr));

    let cases = [
        (
            "invalid graceful",
            HashMap::from([
                ("keyspace".into(), "ks1".into()),
                ("graceful".into(), "tru".into()),
            ]),
            "invalid graceful\n",
        ),
        (
            "negative wait",
            HashMap::from([
                ("keyspace".into(), "ks1".into()),
                ("wait".into(), "-1".into()),
            ]),
            "invalid wait\n",
        ),
        (
            "overflow wait",
            HashMap::from([
                ("keyspace".into(), "ks1".into()),
                ("wait".into(), "9223372036854775807".into()),
            ]),
            "invalid wait\n",
        ),
        (
            "too large wait",
            HashMap::from([
                ("keyspace".into(), "ks1".into()),
                ("wait".into(), "24h1s".into()),
            ]),
            "invalid wait\n",
        ),
        (
            "invalid skip auto id owner",
            HashMap::from([
                ("keyspace".into(), "ks1".into()),
                ("skip_auto_id_owner".into(), "maybe".into()),
            ]),
            "invalid skip_auto_id_owner\n",
        ),
        (
            "invalid need manager free",
            HashMap::from([
                ("keyspace".into(), "ks1".into()),
                ("need_mgr_free".into(), "maybe".into()),
            ]),
            "invalid need_mgr_free\n",
        ),
    ];

    for (name, query, want) in cases {
        let resp = mux.handle(&http_request(Method::Get, "/tidb-pool/exit", query, ""));
        assert_eq!(resp.status, 400, "{name}");
        assert_eq!(body_string(&resp), want, "{name}");
    }

    controller.set_close_connection_wait(Duration::from_secs(3));
    assert_eq!(controller.close_connection_wait(), Duration::from_secs(3));
    controller.set_close_connection_wait(Duration::ZERO);
    assert_eq!(controller.close_connection_wait(), Duration::ZERO);

    restore();
}

#[test]
/// 优雅 exit 无 need_mgr_free 时发 Terminate，并使用默认等待时长。
fn test_exit_graceful_uses_term_signal_without_manager_free() {
    let log_path = std::env::temp_dir().join(format!(
        "astersql-standby-exit-graceful-{}.log",
        std::process::id()
    ));
    let _ = fs::remove_file(&log_path);
    let restore = config::restore_func();
    config::update_global(|conf| {
        conf.keyspace_name = "ks1".into();
    });

    let exit = Arc::new(RecordingExitSignaler::default());
    let controller = LoadKeyspaceController::with_exit_signaler(
        None,
        Arc::clone(&exit) as Arc<dyn ExitSignaler>,
    )
    .with_restart_log_path(&log_path);
    controller.set_starter_mode(true);
    controller.set_local_keyspace("ks1");
    let svr: Arc<dyn StandbyShutdownServer> = Arc::new(SimpleShutdownServer::default());
    let mux = controller.handler(Some(svr));
    let query = HashMap::from([
        ("keyspace".into(), "ks1".into()),
        ("graceful".into(), "true".into()),
    ]);
    let resp = mux.handle(&http_request(Method::Get, "/tidb-pool/exit", query, ""));

    assert_eq!(resp.status, 200);
    assert_eq!(exit.take(), Some(ExitSignal::Terminate));
    assert_eq!(
        controller.close_connection_wait(),
        DEFAULT_CLOSE_CONNECTION_WAIT
    );

    let _ = fs::remove_file(&log_path);
    restore();
}

#[test]
/// 有 Manager 时 graceful+need_mgr_free 成功；无 notifier 时 503。
fn test_exit_rejects_manager_free_without_notifier() {
    let log_path = std::env::temp_dir().join(format!(
        "astersql-standby-exit-mgr-{}.log",
        std::process::id()
    ));
    let _ = fs::remove_file(&log_path);
    let restore = config::restore_func();
    config::update_global(|conf| {
        conf.keyspace_name = "ks1".into();
    });

    // uses default wait
    // wait 为空/0/0s 时使用 DEFAULT_CLOSE_CONNECTION_WAIT。

    for wait in ["", "0", "0s"] {
        let exit = Arc::new(RecordingExitSignaler::default());
        let mgr = Arc::new(MockManager::default());
        let controller = LoadKeyspaceController::with_exit_signaler(
            Some(mgr),
            Arc::clone(&exit) as Arc<dyn ExitSignaler>,
        )
        .with_restart_log_path(&log_path);
        controller.set_starter_mode(true);
        controller.set_local_keyspace("ks1");
        let svr = Arc::new(SimpleShutdownServer::default());
        let mux = controller.handler(Some(svr.clone()));
        let mut query = HashMap::from([
            ("keyspace".into(), "ks1".into()),
            ("graceful".into(), "true".into()),
            ("need_mgr_free".into(), "true".into()),
        ]);
        if !wait.is_empty() {
            query.insert("wait".into(), wait.into());
        }
        let resp = mux.handle(&http_request(Method::Get, "/tidb-pool/exit", query, ""));
        assert_eq!(resp.status, 200, "wait={wait}");
        assert_eq!(exit.take(), Some(ExitSignal::Terminate), "wait={wait}");
        assert!(svr.need_request_manager_free(), "wait={wait}");
        assert_eq!(
            controller.close_connection_wait(),
            DEFAULT_CLOSE_CONNECTION_WAIT,
            "wait={wait}"
        );
    }

    // requires notifier
    // 需要 Manager free 但未注入 Manager 客户端 → 503。

    {
        let exit = Arc::new(RecordingExitSignaler::default());
        let controller = LoadKeyspaceController::with_exit_signaler(
            None,
            Arc::clone(&exit) as Arc<dyn ExitSignaler>,
        )
        .with_restart_log_path(&log_path);
        controller.set_starter_mode(true);
        controller.set_local_keyspace("ks1");
        let svr: Arc<dyn StandbyShutdownServer> = Arc::new(SimpleShutdownServer::default());
        let mux = controller.handler(Some(svr));
        let query = HashMap::from([
            ("keyspace".into(), "ks1".into()),
            ("graceful".into(), "true".into()),
            ("need_mgr_free".into(), "true".into()),
            ("wait".into(), "1s".into()),
        ]);
        let resp = mux.handle(&http_request(Method::Get, "/tidb-pool/exit", query, ""));
        assert_eq!(resp.status, 503);
        assert_eq!(body_string(&resp), "manager notifier is unavailable\n");
        assert_eq!(exit.take(), None);
    }

    let _ = fs::remove_file(&log_path);
    restore();
}

#[test]
/// parse_exit_wait：Go duration、遗留秒、上限与溢出错误。
fn test_parse_exit_wait() {
    let cases = [
        ("empty", "", Ok(Duration::ZERO)),
        ("zero", "0", Ok(Duration::ZERO)),
        ("zero duration", "0s", Ok(Duration::ZERO)),
        ("duration", "1h30m", Ok(Duration::from_secs(3600 + 30 * 60))),
        ("legacy seconds", "3600", Ok(Duration::from_secs(3600))),
        ("max", "24h", Ok(Duration::from_secs(24 * 3600))),
        ("above max", "24h1s", Err(())),
        ("overflow", "9223372036854775807", Err(())),
    ];

    for (name, value, want) in cases {
        let got = parse_exit_wait(value);
        match want {
            Ok(duration) => {
                assert_eq!(got.expect(name), duration, "{name}");
            }
            Err(()) => {
                assert!(got.is_err(), "{name}");
            }
        }
    }
}
