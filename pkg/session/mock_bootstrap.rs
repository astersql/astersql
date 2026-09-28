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

// 测试用 Mock Bootstrap / 升级（Upgrade）辅助。
//
// 在开启 `WithMockUpgrade` 时，向引导版本链注入“升级到最新版本”的模拟步骤，
// 通过执行一组代表性 DDL 验证升级回调与并发 DDL 行为；支持完整与精简两种模式。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

/// 是否启用 Mock 升级路径（全局开关）。
pub static WithMockUpgrade: AtomicBool = AtomicBool::new(false);
/// Mock 升级到最新版本时采用的模式：0=完整 DDL 集，1=精简路径。
pub static MockUpgradeToVerLatestKind: AtomicI32 = AtomicI32::new(defaultMockUpgradeToVerLatest);

/// 默认：完整 Mock 升级到最新版本。
pub const defaultMockUpgradeToVerLatest: i32 = 0;
/// 精简 Mock 升级：仅执行少量代表性 DDL。
pub const MockSimpleUpgradeToVerLatest: i32 = 1;

/// 完整 Mock 升级过程中依次执行的 DDL 语句列表。
pub const allDDLs: &[&str] = &[
    "create unique index c3_index on mock_sys_partition (c1)",
    "alter table mock_sys_partition add primary key c3_index (c1)",
    "alter table mock_sys_t add primary key idx_pc2 (c2)",
    "alter table mock_sys_t drop primary key",
    "alter table mock_sys_t add unique index idx_uc2 (c2)",
    "alter table mock_sys_t add index idx_c2(c2)",
    "alter table mock_sys_t add column c4 bigint",
    "create table test_create_table(a int)",
    "drop table test_create_table",
    "alter table mock_sys_t drop column c3",
    "alter table mock_sys_t_rebase auto_increment = 6000",
    "alter table mock_sys_t_auto shard_row_id_bits = 5",
    "alter table mock_sys_t modify column c11 mediumint",
    "alter table mock_sys_t modify column c11 int",
    "alter table mock_sys_t add column mayNullCol bigint default 1",
    "alter table mock_sys_t modify column mayNullCol bigint default 1 not null",
    "alter table mock_sys_t modify column c11 char(10)",
    "alter table mock_sys_t add constraint fk foreign key a(c1) references mock_sys_t_ref(c1)",
    "alter table mock_sys_t drop foreign key fk",
    "rename table mock_sys_t_rename1 to mock_sys_t_rename11",
    "rename table mock_sys_t_rename11 to mock_sys_t_rename111, mock_sys_t_rename2 to mock_sys_t_rename22",
    "alter table mock_sys_t_cs convert to charset utf8mb4",
    "alter table mock_sys_partition truncate partition p3",
    "alter table mock_sys_t add column c41 bigint, add column c42 bigint",
    "alter table mock_sys_t drop column c41, drop column c42",
    "alter table mock_sys_t add index idx_v(c1)",
    "alter table mock_sys_t alter index idx_v invisible",
    "alter table mock_sys_partition add partition (partition p6 values less than (8192))",
    "alter table mock_sys_partition drop partition p6",
    "alter table mock_sys_t add index rename_idx1(c1)",
    "alter table mock_sys_t rename index rename_idx1 to rename_idx2",
];

/// 完整 Mock 升级前创建 mysql 库中模拟系统表的 SQL。
const FULL_SETUP_SQL: &[&str] = &[
    "use mysql",
    "create table if not exists mock_sys_partition(c1 int, c2 int, c3 int) partition by range(c1) (partition p0 values less than (1024), partition p1 values less than (2048), partition p2 values less than (3072), partition p3 values less than (4096), partition p4 values less than (7096))",
    "create table if not exists mock_sys_t(c1 int, c2 int, c3 int, c11 tinyint, index fk_c1(c1))",
    "create table mock_sys_t_rebase(c1 bigint auto_increment primary key, c2 bigint)",
    "create table mock_sys_t_auto(c1 int not null auto_increment unique) shard_row_id_bits = 0",
    "create table mock_sys_t_ref (c1 int key, c2 int, c3 int, c11 tinyint)",
    "create table mock_sys_t_rename1(c1 bigint, c2 bigint)",
    "create table mock_sys_t_rename2(c1 bigint, c2 bigint)",
    "create table mock_sys_t_cs(a varchar(10)) charset utf8",
    "create table mock_sys_t_partition2(c1 int, c2 int, c3 int)",
    "set @@tidb_enable_exchange_partition=1",
];

/// Mock Bootstrap 运行时能力：执行 SQL 与可控 sleep（模拟 DDL 间隔）。
pub trait MockBootstrapRuntime {
    type Error;

    /// 执行一条 SQL。
    fn execute_sql(&mut self, sql: &str) -> Result<(), Self::Error>;
    /// 休眠指定时长（完整升级在每条 DDL 后 sleep 20ms）。
    fn sleep(&mut self, duration: Duration);
}

/// 升级引导过程中的生命周期回调钩子。
pub trait Callback {
    /// 注入 Mock 升级函数之前调用。
    fn OnBootstrapBefore(&mut self);
    /// 每条升级 DDL 执行前调用。
    fn OnBootstrap(&mut self);
    /// 升级步骤全部完成后调用。
    fn OnBootstrapAfter(&mut self);
}

/// 空实现回调，便于无需观测时使用。
#[derive(Debug, Default, Clone, Copy)]
pub struct BaseCallback;

impl Callback for BaseCallback {
    fn OnBootstrapBefore(&mut self) {}
    fn OnBootstrap(&mut self) {}
    fn OnBootstrapAfter(&mut self) {}
}

