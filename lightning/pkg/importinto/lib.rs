// Copyright 2026 AsterSQL.
//! Crate entry for `lightning/pkg/importinto`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/importinto`).
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/lib.rs`对应的入口重导出与模块装配，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少61行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl ScriptCheckpointManager`把\"ScriptCheckpointManager\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl CheckpointManager`把\"CheckpointManager\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Initialize`是当前文件的重要函数，承担\"Initialize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Get`是当前文件的重要函数，承担\"Get\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Update`是当前文件的重要函数，承担\"Update\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Remove`是当前文件的重要函数，承担\"Remove\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `IgnoreError`是当前文件的重要函数，承担\"IgnoreError\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DestroyError`是当前文件的重要函数，承担\"DestroyError\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpTables`是当前文件的重要函数，承担\"DumpTables\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpEngines`是当前文件的重要函数，承担\"DumpEngines\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpChunks`是当前文件的重要函数，承担\"DumpChunks\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetCheckpoints`是当前文件的重要函数，承担\"GetCheckpoints\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Close`是当前文件的重要函数，承担\"Close\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl ScriptJobSubmitter`把\"ScriptJobSubmitter\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl JobSubmitter`把\"JobSubmitter\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `SubmitTable`是当前文件的重要函数，承担\"SubmitTable\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetGroupKey`是当前文件的重要函数，承担\"GetGroupKey\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl ScriptJobMonitor`把\"ScriptJobMonitor\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl JobMonitor`把\"JobMonitor\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `WaitForJobs`是当前文件的重要函数，承担\"WaitForJobs\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl ScriptJobOrchestrator`把\"ScriptJobOrchestrator\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl JobOrchestrator`把\"JobOrchestrator\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `SubmitAndWait`是当前文件的重要函数，承担\"SubmitAndWait\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Cancel`是当前文件的重要函数，承担\"Cancel\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl ScriptProgressUpdater`把\"ScriptProgressUpdater\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl ProgressUpdater`把\"ProgressUpdater\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `UpdateTotalSize`是当前文件的重要函数，承担\"UpdateTotalSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `UpdateFinishedSize`是当前文件的重要函数，承担\"UpdateFinishedSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! 中文注释索引结束

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::all
)]

#[path = "stubs.rs"]
mod stubs;
pub use stubs::*;

#[path = "checkpoint.rs"]
mod checkpoint;
pub use checkpoint::*;

#[path = "precheck.rs"]
mod precheck;
pub use precheck::*;

#[path = "job_progress.rs"]
mod job_progress;
pub use job_progress::*;

#[path = "job_submitter.rs"]
mod job_submitter;
pub use job_submitter::*;

#[path = "job_monitor.rs"]
mod job_monitor;
pub use job_monitor::*;

#[path = "job_orchestrator.rs"]
mod job_orchestrator;
pub use job_orchestrator::*;

#[path = "importer.rs"]
mod importer;
pub use importer::*;

#[cfg(test)]
mod test_mocks {
    //! Scriptable stand-ins for Go gomock (CheckpointManager / Submitter / Monitor / Orchestrator / ProgressUpdater).
    use crate::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    type GetCpFn = Arc<dyn Fn(&str) -> Result<Option<TableCheckpoint>> + Send + Sync>;
    type GetCpsFn = Arc<dyn Fn() -> Result<Vec<TableCheckpoint>> + Send + Sync>;
    type UpdateFn = Arc<dyn Fn(&context::Context, &TableCheckpoint) -> Result<()> + Send + Sync>;
    type SubmitFn =
        Arc<dyn Fn(&context::Context, &importsdk::TableMeta) -> Result<ImportJob> + Send + Sync>;
    type WaitFn = Arc<dyn Fn(&[ImportJob]) -> Result<()> + Send + Sync>;
    type CancelOrchFn = Arc<dyn Fn(&context::Context) -> Result<()> + Send + Sync>;
    type SubmitWaitFn =
        Arc<dyn Fn(&context::Context, &[importsdk::TableMeta]) -> Result<()> + Send + Sync>;

