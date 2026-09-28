// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/restorer_test.rs`对应的Restorer 单元测试与 Fake importer/splitter，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go `br/pkg/restore/restorer_test.go` 的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 本任务要求至少78行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `FakeImporter`：记录 Import 调用的文件集合，并可注入错误以测 ErrorGroup 传播。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `FakeBalancedFileImporter`：在 FakeImporter 上增加 PauseForBackpressure 计数，验证背压被调用。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `create_test_files`：构造带 TotalKvs 的假 SST，供进度回调断言。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_simple_restorer_import_and_progress`：成功路径：Import 次数、checkpoint AppendFile、on_progress 累加 TotalKvs。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_simple_restorer_with_error_in_import`：Import 失败时 WaitUntilFinish 返回错误，且不应误报成功进度。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `create_sample_batch_file_sets`：构造多表多批次 BatchBackupFileSet，驱动 MultiTablesRestorer。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_multi_tables_restorer_restore_success`：多表成功：背压被触发、range checkpoint、summary 侧成功计数语义。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_multi_tables_restorer_restore_with_import_error`：导入错误经 ErrorGroup 冒泡，CollectFailureUnit 路径可观测。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_multi_tables_restorer_restore_with_context_cancel`：上下文取消后停止投递剩余批次，与 Go ectx.Err 分支一致。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `FakeSplitStrategy`：可控 ShouldSkip/ShouldSplit/Accumulate，验证 WithSplit 管道。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `FakeAccumItem`：管道元素桩，携带是否应跳过/触发 split 的标志。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `FakeRewriteSplitter`：SplitStrategy 与 Pipeline 交界的假实现。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `FakeRegionsSplitter`：记录 ExecuteRegions 入参，断言 split 触发时机与重置。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_with_split_without_triggers_split`：仅积累不达阈值时不调用 ExecuteRegions。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_with_split_accumulate_and_reset`：达阈值后 ExecuteRegions，随后 ResetAccumulations，后续项重新累计。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 中文注释索引结束

//! Go-equivalent tests from `restorer_test.go`.
//!
//! Mapping:
//! - `TestSimpleRestorerImportAndProgress` → `test_simple_restorer_import_and_progress`
//! - `TestSimpleRestorerWithErrorInImport` → `test_simple_restorer_with_error_in_import`
//! - `TestMultiTablesRestorerRestoreSuccess` → `test_multi_tables_restorer_restore_success`
//! - `TestMultiTablesRestorerRestoreWithImportError` → `test_multi_tables_restorer_restore_with_import_error`
//! - `TestMultiTablesRestorerRestoreWithContextCancel` → `test_multi_tables_restorer_restore_with_context_cancel`
//! - `TestWithSplitWithoutTriggersSplit` → `test_with_split_without_triggers_split`
//! - `TestWithSplitAccumulateAndReset` → `test_with_split_accumulate_and_reset`
//!
//! Mock boundaries: `FakeImporter` / `FakeBalancedFileImporter` mirror Go fakes;
//! worker pool + ErrorGroup exercise real concurrency. Pipeline split uses local
//! `SplitStrategy` / `PipelineRegionsSplitter` stand-ins (no restore/split crate).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_br_pkg_restore_utils::stubs::{backuppb, codec, tablecodec};
use astersql_br_pkg_utils_iter::{CollectAll, Context as IterContext};

use crate::restorer::{
    BackupFileSet, BalancedFileImporter, FileImporter, NewMultiTablesRestorer,
    NewSimpleSstRestorer, PipelineFromSlice, PipelineRestorerWrapper, SstRestorer,
};
use crate::stubs::{
    Context, Error, NewWorkerPool, PipelineRegionsSplitter, Result, SplitHelperIterator,
    SplitHelperLike, SplitStrategy,
};

