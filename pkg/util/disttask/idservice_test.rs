// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 执行器 ID 生成与查找的单元测试。
//
// 覆盖 IPv4/IPv6 格式、`FindServerInfo`/`MatchServerInfo` 顺序，以及 Mock 注册表回退。

use super::*;
use infosync::ServerInfo;
use std::sync::{Arc, Mutex, OnceLock};

fn registry_serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct RegistryEtcd;

impl infosync::EtcdClient for RegistryEtcd {
    fn get(&self, _key: &str) -> infosync::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn get_prefix(&self, prefix: &str) -> infosync::Result<Vec<(String, Vec<u8>)>> {
        if prefix != "/tidb/server/info" {
            return Ok(Vec::new());
        }
        Ok(vec![(
            "/tidb/server/info/remote-node".to_owned(),
            br#"{"version":"v1","git_hash":"abc","ddl_id":"remote-node","ip":"2001:db8::1","listening_port":4000,"status_port":10080,"lease":"45s","start_timestamp":1,"server_id":2,"labels":{}}"#.to_vec(),
        )])
    }

    fn put(&self, _key: &str, _value: Vec<u8>) -> infosync::Result<()> {
        Ok(())
    }

    fn delete(&self, _key: &str) -> infosync::Result<()> {
        Ok(())
    }
}

/// 构造仅填充 IP/Port 的测试用 `ServerInfo`。
fn server(ip: &str, port: u32) -> ServerInfo {
    ServerInfo {
        IP: ip.to_owned(),
        Port: port,
        ..ServerInfo::default()
    }
}

/// 对应 Go `TestGenServerID`：校验空地址、IPv4、类主机名与 IPv6 方括号格式。
#[test]
fn TestGenServerID() {
    assert_eq!(GenerateExecID(&server("", 0)), ":0");
    assert_eq!(
        GenerateExecID(&server("10.124.122.25", 3456)),
        "10.124.122.25:3456"
    );
    assert_eq!(GenerateExecID(&server("10.124", 3456)), "10.124:3456");
    assert_eq!(GenerateExecID(&server("", 65537)), ":65537");
    assert_eq!(
        GenerateExecID(&server("ABCD:EF01:2345:6789:ABCD:EF01:2345:6789", 65537)),
        "[ABCD:EF01:2345:6789:ABCD:EF01:2345:6789]:65537",
    );
}

/// 按输入顺序查找；匹配成功/失败分支。
#[test]
fn find_and_match_preserve_input_order() {
    let servers = vec![server("127.0.0.1", 4000), server("::1", 4001)];
    assert_eq!(FindServerInfo(&servers, "[::1]:4001"), 1);
    assert!(MatchServerInfo(&servers, "127.0.0.1:4000"));
    assert!(!MatchServerInfo(&servers, "127.0.0.1:5000"));
}

/// Mock 注册表：缺失返回空串；注册节点后解析为首个 mock 端口。
#[test]
fn test_registry_resolution_matches_go_fallbacks() {
    let _guard = registry_serial();
    let registry = infosync::MockGlobalServerInfoManagerEntry();
    registry.Close();
    assert_eq!(GenerateSubtaskExecID4Test("missing"), "");
    registry.Add("node-1".to_owned(), std::sync::Arc::new(|| 1));
    assert_eq!(GenerateSubtaskExecID4Test("node-1"), "127.0.0.1:4000");
    registry.Close();
}

/// 生产路径从 InfoSyncer 的 etcd 节点注册表读取，而不是测试用 mock 注册表。
#[test]
fn production_registry_resolves_remote_server_info() {
    let _guard = registry_serial();
    let registry = infosync::MockGlobalServerInfoManagerEntry();
    registry.Close();
    infosync::GlobalInfoSyncerInit(
        "local-node".to_owned(),
        Arc::new(|| 1),
        Some(Arc::new(RegistryEtcd)),
        None,
        None,
        infosync::Codec::default(),
        true,
        None,
    )
    .unwrap();

    assert_eq!(GenerateSubtaskExecID("remote-node"), "[2001:db8::1]:4000");
    registry.Close();
}
