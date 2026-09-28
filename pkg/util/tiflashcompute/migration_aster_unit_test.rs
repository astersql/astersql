// Copyright 2026 AsterSQL.
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

// TiFlash Compute 迁移补充单元测试。
//
// 覆盖分发策略映射/错误、RecoveryType、Test/Mock/AWS 拓扑获取器、
// 全局 fetcher 初始化，以及固定池缓存与 JSON/时间戳校验行为。

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

use super::*;

/// 启动本机 HTTP 桩服务器：按序返回 (status, body)，并经 channel 回传请求原文。
fn spawn_server(responses: Vec<(u16, &'static str)>) -> (String, Receiver<String>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (request_tx, request_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0_u8; 1024];
            // 读到 HTTP 头结束标记 \r\n\r\n 即停止，避免阻塞在 keep-alive。
            loop {
                let read = stream.read(&mut buf).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = request_tx.send(String::from_utf8_lossy(&request).into_owned());
            let reason = if status == 200 {
                "OK"
            } else {
                "Internal Server Error"
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
        }
    });
    (addr, request_rx, handle)
}

/// 分发策略：合法列表、双向映射与非法字符串错误文案与 Go 一致。
#[test]
fn dispatch_policy_matches_go_mappings_and_errors() {
    assert_eq!(
        GetValidDispatchPolicy(),
        vec!["consistent_hash", "round_robin"]
    );
    assert_eq!(
        GetDispatchPolicyByStr("consistent_hash").unwrap(),
        DispatchPolicyConsistentHash
    );
    assert_eq!(
        GetDispatchPolicyByStr("round_robin").unwrap(),
        DispatchPolicyRR
    );
    let error = GetDispatchPolicyByStr("random").unwrap_err().to_string();
    assert!(error.contains("expect [consistent_hash round_robin]"));
    assert!(error.contains("got random"));
    assert_eq!(
        GetDispatchPolicy(DispatchPolicyConsistentHash),
        "consistent_hash"
    );
    assert_eq!(GetDispatchPolicy(DispatchPolicyRR), "round_robin");
    assert_eq!(GetDispatchPolicy(99), "invalid");
}

/// RecoveryType 字符串化与 Test fetcher 的空拓扑 / 未实现恢复路径。
#[test]
fn recovery_and_test_fetcher_match_go_behavior() {
    assert_eq!(RecoveryType::RecoveryTypeNull.toString().unwrap(), "Null");
    assert_eq!(
        RecoveryType::RecoveryTypeMemLimit.toString().unwrap(),
        "MemLimit"
    );
    assert_eq!(
        RecoveryType(99).toString().unwrap_err().to_string(),
        "unsupported recovery type for topo_fetcher"
    );

    let fetcher = NewTestAutoScalerFetcher();
    assert!(fetcher.FetchAndGetTopo().unwrap().is_empty());
    assert_eq!(
        fetcher
            .RecoveryAndGetTopo(RecoveryType::RecoveryTypeNull, 0)
            .unwrap_err()
            .to_string(),
        "RecoveryAndGetTopo not implemented"
    );
}

/// Mock fetcher：分号分隔拓扑解析，并校验请求路径为 /fetch_topo。
#[test]
fn mock_fetcher_gets_and_parses_semicolon_topology() {
    let (addr, requests, server) = spawn_server(vec![(200, "cn-1:3930;cn-2:3930")]);
    let fetcher = NewMockAutoScalerFetcher(addr);
    assert_eq!(
        fetcher.FetchAndGetTopo().unwrap(),
        vec!["cn-1:3930", "cn-2:3930"]
    );
    assert!(
        requests
            .recv()
            .unwrap()
            .starts_with("GET /fetch_topo HTTP/1.1")
    );
    server.join().unwrap();
}

/// Mock fetcher：空拓扑与非 200 HTTP 状态保留 Go 错误语义。
#[test]
fn mock_fetcher_preserves_go_empty_and_http_errors() {
    let (addr, _, server) = spawn_server(vec![(200, "")]);
    let error = NewMockAutoScalerFetcher(addr)
        .FetchAndGetTopo()
        .unwrap_err()
        .to_string();
    assert_eq!(error, "topo list is empty");
    server.join().unwrap();

    let (addr, _, server) = spawn_server(vec![(500, "failed")]);
    let error = NewMockAutoScalerFetcher(addr)
        .FetchAndGetTopo()
        .unwrap_err()
        .to_string();
    assert!(error.contains("get tiflash_compute topology failed"));
    server.join().unwrap();
}

/// AWS fetcher：普通拉取与 MemLimit 恢复查询参数（含 URL 编码的 cluster id）。
#[test]
fn aws_fetcher_builds_normal_and_recovery_queries() {
    let first =
        r#"{"hasError":0,"errorInfo":"","state":"ready","topology":["cn-1"],"timestamp":"10"}"#;
    let second =
        r#"{"hasError":0,"errorInfo":"","state":"ready","topology":["cn-2"],"timestamp":"11"}"#;
    let (addr, requests, server) = spawn_server(vec![(200, first), (200, second)]);
    let fetcher = NewAWSAutoScalerFetcher(addr, "cluster/a".to_string(), false);

    assert_eq!(fetcher.FetchAndGetTopo().unwrap(), vec!["cn-1"]);
    let request = requests.recv().unwrap();
    assert!(request.starts_with("GET /resume-and-get-topology?"));
    assert!(request.contains("tidbclusterid=cluster%2Fa"));

    assert_eq!(
        fetcher
            .RecoveryAndGetTopo(RecoveryType::RecoveryTypeMemLimit, 3)
            .unwrap(),
        vec!["cn-2"]
    );
    let request = requests.recv().unwrap();
    assert!(request.contains("cn_cnt=3"));
    assert!(request.contains("recovery=MemLimit"));
    assert!(request.contains("tidbclusterid=cluster%2Fa"));
    server.join().unwrap();
}

