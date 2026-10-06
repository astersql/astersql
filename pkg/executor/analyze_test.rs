// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// `AnalyzeExec` 主流程与全局统计辅助逻辑的单元测试。
//
// 覆盖并发上限、落盘失败/kill 不挂起、预刷 stats delta 取消语义，
// 以及列收集器内存 tracker 的及时释放。

#![allow(non_snake_case)]

use crate::analyze::{
    AnalyzeError, AnalyzeExec, AnalyzeResultValue, analyzeColumnsExec, analyzeContext, analyzeJob,
    analyzeOptionType, analyzePlan, analyzeResultPart, analyzeResults, analyzeRuntime,
    analyzeTableID, analyzeTask, columnInfo, globalStatsKey, globalStatsMap, handleGlobalStats,
    histogram, killSignal, memoryTracker, statsObject, taskType, v2AnalyzeOptions,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Default)]
/// 测试用 kill 信号：可用 Condvar 唤醒等待方。
struct TestKillSignal {
    killed: AtomicBool,
    wake: Condvar,
    mutex: Mutex<()>,
}

impl TestKillSignal {
    /// 置位并唤醒所有等待者。
    fn kill(&self) {
        self.killed.store(true, Ordering::Release);
        self.wake.notify_all();
    }
}

impl killSignal for TestKillSignal {
    fn wait(&self, context: &analyzeContext) -> Option<AnalyzeError> {
        let mut guard = self.mutex.lock().expect("kill signal mutex poisoned");
        while !self.killed.load(Ordering::Acquire) && context.error().is_none() {
            let (next, _) = self
                .wake
                .wait_timeout(guard, Duration::from_millis(5))
                .expect("kill signal mutex poisoned");
            guard = next;
        }
        self.current_error()
    }

    fn current_error(&self) -> Option<AnalyzeError> {
        self.killed
            .load(Ordering::Acquire)
            .then(|| AnalyzeError("query interrupted".into()))
    }
}

#[derive(Default)]
/// 记录并发活跃数、完成数、广播次数等测试观测指标。
struct RuntimeState {
    active: AtomicUsize,
    max_active: AtomicUsize,
    analyzed: AtomicUsize,
    saved: AtomicUsize,
    broadcasts: AtomicUsize,
    finished: Mutex<Vec<Option<String>>>,
    executed_sql: Mutex<Vec<String>>,
}

/// 实现 `analyzeRuntime` 的可控假运行时。
struct TestRuntime {
    state: Arc<RuntimeState>,
    kill: Arc<TestKillSignal>,
    build_concurrency: usize,
    save_error: Option<AnalyzeError>,
    analyze_delay: Duration,
    persist_options: bool,
    dynamic_partition_prune: bool,
}

impl TestRuntime {
    /// 按给定构建并发度构造默认测试运行时。
    fn new(build_concurrency: usize) -> Self {
        Self {
            state: Arc::new(RuntimeState::default()),
            kill: Arc::new(TestKillSignal::default()),
            build_concurrency,
            save_error: None,
            analyze_delay: Duration::ZERO,
            persist_options: false,
            dynamic_partition_prune: true,
        }
    }

