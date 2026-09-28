// 历史统计持久化的单元测试。
//
// 通过可记录调用的存储桩和固定输出的快照桩，验证统计块写入、元信息筛选、
// 分区版本选择以及历史统计开关的边界语义，并保持与 Go 实现的行为一致。

use super::{Error, HistoricalTable, HistoryStore, StatsHistory, StatsSnapshot};
use std::sync::{Arc, Mutex};

#[derive(Debug, PartialEq, Eq)]
/// 一次统计数据块写入的完整参数，用于核对调用顺序与批次共享信息。
struct BlockCall {
    physical_id: i64,
    block: Vec<u8>,
    sequence: usize,
    version: u64,
    timestamp: String,
}

#[derive(Default)]
/// 记录所有存储层交互的测试桩，避免依赖真实数据库。
struct RecordingStore {
    enabled: bool,
    fail_insert_at: Option<usize>,
    meta: Mutex<Vec<(i64, u64, Option<(i64, i64)>)>>,
    meta_queries: Mutex<Vec<(i64, u64)>>,
    replaced: Mutex<Vec<(i64, i64, i64, u64, String)>>,
    blocks: Mutex<Vec<BlockCall>>,
}

impl HistoryStore for RecordingStore {
    fn historical_enabled(&self) -> Result<bool, Error> {
        Ok(self.enabled)
    }

    fn stats_meta(&self, table_id: i64, version: u64) -> Result<Option<(i64, i64)>, Error> {
        self.meta_queries
            .lock()
            .expect("meta queries mutex poisoned")
            .push((table_id, version));
        Ok(self
            .meta
            .lock()
            .expect("meta mutex poisoned")
            .iter()
            .find(|(id, seen_version, _)| *id == table_id && *seen_version == version)
            .and_then(|(_, _, value)| *value))
    }

    fn replace_meta_history(
        &self,
        table_id: i64,
        modify_count: i64,
        count: i64,
        version: u64,
        source: &str,
    ) -> Result<(), Error> {
        self.replaced
            .lock()
            .expect("replaced mutex poisoned")
            .push((table_id, modify_count, count, version, source.to_owned()));
        Ok(())
    }

    fn insert_history_block(
        &self,
        physical_id: i64,
        block: &[u8],
        sequence: usize,
        version: u64,
        timestamp: &str,
    ) -> Result<(), Error> {
        if self.fail_insert_at == Some(sequence) {
            return Err(Error(format!("insert block {sequence} failed")));
        }
        self.blocks
            .lock()
            .expect("blocks mutex poisoned")
            .push(BlockCall {
                physical_id,
                block: block.to_vec(),
                sequence,
                version,
                timestamp: timestamp.to_owned(),
            });
        Ok(())
    }
}

/// 返回预设统计表、数据块和初始化状态的快照桩。
struct FixedSnapshot {
    table: Option<HistoricalTable>,
    blocks: Vec<Vec<u8>>,
    initialized: Vec<i64>,
    dump_error: Option<Error>,
    blocks_error: Option<Error>,
}

impl StatsSnapshot for FixedSnapshot {
    fn dump_stats(
        &self,
        _database: &str,
        _table_id: i64,
        _is_partition: bool,
    ) -> Result<Option<HistoricalTable>, Error> {
        if let Some(error) = &self.dump_error {
            return Err(error.clone());
        }
        Ok(self.table.clone())
    }

    fn table_initialized(&self, table_id: i64) -> bool {
        self.initialized.contains(&table_id)
    }

    fn blocks(&self, _table: &HistoricalTable, _block_size: usize) -> Result<Vec<Vec<u8>>, Error> {
        if let Some(error) = &self.blocks_error {
            return Err(error.clone());
        }
        Ok(self.blocks.clone())
    }
}

/// 构造被测对象，并保留存储桩句柄供测试在调用后检查副作用。
fn history_with(
    store: RecordingStore,
    snapshot: FixedSnapshot,
) -> (StatsHistory, Arc<RecordingStore>) {
    let store = Arc::new(store);
    let history = StatsHistory::new(store.clone(), Arc::new(snapshot));
    (history, store)
}

#[test]
// 分区表应采用所有分区中的最大版本，并让同批数据块共享 Go 兼容的时间戳。
fn record_storage_matches_go_datetime_and_partition_version_semantics() {
    let (history, store) = history_with(
        RecordingStore::default(),
        FixedSnapshot {
            table: Some(HistoricalTable {
                version: 3,
                partition_versions: vec![11, 19, 7],
                encoded: Vec::new(),
            }),
            blocks: vec![vec![1, 2], vec![3]],
            initialized: Vec::new(),
            dump_error: None,
            blocks_error: None,
        },
    );

    let version = history
        .record_historical_stats_to_storage("test", 42, true)
        .expect("record history");
    assert_eq!(version, 19);

    let calls = store.blocks.lock().expect("blocks mutex poisoned");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].physical_id, 42);
    assert_eq!(calls[0].block, vec![1, 2]);
    assert_eq!(calls[0].sequence, 0);
    assert_eq!(calls[0].version, 19);
    assert_eq!(calls[1].sequence, 1);
    assert_eq!(calls[1].block, vec![3]);
    assert_eq!(calls[0].timestamp, calls[1].timestamp);
    assert_eq!(calls[0].timestamp.len(), 26);
    assert_eq!(calls[0].timestamp.as_bytes()[10], b' ');
    assert_eq!(calls[0].timestamp.as_bytes()[19], b'.');
    assert!(calls[0].timestamp[0..10].contains('-'));
}

