// Copyright 2026 AsterSQL.

// TiKVDriver / tikvStore 生命周期行为单元测试。
//
// 对照 Go 侧 open、缓存复用、GC Worker、锁等待聚合与幂等 Close 语义，用可记录事件的
// Mock Backend 验证打开 PD / SafePoint、关闭顺序以及 disableGC、动态 Option 删除。

use std::sync::{Arc, Mutex};

use super::*;
use crate::test_state::global_state_guard;

/// 记录 open/close 调用序列的 Mock DriverBackend。
#[derive(Debug, Default)]
struct RecordingBackend {
    /// 按调用顺序追加的事件名（如 open-pd、close-store）。
    events: Mutex<Vec<String>>,
}

impl DriverBackend for RecordingBackend {
    fn open_pd(
        &self,
        _addrs: &[String],
        _keyspace: &str,
        _security: &Security,
        _options: &PdClientOptions,
    ) -> Result<u64, DriverError> {
        self.events.lock().unwrap().push("open-pd".into());
        Ok(42)
    }

    fn new_safe_point_kv(
        &self,
        _cluster_id: u64,
        _keyspace: &str,
        _tls: Option<&TlsConfig>,
    ) -> Result<SafePointKvSetup, DriverError> {
        // SafePoint：GC 安全点，表示早于该时间戳的版本可被回收。
        self.events.lock().unwrap().push("open-safe-point".into());
        Ok(SafePointKvSetup {
            meta_service_info: MetaServiceInfo {
                pd_addrs: vec!["pd".into()],
                group_addrs: vec!["etcd".into()],
            },
            pd_addrs: vec!["pd".into()],
            group_addrs: vec!["etcd".into()],
            safe_point_id: "sp".into(),
        })
    }

    fn close_pd(&self, _cluster_id: u64) {
        self.events.lock().unwrap().push("close-pd".into());
    }

    fn close_safe_point(&self, _safe_point_id: &str) {
        self.events.lock().unwrap().push("close-safe-point".into());
    }

    fn close_store(&self, _uuid: &str) -> Result<(), DriverError> {
        self.events.lock().unwrap().push("close-store".into());
        Ok(())
    }

    fn current_timestamp(&self, _txn_scope: &str) -> Result<u64, DriverError> {
        Ok(101)
    }

    fn lock_waits(&self) -> Vec<Result<Vec<WaitForEntry>, DriverError>> {
        // 模拟多 store 聚合：部分成功、部分失败，GetLockWaits 应合并成功条目。
        vec![
            Ok(vec![WaitForEntry {
                txn: 1,
                waiting_for_txn: 2,
                ..Default::default()
            }]),
            Err(DriverError::Backend("one store unavailable".into())),
            Ok(vec![WaitForEntry {
                txn: 3,
                waiting_for_txn: 4,
                ..Default::default()
            }]),
        ]
    }
}

/// 打开、缓存复用、GC、锁等待与幂等 Close 与 Go 生命周期一致。
#[test]
fn open_cache_gc_lock_wait_and_idempotent_close_match_go_lifecycle() {
    let _guard = global_state_guard();
    set_global_config(GlobalConfig::default());
    let backend = Arc::new(RecordingBackend::default());
    let mut driver = TiKVDriver::with_backend(backend.clone());
    let store = driver
        .OpenWithOptions(
            "tikv://pd:2379?keyspaceName=ks",
            vec![WithTxnLocalLatches(TxnLocalLatches {
                enabled: true,
                capacity: 2048,
            })],
        )
        .unwrap();
    assert_eq!(store.GetClusterID(), 42);
    assert_eq!(store.GetKeyspace(), "ks");
    assert_eq!(store.local_latches_capacity(), Some(2048));
    assert_eq!(store.GetPDAddrs().unwrap(), ["pd"]);
    assert_eq!(store.EtcdAddrs().unwrap(), ["etcd"]);
    assert_eq!(store.CurrentVersion("global").unwrap(), Version(101));
    assert_eq!(store.Begin().unwrap().start_ts, 101);
    // 两个成功 store 各贡献一条等待边，失败 store 被跳过。
    assert_eq!(store.GetLockWaits().unwrap().len(), 2);

    store.StartGCWorker().unwrap();
    assert!(store.gc_worker_started());

    // 相同路径再次 Open 应命中缓存，返回同一 cluster。
    let cached = driver.Open("tikv://pd:2379?keyspaceName=ks").unwrap();
    assert_eq!(cached.GetClusterID(), store.GetClusterID());
    store.Close().unwrap();
    let events_after_first_close = backend.events.lock().unwrap().clone();
    // 第二次 Close 幂等；缓存句柄也应视为已关闭。
    store.Close().unwrap();
    assert!(cached.is_closed());

    let events = backend.events.lock().unwrap().clone();
    assert_eq!(events, events_after_first_close);
    assert_eq!(
        events,
        [
            "open-pd",
            "open-safe-point",
            // Go 在检查缓存前打开 PD，命中缓存后关闭这个多余连接。
            "open-pd",
            "close-pd",
            "close-safe-point",
            "close-pd",
            "close-store",
        ]
    );
}

/// disableGC 时不启动 GC Worker；SetOption(None) 删除动态选项。
#[test]
fn disabled_gc_does_not_start_worker_and_options_delete_on_none() {
    let _guard = global_state_guard();
    let backend = Arc::new(RecordingBackend::default());
    let mut driver = TiKVDriver::with_backend(backend);
    let store = driver.Open("tikv://pd:2379?disableGC=true").unwrap();
    store.StartGCWorker().unwrap();
    assert!(!store.gc_worker_started());

    store.SetOption("answer", Some(42_u64));
    assert_eq!(*store.GetOption::<u64>("answer").unwrap(), 42);
    // None 表示删除该 Option 键。
    store.SetOption::<u64>("answer", None);
    assert!(store.GetOption::<u64>("answer").is_none());
    store.Close().unwrap();
}
