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

// 同步加载（syncload）单元测试。
//
// 用 MockStorage/MockHandle 覆盖并发加载、超时、panic/失败重试、
// 通道拥塞与存储缺失对象等场景。

use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
/// 可注入失败/panic 的内存存储 mock。
struct MockStorage {
    fail_next: AtomicUsize,
    panic_next: AtomicUsize,
    hist_meta: Mutex<HashMap<i64, (Histogram, i64)>>,
    hist: Mutex<HashMap<i64, Histogram>>,
    cms_topn: Mutex<HashMap<i64, (Option<CmsSketch>, Option<TopN>)>>,
}

impl MockStorage {
    /// 为指定列写入直方图元数据、桶与 TopN 样例。
    fn seed_column(&self, column_id: i64) {
        let hist = Histogram {
            ndv: 3,
            null_count: 0,
            buckets: vec![
                (b"1".to_vec(), b"1".to_vec(), 1),
                (b"2".to_vec(), b"2".to_vec(), 1),
                (b"3".to_vec(), b"3".to_vec(), 1),
            ],
            ..Default::default()
        };
        self.hist_meta
            .lock()
            .unwrap()
            .insert(column_id, (hist.clone(), 2));
        self.hist.lock().unwrap().insert(column_id, hist);
        self.cms_topn.lock().unwrap().insert(
            column_id,
            (
                Some(CmsSketch::default()),
                Some(TopN {
                    values: vec![(b"1".to_vec(), 1)],
                }),
            ),
        );
    }

