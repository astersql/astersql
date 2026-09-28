// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// PD 相关集成测试：全局配置读写/监听，以及 Mock Keyspace 管理器。
//
// Keyspace 是多租户键空间隔离单位；本文件验证 ID/名称唯一性、
// 按 ID 分页列举，以及对非法 ID（超限、NULL）的拒绝。

use crate::pd::{
    GlobalConfigItem, KeyspaceMeta, MAX_KEYSPACE_ID, NULL_KEYSPACE_ID, NewMockPDServiceDiscovery,
    PdClient, is_url, newMockKeyspaceManager,
};
use crate::{Cluster, New, RPCClient};
use std::sync::Arc;

/// 嵌入式 unistore 测试套件持有的 RPC/Cluster/PD 客户端。
struct GlobalConfigTestSuite {
    rpc: Arc<RPCClient>,
    cluster: Arc<Cluster>,
    client: Arc<PdClient>,
}

/// 启动空配置的嵌入式 unistore，供全局配置用例复用。
fn set_up_suite() -> GlobalConfigTestSuite {
    let (rpc, client, cluster) =
        New("", Vec::new(), NULL_KEYSPACE_ID, Vec::new()).expect("create embedded unistore");
    GlobalConfigTestSuite {
        rpc,
        cluster,
        client,
    }
}

impl GlobalConfigTestSuite {
    /// 关闭客户端与集群，释放嵌入式资源。
    fn tear_down_suite(&self) {
        self.client.close();
        let _ = self.rpc.close();
        self.cluster.close();
    }
}

/// 写入后 load：存在项返回值，缺失项返回空字符串。
#[test]
fn test_load() {
    let s = set_up_suite();
    s.client.store_global_config(
        "",
        &[GlobalConfigItem {
            name: "LoadOkGlobalConfig".into(),
            value: "ok".into(),
            ..GlobalConfigItem::default()
        }],
    );
    let (res, _) = s.client.load_global_config(
        &["LoadOkGlobalConfig".into(), "LoadErrGlobalConfig".into()],
        "",
    );
    for item in res {
        match item.name.as_str() {
            "/global/config/LoadOkGlobalConfig" => assert_eq!(item.value, "ok"),
            "/global/config/LoadErrGlobalConfig" => assert_eq!(item.value, ""),
            other => panic!("unexpected global config name {other}"),
        }
    }
    s.tear_down_suite();
}

/// store 后再 load，新配置项应可见。
#[test]
fn test_store() {
    let s = set_up_suite();
    let (res, _) = s.client.load_global_config(&["NewObject".into()], "");
    assert_eq!(res[0].value, "");

    s.client.store_global_config(
        "",
        &[GlobalConfigItem {
            name: "NewObject".into(),
            value: "ok".into(),
            ..GlobalConfigItem::default()
        }],
    );
    let (res, _) = s.client.load_global_config(&["NewObject".into()], "");
    assert_eq!(res[0].value, "ok");
    s.tear_down_suite();
}

/// watch 通道持续收到非空配置变更事件。
#[test]
fn test_watch() {
    let s = set_up_suite();
    s.client.store_global_config(
        "/global/config",
        &[GlobalConfigItem {
            name: "NewObject".into(),
            value: "ok".into(),
            ..GlobalConfigItem::default()
        }],
    );
    let ch = s.client.watch_global_config("/global/config", 0);
    for _ in 0..10 {
        let res = ch.recv().expect("watch event");
        assert_ne!(res[0].value, "");
    }
    drop(ch);
    s.tear_down_suite();
}

/// Mock PD 服务发现：过滤非法 URL，并为合法地址补全 http scheme。
#[test]
fn test_mock_pd_service_discovery() {
    let pd_addrs = [
        "invalid_pd_address",
        "127.0.0.1:2379",
        "http://172.32.21.32:2379",
    ];
    for (i, addr) in pd_addrs.iter().enumerate() {
        let check = is_url(addr);
        if i > 0 {
            assert!(check, "{addr} should be a valid URL");
        } else {
            assert!(!check, "{addr} should be invalid");
        }
    }
    let sd = NewMockPDServiceDiscovery(pd_addrs.iter().map(|s| (*s).to_owned()).collect());
    let clis = sd.all_service_clients();
    assert_eq!(clis.len(), 2);
    assert_eq!(clis[0].address(), "http://127.0.0.1:2379");
    assert_eq!(clis[1].address(), "http://172.32.21.32:2379");
}

