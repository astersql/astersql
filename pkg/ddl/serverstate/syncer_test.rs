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

// server state syncer 在 etcd 中初始化、读取全局状态、watch 状态切换和 IsUpgradingState 缓存更新的语义。
//
// #[derive(Debug, Clone, PartialEq, Eq)]
// struct DraftKeyValue {
//     key: String,
//     value: String,
// }
// check_resp_kv 对应 Go 的 checkRespKV。
// 它先断言返回的 kv 数量；数量非 0 时只检查第一条 KeyValue 的 key/value。
// fn check_resp_kv(expected_count: usize, key: &str, val: &str, kvs: &[DraftKeyValue]) {
//     assert_eq!(expected_count, kvs.len());
//     if expected_count == 0 {
//         return;
//     }
//
//     let kv = &kvs[0];
//     assert_eq!(key, kv.key);
//     assert_eq!(val, kv.value);
// }
//
// #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// enum DraftServerState {
//     NormalRunning,
//     Upgrading,
// }
//
// test_state_syncer_simple_draft_structure 对应 Go 的 TestStateSyncerSimple。
// Go 测试关闭 MDL、跳过 Windows，然后用单节点 etcd cluster 验证 serverstate.NewEtcdSyncer。
// #[test]
// fn test_state_syncer_simple_draft_structure() {
//     let setup_steps = [
//         "vardef.SetEnableMDL(false)",
//         "runtime.GOOS == windows 时跳过 integration.NewClusterV3",
//         "integration.BeforeTestExternal(t)",
//         "临时把 schemaver.CheckVersFirstWaitTime 置 0，并在 defer 中恢复",
//         "NewClusterV3(size=1) 后获取 RandClient",
//         "serverstate.NewEtcdSyncer(cli, util2.ServerGlobalState)",
//         "Init(ctx)；Go 注释说明 newDDL 未接入前测试里显式调用",
//     ];
//     assert_eq!(setup_steps.len(), 7);
//
// 初始状态读取：GetGlobalState 应返回 StateNormalRunning，且 IsUpgradingState 为 false。
//     let mut expected_state = DraftServerState::NormalRunning;
//     assert_eq!(expected_state, DraftServerState::NormalRunning);
//     let is_upgrading_cached = false;
//     assert!(!is_upgrading_cached);
//
// checkValue 闭包中的 watch 分支：3 秒内读取 WatchChan，检查 event kv，并重新调用 GetGlobalState。
// Go 在收到 StateUpgrading event 前先要求 IsUpgradingState 仍为 false，读取后再要求变为 true。
//     expected_state = DraftServerState::Upgrading;
//     let upgrading_payload = "StateInfo{State: StateUpgrading}.Marshal()";
//     let upgrading_kvs = [DraftKeyValue {
//         key: "ServerGlobalState".to_owned(),
//         value: upgrading_payload.to_owned(),
//     }];
//     check_resp_kv(1, "ServerGlobalState", upgrading_payload, &upgrading_kvs);
//
//     let transition_cases = [
//         (
//             DraftServerState::Upgrading,
//             "UpdateGlobalState(ctx, StateUpgrading) 后 wait group 完成，checkErr 为空",
//             true,
//         ),
//         (
//             DraftServerState::NormalRunning,
//             "UpdateGlobalState(ctx, StateNormalRunning) 后 wait group 完成，checkErr 为空",
//             false,
//         ),
//     ];
//     for (state, detail, expect_upgrading_after_get) in transition_cases {
// 这里保留 Go 闭包中 before/after GetGlobalState 的缓存判断语义。
//         assert!(detail.contains("UpdateGlobalState"));
//         match state {
//             DraftServerState::Upgrading => assert!(expect_upgrading_after_get),
//             DraftServerState::NormalRunning => assert!(!expect_upgrading_after_get),
//         }
//     }
// }
// */
// `serverstate` 同步器单测。
//
// 用内存版 `MemSyncer` 验证：初始化后非升级态、更新为升级态后
// watch 通道能收到事件，且 `GetGlobalState` / `IsUpgradingState` 与之一致。

use std::sync::Arc;
use std::time::Duration;

use super::*;

/// 覆盖全局状态切换与 watch 通知的端到端路径。
#[test]
fn state_syncer_transitions_and_watch_are_real() {
    let context = SyncContext::new();
    let syncer = new_mem_syncer();
    syncer.init(&context).unwrap();
    assert!(!syncer.is_upgrading_state());
    // 切到升级态，应触发 watch 事件并更新缓存状态。
    syncer
        .update_global_state(&context, StateInfo::new(STATE_UPGRADING))
        .unwrap();
    let watch = syncer.watch_chan();
    assert!(watch.recv_timeout(Duration::from_secs(1)).is_ok());
    assert_eq!(
        syncer.get_global_state(&context).unwrap(),
        StateInfo::new(STATE_UPGRADING)
    );
    assert!(syncer.is_upgrading_state());
}

/// 对应 Go `TestStateSyncerSimple`：存储写入只产生一个 watch 事件，
/// 且本地缓存仅在 `get_global_state` 后跟随远端状态变化。
#[test]
fn etcd_syncer_matches_go_state_watch_and_cache_semantics() {
    let context = SyncContext::new();
    let store = Arc::new(StateStore::default());
    let path = "/tidb/server/global-state";
    let syncer = new_etcd_syncer(store, path);
    syncer.init(&context).unwrap();

    assert_eq!(
        syncer.get_global_state(&context).unwrap(),
        StateInfo::new(STATE_NORMAL_RUNNING)
    );
    assert!(!syncer.is_upgrading_state());

    for (state, cached_before_get, cached_after_get) in [
        (STATE_UPGRADING, false, true),
        (STATE_NORMAL_RUNNING, true, false),
    ] {
        let expected = StateInfo::new(state);
        syncer
            .update_global_state(&context, expected.clone())
            .unwrap();

        let watch = syncer.watch_chan();
        let response = watch.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(response.key, path);
        assert_eq!(response.value, expected.marshal().unwrap());
        assert_eq!(syncer.is_upgrading_state(), cached_before_get);
        assert_eq!(syncer.get_global_state(&context).unwrap(), expected);
        assert_eq!(syncer.is_upgrading_state(), cached_after_get);

        assert_eq!(
            watch.recv_timeout(Duration::from_millis(20)),
            Err(SyncError::Timeout),
            "one store update must emit exactly one watch event"
        );
    }
}