/// 可注入闭包的测试回调，用于统计与断言升级过程。
pub struct TestCallback {
    /// 可选计数器槽位。
    pub Cnt: Option<usize>,
    /// `OnBootstrapBefore` 导出闭包。
    pub OnBootstrapBeforeExported: Option<Box<dyn FnMut() + Send>>,
    /// `OnBootstrap` 导出闭包。
    pub OnBootstrapExported: Option<Box<dyn FnMut() + Send>>,
    /// `OnBootstrapAfter` 导出闭包。
    pub OnBootstrapAfterExported: Option<Box<dyn FnMut() + Send>>,
}

impl Default for TestCallback {
    fn default() -> Self {
        Self {
            Cnt: None,
            OnBootstrapBeforeExported: None,
            OnBootstrapExported: None,
            OnBootstrapAfterExported: None,
        }
    }
}

impl Callback for TestCallback {
    fn OnBootstrapBefore(&mut self) {
        if let Some(callback) = &mut self.OnBootstrapBeforeExported {
            callback();
        }
    }

    fn OnBootstrap(&mut self) {
        if let Some(callback) = &mut self.OnBootstrapExported {
            callback();
        }
    }

    fn OnBootstrapAfter(&mut self) {
        if let Some(callback) = &mut self.OnBootstrapAfterExported {
            callback();
        }
    }
}

/// Mock 升级动作：完整 DDL 集或精简路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockUpgradeAction {
    /// 执行 `FULL_SETUP_SQL` + `allDDLs`。
    Full,
    /// 仅执行少量建表/加列 DDL。
    Simple,
}

/// 带版本号的升级函数描述，挂到引导版本链末尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub struct versionedUpgradeFunction {
    /// 目标引导版本号。
    pub version: i64,
    /// Full 或 Simple 动作。
    pub action: MockUpgradeAction,
}

/// 测试侧 Mock 升级开关标志结构。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MockUpgradeFlags {
    /// 是否启用 Mock 升级。
    pub with_mock_upgrade: bool,
}

/// 同步写入标志结构与全局 `WithMockUpgrade` 原子变量。
pub fn RegisterMockUpgradeFlag(flags: &mut MockUpgradeFlags, enabled: bool) {
    flags.with_mock_upgrade = enabled;
    WithMockUpgrade.store(enabled, Ordering::SeqCst);
}

/// 完整 Mock 升级到最新版本：建模拟表后逐条执行 `allDDLs` 并触发回调。
pub fn mockUpgradeToVerLatest<R: MockBootstrapRuntime, C: Callback>(
    runtime: &mut R,
    callback: &mut C,
    version: i64,
    mock_latest_version: i64,
) -> Result<(), R::Error> {
    if version >= mock_latest_version {
        return Ok(());
    }
    // 先准备模拟系统表，再逐条 DDL 并在其间 sleep，模拟升级窗口。
    for sql in FULL_SETUP_SQL {
        runtime.execute_sql(sql)?;
    }
    for sql in allDDLs {
        callback.OnBootstrap();
        runtime.execute_sql(sql)?;
        runtime.sleep(Duration::from_millis(20));
    }
    callback.OnBootstrapAfter();
    Ok(())
}

/// 精简 Mock 升级：仅创建单表并执行少量 ALTER。
pub fn mockSimpleUpgradeToVerLatest<R: MockBootstrapRuntime, C: Callback>(
    runtime: &mut R,
    callback: &mut C,
    version: i64,
    mock_latest_version: i64,
) -> Result<(), R::Error> {
    if version >= mock_latest_version {
        return Ok(());
    }
    for sql in [
        "use mysql",
        "create table if not exists mock_sys_t(c1 int, c2 int, c3 int, c11 tinyint, index fk_c1(c1))",
        "alter table mock_sys_t add column mayNullCol bigint default 1",
        "alter table mock_sys_t add index idx_c2(c2)",
    ] {
        runtime.execute_sql(sql)?;
    }
    callback.OnBootstrapAfter();
    Ok(())
}

/// 在支持 HTTP 升级版本之后，将当前引导版本改写为 Mock 最新版本。
pub fn modifyBootstrapVersionForTest(
    version: i64,
    support_upgrade_http_version: i64,
    current_bootstrap_version: &mut i64,
    mock_latest_version: i64,
) {
    if !WithMockUpgrade.load(Ordering::SeqCst) {
        return;
    }
    if version >= support_upgrade_http_version
        && *current_bootstrap_version >= support_upgrade_http_version
    {
        *current_bootstrap_version = mock_latest_version;
    }
}

/// 若开启 Mock 升级，则在版本函数列表末尾追加“升级到 mock 最新”条目。
pub fn addMockBootstrapVersionForTest<C: Callback>(
    callback: &mut C,
    current_bootstrap_version: &mut i64,
    mock_latest_version: i64,
    upgrade_functions: &[versionedUpgradeFunction],
) -> Vec<versionedUpgradeFunction> {
    if !WithMockUpgrade.load(Ordering::SeqCst) {
        return upgrade_functions.to_vec();
    }

    callback.OnBootstrapBefore();
    *current_bootstrap_version = mock_latest_version;
    // 根据全局 Kind 选择 Full 或 Simple 动作。
    let action =
        if MockUpgradeToVerLatestKind.load(Ordering::SeqCst) == MockSimpleUpgradeToVerLatest {
            MockUpgradeAction::Simple
        } else {
            MockUpgradeAction::Full
        };
    let mut functions = upgrade_functions.to_vec();
    functions.push(versionedUpgradeFunction {
        version: mock_latest_version,
        action,
    });
    functions
}