/// 断言 Keyspace 列表按 ID 升序，且名称映射与元数据一致。
fn check_elements(m: &crate::pd::MockKeyspaceManager, ids: &[u32], names: &[&str]) {
    assert_eq!(ids.len(), names.len());
    let keyspaces = m.keyspaces();
    let name_map = m.keyspace_names_map();
    assert_eq!(ids.len(), keyspaces.len(), "keyspace meta 数量不匹配");
    assert_eq!(ids.len(), name_map.len(), "keyspace 名称映射数量不匹配");
    for (i, keyspace) in keyspaces.iter().enumerate() {
        if i > 0 {
            assert!(keyspace.id > keyspaces[i - 1].id);
        }
        assert_eq!(ids[i], keyspace.id);
        assert_eq!(names[i], keyspace.name);
        let mapped = name_map.get(&keyspace.name).copied();
        assert_eq!(mapped, Some(keyspace.id));
    }
}

/// 按名称 load：存在时校验 id/name，不存在时期望 ENTRY_NOT_FOUND。
fn must_load_keyspace(
    m: &crate::pd::MockKeyspaceManager,
    name: &str,
    expected_exists: bool,
    expected_id: u32,
) {
    let result = m.load(name);
    if expected_exists {
        let meta = result.expect("keyspace should exist");
        assert_eq!(name, meta.name);
        assert_eq!(expected_id, meta.id);
    } else {
        let err = result.expect_err("keyspace should be missing");
        assert!(err.to_string().contains("ENTRY_NOT_FOUND"));
    }
}

/// 校验 all(start_id, limit) 分页结果的 ID/名称序列。
fn must_list_keyspaces(
    m: &crate::pd::MockKeyspaceManager,
    start_id: u32,
    limit: u32,
    expected_ids: &[u32],
    expected_names: &[&str],
) {
    assert_eq!(expected_ids.len(), expected_names.len());
    let keyspaces = m.all(start_id, limit);
    assert_eq!(keyspaces.len(), expected_ids.len());
    for (i, keyspace) in keyspaces.iter().enumerate() {
        assert_eq!(expected_ids[i], keyspace.id);
        assert_eq!(expected_names[i], keyspace.name);
    }
}