    /// 无锁更新观测到的最大并发活跃任务数。
    fn update_max_active(&self, active: usize) {
        let mut observed = self.state.max_active.load(Ordering::Acquire);
        while active > observed {
            match self.state.max_active.compare_exchange_weak(
                observed,
                active,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
    }
}

impl analyzeRuntime for TestRuntime {
    fn broadcast(&self, _: &analyzeContext, _: &str) -> AnalyzeResultValue {
        self.state.broadcasts.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    fn append_warning(&self, _: AnalyzeError) {}
    fn all_server_rpc_info(
        &self,
        _: &analyzeContext,
    ) -> AnalyzeResultValue<Vec<crate::analyze::serverRPCInfo>> {
        Ok(Vec::new())
    }
    fn dump_stats_delta(&self, _: &[i64]) -> AnalyzeResultValue {
        Ok(())
    }
    fn locked_table_ids(&self, _: &[i64]) -> AnalyzeResultValue<BTreeSet<i64>> {
        Ok(BTreeSet::new())
    }
    fn describe_table_or_partition(&self, _: analyzeTableID) -> Option<String> {
        None
    }
    fn build_stats_concurrency(&self) -> AnalyzeResultValue<usize> {
        Ok(self.build_concurrency)
    }
    fn sampling_stats_concurrency(&self) -> AnalyzeResultValue<usize> {
        Ok(self.build_concurrency)
    }
    fn save_stats_concurrency(&self) -> usize {
        self.build_concurrency
    }
    fn dynamic_partition_prune(&self) -> bool {
        self.dynamic_partition_prune
    }
    fn in_restricted_sql(&self) -> bool {
        false
    }
    fn persist_analyze_options(&self) -> bool {
        self.persist_options
    }
    fn analyze_snapshot_enabled(&self) -> bool {
        false
    }
    fn kill_signal(&self) -> Arc<dyn killSignal> {
        self.kill.clone()
    }
    fn prepare_columns_job(&self, _: &analyzeTask) {}
    fn insert_analyze_job(&self, _: &analyzeJob) -> AnalyzeResultValue {
        Ok(())
    }
    fn start_analyze_job(&self, _: Option<&analyzeJob>) {}
    fn finish_analyze_job(&self, _: Option<&analyzeJob>, error: Option<&AnalyzeError>) {
        self.state
            .finished
            .lock()
            .expect("finished jobs mutex poisoned")
            .push(error.map(|error| error.0.clone()));
    }
    fn analyze_columns(
        &self,
        context: &analyzeContext,
        executor: &analyzeColumnsExec,
    ) -> analyzeResults {
        let active = self.state.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.update_max_active(active);
        let deadline = Instant::now() + self.analyze_delay;
        while Instant::now() < deadline && context.error().is_none() {
            thread::yield_now();
        }
        self.state.active.fetch_sub(1, Ordering::AcqRel);
        self.state.analyzed.fetch_add(1, Ordering::AcqRel);
        analyzeResults {
            error: context.error(),
            tableID: executor.tableID,
            statsVersion: 2,
            parts: vec![analyzeResultPart {
                isIndex: 0,
                histograms: vec![Some(histogram {
                    id: executor.tableID.statisticsID(),
                })],
            }],
            ..Default::default()
        }
    }
    fn analyze_index(
        &self,
        _: &analyzeContext,
        executor: &crate::analyze::analyzeIndexExec,
    ) -> analyzeResults {
        analyzeResults {
            tableID: executor.tableID,
            ..Default::default()
        }
    }
    fn check_killed(&self) -> AnalyzeResultValue {
        self.kill.current_error().map_or(Ok(()), Err)
    }
    fn save_analyze_result(&self, _: &analyzeResults, _: bool) -> AnalyzeResultValue {
        self.state.saved.fetch_add(1, Ordering::AcqRel);
        self.save_error.clone().map_or(Ok(()), Err)
    }
    fn historical_stats_enabled(&self) -> AnalyzeResultValue<bool> {
        Ok(false)
    }
    fn enqueue_historical_stats(&self, _: i64) {}
    fn merge_global_stats(&self, _: &globalStatsMap) -> AnalyzeResultValue {
        Ok(())
    }
    fn execute_internal_sql(&self, sql: &str) -> AnalyzeResultValue {
        self.state
            .executed_sql
            .lock()
            .expect("executed SQL mutex poisoned")
            .push(sql.to_owned());
        Ok(())
    }
    fn update_stats(&self, _: &[i64]) -> AnalyzeResultValue {
        Ok(())
    }
    fn manual_analyze_metric(&self, _: &str) {}
    fn log_warning(&self, _: &str) {}
}

/// 构造列分析任务夹具。
fn column_task(table_id: i64, tracker: Option<Arc<dyn memoryTracker>>) -> Arc<analyzeTask> {
    Arc::new(analyzeTask {
        taskType: taskType::colTask,
        idxExec: None,
        colExec: Some(analyzeColumnsExec {
            tableID: analyzeTableID {
                tableID: table_id,
                partitionID: 0,
            },
            samplingStatsConcurrency: Arc::new(AtomicUsize::new(0)),
            memTracker: tracker,
        }),
        job: Some(analyzeJob {
            id: table_id as u64,
            databaseName: "test".into(),
            tableName: format!("t{table_id}"),
            ..Default::default()
        }),
    })
}

/// 用给定任务列表构造 `AnalyzeExec`。
fn executor(runtime: Arc<dyn analyzeRuntime>, tasks: Vec<Arc<analyzeTask>>) -> AnalyzeExec {
    AnalyzeExec {
        tasks,
        opts: BTreeMap::new(),
        OptionsMap: BTreeMap::<i64, v2AnalyzeOptions>::new(),
        errExitCh: Arc::new(AtomicBool::new(false)),
        runtime,
    }
}

#[test]
/// 索引结果写入全局统计映射时应按直方图 ID 拆分键。
fn TestAnalyzeIndexExtractTopN() {
    let mut global = globalStatsMap::new();
    let result = analyzeResults {
        tableID: analyzeTableID {
            tableID: 7,
            partitionID: 71,
        },
        statsVersion: 2,
        parts: vec![analyzeResultPart {
            isIndex: 1,
            histograms: vec![Some(histogram { id: 11 }), None, Some(histogram { id: 13 })],
        }],
        ..Default::default()
    };
    handleGlobalStats(true, &mut global, &result);
    assert_eq!(
        global[&globalStatsKey {
            tableID: 7,
            indexID: 11
        }]
            .histogramIDs,
        vec![11]
    );
    assert_eq!(
        global[&globalStatsKey {
            tableID: 7,
            indexID: 13
        }]
            .histogramIDs,
        vec![13]
    );
}

#[test]
/// 动态分区模式下构建并发上限应约束实际活跃 worker 数。
fn TestAnalyzePartitionTableByConcurrencyInDynamic() {
    for concurrency in 1..=5 {
        let runtime = Arc::new(TestRuntime {
            analyze_delay: Duration::from_millis(10),
            ..TestRuntime::new(concurrency)
        });
        let tasks = (1..=10).map(|id| column_task(id, None)).collect();
        executor(runtime.clone(), tasks)
            .Next(analyzeContext::default())
            .unwrap();
        assert_eq!(runtime.state.analyzed.load(Ordering::Acquire), 10);
        assert_eq!(runtime.state.saved.load(Ordering::Acquire), 10);
        assert!(runtime.state.max_active.load(Ordering::Acquire) <= concurrency);
    }
}

#[test]
/// 落盘失败必须尽快返回错误，不得死锁挂起。
fn TestAnalyzeSaveResultErrorDoesNotHang() {
    let runtime = Arc::new(TestRuntime {
        save_error: Some(AnalyzeError("mock save analyze result error".into())),
        ..TestRuntime::new(1)
    });
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = executor(runtime, vec![column_task(1, None)]).Next(analyzeContext::default());
        sender.send(result).unwrap();
    });
    let error = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("analyze save failure must not hang")
        .unwrap_err();
    assert_eq!(error.0, "mock save analyze result error");
}

#[test]
/// 预刷 stats delta 前若上下文已取消，应直接失败且不广播。
fn TestBuildAnalyzePreFlushUsesStatementContext() {
    let runtime = TestRuntime::new(1);
    let context = analyzeContext::default();
    context.cancel(AnalyzeError("context canceled".into()));
    let plan = analyzePlan {
        columnTasks: vec![crate::analyze::analyzeColumnsPlanTask {
            databaseName: "test".into(),
            tableName: "t".into(),
            ..Default::default()
        }],
    };
    let error = crate::analyze::flushStatsDeltaForAnalyze(&context, &runtime, &plan).unwrap_err();
    assert_eq!(error.0, "context canceled");
    assert_eq!(runtime.state.broadcasts.load(Ordering::Acquire), 0);
}

#[test]
/// 分析过程中 kill 必须可中断且不挂起。
fn TestAnalyzeKillDuringSaveDoesNotHang() {
    let runtime = Arc::new(TestRuntime {
        analyze_delay: Duration::from_secs(10),
        ..TestRuntime::new(1)
    });
    let kill = runtime.kill.clone();
    let state = runtime.state.clone();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = executor(runtime, vec![column_task(1, None)]).Next(analyzeContext::default());
        sender.send(result).unwrap();
    });
    while state.active.load(Ordering::Acquire) == 0 {
        thread::yield_now();
    }
    kill.kill();
    let error = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("killed analyze must not hang")
        .unwrap_err();
    assert_eq!(error.0, "query interrupted");
    assert!(
        state
            .finished
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .any(|error| error == "query interrupted")
    );
}

