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

// Standby 经典（非 starter）路径单元测试。
//
// 覆盖激活元数据、activate 等待 Server 就绪、非 starter 下 shutdown 不通知 Manager，
// 以及 status 不回传 export_id。NextGen 构建下临时切到 Premium 以断言非 starter。

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use astersql_config_deploymode as deploymode;
use astersql_config_kerneltype as kerneltype;
use astersql_server::http_status::{Method, Request};
use astersql_server::standby::{StandbyController, StandbyReadyServer, StandbyShutdownServer};
use serde_json::json;

use crate::{ActivateRequest, LoadKeyspaceController, ManagerClient, State};

#[derive(Default)]
/// Manager 替身：可注入前若干次失败并记录 free 原因。
struct MockManagerClient {
    /// 是否至少调用过一次 free。
    called: AtomicBool,
    /// free 调用次数。
    calls: Mutex<usize>,
    /// 前 N 次 free 返回错误。
    failures: usize,
    /// 最近一次 free 收到的原因。
    got_reason: Mutex<String>,
}

/// 按 failures 注入瞬时错误。
impl ManagerClient for MockManagerClient {
    fn free(&self, exit_reason: &str) -> Result<(), String> {
        self.called.store(true, Ordering::Release);
        let mut calls = self.calls.lock().expect("calls lock");
        *calls += 1;
        *self.got_reason.lock().expect("reason lock") = exit_reason.to_string();
        if *calls <= self.failures {
            return Err("temporary error".into());
        }
        Ok(())
    }
}

#[derive(Default)]
/// StandbyReadyServer 替身：记录 init_tidb_listener 调用。
struct MockReadyServer {
    /// 是否调用过 init_tidb_listener。
    called: AtomicBool,
    /// 若设置则 init 返回该错误。
    err: Option<String>,
}

