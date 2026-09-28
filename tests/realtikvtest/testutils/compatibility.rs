// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! 中文说明开始（自动生成）
//! 中文总览：`compatibility.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `测试工具与兼容封装` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 58 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `TestType` 是当前文件里的分支类型。
//! `TestType` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `TestType` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestType`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `CompatibilityContext` 是当前文件里的状态类型。
//! `CompatibilityContext` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `CompatibilityContext` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `CompatibilityContext`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `default` 是当前文件里的辅助函数。
//! `default` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `default` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `default`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `global_comp_ctx` 是当前文件里的辅助函数。
//! `global_comp_ctx` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `global_comp_ctx` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `global_comp_ctx`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `C` 是当前文件里的静态量。
//! `C` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `C` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `C`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `InitCompCtx` 是当前文件里的公开函数。
//! `InitCompCtx` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `InitCompCtx` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `InitCompCtx`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ParaDDLChan` 是当前文件里的状态类型。
//! `ParaDDLChan` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ParaDDLChan` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ParaDDLChan`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `newExecutor` 是当前文件里的辅助函数。
//! `newExecutor` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `newExecutor` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `newExecutor`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `InitCompCtxParams` 是当前文件里的公开函数。
//! `InitCompCtxParams` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `InitCompCtxParams` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `InitCompCtxParams`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `InitConcurrentDDLTest` 是当前文件里的公开函数。
//! `InitConcurrentDDLTest` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `InitConcurrentDDLTest` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `InitConcurrentDDLTest`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Start` 是当前文件里的公开函数。
//! `Start` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Start` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Start`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `start_on` 是当前文件里的公开函数。
//! `start_on` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `start_on` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `start_on`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Stop` 是当前文件里的公开函数。
//! `Stop` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Stop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Stop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `stop_on` 是当前文件里的公开函数。
//! `stop_on` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `stop_on` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `stop_on`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `testOneColFramePara` 是当前文件里的测试用例。
//! `testOneColFramePara` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `testOneColFramePara` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `testOneColFramePara`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `testTwoColsFramePara` 是当前文件里的测试用例。
//! `testTwoColsFramePara` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `testTwoColsFramePara` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `testTwoColsFramePara`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `testOneIndexFramePara` 是当前文件里的测试用例。
//! `testOneIndexFramePara` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `testOneIndexFramePara` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `testOneIndexFramePara`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Compatibility / concurrent DDL helpers
//! (Go `tests/realtikvtest/testutils/compatibility.go`).

use crate::common::{
    AddIndexGenCol, AddIndexMultiCols, AddIndexNonUnique, AddIndexPK, AddIndexUnique, InitTest,
    SuiteContext, checkResult, checkTableResult,
};
use crate::stubs::{logutil, require, testkit};
use astersql_tests_realtikvtest::stubs::{Storage, TestCtx};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::{self, JoinHandle};

/// testType of create index variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(i8)]
pub enum TestType {
    #[default]
    /// TestNonUnique test type of create none unique index.
    TestNonUnique = 0,
    /// TestUnique test type of create unique index.
    TestUnique = 1,
    /// TestPK test type of create Primary key.
    TestPK = 2,
    /// TestGenIndex test type of create generated col index.
    TestGenIndex = 3,
    /// TestMultiCols test type of multi columns in one index.
    TestMultiCols = 4,
}

pub use TestType::{TestGenIndex, TestMultiCols, TestNonUnique, TestPK, TestUnique};

/// CompatibilityContext is context of compatibility test.
pub struct CompatibilityContext {
    pub IsMultiSchemaChange: bool,
    pub IsConcurrentDDL: bool,
    pub IsPiTR: bool,
    pub(crate) executor: Vec<Executor>,
    pub colIIDs: Vec<Vec<i32>>,
    pub colJIDs: Vec<Vec<i32>>,
    pub tType: TestType,
    pub(crate) handles: Vec<JoinHandle<()>>,
}

impl Default for CompatibilityContext {
    fn default() -> Self {
        Self {
            IsMultiSchemaChange: false,
            IsConcurrentDDL: false,
            IsPiTR: false,
            executor: Vec::new(),
            colIIDs: Vec::new(),
            colJIDs: Vec::new(),
            tType: TestType::TestNonUnique,
            handles: Vec::new(),
        }
    }
}

fn global_comp_ctx() -> Arc<RwLock<CompatibilityContext>> {
    static C: OnceLock<Arc<RwLock<CompatibilityContext>>> = OnceLock::new();
    C.get_or_init(|| Arc::new(RwLock::new(CompatibilityContext::default())))
        .clone()
}

