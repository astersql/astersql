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

// Bootstrap 升级路径的 mock 运行时测试。
//
// 通过录制 SQL 与 sleep 的假运行时，验证 `mockSimpleUpgradeToVerLatest` 在版本前进时
// 只执行一次真实 schema 迁移序列；版本已是最新时不再重复执行。

use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_session::mock_bootstrap::{
    BaseCallback, Callback, MockBootstrapRuntime, MockSimpleUpgradeToVerLatest, MockUpgradeAction,
    MockUpgradeFlags, MockUpgradeToVerLatestKind, RegisterMockUpgradeFlag, WithMockUpgrade,
    addMockBootstrapVersionForTest, allDDLs, mockSimpleUpgradeToVerLatest, mockUpgradeToVerLatest,
    modifyBootstrapVersionForTest,
};

/// 录制升级过程中执行的 SQL 与 sleep 调用，供断言迁移序列。
#[derive(Default)]
struct RecordingRuntime {
    /// 按调用顺序记录的 SQL 文本。
    sql: Vec<String>,
    /// 按调用顺序记录的 sleep 时长。
    sleeps: Vec<Duration>,
}

impl MockBootstrapRuntime for RecordingRuntime {
    type Error = String;

    fn execute_sql(&mut self, sql: &str) -> Result<(), Self::Error> {
        self.sql.push(sql.to_owned());
        Ok(())
    }

    fn sleep(&mut self, duration: Duration) {
        self.sleeps.push(duration);
    }
}

/// 验证从旧版本升到最新时执行固定 SQL 序列，且同版本再次调用不再追加 SQL。
#[test]
fn simple_upgrade_executes_real_schema_transition_sequence_once() {
    let mut runtime = RecordingRuntime::default();
    mockSimpleUpgradeToVerLatest(&mut runtime, &mut BaseCallback, 1, 2).unwrap();
    assert_eq!(runtime.sql.len(), 4);
    assert_eq!(runtime.sql[0], "use mysql");
    assert!(runtime.sql[2].contains("mayNullCol"));
    assert!(runtime.sleeps.is_empty());
    // 版本已是最新（from==to）时不应再次执行迁移 SQL。
    let before = runtime.sql.len();
    mockSimpleUpgradeToVerLatest(&mut runtime, &mut BaseCallback, 2, 2).unwrap();
    assert_eq!(runtime.sql.len(), before);
}

/// 对应 Go `TestUpgradeVersionMockLatest`：完整 mock 升级必须执行准备 DDL、全部升级 DDL，
/// 并在每条迁移语句后等待固定间隔。
#[test]
fn full_upgrade_executes_every_schema_transition_and_sleeps_between_steps() {
    let mut runtime = RecordingRuntime::default();
    mockUpgradeToVerLatest(&mut runtime, &mut BaseCallback, 1, 2).unwrap();

    // FULL_SETUP_SQL is intentionally private; the eleven setup statements are part of the
    // Go mock contract and allDDLs remains the authoritative public transition list.
    assert_eq!(runtime.sql.len(), 11 + allDDLs.len());
    assert_eq!(runtime.sleeps.len(), allDDLs.len());
    assert_eq!(runtime.sleeps[0], Duration::from_millis(20));
    assert_eq!(runtime.sql[0], "use mysql");
    assert_eq!(runtime.sql.last().unwrap(), allDDLs.last().unwrap());

    let sql_count = runtime.sql.len();
    mockUpgradeToVerLatest(&mut runtime, &mut BaseCallback, 2, 2).unwrap();
    assert_eq!(runtime.sql.len(), sql_count);
    assert_eq!(runtime.sleeps.len(), allDDLs.len());
}

#[derive(Default)]
struct CountingCallback {
    before: usize,
    on: usize,
    after: usize,
}

impl Callback for CountingCallback {
    fn OnBootstrapBefore(&mut self) {
        self.before += 1;
    }

    fn OnBootstrap(&mut self) {
        self.on += 1;
    }

    fn OnBootstrapAfter(&mut self) {
        self.after += 1;
    }
}

/// 对应 Go mock upgrade callback：before/each-step/after 的生命周期顺序不能被吞掉。
#[test]
fn full_upgrade_preserves_callback_lifecycle() {
    let mut runtime = RecordingRuntime::default();
    let mut callback = CountingCallback::default();
    mockUpgradeToVerLatest(&mut runtime, &mut callback, 1, 2).unwrap();

    assert_eq!(callback.before, 0);
    assert_eq!(callback.on, allDDLs.len());
    assert_eq!(callback.after, 1);
}

/// 对应 Go `TestUpgradeVersionWithUpgradeHTTPOp` 的 mock 版本接线：开启开关后，
/// 版本改写和新增升级函数必须同时发生；测试结束恢复全局开关，避免串扰其他测试。
#[test]
fn mock_upgrade_flag_rewrites_latest_version_and_appends_action() {
    let mut flags = MockUpgradeFlags::default();
    let mut current = 20;

    // Go `addMockBootstrapVersionForTest` returns the original function list unchanged when
    // `WithMockUpgrade` is disabled, and version rewriting is likewise a no-op.
    modifyBootstrapVersionForTest(20, 15, &mut current, 99);
    let mut callback = CountingCallback::default();
    let functions = addMockBootstrapVersionForTest(&mut callback, &mut current, 99, &[]);
    assert_eq!(current, 20);
    assert!(functions.is_empty());
    assert_eq!(callback.before, 0);

    // Go `TestUpgradeVersionWithUpgradeHTTPOp` explicitly selects the simple mock action.
    RegisterMockUpgradeFlag(&mut flags, true);
    MockUpgradeToVerLatestKind.store(MockSimpleUpgradeToVerLatest, Ordering::SeqCst);
    modifyBootstrapVersionForTest(20, 15, &mut current, 99);
    assert_eq!(current, 99);

    let functions = addMockBootstrapVersionForTest(&mut callback, &mut current, 99, &[]);
    assert_eq!(current, 99);
    assert_eq!(callback.before, 1);
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0].version, 99);
    assert_eq!(functions[0].action, MockUpgradeAction::Simple);
    assert!(WithMockUpgrade.load(Ordering::SeqCst));

    RegisterMockUpgradeFlag(&mut flags, false);
    MockUpgradeToVerLatestKind.store(0, Ordering::SeqCst);
    assert!(!WithMockUpgrade.load(Ordering::SeqCst));
}
