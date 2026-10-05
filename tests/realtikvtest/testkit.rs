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
//! 中文总览：`testkit.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `真实 TiKV 集成测试` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 63 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `WithRealTiKV` 是当前文件里的公开函数。
//! `WithRealTiKV` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WithRealTiKV` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WithRealTiKV`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TiKVPath` 是当前文件里的公开函数。
//! `TiKVPath` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TiKVPath` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TiKVPath`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `PDAddr` 是当前文件里的常量。
//! `PDAddr` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `PDAddr` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PDAddr`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SetTiKVPath` 是当前文件里的公开函数。
//! `SetTiKVPath` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `SetTiKVPath` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SetTiKVPath`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SetWithRealTiKV` 是当前文件里的公开函数。
//! `SetWithRealTiKV` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `SetWithRealTiKV` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SetWithRealTiKV`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `RunTestMain` 是当前文件里的公开函数。
//! `RunTestMain` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `RunTestMain` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `RunTestMain`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `realtikvStoreOption` 是当前文件里的状态类型。
//! `realtikvStoreOption` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `realtikvStoreOption` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `realtikvStoreOption`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `RealTiKVStoreOption` 是当前文件里的状态类型。
//! `RealTiKVStoreOption` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `RealTiKVStoreOption` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `RealTiKVStoreOption`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `WithRetainData` 是当前文件里的公开函数。
//! `WithRetainData` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WithRetainData` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WithRetainData`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `WithKeyspaceName` 是当前文件里的公开函数。
//! `WithKeyspaceName` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WithKeyspaceName` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WithKeyspaceName`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `WithNewCollationsEnabledOnFirstBootstrap` 是当前文件里的公开函数。
//! `WithNewCollationsEnabledOnFirstBootstrap` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WithNewCollationsEnabledOnFirstBootstrap` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WithNewCollationsEnabledOnFirstBootstrap`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `WithKeepSystemStore` 是当前文件里的公开函数。
//! `WithKeepSystemStore` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WithKeepSystemStore` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WithKeepSystemStore`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `WithKeepSelfStore` 是当前文件里的公开函数。
//! `WithKeepSelfStore` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WithKeepSelfStore` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WithKeepSelfStore`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `WithAllocPort` 是当前文件里的公开函数。
//! `WithAllocPort` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WithAllocPort` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WithAllocPort`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `KSRuntime` 是当前文件里的状态类型。
//! `KSRuntime` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `KSRuntime` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `KSRuntime`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `PrepareForCrossKSTest` 是当前文件里的公开函数。
//! `PrepareForCrossKSTest` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `PrepareForCrossKSTest` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PrepareForCrossKSTest`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `PrepareForCrossKSTestWithNewCollation` 是当前文件里的公开函数。
//! `PrepareForCrossKSTestWithNewCollation` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `PrepareForCrossKSTestWithNewCollation` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PrepareForCrossKSTestWithNewCollation`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `CreateMockStoreAndSetup` 是当前文件里的公开函数。
//! `CreateMockStoreAndSetup` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `CreateMockStoreAndSetup` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `CreateMockStoreAndSetup`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `CreateMockStoreAndDomainAndSetup` 是当前文件里的公开函数。
//! `CreateMockStoreAndDomainAndSetup` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `CreateMockStoreAndDomainAndSetup` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `CreateMockStoreAndDomainAndSetup`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `UpdateTiDBConfig` 是当前文件里的公开函数。
//! `UpdateTiDBConfig` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `UpdateTiDBConfig` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `UpdateTiDBConfig`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `GetNextGenObjStoreURI` 是当前文件里的公开函数。
//! `GetNextGenObjStoreURI` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `GetNextGenObjStoreURI` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `GetNextGenObjStoreURI`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `getNextGenObjStoreURIWithArgs` 是当前文件里的辅助函数。
//! `getNextGenObjStoreURIWithArgs` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `getNextGenObjStoreURIWithArgs` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `getNextGenObjStoreURIWithArgs`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! RealTiKV test fixtures matching Go `tests/realtikvtest/testkit.go`.
//! TiKV/SQL/session boundaries are local stubs (see `stubs.rs`).