    /// 原子递减计数器；计数 >0 时返回 true（触发一次注入行为）。
    /// 原子计数减一；已为 0 则返回 false（未触发注入）。
    fn take_counter(counter: &AtomicUsize) -> bool {
        loop {
            let current = counter.load(Ordering::SeqCst);
            if current == 0 {
                return false;
            }
            if counter
                .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// 是否消耗一次 panic 注入配额。
    fn take_panic(&self) -> bool {
        Self::take_counter(&self.panic_next)
    }

    /// 是否消耗一次失败注入配额。
    fn take_fail(&self) -> bool {
        Self::take_counter(&self.fail_next)
    }
}

/// MockStorage 实现 StatsStorage：按注入策略返回或 panic。
impl StatsStorage for MockStorage {
    fn HistMetaFromStorageWithHighPriority(
        &self,
        item: TableItemID,
        _column_info: Option<&ColumnInfo>,
    ) -> Result<Option<(Histogram, i64)>> {
        if self.take_panic() {
            panic!("mockReadStatsForOnePanic");
        }
        if self.take_fail() {
            return Err(Error::ChannelClosed("mockReadStatsForOneFail"));
        }
        Ok(self.hist_meta.lock().unwrap().get(&item.ID).cloned())
    }

    fn HistogramFromStorageWithHighPriority(
        &self,
        item: TableItemID,
        _column_info: Option<&ColumnInfo>,
        metadata: &Histogram,
    ) -> Result<Histogram> {
        Ok(self
            .hist
            .lock()
            .unwrap()
            .get(&item.ID)
            .cloned()
            .unwrap_or_else(|| metadata.clone()))
    }

    fn CMSketchAndTopNFromStorageWithHighPriority(
        &self,
        item: TableItemID,
        _stats_version: i64,
    ) -> Result<(Option<CmsSketch>, Option<TopN>)> {
        Ok(self
            .cms_topn
            .lock()
            .unwrap()
            .get(&item.ID)
            .cloned()
            .unwrap_or((None, None)))
    }
}

/// 内存表缓存 + MockStorage 的 StatsHandle 实现。
struct MockHandle {
    tables: Mutex<HashMap<i64, TableStats>>,
    infos: Mutex<HashMap<i64, TableInfo>>,
    storage: MockStorage,
}

impl MockHandle {
    /// 构造已标记 analyzed 的表信息，并为各列 seed 存储数据。
    fn new(table_id: i64, column_ids: &[i64]) -> Arc<Self> {
        let mut columns = HashMap::new();
        let mut analyzed = std::collections::HashSet::new();
        for &id in column_ids {
            columns.insert(
                id,
                ColumnInfo {
                    id,
                    field_type: "long".into(),
                    primary_key: id == column_ids[0],
                },
            );
            analyzed.insert(id);
        }
        let info = TableInfo {
            pk_is_handle: true,
            columns,
            indices: HashMap::new(),
        };
        let stats = TableStats {
            analyzed_columns: analyzed,
            ..Default::default()
        };
        let storage = MockStorage::default();
        for &id in column_ids {
            storage.seed_column(id);
        }
        Arc::new(Self {
            tables: Mutex::new(HashMap::from([(table_id, stats)])),
            infos: Mutex::new(HashMap::from([(table_id, info)])),
            storage,
        })
    }
}

/// 读写内存中的 TableStats / TableInfo。
impl StatsHandle for MockHandle {
    fn Get(&self, table_id: i64) -> Option<TableStats> {
        self.tables.lock().unwrap().get(&table_id).cloned()
    }

    fn TableInfoByID(&self, table_id: i64) -> Option<TableInfo> {
        self.infos.lock().unwrap().get(&table_id).cloned()
    }

    fn UpdateStatsCache(&self, table_id: i64, table: TableStats) -> Result<()> {
        self.tables.lock().unwrap().insert(table_id, table);
        Ok(())
    }

    fn Storage(&self) -> &dyn StatsStorage {
        &self.storage
    }

    fn Lease(&self) -> Duration {
        Duration::from_millis(1)
    }
}

/// 构造指定列的 FullLoad 请求列表。
/// 构造一批需要全量加载的列 `StatsLoadItem`。
fn needed_columns(table_id: i64, column_ids: &[i64]) -> Vec<StatsLoadItem> {
    column_ids
        .iter()
        .map(|&id| StatsLoadItem {
            TableItemID: TableItemID {
                TableID: table_id,
                ID: id,
                IsIndex: false,
            },
            FullLoad: true,
        })
        .collect()
}

/// 桶数 + TopN 条数，用于断言完整加载成功。
fn hist_topn_len(col: &Column) -> usize {
    col.histogram.buckets.len() + col.top_n.as_ref().map(|t| t.values.len()).unwrap_or(0)
}

/// 在 scope 内启动 worker 循环，直到 SyncWaitStatsLoad 返回后置 exit。
fn drain_until_wait(sync: &statsSyncLoad, stmt: &mut StatementContext, exit: &AtomicBool) {
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !exit.load(Ordering::Acquire) {
                match sync.HandleOneTask(None, exit) {
                    Ok(_) => {}
                    Err(Error::Exit) => break,
                    Err(_) => {}
                }
            }
        });
        sync.SyncWaitStatsLoad(stmt).unwrap();
        exit.store(true, Ordering::Release);
    });
}

#[test]
/// 并发加载多列直方图，断言目标列已写入缓存且含桶/TopN。
fn TestConcurrentLoadHist() {
    let table_id = 100;
    let column_ids = [1_i64, 2, 3];
    let handle = MockHandle::new(table_id, &column_ids);
    let sync = NewStatsSyncLoad(handle.clone(), 64);
    let exit = AtomicBool::new(false);

    let mut stmt = StatementContext::default();
    let items = needed_columns(table_id, &column_ids);
    sync.SendLoadRequests(&mut stmt, &items, Duration::from_secs(5))
        .unwrap();
    drain_until_wait(&sync, &mut stmt, &exit);

    let stat = handle.Get(table_id).unwrap();
    let col = stat.columns.get(&3).expect("column c loaded");
    assert!(hist_topn_len(col) > 0);
}

#[test]
/// Timeout=0 时 SyncWaitStatsLoad 应立即返回 Timeout。
fn TestConcurrentLoadHistTimeout() {
    let table_id = 101;
    let column_ids = [1_i64, 2, 3];
    let handle = MockHandle::new(table_id, &column_ids);
    let sync = NewStatsSyncLoad(handle.clone(), 64);

    let mut stmt = StatementContext::default();
    let items = needed_columns(table_id, &column_ids);
    sync.SendLoadRequests(&mut stmt, &items, Duration::ZERO)
        .unwrap();
    let err = sync.SyncWaitStatsLoad(&mut stmt).unwrap_err();
    assert!(matches!(err, Error::Timeout));
}