#[derive(Default)]
/// 统计 `detach` 调用次数的内存 tracker。
struct CountingTracker(AtomicUsize);

impl memoryTracker for CountingTracker {
    fn detach(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
/// Next 结束后应立即 detach 列收集器内存 tracker。
fn TestAnalyzeV2ReleaseColumnCollectorMemoryImmediately() {
    let tracker = Arc::new(CountingTracker::default());
    let runtime = Arc::new(TestRuntime::new(1));
    executor(runtime, vec![column_task(1, Some(tracker.clone()))])
        .Next(analyzeContext::default())
        .unwrap();
    assert_eq!(tracker.0.load(Ordering::Acquire), 1);
}

#[test]
/// v2 选项应保留列 ID 列表与 stats 对象表名。
fn analyze_options_keep_column_identity() {
    let options = v2AnalyzeOptions {
        physicalTableID: 9,
        columnChoice: "LIST".into(),
        columnList: vec![columnInfo { id: 2 }, columnInfo { id: 5 }],
        ..Default::default()
    };
    assert_eq!(
        options.columnList.iter().map(|c| c.id).collect::<Vec<_>>(),
        [2, 5]
    );
    let object = statsObject {
        databaseName: "test".into(),
        tableName: "t".into(),
    };
    assert_eq!(object.tableName, "t");
}

#[test]
/// Go 在动态裁剪模式仅保存表级选项，静态模式则同时保存分区选项。
fn save_analyze_options_matches_go_partition_filtering() {
    let options = BTreeMap::from([
        (
            10,
            v2AnalyzeOptions {
                physicalTableID: 10,
                ..Default::default()
            },
        ),
        (
            11,
            v2AnalyzeOptions {
                physicalTableID: 11,
                isPartition: true,
                ..Default::default()
            },
        ),
    ]);

    for (dynamic_partition_prune, partition_is_saved) in [(true, false), (false, true)] {
        let runtime = Arc::new(TestRuntime {
            persist_options: true,
            dynamic_partition_prune,
            ..TestRuntime::new(1)
        });
        let mut analyze = executor(runtime.clone(), Vec::new());
        analyze.OptionsMap = options.clone();
        analyze.saveAnalyzeOptions().unwrap();

        let sql = runtime
            .state
            .executed_sql
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap();
        assert!(sql.contains("(10,"));
        assert_eq!(sql.contains("(11,"), partition_is_saved);
    }
}

#[test]
/// Missing raw options must use the mysql.analyze_options column defaults.
fn save_analyze_options_uses_defaults_for_unset_values() {
    let runtime = Arc::new(TestRuntime {
        persist_options: true,
        ..TestRuntime::new(1)
    });
    let mut analyze = executor(runtime.clone(), Vec::new());
    analyze.OptionsMap.insert(
        10,
        v2AnalyzeOptions {
            physicalTableID: 10,
            rawOptions: BTreeMap::from([(analyzeOptionType::NumTopN, 0)]),
            ..Default::default()
        },
    );

    analyze.saveAnalyzeOptions().unwrap();

    let sql = runtime
        .state
        .executed_sql
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(
        sql,
        "REPLACE INTO mysql.analyze_options (table_id,sample_num,sample_rate,buckets,topn,column_choice,column_ids) VALUES (10,DEFAULT,DEFAULT,DEFAULT,0,'','')"
    );
}

#[test]
fn save_analyze_options_resets_dynamic_partition_overrides() {
    let runtime = Arc::new(TestRuntime {
        persist_options: true,
        dynamic_partition_prune: true,
        ..TestRuntime::new(1)
    });
    let mut analyze = executor(runtime.clone(), Vec::new());
    analyze.OptionsMap = BTreeMap::from([
        (
            10,
            v2AnalyzeOptions {
                physicalTableID: 10,
                resetOptions: BTreeSet::from([
                    analyzeOptionType::NumBuckets,
                    analyzeOptionType::NumTopN,
                ]),
                ..Default::default()
            },
        ),
        (
            11,
            v2AnalyzeOptions {
                physicalTableID: 11,
                isPartition: true,
                ..Default::default()
            },
        ),
        (
            12,
            v2AnalyzeOptions {
                physicalTableID: 12,
                isPartition: true,
                ..Default::default()
            },
        ),
    ]);

    analyze.saveAnalyzeOptions().unwrap();

    let sql = runtime.state.executed_sql.lock().unwrap().clone();
    assert_eq!(sql.len(), 2);
    assert_eq!(
        sql[1],
        "UPDATE mysql.analyze_options SET buckets=DEFAULT,topn=DEFAULT WHERE table_id IN (11,12)"
    );
}