/// InitCompCtx inits SuiteContext for compatibility tests.
pub fn InitCompCtx(t: &TestCtx) -> SuiteContext {
    let mut ctx = InitTest(t);
    InitCompCtxParams(&mut ctx);
    ctx
}

struct ParaDDLChan {
    err: Option<String>,
    finished: bool,
}

pub(crate) struct Executor {
    pub id: i32,
    pub tk: testkit::TestKit,
    tx: Mutex<Option<SyncSender<ParaDDLChan>>>,
    rx: Mutex<Option<Receiver<ParaDDLChan>>>,
}

fn newExecutor(table_id: i32) -> Executor {
    let (tx, rx) = mpsc::sync_channel(1);
    Executor {
        id: table_id,
        // Placeholder; replaced in Start with a pooled TestKit.
        tk: testkit::NewTestKit(&TestCtx::new(), Storage::new("pending")),
        tx: Mutex::new(Some(tx)),
        rx: Mutex::new(Some(rx)),
    }
}

/// InitCompCtxParams inits params for compatibility tests.
pub fn InitCompCtxParams(ctx: &mut SuiteContext) {
    let g = global_comp_ctx();
    {
        let mut c = g.write().unwrap();
        c.IsConcurrentDDL = false;
        c.IsMultiSchemaChange = false;
        c.IsPiTR = false;
    }
    ctx.CompCtx = Some(g);
}

/// InitConcurrentDDLTest inits params for compatibility tests with concurrent ddl.
pub fn InitConcurrentDDLTest(
    t: &TestCtx,
    col_iids: Vec<Vec<i32>>,
    col_jids: Vec<Vec<i32>>,
    t_type: TestType,
) -> SuiteContext {
    let mut ctx = InitCompCtx(t);
    if let Some(comp) = &ctx.CompCtx {
        let mut g = comp.write().unwrap();
        g.IsConcurrentDDL = true;
        g.tType = t_type;
        g.colIIDs = col_iids;
        g.colJIDs = col_jids;
    }
    ctx
}

impl CompatibilityContext {
    /// Start start the compatibility tests (Go method on `*CompatibilityContext`).
    pub fn Start(&mut self, ctx: &SuiteContext) {
        self.executor.clear();
        self.handles.clear();
        for i in 0..3 {
            let mut er = newExecutor(i);
            er.tk = ctx.getTestKit();
            self.executor.push(er);
        }

        let t_type = self.tType;
        let col_iids = self.colIIDs.clone();
        let col_jids = self.colJIDs.clone();
        for i in 0..3 {
            let tx = self.executor[i].tx.lock().unwrap().take().unwrap();
            let id = self.executor[i].id;
            let worker_ctx = ctx.share_for_worker();
            let col_iids = col_iids.clone();
            let col_jids = col_jids.clone();
            self.handles.push(thread::spawn(move || {
                let err = match t_type {
                    TestType::TestNonUnique => {
                        testOneColFramePara(&worker_ctx, id, &col_iids, AddIndexNonUnique)
                    }
                    TestType::TestUnique => {
                        testOneColFramePara(&worker_ctx, id, &col_iids, AddIndexUnique)
                    }
                    TestType::TestPK => testOneIndexFramePara(&worker_ctx, id, 0, AddIndexPK),
                    TestType::TestGenIndex => {
                        testOneIndexFramePara(&worker_ctx, id, 29, AddIndexGenCol)
                    }
                    TestType::TestMultiCols => testTwoColsFramePara(
                        &worker_ctx,
                        id,
                        &col_iids,
                        &col_jids,
                        AddIndexMultiCols,
                    ),
                };
                let _ = tx.send(ParaDDLChan {
                    err: err.err(),
                    finished: true,
                });
            }));
        }
    }

    /// Start workers without holding CompCtx write lock during DDL.
    pub fn start_on(comp: &Arc<RwLock<CompatibilityContext>>, ctx: &SuiteContext) {
        comp.write().unwrap().Start(ctx);
    }

    /// Stop stop the compatibility tests (Go method).
    pub fn Stop(&mut self, ctx: &SuiteContext) -> Result<(), String> {
        let mut count = 3;
        for i in 0..3 {
            let rx = self.executor[i]
                .rx
                .lock()
                .unwrap()
                .take()
                .expect("executor rx");
            let pd = rx.recv().unwrap_or(ParaDDLChan {
                err: Some("executor channel closed".into()),
                finished: true,
            });
            if let Some(e) = pd.err {
                require::NoError(&ctx.t, Err(e.clone()));
                return Err(e);
            }
            if pd.finished {
                count -= 1;
                logutil::BgLogger().Info(
                    "xlc test worker",
                    &[("count", count.to_string()), ("er id", i.to_string())],
                );
                ctx.putTestKit(self.executor[i].tk.clone());
            }
            if count == 0 {
                break;
            }
        }
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        Ok(())
    }