#[test]
/// 首次 panic/失败可重试，第二次成功后两个 waiter 均收到无错结果。
fn TestConcurrentLoadHistWithPanicAndFail() {
    let table_id = 102;
    let column_ids = [1_i64, 2, 3];
    let handle = MockHandle::new(table_id, &column_ids);
    let sync = NewStatsSyncLoad(handle.clone(), 64);
    let exit = AtomicBool::new(false);
    let items = needed_columns(table_id, &[3]);
    let timeout = Duration::from_secs(5);

    // 分别注入 panic 与 fail 各一轮，验证 singleflight 等待与重试。
    for mode in ["panic", "fail"] {
        {
            let mut tables = handle.tables.lock().unwrap();
            tables.insert(
                table_id,
                TableStats {
                    analyzed_columns: column_ids.iter().copied().collect(),
                    ..Default::default()
                },
            );
        }
        if mode == "panic" {
            handle.storage.panic_next.store(1, Ordering::SeqCst);
        } else {
            handle.storage.fail_next.store(1, Ordering::SeqCst);
        }

        let mut stmt1 = StatementContext::default();
        let mut stmt2 = StatementContext::default();
        sync.SendLoadRequests(&mut stmt1, &items, timeout).unwrap();
        sync.SendLoadRequests(&mut stmt2, &items, timeout).unwrap();

        let task1 = sync.HandleOneTask(None, &exit).unwrap();
        assert!(task1.is_some(), "failed load should be retriable once");
        for receiver in stmt1.StatsLoad.ResultCh() {
            assert!(receiver.try_recv().is_err());
        }
        for receiver in stmt2.StatsLoad.ResultCh() {
            assert!(receiver.try_recv().is_err());
        }

        handle.storage.panic_next.store(0, Ordering::SeqCst);
        handle.storage.fail_next.store(0, Ordering::SeqCst);
        let task3 = sync.HandleOneTask(task1, &exit).unwrap();
        assert!(task3.is_none());

        for receiver in stmt1.StatsLoad.ResultCh() {
            let result = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(result.Item.ID, 3);
            assert!(result.Error.is_none());
        }
        for receiver in stmt2.StatsLoad.ResultCh() {
            let result = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(result.Item.ID, 3);
            assert!(result.Error.is_none());
        }

        let stat = handle.Get(table_id).unwrap();
        let col = stat.columns.get(&3).expect("column c loaded after retry");
        assert!(hist_topn_len(col) > 0);
    }
}

#[test]
/// 连续失败超过 RetryCount 后任务结束并向 waiter 返回错误。
fn TestRetry() {
    let table_id = 103;
    let column_ids = [1_i64, 2, 3];
    let handle = MockHandle::new(table_id, &column_ids);
    let sync = NewStatsSyncLoad(handle.clone(), 64);
    let exit = AtomicBool::new(false);
    let items = needed_columns(table_id, &[3]);
    let timeout = Duration::from_secs(5);

    handle.storage.fail_next.store(100, Ordering::SeqCst);
    let mut stmt1 = StatementContext::default();
    sync.SendLoadRequests(&mut stmt1, &items, timeout).unwrap();

    let mut task1 = None;
    for _ in 0..RetryCount {
        let task = sync.HandleOneTask(task1.take(), &exit).unwrap();
        assert!(task.is_some());
        task1 = task;
    }
    let result = sync.HandleOneTask(task1, &exit).unwrap();
    assert!(result.is_none());
    for receiver in stmt1.StatsLoad.ResultCh() {
        let result = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(result.Error.is_some());
    }
}

