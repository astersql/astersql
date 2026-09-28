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

#![allow(non_snake_case, non_upper_case_globals)]

// 会话事务 failpoint / 测试观测工具。
//
// Failpoint（故障注入点）用于在测试中打断执行路径、统计 TSO 请求次数、
// 校验 InfoSchema（信息模式，描述库表元数据）版本与锁错误重试行为。
// 本文件将 Go 侧 session 上下文键值与断言辅助函数迁移为 Rust trait/函数。

use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::{GetTxnManager, InfoSchemaRef, TxnManagerContext};

/// 会话侧断言记录表在 Context 中的键。
pub const AssertRecordsKey: &str = "assertTxnManagerRecords";
/// 期望的事务 InfoSchema 在 Context 中的键。
pub const AssertTxnInfoSchemaKey: &str = "assertTxnInfoSchemaKey";
/// 语句重试后期望的事务 InfoSchema 键。
pub const AssertTxnInfoSchemaAfterRetryKey: &str = "assertTxnInfoSchemaAfterRetryKey";
/// 执行器首次运行前的断点名。
pub const BreakPointBeforeExecutorFirstRun: &str = "beforeExecutorFirstRun";
/// 锁错误后进入语句重试时的断点名。
pub const BreakPointOnStmtRetryAfterLockError: &str = "lockErrorAndThenOnStmtRetryCalled";
/// TSO 请求次数计数器键。TSO 为全局时间戳服务。
pub const TsoRequestCount: &str = "tsoRequestCount";
/// 等待 TSO 返回的次数计数器键。
pub const TsoWaitCount: &str = "tsoWaitCount";
/// 使用常量 TSO（不真正请求）的次数计数器键。
pub const TsoUseConstantCount: &str = "tsoUseConstantCount";
/// `OnStmtRetry` 被调用次数的计数器键。
pub const CallOnStmtRetryCount: &str = "callOnStmtRetryCount";
/// 锁错误入口断言记录表键。
pub const AssertLockErr: &str = "assertLockError";

/// 会话 Context 中可存的任意类型值。
pub type SessionValue = Box<dyn Any>;
/// 按名称索引的断言记录集合。
pub type AssertRecords = HashMap<String, SessionValue>;
/// 按错误名累计进入次数的锁错误记录。
pub type LockErrorRecords = HashMap<String, i64>;
/// 测试钩子：从 channel 接收一次性回调并执行。
pub type TestHook = Receiver<Box<dyn FnOnce() + Send>>;

/// Typed adapter for Go's Context.Value/SetValue pair. Session implementations
/// keep ownership of the map and can expose their existing value store here.
///
/// 对应 Go `Context.Value` / `SetValue`：会话实现持有映射并在此暴露访问接口。
pub trait SessionValueStore {
    fn Value(&self, key: &str) -> Option<&dyn Any>;
    fn ValueMut(&mut self, key: &str) -> Option<&mut dyn Any>;
    fn SetValue(&mut self, key: &'static str, value: SessionValue);
}

/// Extra observations required only by the failpoint assertion that verifies
/// local temporary tables are preserved in a session-extended InfoSchema.
///
/// 额外观测：校验本地临时表是否仍挂在会话扩展的 InfoSchema 上。
pub trait TxnAssertionContext: SessionValueStore + TxnManagerContext {
    /// 本地临时表对象的身份标识（用于指针/身份相等比较）。
    fn LocalTemporaryTablesIdentity(&self) -> Option<usize> {
        None
    }