/// AWS fetcher：恢复前本地校验 CN 计数与未知 RecoveryType，不发起网络。
#[test]
fn aws_fetcher_validates_recovery_before_network() {
    let fetcher = NewAWSAutoScalerFetcher("127.0.0.1:1".to_string(), "cluster".to_string(), false);
    assert_eq!(
        fetcher
            .RecoveryAndGetTopo(RecoveryType::RecoveryTypeMemLimit, 0)
            .unwrap_err()
            .to_string(),
        "ori CN count should not be zero"
    );
    assert_eq!(
        fetcher
            .RecoveryAndGetTopo(RecoveryType(99), 1)
            .unwrap_err()
            .to_string(),
        "topo_fetcher cannot handle error: 99"
    );
}

/// 全局 fetcher 初始化：test/gcp/invalid/空地址与 GetGlobalTopoFetcher 可见性。
#[test]
fn global_fetcher_initialization_matches_go_type_switch() {
    assert!(
        InitGlobalTopoFetcher(
            "test".to_string(),
            "unused".to_string(),
            "cluster".to_string(),
            false,
        )
        .is_ok()
    );
    assert!(
        GetGlobalTopoFetcher()
            .unwrap()
            .FetchAndGetTopo()
            .unwrap()
            .is_empty()
    );

    assert!(
        InitGlobalTopoFetcher(
            "gcp".to_string(),
            "unused".to_string(),
            "cluster".to_string(),
            false,
        )
        .unwrap_err()
        .to_string()
        .contains("not implemented")
    );
    assert!(GetGlobalTopoFetcher().is_some());

    assert!(
        InitGlobalTopoFetcher(
            "invalid".to_string(),
            "unused".to_string(),
            "cluster".to_string(),
            false,
        )
        .is_err()
    );
    assert!(GetGlobalTopoFetcher().is_none());

    assert!(
        InitGlobalTopoFetcher(
            "test".to_string(),
            "".to_string(),
            "cluster".to_string(),
            false,
        )
        .unwrap_err()
        .to_string()
        .contains("addr is empty")
    );
}

/// 固定池模式：首次请求 /sharedfixedpool，后续命中缓存不再发网。
#[test]
fn fixed_pool_fetcher_fetches_once_and_reuses_cache() {
    let response =
        r#"{"hasError":0,"errorInfo":"","state":"ready","topology":["fixed-cn"],"timestamp":"5"}"#;
    let (addr, requests, server) = spawn_server(vec![(200, response)]);
    let fetcher = NewAWSAutoScalerFetcher(addr, "cluster".to_string(), true);

    assert_eq!(fetcher.FetchAndGetTopo().unwrap(), vec!["fixed-cn"]);
    assert_eq!(fetcher.FetchAndGetTopo().unwrap(), vec!["fixed-cn"]);
    assert!(
        requests
            .recv()
            .unwrap()
            .starts_with("GET /sharedfixedpool HTTP/1.1")
    );
    server.join().unwrap();
}

/// AWS fetcher：非法 JSON 与不可解析 timestamp 的错误文案。
#[test]
fn aws_fetcher_rejects_bad_json_and_timestamp() {
    let bad_timestamp =
        r#"{"hasError":0,"errorInfo":"","state":"ready","topology":["cn"],"timestamp":"bad"}"#;
    let (addr, _, server) = spawn_server(vec![(200, "not-json"), (200, bad_timestamp)]);
    let fetcher = NewAWSAutoScalerFetcher(addr, "cluster".to_string(), false);
    assert!(
        fetcher
            .FetchAndGetTopo()
            .unwrap_err()
            .to_string()
            .contains("get tiflash_compute topology failed")
    );
    assert!(
        fetcher
            .FetchAndGetTopo()
            .unwrap_err()
            .to_string()
            .contains("parse timestamp of tiflash_compute topology failed")
    );
    server.join().unwrap();
}

/// Go `encoding/json` 对缺失响应字段保留零值，Rust 也应先成功解析，再由时间戳校验报错。
#[test]
fn aws_fetcher_defaults_missing_json_fields_like_go() {
    let (addr, _, server) = spawn_server(vec![(200, "{}")]);
    let error = NewAWSAutoScalerFetcher(addr, "cluster".to_string(), false)
        .FetchAndGetTopo()
        .unwrap_err()
        .to_string();
    assert_eq!(error, "parse timestamp of tiflash_compute topology failed");
    server.join().unwrap();
}

/// Go `encoding/json` 将 `null` 解码为字段零值，Rust 也应接受后继续处理拓扑。
#[test]
fn aws_fetcher_defaults_null_json_fields_like_go() {
    let response =
        r#"{"hasError":null,"errorInfo":null,"state":null,"topology":null,"timestamp":"1"}"#;
    let (addr, _, server) = spawn_server(vec![(200, response)]);
    assert!(
        NewAWSAutoScalerFetcher(addr, "cluster".to_string(), false)
            .FetchAndGetTopo()
            .unwrap()
            .is_empty()
    );
    server.join().unwrap();

    let response =
        r#"{"hasError":null,"errorInfo":null,"state":null,"topology":[],"timestamp":null}"#;
    let (addr, _, server) = spawn_server(vec![(200, response)]);
    let error = NewAWSAutoScalerFetcher(addr, "cluster".to_string(), false)
        .FetchAndGetTopo()
        .unwrap_err()
        .to_string();
    assert_eq!(error, "parse timestamp of tiflash_compute topology failed");
    server.join().unwrap();
}
