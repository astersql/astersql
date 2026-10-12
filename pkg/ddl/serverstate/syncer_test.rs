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