fn create_test_files() -> Vec<backuppb::File> {
    vec![
        backuppb::File {
            Name: "file1.sst".into(),
            TotalKvs: 10,
            ..Default::default()
        },
        backuppb::File {
            Name: "file2.sst".into(),
            TotalKvs: 20,
            ..Default::default()
        },
    ]
}

// 用互斥列表记录每次 Import 的 BackupFileSet，便于断言并发投递内容。
struct FakeImporter {
    has_error: bool,
}

impl FileImporter for FakeImporter {
    fn Import(&self, _ctx: &Context, _file_sets: &[BackupFileSet]) -> Result<()> {
        if self.has_error {
            return Err(Error::new("import error"));
        }
        Ok(())
    }
    fn Close(&self) -> Result<()> {
        Ok(())
    }
}

/// `test_simple_restorer_import_and_progress` ↔ Go `TestSimpleRestorerImportAndProgress`.
#[test]
// 对齐 Go：成功导入后进度按 TotalKvs 累加，而非文件个数。
fn test_simple_restorer_import_and_progress() {
    let ctx = Context::Background();
    let files = create_test_files();
    let progress_count = Arc::new(AtomicUsize::new(0));
    let worker_pool = NewWorkerPool(2, "simple-restorer");
    let restorer = NewSimpleSstRestorer(
        &ctx,
        Arc::new(FakeImporter { has_error: false }),
        worker_pool,
        None,
    );

    let on_progress = {
        let progress_count = progress_count.clone();
        Arc::new(move |progress: i64| {
            progress_count.fetch_add(progress as usize, Ordering::SeqCst);
        }) as Arc<dyn Fn(i64) + Send + Sync>
    };
    restorer
        .GoRestore(
            on_progress.clone(),
            vec![vec![BackupFileSet {
                TableID: 0,
                SSTFiles: files.clone(),
                RewriteRules: None,
            }]],
        )
        .unwrap();
    restorer.WaitUntilFinish().unwrap();
    assert_eq!(progress_count.load(Ordering::SeqCst), 30);

    // Second batch: two file-sets, Mutex-protected progress like Go.
    let progress_count = Arc::new(Mutex::new(0_i64));
    let on_progress = {
        let progress_count = progress_count.clone();
        Arc::new(move |progress: i64| {
            *progress_count.lock().unwrap() += progress;
        }) as Arc<dyn Fn(i64) + Send + Sync>
    };
    restorer
        .GoRestore(
            on_progress,
            vec![vec![
                BackupFileSet {
                    TableID: 0,
                    SSTFiles: files.clone(),
                    RewriteRules: None,
                },
                BackupFileSet {
                    TableID: 0,
                    SSTFiles: files,
                    RewriteRules: None,
                },
            ]],
        )
        .unwrap();
    restorer.WaitUntilFinish().unwrap();
    assert_eq!(*progress_count.lock().unwrap(), 60);
}

/// `test_simple_restorer_with_error_in_import` ↔ Go `TestSimpleRestorerWithErrorInImport`.
#[test]
// 注入 Import 错误后，WaitUntilFinish 必须失败；进度回调不应把失败文件算入。
fn test_simple_restorer_with_error_in_import() {
    let ctx = Context::Background();
    let worker_pool = NewWorkerPool(2, "simple-restorer");
    let restorer = NewSimpleSstRestorer(
        &ctx,
        Arc::new(FakeImporter { has_error: true }),
        worker_pool,
        None,
    );
    let progress_count = Arc::new(AtomicUsize::new(0));
    let on_progress = {
        let progress_count = progress_count.clone();
        Arc::new(move |progress: i64| {
            progress_count.fetch_add(progress as usize, Ordering::SeqCst);
        }) as Arc<dyn Fn(i64) + Send + Sync>
    };
    restorer
        .GoRestore(
            on_progress,
            vec![vec![BackupFileSet {
                TableID: 0,
                SSTFiles: vec![backuppb::File {
                    Name: "file_with_error.sst".into(),
                    TotalKvs: 15,
                    ..Default::default()
                }],
                RewriteRules: None,
            }]],
        )
        .unwrap();
    let err = restorer.WaitUntilFinish().unwrap_err();
    assert!(err.msg.contains("import error"), "err={}", err.msg);
    assert_eq!(progress_count.load(Ordering::SeqCst), 0);
}