    /// 事务 InfoSchema 中本地临时表的身份标识。
    fn TxnInfoSchemaLocalTemporaryTablesIdentity(&mut self) -> Option<usize> {
        None
    }
}

/// 将断点/钩子键转为字符串，便于在会话值存储中查找。
pub trait HookKey {
    fn String(&self) -> String;
}

impl HookKey for str {
    fn String(&self) -> String {
        self.to_owned()
    }
}

impl HookKey for String {
    fn String(&self) -> String {
        self.clone()
    }
}

/// 向会话断言记录表写入一条命名观测值；若表不存在则先创建。
pub fn RecordAssert<C: SessionValueStore + ?Sized>(
    sctx: &mut C,
    name: impl Into<String>,
    value: SessionValue,
) {
    // 惰性初始化断言记录表
    if sctx
        .Value(AssertRecordsKey)
        .and_then(|value| value.downcast_ref::<AssertRecords>())
        .is_none()
    {
        sctx.SetValue(AssertRecordsKey, Box::new(AssertRecords::new()));
    }
    sctx.ValueMut(AssertRecordsKey)
        .and_then(|value| value.downcast_mut::<AssertRecords>())
        .expect("assert record store was just initialized")
        .insert(name.into(), value);
}

/// 断言当前事务 InfoSchema 的元数据版本与期望一致，并校验本地临时表身份共享。
pub fn AssertTxnManagerInfoSchema<C: TxnAssertionContext + ?Sized>(
    sctx: &mut C,
    expected: Option<InfoSchemaRef>,
) {
    let stored_expected = sctx
        .Value(AssertTxnInfoSchemaKey)
        .and_then(|value| value.downcast_ref::<InfoSchemaRef>())
        .cloned();
    let local_tables = sctx.LocalTemporaryTablesIdentity();
    // 本地临时表必须与事务 InfoSchema 共享同一实例
    if let Some(expected_identity) = local_tables {
        assert_eq!(
            sctx.TxnInfoSchemaLocalTemporaryTablesIdentity(),
            Some(expected_identity),
            "local temporary tables must be shared with transaction InfoSchema"
        );
    }

    for expected in [expected, stored_expected].into_iter().flatten() {
        assert_eq!(
            GetTxnManager(sctx).GetTxnInfoSchema().SchemaMetaVersion(),
            expected.SchemaMetaVersion(),
            "transaction InfoSchema version mismatch"
        );
    }
}

/// 断言事务管理器当前语句读时间戳（ReadTS）等于期望值。
pub fn AssertTxnManagerReadTS<C: TxnManagerContext + ?Sized>(sctx: &mut C, expected: u64) {
    let actual = GetTxnManager(sctx)
        .GetStmtReadTS()
        .unwrap_or_else(|error| panic!("get transaction read timestamp: {error}"));
    assert_eq!(actual, expected, "transaction read timestamp mismatch");
}

/// 记录某类锁错误进入断言路径的次数（按名称累加）。
pub fn AddAssertEntranceForLockError<C: SessionValueStore + ?Sized>(
    sctx: &mut C,
    name: impl Into<String>,
) {
    if sctx
        .Value(AssertLockErr)
        .and_then(|value| value.downcast_ref::<LockErrorRecords>())
        .is_none()
    {
        sctx.SetValue(AssertLockErr, Box::new(LockErrorRecords::new()));
    }
    let records = sctx
        .ValueMut(AssertLockErr)
        .and_then(|value| value.downcast_mut::<LockErrorRecords>())
        .expect("lock error record store was just initialized");
    *records.entry(name.into()).or_default() += 1;
}

/// 将会话中指定键的 `u64` 计数器加一；不存在时从 0 起算。
fn increment_u64<C: SessionValueStore + ?Sized>(sctx: &mut C, key: &'static str) {
    let next = sctx
        .Value(key)
        .and_then(|value| value.downcast_ref::<u64>())
        .copied()
        .unwrap_or_default()
        + 1;
    sctx.SetValue(key, Box::new(next));
}

/// TSO 请求次数 +1。
pub fn TsoRequestCountInc<C: SessionValueStore + ?Sized>(sctx: &mut C) {
    increment_u64(sctx, TsoRequestCount);
}

/// TSO 等待次数 +1。
pub fn TsoWaitCountInc<C: SessionValueStore + ?Sized>(sctx: &mut C) {
    increment_u64(sctx, TsoWaitCount);
}

/// 使用常量 TSO 的次数 +1。
pub fn TsoUseConstantCountInc<C: SessionValueStore + ?Sized>(sctx: &mut C) {
    increment_u64(sctx, TsoUseConstantCount);
}

/// 语句重试回调调用次数 +1（使用 `i64` 以匹配 Go 侧类型）。
pub fn OnStmtRetryCountInc<C: SessionValueStore + ?Sized>(sctx: &mut C) {
    let next = sctx
        .Value(CallOnStmtRetryCount)
        .and_then(|value| value.downcast_ref::<i64>())
        .copied()
        .unwrap_or_default()
        + 1;
    sctx.SetValue(CallOnStmtRetryCount, Box::new(next));
}

/// 执行测试钩子：若会话存有对应 `TestHook`，则在超时内接收并运行回调。
pub fn ExecTestHook<C, K>(sctx: &C, hook_key: &K)
where
    C: SessionValueStore + ?Sized,
    K: HookKey + ?Sized,
{
    let key = hook_key.String();
    let Some(receiver) = sctx
        .Value(&key)
        .and_then(|value| value.downcast_ref::<TestHook>())
    else {
        return;
    };
    // 最长等待 10 秒，避免测试死锁时无限挂起
    let hook = receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| panic!("timeout waiting for test hook {key}"));
    hook();
}