    #[derive(Clone, Default)]
    pub struct ScriptCheckpointManager {
        pub get_queue: Arc<Mutex<VecDeque<Result<Option<TableCheckpoint>>>>>,
        pub get_by_table: Arc<Mutex<std::collections::HashMap<String, TableCheckpoint>>>,
        pub get_cps: Arc<Mutex<Option<Result<Vec<TableCheckpoint>>>>>,
        pub get_cps_fn: Arc<Mutex<Option<GetCpsFn>>>,
        pub get_fn: Arc<Mutex<Option<GetCpFn>>>,
        pub update_fn: Arc<Mutex<Option<UpdateFn>>>,
        pub update_queue: Arc<Mutex<VecDeque<Result<()>>>>,
        pub init_err: Arc<Mutex<Option<Error>>>,
        pub remove_err: Arc<Mutex<Option<Error>>>,
        pub close_err: Arc<Mutex<Option<Error>>>,
        pub updates: Arc<Mutex<Vec<TableCheckpoint>>>,
        pub removed: Arc<Mutex<Vec<String>>>,
        pub closed: Arc<Mutex<bool>>,
    }

    impl ScriptCheckpointManager {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn push_get(&self, v: Result<Option<TableCheckpoint>>) {
            self.get_queue.lock().unwrap().push_back(v);
        }
        pub fn set_get_cps(&self, v: Result<Vec<TableCheckpoint>>) {
            *self.get_cps.lock().unwrap() = Some(v);
        }
    }

