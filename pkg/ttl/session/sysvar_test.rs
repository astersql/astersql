// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL 相关系统变量（sysvar）回归测试。
//
// 覆盖 `tidb_ttl_job_enable`、扫描/删除 batch size 与 delete rate limit 的
// 读写与越界钳制（clamp）行为，对应 Go SysVar 校验。

use std::sync::{Mutex, MutexGuard};

use astersql_sessionctx_vardef as vardef;

/// 串行化改写全局 vardef 的测试锁。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 获取测试互斥锁（poison 时接管）。
fn lock_tests() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 测试结束时恢复 EnableTTLJob 原值。
struct RestoreBool {
    value: bool,
}

impl Drop for RestoreBool {
    fn drop(&mut self) {
        vardef::EnableTTLJob.Store(self.value);
    }
}

/// 测试结束时恢复某个 AtomicI64 系统变量原值。
struct RestoreI64 {
    target: &'static vardef::AtomicI64Value,
    value: i64,
}

impl Drop for RestoreI64 {
    fn drop(&mut self) {
        self.target.Store(self.value);
    }
}

/// Clamp helper matching Go SysVar.checkInt64SystemVar MinValue/MaxValue.
/// 将整型钳制到 [min, max]，对齐 Go SysVar.checkInt64SystemVar。
fn normalize_int(value: i64, min: i64, max: i64) -> i64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// SET @@global.tidb_ttl_job_enable / GetGlobal path (vardef.EnableTTLJob).
/// 设置并返回 `"0"`/`"1"` 字符串形式。
fn set_ttl_job_enable(on: bool) -> String {
    vardef::EnableTTLJob.Store(on);
    if on { "1" } else { "0" }.to_string()
}

/// SET @@global.tidb_ttl_scan_batch_size with TypeInt clamp then Store.
/// 钳制后写入 TTLScanBatchSize 并返回字符串。
fn set_ttl_scan_batch_size(raw: i64) -> String {
    let val = normalize_int(
        raw,
        vardef::DefTiDBTTLScanBatchMinSize,
        vardef::DefTiDBTTLScanBatchMaxSize,
    );
    vardef::TTLScanBatchSize.Store(val);
    val.to_string()
}

/// SET @@global.tidb_ttl_delete_batch_size with TypeInt clamp then Store.
/// 钳制后写入 TTLDeleteBatchSize 并返回字符串。
fn set_ttl_delete_batch_size(raw: i64) -> String {
    let val = normalize_int(
        raw,
        vardef::DefTiDBTTLDeleteBatchMinSize,
        vardef::DefTiDBTTLDeleteBatchMaxSize,
    );
    vardef::TTLDeleteBatchSize.Store(val);
    val.to_string()
}

/// SET @@global.tidb_ttl_delete_rate_limit with MinValue 0 then Store.
/// 下限 0 钳制后写入 TTLDeleteRateLimit。
fn set_ttl_delete_rate_limit(raw: i64) -> String {
    let val = normalize_int(raw, 0, i64::MAX);
    vardef::TTLDeleteRateLimit.Store(val);
    val.to_string()
}

/// 布尔值的 `"0"`/`"1"` 查询展示。
fn bool_query(v: bool) -> String {
    if v { "1" } else { "0" }.to_string()
}

/// 开关 tidb_ttl_job_enable 的开/关往返。
#[test]
fn TestSysVarTTLJobEnable() {
    let _guard = lock_tests();
    let _restore = RestoreBool {
        value: vardef::EnableTTLJob.Load(),
    };

    assert_eq!(set_ttl_job_enable(false), "0");
    assert!(!vardef::EnableTTLJob.Load());
    assert_eq!(bool_query(vardef::EnableTTLJob.Load()), "0");

    assert_eq!(set_ttl_job_enable(true), "1");
    assert!(vardef::EnableTTLJob.Load());
    assert_eq!(bool_query(vardef::EnableTTLJob.Load()), "1");

    assert_eq!(set_ttl_job_enable(false), "0");
    assert!(!vardef::EnableTTLJob.Load());
    assert_eq!(bool_query(vardef::EnableTTLJob.Load()), "0");
}