#[test]
/// 无 worker 时，singleflight leader 自行超时并向每个结果通道返回错误。
fn TestSendLoadRequestsWaitTooLong() {
    let table_id = 104;
    let column_ids = [1_i64, 2, 3];
    let handle = MockHandle::new(table_id, &column_ids);
    // 与 Go 回归保持一致：队列足够容纳任务，但没有 worker 返回结果。
    let sync = NewStatsSyncLoad(handle.clone(), 10_000);
    let items = needed_columns(table_id, &column_ids);
    let timeout = Duration::from_millis(20);

    let mut stmt = StatementContext::default();
    sync.SendLoadRequests(&mut stmt, &items, timeout).unwrap();
    for receiver in stmt.StatsLoad.ResultCh() {
        let result = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("leader must publish its own timeout");
        assert_eq!(
            result.Error.as_deref(),
            Some("sync load took too long to return")
        );
    }

    let mut stmt1 = StatementContext::default();
    sync.SendLoadRequests(&mut stmt1, &items, timeout).unwrap();
    for receiver in stmt1.StatsLoad.ResultCh() {
        let result = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("timed-out singleflight key must be reusable");
        assert_eq!(
            result.Error.as_deref(),
            Some("sync load took too long to return")
        );
    }
}

#[test]
/// Go 的容量 0 队列是无缓冲通道；无 worker 时应在发送阶段超时。
fn zero_capacity_queue_preserves_unbuffered_send_timeout() {
    let table_id = 106;
    let handle = MockHandle::new(table_id, &[1]);
    let sync = NewStatsSyncLoad(handle, 0);
    let mut stmt = StatementContext::default();

    sync.SendLoadRequests(
        &mut stmt,
        &needed_columns(table_id, &[1]),
        Duration::from_millis(20),
    )
    .unwrap();

    let result = stmt.StatsLoad.ResultCh()[0]
        .recv_timeout(Duration::from_secs(1))
        .expect("leader must publish the send timeout");
    assert_eq!(
        result.Error.as_deref(),
        Some("sync load stats channel is full and timeout sending task to channel")
    );
    assert_eq!(sync.metrics().2, 0, "only successful enqueues are counted");
}

#[test]
/// 存储缺失且未 analyzed 的列不应被标记为仍需加载。
fn TestSyncLoadOnObjectWhichCanNotFoundInStorage() {
    let table_id = 105;
    let column_ids = [1_i64, 2, 3, 4];
    let handle = MockHandle::new(table_id, &column_ids);
    handle.storage.hist_meta.lock().unwrap().remove(&3);
    handle.storage.hist.lock().unwrap().remove(&3);
    handle.storage.cms_topn.lock().unwrap().remove(&3);
    {
        let mut tables = handle.tables.lock().unwrap();
        let stats = tables.get_mut(&table_id).unwrap();
        stats.analyzed_columns = [1_i64, 2, 4].into_iter().collect();
    }

    let sync = NewStatsSyncLoad(handle.clone(), 64);
    let exit = AtomicBool::new(false);
    let items = needed_columns(table_id, &column_ids);
    let mut stmt = StatementContext::default();
    sync.SendLoadRequests(&mut stmt, &items, Duration::from_secs(5))
        .unwrap();
    drain_until_wait(&sync, &mut stmt, &exit);

    let stats = handle.Get(table_id).unwrap();
    assert!(stats.columns.get(&1).is_some());
    assert!(stats.columns.get(&2).is_some());
    assert!(stats.columns.get(&4).is_some());
    let (_col, load_needed, analyzed) = stats.ColumnIsLoadNeeded(3, false);
    assert!(!load_needed);
    assert!(!analyzed);
}

#[test]
/// 并发度建议值落在 5..=10。
fn GetSyncLoadConcurrencyByCPU_is_stable() {
    let concurrency = GetSyncLoadConcurrencyByCPU();
    assert!((5..=10).contains(&concurrency));
}

#[test]
/// isVaildForRetry 递增计数并在超过 RetryCount 后返回 false。
fn isVaildForRetry_respects_retry_count() {
    let (result_sender, _result_receiver) = std::sync::mpsc::sync_channel(1);
    let mut task = NeededItemTask {
        Item: StatsLoadItem {
            TableItemID: TableItemID {
                TableID: 1,
                ID: 1,
                IsIndex: false,
            },
            FullLoad: true,
        },
        ToTimeout: std::time::Instant::now(),
        ResultCh: result_sender,
        Retry: 0,
    };
    assert!(isVaildForRetry(&mut task));
    assert_eq!(task.Retry, 1);
    assert!(!isVaildForRetry(&mut task));
}