/// 覆盖空/单条/乱序多条 Keyspace、分页边界，以及重复 ID/名称与非法 ID 拒绝。
#[test]
fn test_mock_keyspace_manager() {
    let mut m = newMockKeyspaceManager(vec![]).unwrap();
    check_elements(&m, &[], &[]);
    must_load_keyspace(&m, "DEFAULT", false, 0);
    must_list_keyspaces(&m, 0, 0, &[], &[]);

    // 仅 DEFAULT（id=0）时的 load/list 行为。
    m = newMockKeyspaceManager(vec![KeyspaceMeta {
        id: 0,
        name: "DEFAULT".into(),
        ..KeyspaceMeta::default()
    }])
    .unwrap();
    check_elements(&m, &[0], &["DEFAULT"]);
    must_load_keyspace(&m, "DEFAULT", true, 0);
    must_load_keyspace(&m, "ks1", false, 0);
    must_list_keyspaces(&m, 0, 0, &[0], &["DEFAULT"]);
    must_list_keyspaces(&m, 1, 0, &[], &[]);

    // 乱序输入应在内部按 ID 排序后再列举。
    m = newMockKeyspaceManager(vec![
        KeyspaceMeta {
            id: 1,
            name: "ks1".into(),
            ..KeyspaceMeta::default()
        },
        KeyspaceMeta {
            id: 4,
            name: "ks4".into(),
            ..KeyspaceMeta::default()
        },
        KeyspaceMeta {
            id: 2,
            name: "ks2".into(),
            ..KeyspaceMeta::default()
        },
        KeyspaceMeta {
            id: 5,
            name: "ks5".into(),
            ..KeyspaceMeta::default()
        },
        KeyspaceMeta {
            id: 3,
            name: "ks3".into(),
            ..KeyspaceMeta::default()
        },
    ])
    .unwrap();
    check_elements(&m, &[1, 2, 3, 4, 5], &["ks1", "ks2", "ks3", "ks4", "ks5"]);
    for (i, name) in ["ks1", "ks2", "ks3", "ks4", "ks5"].iter().enumerate() {
        must_load_keyspace(&m, name, true, i as u32 + 1);
    }
    must_load_keyspace(&m, "ks6", false, 0);
    // 多种 start_id/limit 组合覆盖半开分页语义。
    must_list_keyspaces(
        &m,
        0,
        0,
        &[1, 2, 3, 4, 5],
        &["ks1", "ks2", "ks3", "ks4", "ks5"],
    );
    must_list_keyspaces(&m, 0, 3, &[1, 2, 3], &["ks1", "ks2", "ks3"]);
    must_list_keyspaces(
        &m,
        1,
        0,
        &[1, 2, 3, 4, 5],
        &["ks1", "ks2", "ks3", "ks4", "ks5"],
    );
    must_list_keyspaces(&m, 3, 0, &[3, 4, 5], &["ks3", "ks4", "ks5"]);
    must_list_keyspaces(&m, 3, 2, &[3, 4], &["ks3", "ks4"]);
    must_list_keyspaces(&m, 5, 0, &[5], &["ks5"]);

    // 含 MAX_KEYSPACE_ID 的边界列举。
    m = newMockKeyspaceManager(vec![
        KeyspaceMeta {
            id: 100,
            name: "ks100".into(),
            ..KeyspaceMeta::default()
        },
        KeyspaceMeta {
            id: 1,
            name: "ks1".into(),
            ..KeyspaceMeta::default()
        },
        KeyspaceMeta {
            id: MAX_KEYSPACE_ID,
            name: "lastks".into(),
            ..KeyspaceMeta::default()
        },
        KeyspaceMeta {
            id: 10,
            name: "ks10".into(),
            ..KeyspaceMeta::default()
        },
    ])
    .unwrap();
    let max = MAX_KEYSPACE_ID;
    check_elements(&m, &[1, 10, 100, max], &["ks1", "ks10", "ks100", "lastks"]);
    must_list_keyspaces(
        &m,
        0,
        0,
        &[1, 10, 100, max],
        &["ks1", "ks10", "ks100", "lastks"],
    );
    must_list_keyspaces(&m, 5, 0, &[10, 100, max], &["ks10", "ks100", "lastks"]);
    must_list_keyspaces(&m, 5, 1, &[10], &["ks10"]);
    must_list_keyspaces(&m, 10, 0, &[10, 100, max], &["ks10", "ks100", "lastks"]);
    must_list_keyspaces(&m, 11, 0, &[100, max], &["ks100", "lastks"]);
    must_list_keyspaces(&m, 99, 0, &[100, max], &["ks100", "lastks"]);
    must_list_keyspaces(&m, 101, 0, &[max], &["lastks"]);
    must_list_keyspaces(&m, max, 0, &[max], &["lastks"]);

    // 重复 ID 应失败。
    assert!(
        newMockKeyspaceManager(vec![
            KeyspaceMeta {
                id: 1,
                name: "ks1".into(),
                ..KeyspaceMeta::default()
            },
            KeyspaceMeta {
                id: 2,
                name: "ks2".into(),
                ..KeyspaceMeta::default()
            },
            KeyspaceMeta {
                id: 3,
                name: "ks3".into(),
                ..KeyspaceMeta::default()
            },
            KeyspaceMeta {
                id: 1,
                name: "ks4".into(),
                ..KeyspaceMeta::default()
            },
        ])
        .is_err()
    );

    // 重复名称应失败。
    assert!(
        newMockKeyspaceManager(vec![
            KeyspaceMeta {
                id: 1,
                name: "ks1".into(),
                ..KeyspaceMeta::default()
            },
            KeyspaceMeta {
                id: 2,
                name: "ks2".into(),
                ..KeyspaceMeta::default()
            },
            KeyspaceMeta {
                id: 3,
                name: "ks3".into(),
                ..KeyspaceMeta::default()
            },
            KeyspaceMeta {
                id: 4,
                name: "ks1".into(),
                ..KeyspaceMeta::default()
            },
        ])
        .is_err()
    );

    // 超出合法范围的 ID。
    assert!(
        newMockKeyspaceManager(vec![KeyspaceMeta {
            id: 0x1000000,
            name: "illegal".into(),
            ..KeyspaceMeta::default()
        },])
        .is_err()
    );

    // NULL_KEYSPACE_ID 非法。
    assert!(
        newMockKeyspaceManager(vec![KeyspaceMeta {
            id: NULL_KEYSPACE_ID,
            name: "".into(),
            ..KeyspaceMeta::default()
        },])
        .is_err()
    );
}