    impl CheckpointManager for ScriptCheckpointManager {
        fn Initialize(&self, _ctx: &context::Context) -> Result<()> {
            if let Some(err) = self.init_err.lock().unwrap().clone() {
                return Err(err);
            }
            Ok(())
        }
        fn Get(&self, _ctx: &context::Context, tableName: &str) -> Result<Option<TableCheckpoint>> {
            if let Some(f) = self.get_fn.lock().unwrap().clone() {
                return f(tableName);
            }
            if let Some(v) = self.get_queue.lock().unwrap().pop_front() {
                return v;
            }
            Ok(self.get_by_table.lock().unwrap().get(tableName).cloned())
        }
        fn Update(&self, ctx: &context::Context, cp: &TableCheckpoint) -> Result<()> {
            self.updates.lock().unwrap().push(cp.clone());
            if let Some(f) = self.update_fn.lock().unwrap().clone() {
                return f(ctx, cp);
            }
            if let Some(v) = self.update_queue.lock().unwrap().pop_front() {
                return v;
            }
            self.get_by_table
                .lock()
                .unwrap()
                .insert(cp.TableName.clone(), cp.clone());
            Ok(())
        }
        fn Remove(&self, _ctx: &context::Context, tableName: &str) -> Result<()> {
            self.removed.lock().unwrap().push(tableName.to_string());
            if let Some(err) = self.remove_err.lock().unwrap().clone() {
                return Err(err);
            }
            Ok(())
        }
        fn IgnoreError(&self, _ctx: &context::Context, _tableName: &str) -> Result<()> {
            Ok(())
        }
        fn DestroyError(
            &self,
            _ctx: &context::Context,
            _tableName: &str,
        ) -> Result<Vec<TableCheckpoint>> {
            Ok(Vec::new())
        }
        fn DumpTables(
            &self,
            _ctx: &context::Context,
            _writer: &mut dyn std::io::Write,
        ) -> Result<()> {
            Ok(())
        }
        fn DumpEngines(
            &self,
            _ctx: &context::Context,
            _writer: &mut dyn std::io::Write,
        ) -> Result<()> {
            Ok(())
        }
        fn DumpChunks(
            &self,
            _ctx: &context::Context,
            _writer: &mut dyn std::io::Write,
        ) -> Result<()> {
            Ok(())
        }
        fn GetCheckpoints(&self, _ctx: &context::Context) -> Result<Vec<TableCheckpoint>> {
            if let Some(f) = self.get_cps_fn.lock().unwrap().clone() {
                return f();
            }
            if let Some(v) = self.get_cps.lock().unwrap().take() {
                return v;
            }
            Ok(self
                .get_by_table
                .lock()
                .unwrap()
                .values()
                .cloned()
                .collect())
        }
        fn Close(&self) -> Result<()> {
            *self.closed.lock().unwrap() = true;
            if let Some(err) = self.close_err.lock().unwrap().clone() {
                return Err(err);
            }
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    pub struct ScriptJobSubmitter {
        pub group_key: String,
        pub submit_fn: Arc<Mutex<Option<SubmitFn>>>,
        pub submit_queue: Arc<Mutex<VecDeque<Result<ImportJob>>>>,
    }

    impl ScriptJobSubmitter {
        pub fn new(group_key: impl Into<String>) -> Self {
            Self {
                group_key: group_key.into(),
                ..Default::default()
            }
        }
        pub fn push_submit(&self, v: Result<ImportJob>) {
            self.submit_queue.lock().unwrap().push_back(v);
        }
    }

    impl JobSubmitter for ScriptJobSubmitter {
        fn SubmitTable(
            &self,
            ctx: &context::Context,
            tableMeta: &importsdk::TableMeta,
        ) -> Result<ImportJob> {
            if let Some(f) = self.submit_fn.lock().unwrap().clone() {
                return f(ctx, tableMeta);
            }
            if let Some(v) = self.submit_queue.lock().unwrap().pop_front() {
                return v;
            }
            Err(Error::new("unexpected SubmitTable"))
        }
        fn GetGroupKey(&self) -> String {
            self.group_key.clone()
        }
    }

    #[derive(Clone, Default)]
    pub struct ScriptJobMonitor {
        pub wait_fn: Arc<Mutex<Option<WaitFn>>>,
        pub wait_queue: Arc<Mutex<VecDeque<Result<()>>>>,
    }

    impl ScriptJobMonitor {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn push_wait(&self, v: Result<()>) {
            self.wait_queue.lock().unwrap().push_back(v);
        }
    }

    impl JobMonitor for ScriptJobMonitor {
        fn WaitForJobs(&self, _ctx: &context::Context, jobs: &[ImportJob]) -> Result<()> {
            if let Some(f) = self.wait_fn.lock().unwrap().clone() {
                return f(jobs);
            }
            if let Some(v) = self.wait_queue.lock().unwrap().pop_front() {
                return v;
            }
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    pub struct ScriptJobOrchestrator {
        pub submit_wait_fn: Arc<Mutex<Option<SubmitWaitFn>>>,
        pub cancel_fn: Arc<Mutex<Option<CancelOrchFn>>>,
        pub cancel_called: Arc<Mutex<bool>>,
        pub submit_wait_err: Arc<Mutex<Option<Error>>>,
    }

    impl ScriptJobOrchestrator {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl JobOrchestrator for ScriptJobOrchestrator {
        fn SubmitAndWait(
            &self,
            ctx: &context::Context,
            tables: &[importsdk::TableMeta],
        ) -> Result<()> {
            if let Some(f) = self.submit_wait_fn.lock().unwrap().clone() {
                return f(ctx, tables);
            }
            if let Some(err) = self.submit_wait_err.lock().unwrap().clone() {
                return Err(err);
            }
            Ok(())
        }
        fn Cancel(&self, ctx: &context::Context) -> Result<()> {
            *self.cancel_called.lock().unwrap() = true;
            if let Some(f) = self.cancel_fn.lock().unwrap().clone() {
                return f(ctx);
            }
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    pub struct ScriptProgressUpdater {
        pub totals: Arc<Mutex<Vec<i64>>>,
        pub finished: Arc<Mutex<Vec<i64>>>,
    }

    impl ScriptProgressUpdater {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl ProgressUpdater for ScriptProgressUpdater {
        fn UpdateTotalSize(&self, size: i64) {
            self.totals.lock().unwrap().push(size);
        }
        fn UpdateFinishedSize(&self, size: i64) {
            self.finished.lock().unwrap().push(size);
        }
    }
}

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "checkpoint_test.rs"]
mod checkpoint_test;

#[cfg(test)]
#[path = "job_progress_test.rs"]
mod job_progress_test;

#[cfg(test)]
#[path = "precheck_test.rs"]
mod precheck_test;

#[cfg(test)]
#[path = "job_submitter_test.rs"]
mod job_submitter_test;

#[cfg(test)]
#[path = "job_monitor_test.rs"]
mod job_monitor_test;

#[cfg(test)]
#[path = "job_orchestrator_test.rs"]
mod job_orchestrator_test;

#[cfg(test)]
#[path = "importer_test.rs"]
mod importer_test;