#[test]
// 快照中没有统计表属于正常空结果，不应产生任何数据块写入。
fn record_storage_returns_zero_when_snapshot_is_absent() {
    let (history, store) = history_with(
        RecordingStore::default(),
        FixedSnapshot {
            table: None,
            blocks: vec![vec![1]],
            initialized: Vec::new(),
            dump_error: None,
            blocks_error: None,
        },
    );

    assert_eq!(
        history
            .record_historical_stats_to_storage("test", 42, false)
            .expect("empty history is not an error"),
        0
    );
    assert!(
        store
            .blocks
            .lock()
            .expect("blocks mutex poisoned")
            .is_empty()
    );
}

#[test]
// 批量记录元信息时仅查询已初始化表；单表缺少元信息不能中断后续处理。
fn record_meta_filters_uninitialized_tables_and_continues_after_missing_meta() {
    let store = RecordingStore {
        enabled: true,
        meta: Mutex::new(vec![(11, 9, Some((4, 5))), (12, 9, None)]),
        ..RecordingStore::default()
    };
    let (history, store) = history_with(
        store,
        FixedSnapshot {
            table: None,
            blocks: Vec::new(),
            initialized: vec![11, 12],
            dump_error: None,
            blocks_error: None,
        },
    );

    history.record_historical_stats_meta(9, "flush stats", false, &[0, 11, 12, 13]);

    assert_eq!(
        *store
            .meta_queries
            .lock()
            .expect("meta queries mutex poisoned"),
        vec![(11, 9), (12, 9)]
    );
    assert_eq!(
        *store.replaced.lock().expect("replaced mutex poisoned"),
        vec![(11, 4, 5, 9, "flush stats".to_owned())]
    );
}

#[test]
// 单表记录入口必须拒绝无效标识与不存在的当前元信息。
fn record_meta_rejects_zero_and_missing_current_meta() {
    let store = RecordingStore {
        meta: Mutex::new(vec![(11, 9, None)]),
        ..RecordingStore::default()
    };

    assert_eq!(
        super::record_historical_stats_meta(&store, 0, "test", 11),
        Err(Error("tableID 11, version 0 are invalid".to_owned()))
    );
    assert_eq!(
        super::record_historical_stats_meta(&store, 9, "test", 0),
        Err(Error("tableID 0, version 9 are invalid".to_owned()))
    );
    assert_eq!(
        super::record_historical_stats_meta(&store, 9, "test", 11),
        Err(Error("no historical meta stats can be recorded".to_owned()))
    );
}

#[test]
// 零版本或关闭历史统计时应在访问元信息存储前直接返回。
fn record_meta_does_nothing_for_zero_version_or_disabled_history() {
    let store = RecordingStore {
        enabled: false,
        meta: Mutex::new(vec![(11, 9, Some((4, 5)))]),
        ..RecordingStore::default()
    };
    let (history, store) = history_with(
        store,
        FixedSnapshot {
            table: None,
            blocks: Vec::new(),
            initialized: vec![11],
            dump_error: None,
            blocks_error: None,
        },
    );

    history.record_historical_stats_meta(0, "flush stats", true, &[11]);
    history.record_historical_stats_meta(9, "flush stats", true, &[11]);

    assert!(
        store
            .meta_queries
            .lock()
            .expect("meta queries mutex poisoned")
            .is_empty()
    );
    assert!(
        store
            .replaced
            .lock()
            .expect("replaced mutex poisoned")
            .is_empty()
    );
    assert!(
        !history
            .check_historical_stats_enable()
            .expect("history switch query")
    );
}

#[test]
// 导出或分块失败必须原样返回，且不能产生部分持久化副作用。
fn record_storage_propagates_snapshot_errors_without_writes() {
    for (dump_error, blocks_error, expected) in [
        (Some(Error("dump failed".to_owned())), None, "dump failed"),
        (
            None,
            Some(Error("blocks failed".to_owned())),
            "blocks failed",
        ),
    ] {
        let (history, store) = history_with(
            RecordingStore::default(),
            FixedSnapshot {
                table: Some(HistoricalTable {
                    version: 7,
                    partition_versions: Vec::new(),
                    encoded: vec![1, 2, 3],
                }),
                blocks: vec![vec![1]],
                initialized: Vec::new(),
                dump_error,
                blocks_error,
            },
        );

        assert_eq!(
            history.record_historical_stats_to_storage("test", 42, false),
            Err(Error(expected.to_owned()))
        );
        assert!(
            store
                .blocks
                .lock()
                .expect("blocks mutex poisoned")
                .is_empty()
        );
    }
}

#[test]
// Go 在首个 INSERT 错误处停止，之前成功写入的块保留、后续块不得执行。
fn record_storage_stops_at_first_insert_error() {
    let store = RecordingStore {
        fail_insert_at: Some(1),
        ..RecordingStore::default()
    };
    let (history, store) = history_with(
        store,
        FixedSnapshot {
            table: Some(HistoricalTable {
                version: 23,
                partition_versions: Vec::new(),
                encoded: Vec::new(),
            }),
            blocks: vec![vec![1], vec![2], vec![3]],
            initialized: Vec::new(),
            dump_error: None,
            blocks_error: None,
        },
    );

    assert_eq!(
        history.record_historical_stats_to_storage("test", 42, false),
        Err(Error("insert block 1 failed".to_owned()))
    );
    let calls = store.blocks.lock().expect("blocks mutex poisoned");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].sequence, 0);
    assert_eq!(calls[0].block, vec![1]);
}