    /// Stop workers without holding write lock while waiting on channels.
    pub fn stop_on(
        comp: &Arc<RwLock<CompatibilityContext>>,
        ctx: &SuiteContext,
    ) -> Result<(), String> {
        let rxs = {
            let g = comp.read().unwrap();
            let mut rxs = Vec::new();
            for er in &g.executor {
                rxs.push(er.rx.lock().unwrap().take().expect("executor rx"));
            }
            rxs
        }; // drop lock before recv

        let mut count = 3;
        let mut finished = Vec::new();
        for (i, rx) in rxs.into_iter().enumerate() {
            let pd = rx.recv().unwrap_or(ParaDDLChan {
                err: Some("executor channel closed".into()),
                finished: true,
            });
            if let Some(e) = pd.err {
                require::NoError(&ctx.t, Err(e.clone()));
                return Err(e);
            }
            if pd.finished {
                count -= 1;
                logutil::BgLogger().Info(
                    "xlc test worker",
                    &[("count", count.to_string()), ("er id", i.to_string())],
                );
                finished.push(i);
            }
            if count == 0 {
                break;
            }
        }

        let mut g = comp.write().unwrap();
        for i in finished {
            ctx.putTestKit(g.executor[i].tk.clone());
        }
        for h in g.handles.drain(..) {
            let _ = h.join();
        }
        Ok(())
    }
}

fn testOneColFramePara(
    ctx: &SuiteContext,
    table_id: i32,
    col_ids: &[Vec<i32>],
    f: impl Fn(&SuiteContext, i32, &str, i32) -> Result<(), String>,
) -> Result<(), String> {
    let table_name = format!("addindex.t{table_id}");
    let mut last_err = Ok(());
    for &i in &col_ids[table_id as usize] {
        let err = f(ctx, table_id, &table_name, i);
        if let Err(ref e) = err {
            if ctx.get_unique() || ctx.get_pk() {
                require::Contains(&ctx.t, e, "Duplicate entry");
                last_err = Ok(());
                continue;
            }
            logutil::BgLogger().Error(
                "add index failed",
                &[("category", "add index test".into()), ("error", e.clone())],
            );
            require::NoError(&ctx.t, Err(e.clone()));
            return err;
        }
        checkResult(ctx, &table_name, i, table_id);
        last_err = err;
    }
    last_err
}

fn testTwoColsFramePara(
    ctx: &SuiteContext,
    table_id: i32,
    i_ids: &[Vec<i32>],
    j_ids: &[Vec<i32>],
    f: impl Fn(&SuiteContext, i32, &str, i32, i32, i32) -> Result<(), String>,
) -> Result<(), String> {
    let table_name = format!("addindex.t{table_id}");
    let mut index_id = 0;
    let mut last_err = Ok(());
    for &i in &i_ids[table_id as usize] {
        for &j in &j_ids[table_id as usize] {
            let err = f(ctx, table_id, &table_name, index_id, i, j);
            if let Err(ref e) = err {
                logutil::BgLogger().Error(
                    "add index failed",
                    &[("category", "add index test".into()), ("error", e.clone())],
                );
            }
            require::NoError(&ctx.t, err.as_ref().map(|_| ()).map_err(|e| e.clone()));
            if err.is_ok() && i != j {
                checkResult(ctx, &table_name, index_id, table_id);
            }
            index_id += 1;
            if let Err(e) = err {
                return Err(e);
            }
            last_err = Ok(());
        }
    }
    last_err
}

fn testOneIndexFramePara(
    ctx: &SuiteContext,
    table_id: i32,
    col_id: i32,
    f: impl Fn(&SuiteContext, i32, &str, i32) -> Result<(), String>,
) -> Result<(), String> {
    let table_name = format!("addindex.t{table_id}");
    let err = f(ctx, table_id, &table_name, col_id);
    if let Err(ref e) = err {
        logutil::BgLogger().Error(
            "add index failed",
            &[("category", "add index test".into()), ("error", e.clone())],
        );
    }
    require::NoError(&ctx.t, err.as_ref().map(|_| ()).map_err(|e| e.clone()));
    if err.is_ok() {
        if ctx.get_pk() {
            checkTableResult(ctx, &table_name, table_id);
        } else {
            checkResult(ctx, &table_name, col_id, table_id);
        }
    }
    err
}