/// 模拟监听初始化成功或失败。
impl StandbyReadyServer for MockReadyServer {
    fn init_tidb_listener(&self) -> Result<(), String> {
        self.called.store(true, Ordering::Release);
        match &self.err {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }
}

#[derive(Default)]
/// StandbyShutdownServer 替身：force/need_manager_free 标志。
struct MockShutdownServer {
    need_request_mgr_free: AtomicBool,
    force_shutdown: AtomicBool,
    healthy: AtomicBool,
    normal_closed: Mutex<Option<String>>,
}

/// 最小关闭钩子实现。
impl StandbyShutdownServer for MockShutdownServer {
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
    fn health(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
    fn normal_closed_connection(&self, _keyspace: &str, _connection_id: &str) -> Option<String> {
        self.normal_closed
            .lock()
            .expect("normal closed lock")
            .clone()
    }
}

/// 构造无 query/headers 的 HTTP 请求。
fn http_request(method: Method, path: &str, body: &str) -> Request {
    Request {
        method,
        path: path.into(),
        query: HashMap::new(),
        raw_query: String::new(),
        headers: HashMap::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// 按 JSON 语义比较响应体与期望字符串。
fn assert_json_eq(actual: &[u8], expected: &str) {
    let actual_value: serde_json::Value =
        serde_json::from_slice(actual).expect("response body must be json");
    let expected_value: serde_json::Value =
        serde_json::from_str(expected).expect("expected json must parse");
    assert_eq!(actual_value, expected_value);
}

#[test]
/// 激活请求元数据反序列化，且控制器返回的 map 为独立副本。
fn test_activate_request_metadata() {
    let req: ActivateRequest = serde_json::from_value(json!({
        "keyspace_name": "ks",
        "metadata": {
            "meta_a": "value_a"
        }
    }))
    .expect("activate request json");
    assert_eq!(
        req.metadata,
        HashMap::from([("meta_a".into(), "value_a".into())])
    );

    let controller = LoadKeyspaceController::new(None);
    controller.set_activation_request(req.clone());
    let mut metadata = controller.activation_metadata().expect("metadata present");
    assert_eq!(req.metadata, metadata);
    metadata.insert("meta_a".into(), "changed".into());
    assert_eq!(
        controller.activation_metadata().unwrap().get("meta_a"),
        Some(&"value_a".to_string())
    );
}

#[test]
/// 缺少 keyspace_name 时 activate 返回 400。
fn test_activate_requires_keyspace_name() {
    let controller = LoadKeyspaceController::new(None);
    let mux = controller.handler(None);
    let resp = mux.handle(&http_request(Method::Post, "/tidb-pool/activate", "{}"));
    assert_eq!(resp.status, 400);
    assert!(resp.body.is_empty());

    // Go's ServeMux does not impose a POST-only restriction on this route.
    let get_resp = mux.handle(&http_request(Method::Get, "/tidb-pool/activate", "{}"));
    assert_eq!(get_resp.status, 400);
    assert!(get_resp.body.is_empty());
}

#[test]
/// activate HTTP 在 prepare_for_activation 前阻塞，就绪后返回 200。
fn test_activate_waits_until_server_ready() {
    let controller = LoadKeyspaceController::new(None);
    let mux = controller.handler(None);
    let done = Arc::new(AtomicBool::new(false));
    let done_flag = Arc::clone(&done);
    let resp_holder = Arc::new(Mutex::new(None));
    let resp_clone = Arc::clone(&resp_holder);
    let mux_clone = mux.clone();

    // 后台线程发起 activate，主线程再 prepare，验证阻塞语义。
    let join = thread::spawn(move || {
        let resp = mux_clone.handle(&http_request(
            Method::Post,
            "/tidb-pool/activate",
            r#"{"keyspace_name":"ks"}"#,
        ));
        *resp_clone.lock().expect("resp lock") = Some(resp);
        done_flag.store(true, Ordering::Release);
    });

    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if controller.state() == State::Activated {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(controller.state(), State::Activated);
    assert!(
        !done.load(Ordering::Acquire),
        "activate request returned before server ready"
    );

    let ready_server = MockReadyServer::default();
    controller
        .prepare_for_activation(&ready_server)
        .expect("prepare activation");
    join.join().expect("activate thread");

    assert!(ready_server.called.load(Ordering::Acquire));
    let resp = resp_holder
        .lock()
        .expect("resp lock")
        .take()
        .expect("response");
    assert_eq!(resp.status, 200);
    assert_json_eq(&resp.body, r#"{"state":"activated","keyspace_name":"ks"}"#);
}

#[test]
/// 非 starter 时 on_server_shutdown 不调用 Manager.free。
fn test_on_server_shutdown_noop_outside_starter() {
    if kerneltype::IsNextGen() {
        let original = deploymode::Get();
        deploymode::Set(deploymode::Premium).expect("set premium");
        // Restore after test; classic builds never enter here.
        // 测毕恢复部署模式；经典构建不会进入该分支。

        let _guard = scopeguard_restore(original);
    }
    assert!(!deploymode::IsStarter());

    let mgr = Arc::new(MockManagerClient::default());
    let controller = LoadKeyspaceController::new(Some(mgr.clone()));
    controller.set_starter_mode(false);
    let svr = MockShutdownServer::default();
    svr.set_need_request_manager_free();

    controller.on_server_shutdown(&svr);
    assert!(!mgr.called.load(Ordering::Acquire));
}

#[test]
/// 非 starter 的 status JSON 不含 export_id。
fn test_status_does_not_return_export_id_outside_starter() {
    if kerneltype::IsNextGen() {
        let original = deploymode::Get();
        deploymode::Set(deploymode::Premium).expect("set premium");
        let _guard = scopeguard_restore(original);
    }
    assert!(!deploymode::IsStarter());

    let controller = LoadKeyspaceController::new(None);
    controller.set_starter_mode(false);
    controller.set_activation_request(ActivateRequest {
        keyspace_name: "ks1".into(),
        export_id: "export-1".into(),
        ..Default::default()
    });

    let resp = controller
        .handler(None)
        .handle(&http_request(Method::Get, "/tidb-pool/status", ""));
    assert_json_eq(
        &resp.body,
        r#"{"state": "standby", "keyspace_name": "ks1"}"#,
    );
}

#[test]
fn test_status_escapes_control_characters_like_go_json_encoder() {
    let controller = LoadKeyspaceController::new(None);
    controller.set_activation_request(ActivateRequest {
        keyspace_name: "ks\n\t".into(),
        ..Default::default()
    });
    let response =
        controller
            .handler(None)
            .handle(&http_request(Method::Get, "/tidb-pool/status", ""));
    assert_json_eq(
        &response.body,
        r#"{"state":"standby","keyspace_name":"ks\n\t"}"#,
    );
}

#[test]
fn test_activate_rejects_unhealthy_server() {
    let controller = LoadKeyspaceController::new(None);
    controller
        .activate(ActivateRequest {
            keyspace_name: "ks".into(),
            ..Default::default()
        })
        .expect("initial activation");
    controller.set_activation_timeout(Duration::from_millis(1));
    let server = Arc::new(MockShutdownServer::default());
    let response = controller.handler(Some(server)).handle(&http_request(
        Method::Post,
        "/tidb-pool/activate",
        r#"{"keyspace_name":"ks"}"#,
    ));
    assert_eq!(response.status, 503);
    assert_eq!(response.body, b"server is going to shutdown");
}

#[test]
fn test_checkconn_prefers_server_normal_close_cache() {
    let controller = LoadKeyspaceController::new(None);
    let server = Arc::new(MockShutdownServer::default());
    *server.normal_closed.lock().expect("normal closed lock") = Some("drained".into());
    let mut request = http_request(Method::Get, "/tidb-pool/checkconn", "");
    request.query = HashMap::from([
        ("keyspace_name".into(), "ks".into()),
        ("conn_id".into(), "42".into()),
    ]);
    let response = controller.handler(Some(server)).handle(&request);
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"normal closed");
}

#[test]
fn test_wait_for_activate_caches_and_removes_previous_restart_log() {
    let path = std::env::temp_dir().join(format!(
        "astersql-standby-classic-restart-{}-{:?}.log",
        std::process::id(),
        Instant::now()
    ));
    fs::write(&path, b"ks:planned restart").expect("write restart log");
    let controller = LoadKeyspaceController::new(None).with_restart_log_path(&path);
    controller
        .activate(ActivateRequest {
            keyspace_name: "ks".into(),
            ..Default::default()
        })
        .expect("activate before wait");
    controller.wait_for_activate();
    assert!(
        !path.exists(),
        "restart log should be removed after loading"
    );

    let mut request = http_request(Method::Get, "/tidb-pool/checkconn", "");
    request.query = HashMap::from([
        ("keyspace_name".into(), "ks".into()),
        ("conn_id".into(), "42".into()),
    ]);
    let response = controller.handler(None).handle(&request);
    assert_eq!(response.body, b"normal closed");
}

/// RAII：Drop 时恢复原始部署模式。
fn scopeguard_restore(original: deploymode::Mode) -> impl Drop {
    struct Guard(deploymode::Mode);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = deploymode::Set(self.0);
        }
    }
    Guard(original)
}