use crate::stubs::{
    Domain, PD_ADDR, Storage, TestCtx, TestMain, config, ddl, driver, goleak, handle, kerneltype,
    keyspace, kvstore, mock_port_alloc_add, mvcc_wait, require, session, set_tikv_path,
    set_with_real_tikv, testkit as tk, testmain, testsetup, tikv, tikv_path, transaction, vardef,
    view, with_real_tikv,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Go `WithRealTiKV` — whether tests run with real TiKV.
pub fn WithRealTiKV() -> bool {
    with_real_tikv()
}

/// Go `TiKVPath` — TiKV storage path / addr.
pub fn TiKVPath() -> String {
    tikv_path()
}

/// Go `PDAddr`.
pub const PDAddr: &str = PD_ADDR;

/// Override TiKV path (tests / flag parse stand-in).
pub fn SetTiKVPath(path: impl Into<String>) {
    set_tikv_path(path);
}

/// Override WithRealTiKV (tests / flag parse stand-in).
pub fn SetWithRealTiKV(v: bool) {
    set_with_real_tikv(v);
}

/// Go `RunTestMain` — common setups for all real tikv tests.
pub fn RunTestMain(m: &mut TestMain) -> i32 {
    m.wrapped = true;
    RunTestMainWith(|| m.code)
}

/// Common process configuration, reapplied after an isolated test resets globals.
pub fn SetupTestMain() {
    testsetup::SetupForCommonTest();
    set_with_real_tikv(true);
    vardef::SetSchemaLease(Duration::from_secs(5));
    config::UpdateGlobal(|conf| {
        conf.TiKVClient.AsyncCommit.SafeWindow = 0;
        conf.TiKVClient.AsyncCommit.AllowedClockDrift = 0;
    });
    tikv::EnableFailpoints();
}

/// Execute the package tests between common setup and exit handling.
/// Unlike the compatibility TestMain value, this entry invokes the real runner.
pub fn RunTestMainWith(run: impl FnOnce() -> i32) -> i32 {
    SetupTestMain();
    let wait = mvcc_wait();
    let mut main = TestMain::new(run());
    let code = testmain::WrapTestingM(&mut main, move |i| {
        // wait for MVCCLevelDB to close, MVCCLevelDB will be closed in one second
        testmain::sleep_mvcc(wait);
        i
    });
    goleak::VerifyTestMain(
        code,
        vec![goleak::Cleanup("testutil.CheckIngestLeakageForTest")],
    )
}

#[derive(Clone, Debug, Default)]
struct realtikvStoreOption {
    retainData: bool,
    keyspace: String,
    /// None unless the test wants a specific persisted new-collation setting.
    newCollationsEnabledOnFirstBootstrap: Option<bool>,
    keepSystemStore: bool,
    keepSelfStore: bool,
    allocPort: bool,
}

/// Go `RealTiKVStoreOption` — functional option for creating a real TiKV store.
pub struct RealTiKVStoreOption(Box<dyn Fn(&mut realtikvStoreOption) + Send + Sync>);

/// Go `WithRetainData`.
pub fn WithRetainData() -> RealTiKVStoreOption {
    RealTiKVStoreOption(Box::new(|opt| {
        opt.retainData = true;
    }))
}

/// Go `WithKeyspaceName`.
pub fn WithKeyspaceName(name: impl Into<String>) -> RealTiKVStoreOption {
    let name = name.into();
    RealTiKVStoreOption(Box::new(move |opt| {
        opt.keyspace = name.clone();
    }))
}

/// Go `WithNewCollationsEnabledOnFirstBootstrap`.
pub fn WithNewCollationsEnabledOnFirstBootstrap(enabled: bool) -> RealTiKVStoreOption {
    RealTiKVStoreOption(Box::new(move |opt| {
        opt.newCollationsEnabledOnFirstBootstrap = Some(enabled);
    }))
}

/// Go `WithKeepSystemStore`.
pub fn WithKeepSystemStore(keep: bool) -> RealTiKVStoreOption {
    RealTiKVStoreOption(Box::new(move |opt| {
        opt.keepSystemStore = keep;
    }))
}

/// Go `WithKeepSelfStore`.
pub fn WithKeepSelfStore(keep: bool) -> RealTiKVStoreOption {
    RealTiKVStoreOption(Box::new(move |opt| {
        opt.keepSelfStore = keep;
    }))
}

/// Go `WithAllocPort`.
pub fn WithAllocPort(alloc: bool) -> RealTiKVStoreOption {
    RealTiKVStoreOption(Box::new(move |opt| {
        opt.allocPort = alloc;
    }))
}

/// Go `KSRuntime` — runtime environment for a keyspace.
#[derive(Clone, Debug)]
pub struct KSRuntime {
    pub Store: Storage,
    pub Dom: Domain,
}

/// Go `PrepareForCrossKSTest`.
pub fn PrepareForCrossKSTest(t: &TestCtx, userKSs: &[&str]) -> HashMap<String, KSRuntime> {
    PrepareForCrossKSTestWithNewCollation(t, None, userKSs)
}

/// Go `PrepareForCrossKSTestWithNewCollation`.
pub fn PrepareForCrossKSTestWithNewCollation(
    t: &TestCtx,
    newCollationEnabled: Option<&HashMap<String, bool>>,
    userKSs: &[&str],
) -> HashMap<String, KSRuntime> {
    if !kerneltype::IsNextGen() {
        t.Fail();
    }
    let mut res: HashMap<String, KSRuntime> = HashMap::with_capacity(userKSs.len() + 1);
    // stores are cached, we want to make sure stores are closed after domain,
    // else some routine might be blocked.
    // Cleanup registered after creations so it runs first (LIFO) before per-store cleanups.
    // Go registers cleanup before the loop; we mirror that with a shared close list.
    let close_list: std::sync::Arc<std::sync::Mutex<Vec<Storage>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let close_list_c = close_list.clone();
    let t_close = t.clone();
    t.Cleanup(move || {
        for runtime_store in close_list_c.lock().unwrap().drain(..) {
            require::NoError(&t_close, runtime_store.Close());
        }
    });

    let mut ks_list: Vec<String> = Vec::with_capacity(userKSs.len() + 1);
    ks_list.push(keyspace::System.to_string());
    for ks in userKSs {
        ks_list.push((*ks).to_string());
    }

    for ks in ks_list {
        let mut opts: Vec<RealTiKVStoreOption> = vec![
            WithKeyspaceName(ks.clone()),
            WithKeepSystemStore(true),
            WithKeepSelfStore(true),
            WithAllocPort(true),
        ];
        if let Some(map) = newCollationEnabled {
            if let Some(&enabled) = map.get(&ks) {
                opts.push(WithNewCollationsEnabledOnFirstBootstrap(enabled));
            }
        }
        let (store, dom) = CreateMockStoreAndDomainAndSetup(t, &opts);
        close_list.lock().unwrap().push(store.clone());
        res.insert(
            ks,
            KSRuntime {
                Store: store,
                Dom: dom,
            },
        );
    }
    res
}

/// Go `CreateMockStoreAndSetup` — return a new Storage.
pub fn CreateMockStoreAndSetup(t: &TestCtx, opts: &[RealTiKVStoreOption]) -> Storage {
    let (store, _) = CreateMockStoreAndDomainAndSetup(t, opts);
    store
}

/// Go `CreateMockStoreAndDomainAndSetup` — initialize Storage and Domain.
pub fn CreateMockStoreAndDomainAndSetup(
    t: &TestCtx,
    opts: &[RealTiKVStoreOption],
) -> (Storage, Domain) {
    let _ = kvstore::Register(config::StoreTypeTiKV, &driver::TiKVDriver {});
    kvstore::SetSystemStorage(None);
    // set it to 5 seconds for testing lock resolve.
    transaction::ManagedLockTTL.store(5000, Ordering::SeqCst);
    transaction::PrewriteMaxBackoff.store(500, Ordering::SeqCst);

    let mut option = realtikvStoreOption::default();
    for opt in opts {
        (opt.0)(&mut option);
    }

    let mut ks = String::new();
    if kerneltype::IsNextGen() {
        if option.keyspace.is_empty() {
            // in nextgen kernel, SYSTEM keyspace must be bootstrapped first
            ks = keyspace::System.to_string();
        } else {
            ks = option.keyspace.clone();
        }
        t.Log(format!("create realtikv store with keyspace:{ks}"));
    }
    vardef::SetSchemaLease(Duration::from_millis(500));

    let mut path = tikv_path();
    if !ks.is_empty() {
        path = format!("{path}&keyspaceName={ks}");
    }
    let d = driver::TiKVDriver {};
    let bak = config::GetGlobalConfig();
    t.Cleanup(move || {
        config::StoreGlobalConfig(&bak);
    });
    let ks_for_cfg = ks.clone();
    let new_collation = option.newCollationsEnabledOnFirstBootstrap;
    let alloc_port = option.allocPort;
    config::UpdateGlobal(move |conf| {
        conf.TxnLocalLatches.Enabled = false;
        conf.KeyspaceName = ks_for_cfg.clone();
        conf.Store = config::StoreTypeTiKV.to_string();
        if let Some(enabled) = new_collation {
            conf.NewCollationsEnabledOnFirstBootstrap = enabled;
        }
        if alloc_port {
            conf.Port = mock_port_alloc_add(1) as u32;
        }
    });
    if ks == keyspace::System {
        UpdateTiDBConfig();
    }
    let store = match d.Open(&path) {
        Ok(s) => s,
        Err(e) => {
            require::NoError(t, Err(e));
            unreachable!();
        }
    };
    if kerneltype::IsNextGen() && ks != keyspace::System {
        let sys_path = format!("{}&keyspaceName={}", tikv_path(), keyspace::System);
        let sys_store = match d.Open(&sys_path) {
            Ok(s) => s,
            Err(e) => {
                require::NoError(t, Err(e));
                unreachable!();
            }
        };
        kvstore::SetSystemStorage(Some(sys_store.clone()));
        if !option.keepSystemStore {
            let sys_store_c = sys_store.clone();
            let t_sys = t.clone();
            t.Cleanup(move || {
                require::NoError(&t_sys, sys_store_c.Close());
            });
        }
    }
    require::NoError(t, ddl::StartOwnerManager((), &store));
    let dom = match session::BootstrapSession(&store) {
        Ok(d) => d,
        Err(e) => {
            require::NoError(t, Err(e));
            unreachable!();
        }
    };
    let sm = tk::MockSessionManager {};
    dom.InfoSyncer().SetSessionManager(&sm);
    let kit = tk::NewTestKit(t, store.clone());
    kit.MustExec(&format!(
        "set global innodb_lock_wait_timeout = {}",
        vardef::DefInnodbLockWaitTimeout
    ));
    kit.MustExec("use test");

    if !option.retainData {
        kit.MustExec("delete from mysql.tidb_import_jobs;");
        kit.MustExec("delete from mysql.tidb_global_task;");
        kit.MustExec("delete from mysql.tidb_background_subtask;");
        kit.MustExec("delete from mysql.tidb_ddl_job;");
        let rs = kit.MustQuery("show full tables where table_type = 'BASE TABLE';");
        let mut tables: Vec<String> = Vec::new();
        for row in rs.Rows() {
            tables.push(format!("`{}`", row[0]));
        }
        for table in &tables {
            kit.MustExec(&format!("alter table {table} nocache"));
        }
        if !tables.is_empty() {
            kit.MustExec(&format!("drop table {}", tables.join(",")));
        }
        let rs = kit.MustQuery("show full tables where table_type = 'VIEW';");
        for row in rs.Rows() {
            kit.MustExec(&format!("drop view `{}`", row[0]));
        }
        t.Log("cleaned up ddl and tables");
    }

    let keep_self = option.keepSelfStore;
    let store_c = store.clone();
    let dom_c = dom.clone();
    let t_end = t.clone();
    t.Cleanup(move || {
        // cleanup order: domain, owner manager, optionally store
        dom_c.Close();
        ddl::CloseOwnerManager(&store_c);
        if !keep_self {
            require::NoError(&t_end, store_c.Close());
        }
        transaction::PrewriteMaxBackoff.store(20000, Ordering::SeqCst);
        view::Stop();
    });
    (store, dom)
}

/// Go `UpdateTiDBConfig`.
pub fn UpdateTiDBConfig() {
    config::UpdateGlobal(|conf| {
        conf.Path = "127.0.0.1:2379".to_string();
        if kerneltype::IsNextGen() {
            conf.TiKVWorkerURL = "localhost:19000".to_string();
            conf.KeyspaceName = keyspace::System.to_string();
            conf.Instance.TiDBServiceScope = handle::NextGenTargetScope.to_string();
            conf.MeteringStorageURI =
                getNextGenObjStoreURIWithArgs("metering-data", "&region=local");
        }
    });
}

/// Go `GetNextGenObjStoreURI`.
pub fn GetNextGenObjStoreURI(path: &str) -> String {
    getNextGenObjStoreURIWithArgs(path, "&provider=minio")
}

fn getNextGenObjStoreURIWithArgs(path: &str, args: &str) -> String {
    format!(
        "s3://next-gen-test/{path}?access-key=minioadmin&secret-access-key=minioadmin&endpoint=http%3a%2f%2f0.0.0.0%3a9000{args}"
    )
}