/// 扫描 batch size 正常值与上下界钳制。
#[test]
fn TestSysVarTTLScanBatchSize() {
    let _guard = lock_tests();
    let _restore = RestoreI64 {
        target: &vardef::TTLScanBatchSize,
        value: vardef::TTLScanBatchSize.Load(),
    };

    assert_eq!(set_ttl_scan_batch_size(789), "789");
    assert_eq!(vardef::TTLScanBatchSize.Load(), 789);
    assert_eq!(vardef::TTLScanBatchSize.Load().to_string(), "789");

    // Go clamps 0 to MinValue 1.
    assert_eq!(set_ttl_scan_batch_size(0), "1");
    assert_eq!(vardef::TTLScanBatchSize.Load(), 1);
    assert_eq!(vardef::TTLScanBatchSize.Load().to_string(), "1");

    let max_val = vardef::DefTiDBTTLScanBatchMaxSize;
    assert_eq!(set_ttl_scan_batch_size(max_val + 1), max_val.to_string());
    assert_eq!(vardef::TTLScanBatchSize.Load(), max_val);
    assert_eq!(
        vardef::TTLScanBatchSize.Load().to_string(),
        max_val.to_string()
    );
}

/// 删除 batch size 正常值与上下界钳制。
#[test]
fn TestSysVarTTLScanDeleteBatchSize() {
    let _guard = lock_tests();
    let _restore = RestoreI64 {
        target: &vardef::TTLDeleteBatchSize,
        value: vardef::TTLDeleteBatchSize.Load(),
    };

    assert_eq!(set_ttl_delete_batch_size(789), "789");
    assert_eq!(vardef::TTLDeleteBatchSize.Load(), 789);
    assert_eq!(vardef::TTLDeleteBatchSize.Load().to_string(), "789");

    assert_eq!(set_ttl_delete_batch_size(0), "1");
    assert_eq!(vardef::TTLDeleteBatchSize.Load(), 1);
    assert_eq!(vardef::TTLDeleteBatchSize.Load().to_string(), "1");

    let max_val = vardef::DefTiDBTTLDeleteBatchMaxSize;
    assert_eq!(set_ttl_delete_batch_size(max_val + 1), max_val.to_string());
    assert_eq!(vardef::TTLDeleteBatchSize.Load(), max_val);
    assert_eq!(
        vardef::TTLDeleteBatchSize.Load().to_string(),
        max_val.to_string()
    );
}

/// 删除速率限制默认值、正常值与负值钳制到 0。
#[test]
fn TestSysVarTTLScanDeleteLimit() {
    let _guard = lock_tests();
    let _restore = RestoreI64 {
        target: &vardef::TTLDeleteRateLimit,
        value: vardef::TTLDeleteRateLimit.Load(),
    };

    assert_eq!(vardef::TTLDeleteRateLimit.Load().to_string(), "0");

    assert_eq!(set_ttl_delete_rate_limit(100_000), "100000");
    assert_eq!(vardef::TTLDeleteRateLimit.Load(), 100_000);
    assert_eq!(vardef::TTLDeleteRateLimit.Load().to_string(), "100000");

    assert_eq!(set_ttl_delete_rate_limit(0), "0");
    assert_eq!(vardef::TTLDeleteRateLimit.Load(), 0);
    assert_eq!(vardef::TTLDeleteRateLimit.Load().to_string(), "0");

    // Go clamps -1 to MinValue 0.
    assert_eq!(set_ttl_delete_rate_limit(-1), "0");
    assert_eq!(vardef::TTLDeleteRateLimit.Load(), 0);
    assert_eq!(vardef::TTLDeleteRateLimit.Load().to_string(), "0");
}