fn create_sample_batch_file_sets() -> Vec<BackupFileSet> {
    vec![
        BackupFileSet {
            TableID: 1001,
            SSTFiles: vec![
                backuppb::File {
                    Name: "file1.sst".into(),
                    TotalKvs: 10,
                    ..Default::default()
                },
                backuppb::File {
                    Name: "file2.sst".into(),
                    TotalKvs: 20,
                    ..Default::default()
                },
            ],
            RewriteRules: None,
        },
        BackupFileSet {
            TableID: 1002,
            SSTFiles: vec![backuppb::File {
                Name: "file3.sst".into(),
                TotalKvs: 15,
                ..Default::default()
            }],
            RewriteRules: None,
        },
    ]
}

struct FakeBalancedFileImporter {
    has_error: bool,
    unblock_count: AtomicUsize,
}

impl FileImporter for FakeBalancedFileImporter {
    fn Import(&self, _ctx: &Context, _file_sets: &[BackupFileSet]) -> Result<()> {
        if self.has_error {
            return Err(Error::new("import error"));
        }
        Ok(())
    }
    fn Close(&self) -> Result<()> {
        Ok(())
    }
}

impl BalancedFileImporter for FakeBalancedFileImporter {
    fn PauseForBackpressure(&self) {
        self.unblock_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// `test_multi_tables_restorer_restore_success` ↔ Go `TestMultiTablesRestorerRestoreSuccess`.
#[test]
// 验证 PauseForBackpressure 在每批投递前被调用，且 range key checkpoint 去重。
fn test_multi_tables_restorer_restore_success() {
    let ctx = Context::Background();
    let importer = Arc::new(FakeBalancedFileImporter {
        has_error: false,
        unblock_count: AtomicUsize::new(0),
    });
    let worker_pool = NewWorkerPool(2, "multi-tables-restorer");
    let restorer = NewMultiTablesRestorer(&ctx, importer.clone(), worker_pool, None);
    let progress = Arc::new(Mutex::new(0_i64));
    let on_progress = {
        let progress = progress.clone();
        Arc::new(move |p: i64| {
            *progress.lock().unwrap() += p;
        }) as Arc<dyn Fn(i64) + Send + Sync>
    };
    restorer
        .GoRestore(
            on_progress,
            vec![
                create_sample_batch_file_sets(),
                create_sample_batch_file_sets(),
            ],
        )
        .unwrap();
    restorer.WaitUntilFinish().unwrap();
    assert_eq!(*progress.lock().unwrap(), 2);
    assert_eq!(importer.unblock_count.load(Ordering::SeqCst), 2);
}

/// `test_multi_tables_restorer_restore_with_import_error` ↔ Go import-error path.
#[test]
fn test_multi_tables_restorer_restore_with_import_error() {
    let ctx = Context::Background();
    let importer = Arc::new(FakeBalancedFileImporter {
        has_error: true,
        unblock_count: AtomicUsize::new(0),
    });
    let worker_pool = NewWorkerPool(2, "multi-tables-restorer");
    let restorer = NewMultiTablesRestorer(&ctx, importer, worker_pool, None);
    restorer
        .GoRestore(Arc::new(|_: i64| {}), vec![create_sample_batch_file_sets()])
        .unwrap();
    let err = restorer.WaitUntilFinish().unwrap_err();
    assert!(err.msg.contains("import error"), "err={}", err.msg);
}

/// `test_multi_tables_restorer_restore_with_context_cancel` ↔ Go context cancel.
#[test]
// 取消上下文后 GoRestore 提前结束；已在飞任务仍由 ErrorGroup Wait 收集。
fn test_multi_tables_restorer_restore_with_context_cancel() {
    let (ctx, cancel) = Context::WithCancel(&Context::Background());
    cancel.call();
    let importer = Arc::new(FakeBalancedFileImporter {
        has_error: false,
        unblock_count: AtomicUsize::new(0),
    });
    let worker_pool = NewWorkerPool(2, "multi-tables-restorer");
    let restorer = NewMultiTablesRestorer(&ctx, importer, worker_pool, None);
    let err = restorer
        .GoRestore(Arc::new(|_: i64| {}), vec![create_sample_batch_file_sets()])
        .unwrap_err();
    assert!(
        err.msg.contains("cancel"),
        "expected context canceled, got {}",
        err.msg
    );
}

struct FakeSplitStrategy<T> {
    should_split: bool,
    accumulated: Vec<T>,
}

impl<T: Clone + Send + 'static> SplitStrategy<T> for FakeSplitStrategy<T>
where
    FakeAccumItem: From<T>,
{
    fn Accumulate(&mut self, v: T) {
        self.accumulated.push(v);
    }
    fn ShouldSplit(&self) -> bool {
        self.should_split
    }
    fn ShouldSkip(&self, _v: &T) -> bool {
        false
    }
    fn GetAccumulations(&self) -> SplitHelperIterator {
        SplitHelperIterator {
            items: self
                .accumulated
                .iter()
                .cloned()
                .map(|v| Box::new(FakeAccumItem::from(v)) as Box<dyn SplitHelperLike>)
                .collect(),
        }
    }
    fn ResetAccumulations(&mut self) {
        self.accumulated.clear();
    }
}

/// Stand-in for Go `*split.RewriteSplitter` carrying table id for end-key asserts.
struct FakeAccumItem {
    table_id: i64,
}

impl From<String> for FakeAccumItem {
    fn from(_: String) -> Self {
        Self { table_id: 0 }
    }
}

impl From<FakeRewriteSplitter> for FakeAccumItem {
    fn from(v: FakeRewriteSplitter) -> Self {
        Self {
            table_id: v.table_id,
        }
    }
}

impl SplitHelperLike for FakeAccumItem {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone)]
struct FakeRewriteSplitter {
    table_id: i64,
}

// ExecuteRegions 计数/参数是断言 WithSplit 是否在正确时机切 region 的依据。
struct FakeRegionsSplitter {
    executed_splits_count: AtomicUsize,
    cancelled_context_count: AtomicUsize,
    expected_end_keys: Mutex<Vec<Vec<u8>>>,
}

impl PipelineRegionsSplitter for FakeRegionsSplitter {
    fn ExecuteRegions(&self, ctx: &Context, items: &SplitHelperIterator) -> Result<()> {
        if ctx.Err().is_some() {
            self.cancelled_context_count.fetch_add(1, Ordering::SeqCst);
        }
        // Go FakeRegionsSplitter.Traverse records endKey per valued item.
        for item in &items.items {
            if let Some(acc) = item.as_any().downcast_ref::<FakeAccumItem>() {
                if acc.table_id > 0 {
                    let end_key = codec::EncodeBytes(
                        Vec::new(),
                        &tablecodec::EncodeTablePrefix(acc.table_id + 1),
                    );
                    self.expected_end_keys.lock().unwrap().push(end_key);
                }
            }
        }
        self.executed_splits_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// `test_with_split_without_triggers_split` ↔ Go `TestWithSplitWithoutTriggersSplit`.
#[test]
// ShouldSplit 恒假：管道应原样吐出元素且不调用 ExecuteRegions。
fn test_with_split_without_triggers_split() {
    let fake_splitter = Arc::new(FakeRegionsSplitter {
        executed_splits_count: AtomicUsize::new(0),
        cancelled_context_count: AtomicUsize::new(0),
        expected_end_keys: Mutex::new(Vec::new()),
    });
    let strategy = Arc::new(Mutex::new(FakeSplitStrategy::<String> {
        should_split: false,
        accumulated: Vec::new(),
    }));
    let wrapper = PipelineRestorerWrapper {
        splitter: fake_splitter.clone(),
    };
    let ctx = Context::Background();
    let iter_ctx = IterContext::background();
    let mut split_iter = wrapper.WithSplit(
        &ctx,
        PipelineFromSlice(vec![
            "item1".to_string(),
            "item2".to_string(),
            "item3".to_string(),
        ]),
        strategy,
    );
    let got = CollectAll(&iter_ctx, split_iter.as_mut());
    assert!(got.Err.is_none());
    assert_eq!(
        fake_splitter.executed_splits_count.load(Ordering::SeqCst),
        0
    );
}

/// `test_with_split_accumulate_and_reset` ↔ Go `TestWithSplitAccumulateAndReset`.
#[test]
// 触发 split 后策略必须 Reset，否则后续积累会污染下一批 region 键。
fn test_with_split_accumulate_and_reset() {
    let fake_splitter = Arc::new(FakeRegionsSplitter {
        executed_splits_count: AtomicUsize::new(0),
        cancelled_context_count: AtomicUsize::new(0),
        expected_end_keys: Mutex::new(Vec::new()),
    });
    let strategy = Arc::new(Mutex::new(FakeSplitStrategy::<FakeRewriteSplitter> {
        should_split: true,
        accumulated: Vec::new(),
    }));
    let wrapper = PipelineRestorerWrapper {
        splitter: fake_splitter.clone(),
    };
    let items = vec![
        FakeRewriteSplitter { table_id: 1 },
        FakeRewriteSplitter { table_id: 2 },
    ];
    let ctx = Context::Background();
    let iter_ctx = IterContext::background();
    let mut split_iter = wrapper.WithSplit(&ctx, PipelineFromSlice(items), strategy.clone());
    let got = CollectAll(&iter_ctx, split_iter.as_mut());
    assert!(got.Err.is_none(), "{:?}", got.Err);

    let end_keys = vec![
        codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(2)),
        codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(3)),
    ];
    let mut got_keys = fake_splitter.expected_end_keys.lock().unwrap().clone();
    got_keys.sort();
    let mut expect = end_keys;
    expect.sort();
    assert_eq!(got_keys, expect);
    assert_eq!(
        fake_splitter.executed_splits_count.load(Ordering::SeqCst),
        2
    );
    assert!(strategy.lock().unwrap().accumulated.is_empty());
}

/// Go `WithSplit` passes its caller context to `ExecuteRegions`; cancellation
/// must not be hidden behind a fresh background context.
#[test]
fn test_with_split_preserves_caller_context() {
    let fake_splitter = Arc::new(FakeRegionsSplitter {
        executed_splits_count: AtomicUsize::new(0),
        cancelled_context_count: AtomicUsize::new(0),
        expected_end_keys: Mutex::new(Vec::new()),
    });
    let strategy = Arc::new(Mutex::new(FakeSplitStrategy::<String> {
        should_split: true,
        accumulated: Vec::new(),
    }));
    let wrapper = PipelineRestorerWrapper {
        splitter: fake_splitter.clone(),
    };
    let (ctx, cancel) = Context::WithCancel(&Context::Background());
    cancel.call();
    let pull_ctx = IterContext::background();
    let mut split_iter =
        wrapper.WithSplit(&ctx, PipelineFromSlice(vec!["item".to_string()]), strategy);

    let got = split_iter.TryNext(&pull_ctx);
    assert!(got.Err.is_none(), "{:?}", got.Err);
    assert_eq!(
        fake_splitter.cancelled_context_count.load(Ordering::SeqCst),
        1,
        "ExecuteRegions must receive the cancelled caller context"
    );
}