/// `encoding/json` combines UTF-16 surrogate pairs used by JSON unicode escapes.
#[test]
fn state_info_unmarshal_matches_go_unicode_escape_semantics() {
    assert_eq!(
        StateInfo::unmarshal(br#"{"state":"\ud83d\ude00"}"#).unwrap(),
        StateInfo::new("😀")
    );
}

/// Unknown fields are ignored by Go, but their values must still be valid JSON.
#[test]
fn state_info_unmarshal_rejects_invalid_unknown_field_values() {
    assert!(StateInfo::unmarshal(br#"{"unknown":truth,"state":"upgrading"}"#).is_err());
}

/// Go's JSON decoder matches struct fields case-insensitively and treats null as no value.
#[test]
fn state_info_unmarshal_matches_go_field_matching_and_null_semantics() {
    assert_eq!(
        StateInfo::unmarshal(br#"{"STATE":"upgrading"}"#).unwrap(),
        StateInfo::new(STATE_UPGRADING)
    );
    assert_eq!(
        StateInfo::unmarshal(br#"{"state":null}"#).unwrap(),
        StateInfo::default()
    );
}

#[test]
fn crossks_align_schema_protocol_refreshes_state_without_watch_or_session() {
    use astersql_ddl_schemaver::{
        Context, EtcdClient, GetResponse, MemoryEtcdClient, Session, SyncError as TransportError,
        WatchChan,
    };
    struct NetworkBoundary(MemoryEtcdClient);
    impl EtcdClient for NetworkBoundary {
        fn NewSession(&self, _: &Context, _: i32) -> Result<Session, TransportError> {
            panic!("crossks must not allocate a state lease")
        }
        fn Watch(&self, _: &Context, _: &str, _: bool, _: i64) -> WatchChan {
            panic!("crossks must not start a state watch")
        }
        fn Get(&self, c: &Context, k: &str, p: bool) -> Result<GetResponse, TransportError> {
            self.0.Get(c, k, p)
        }
        fn Put(&self, c: &Context, k: &str, v: &str, l: Option<i64>) -> Result<(), TransportError> {
            self.0.Put(c, k, v, l)
        }
        fn PutMono(&self, c: &Context, k: &str, v: &str) -> Result<(), TransportError> {
            self.0.PutMono(c, k, v)
        }
        fn PutIfAbsent(&self, c: &Context, k: &str, v: &str) -> Result<bool, TransportError> {
            self.0.PutIfAbsent(c, k, v)
        }
        fn Delete(&self, c: &Context, k: &str) -> Result<(), TransportError> {
            self.0.Delete(c, k)
        }
    }
    let client = Arc::new(NetworkBoundary(MemoryEtcdClient::default()));
    let syncer = EtcdSyncer::with_client(client.clone(), "/tidb/server/global_state");
    let ctx = SyncContext::new();
    assert_eq!(syncer.get_global_state(&ctx).unwrap(), StateInfo::default());
    syncer
        .update_global_state(&ctx, StateInfo::new(STATE_UPGRADING))
        .unwrap();
    assert!(!syncer.is_upgrading_state());
    assert_eq!(
        syncer.get_global_state(&ctx).unwrap(),
        StateInfo::new(STATE_UPGRADING)
    );
    assert!(syncer.is_upgrading_state());
    client
        .Put(
            &Context::Background(),
            "/tidb/server/global_state",
            r#"{"STATE":null,"future":[1,true]}"#,
            None,
        )
        .unwrap();
    assert_eq!(syncer.get_global_state(&ctx).unwrap(), StateInfo::default());
    assert!(!syncer.is_upgrading_state());
    client
        .Put(
            &Context::Background(),
            "/tidb/server/global_state",
            "invalid JSON",
            None,
        )
        .unwrap();
    assert!(matches!(
        syncer.get_global_state(&ctx),
        Err(SyncError::InvalidState(_))
    ));
    ctx.cancel();
    assert_eq!(syncer.get_global_state(&ctx), Err(SyncError::Cancelled));
}

#[test]
fn crossks_align_schema_protocol_public_client_preserves_init_and_watch() {
    let client = Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default());
    let ctx = SyncContext::new();
    let syncer = EtcdSyncer::with_client(client, "/tidb/server/global_state");
    syncer.init(&ctx).unwrap();
    syncer
        .update_global_state(&ctx, StateInfo::new(STATE_UPGRADING))
        .unwrap();
    assert_eq!(
        syncer
            .watch_chan()
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .value,
        StateInfo::new(STATE_UPGRADING).marshal().unwrap()
    );
    assert!(!syncer.is_upgrading_state());
    syncer.get_global_state(&ctx).unwrap();
    assert!(syncer.is_upgrading_state());
    syncer.rewatch(&ctx);
    syncer
        .update_global_state(&ctx, StateInfo::default())
        .unwrap();
    assert_eq!(
        syncer
            .watch_chan()
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .value,
        StateInfo::default().marshal().unwrap()
    );
    drop(syncer);
    assert!(
        ctx.error().is_none(),
        "closing a watch must not cancel the caller"
    );
    ctx.cancel();
}
