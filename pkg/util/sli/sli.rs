// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 事务写入吞吐 SLI 累计与上报。
//
// 由 `pkg/util/sli/sli.go` 迁移。在语句执行过程中累计写入大小、键数与耗时，
// 事务提交（`inTxn=false`）时按小事务耗时或写入吞吐上报指标，并可经 failpoint
// 跳过 Reset 以便测试断言。

#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use crate::{failpoint, metrics};
use std::time::Duration;

/// 一笔事务内写入吞吐 SLI 的累计状态。
// TxnWriteThroughputSLI uses to report transaction write throughput metrics for SLI.
// TxnWriteThroughputSLI 保存一笔事务内写入吞吐 SLI 所需的累计状态。
// 字段顺序保持 Go 结构体一致；Go 的 int 在这里临时映射为 isize，time.Duration 映射为 Duration。
#[derive(Default)]
pub struct TxnWriteThroughputSLI {
    invalid: bool,
    affectRow: u64,
    writeSize: isize,
    readKeys: isize,
    writeKeys: isize,
    writeTime: Duration,
}

impl TxnWriteThroughputSLI {
    /// 记录写语句耗时与影响行数；事务结束（`inTxn=false`）时上报指标并重置。
    // FinishExecuteStmt records the cost for write statement which affect rows more than 0.
    // And report metrics when the transaction is committed.
    // FinishExecuteStmt 记录写语句耗时和影响行数，并在事务结束时上报指标。
    // inTxn=false 对应 Go 里“当前语句已经结束最后一个事务”的判断。
    pub fn FinishExecuteStmt(&mut self, cost: Duration, affectRow: u64, inTxn: bool) {
        if affectRow > 0 {
            self.writeTime += cost;
            self.affectRow += affectRow;
        }

        // Currently not in transaction means the last transaction is finish, should report metrics and reset data.
        // Go 代码在非事务内状态下认为本次事务已经完成，需要补记 commit 耗时、上报并重置。
        if !inTxn {
            if affectRow == 0 {
                // AffectRows is 0 when statement is commit.
                // commit 语句本身不影响行数，但它的耗时属于本事务写入路径的一部分。
                self.writeTime += cost;
            }
            // Report metrics after commit this transaction.
            // 指标上报在 commit 后进行；保留 metrics 调用形状，不接入真实指标系统。
            self.reportMetric();

            // Skip reset for test.
            // 对应 Go failpoint.Inject("CheckTxnWriteThroughput", func(){ failpoint.Return() })。
            // 测试打开 failpoint 时这里会提前返回，从而保留累计状态给断言使用。
            if failpoint::inject("CheckTxnWriteThroughput") {
                return;
            }

            // Reset for next transaction.
            // 正常路径为下一笔事务清空状态。
            self.Reset();
        }
    }

    /// 累加本事务读取的 key 数。
    // AddReadKeys adds the read keys.
    // AddReadKeys 累加本事务读取 key 数；Go 参数是 int64，原实现显式转换为 int。
    pub fn AddReadKeys(&mut self, readKeys: i64) {
        self.readKeys += readKeys as isize;
    }

    /// 累加事务写入字节数与写入 key 数。
    // AddTxnWriteSize adds the transaction write size and keys.
    // AddTxnWriteSize 累加事务写入字节数和写入 key 数，字段类型沿用 Go int 的机器字长语义。
    pub fn AddTxnWriteSize(&mut self, size: isize, keys: isize) {
        self.writeSize += size;
        self.writeKeys += keys;
    }

    /// 按事务大小选择上报小事务耗时或写入吞吐；无效事务直接跳过。
    // reportMetric 根据事务大小选择小事务耗时指标或写入吞吐指标。
    // Go 中这是未导出方法；同样保持私有方法形状。
    fn reportMetric(&self) {
        if self.IsInvalid() {
            return;
        }
        if self.IsSmallTxn() {
            // time.Duration.Seconds() 在 实现中用 as_secs_f64() 表达秒级浮点值。
            metrics::SmallTxnWriteDuration.Observe(self.writeTime.as_secs_f64());
        } else {
            // 写入吞吐量保持 Go 公式：writeSize / writeTime.Seconds()。
            metrics::TxnWriteThroughput
                .Observe(self.writeSize as f64 / self.writeTime.as_secs_f64());
        }
    }

    /// 标记当前事务不适合上报 SLI 指标。
    // SetInvalid marks this transaction is invalid to report SLI metrics.
    // SetInvalid 标记当前事务不适合上报 SLI，后续 reportMetric 会直接跳过。
    pub fn SetInvalid(&mut self) {
        self.invalid = true;
    }

    /// 判断事务是否无效（显式 invalid、读多于写、无写入大小或无写入耗时）。
    // IsInvalid checks the transaction is valid to report SLI metrics. Currently, the following case will cause invalid:
    // 1. The transaction contains `insert|replace into ... select ... from ...` statement.
    // 2. The write SQL statement has more read keys than write keys.
    // IsInvalid 保留 Go 的有效性判断：显式 invalid、读 key 多于写 key、无写入大小或无写入耗时都会跳过指标。
    pub fn IsInvalid(&self) -> bool {
        self.invalid
            || self.readKeys > self.writeKeys
            || self.writeSize == 0
            || self.writeTime == Duration::ZERO
    }
}

/// 小事务影响行数上限（含）：≤20 行视为小事务。
const smallTxnAffectRow: u64 = 20;
/// 小事务写入大小上限（含）：≤1MB 视为小事务。
const smallTxnSize: isize = 1 * 1024 * 1024; // 1MB

impl TxnWriteThroughputSLI {
    /// 按影响行数与写入大小判断是否为小事务（测试导出）。
    // IsSmallTxn exports for testing.
    // IsSmallTxn 按影响行数和写入大小判断是否是小事务；该方法在 Go 中为测试导出。
    pub fn IsSmallTxn(&self) -> bool {
        self.affectRow <= smallTxnAffectRow && self.writeSize <= smallTxnSize
    }

    /// 清空累计状态，供下一笔事务重新统计（测试导出）。
    // Reset exports for testing.
    // Reset 清空累计状态，保持 Go 字段归零顺序，供下一笔事务重新统计；Go 中同样为测试导出。
    pub fn Reset(&mut self) {
        self.invalid = false;
        self.affectRow = 0;
        self.writeSize = 0;
        self.readKeys = 0;
        self.writeKeys = 0;
        self.writeTime = Duration::ZERO;
    }

    /// 生成与 Go `fmt.Sprintf` + `Duration.String()` 对齐的调试字符串（测试导出）。
    // String exports for testing.
    // String 生成测试用调试字符串；Go 使用 fmt.Sprintf 和 time.Duration.String()。
    pub fn String(&self) -> String {
        format!(
            "invalid: {}, affectRow: {}, writeSize: {}, readKeys: {}, writeKeys: {}, writeTime: {}",
            self.invalid,
            self.affectRow,
            self.writeSize,
            self.readKeys,
            self.writeKeys,
            format_go_duration(self.writeTime)
        )
    }
}

/// 将 `Duration` 格式化为接近 Go `time.Duration.String()` 的文本。
fn format_go_duration(duration: Duration) -> String {
    let nanos = duration.as_nanos();
    if nanos == 0 {
        return "0s".to_owned();
    }

    const NANOSECOND: u128 = 1;
    const MICROSECOND: u128 = 1_000 * NANOSECOND;
    const MILLISECOND: u128 = 1_000 * MICROSECOND;
    const SECOND: u128 = 1_000 * MILLISECOND;
    const MINUTE: u128 = 60 * SECOND;
    const HOUR: u128 = 60 * MINUTE;

    /// 按单位与小数位数格式化；去掉尾部多余 0。
    fn decimal(value: u128, unit: u128, fractional_digits: usize, suffix: &str) -> String {
        let whole = value / unit;
        let remainder = value % unit;
        if remainder == 0 {
            return format!("{whole}{suffix}");
        }
        let scale = 10_u128.pow(fractional_digits as u32);
        let mut fraction = format!("{:0fractional_digits$}", remainder * scale / unit);
        while fraction.ends_with('0') {
            fraction.pop();
        }
        format!("{whole}.{fraction}{suffix}")
    }

    // 按 Go 规则选择 ns / µs / ms / 或 h+m+s 组合格式。
    if nanos < MICROSECOND {
        return format!("{nanos}ns");
    }
    if nanos < MILLISECOND {
        return decimal(nanos, MICROSECOND, 3, "µs");
    }
    if nanos < SECOND {
        return decimal(nanos, MILLISECOND, 6, "ms");
    }

    let hours = nanos / HOUR;
    let after_hours = nanos % HOUR;
    let minutes = after_hours / MINUTE;
    let after_minutes = after_hours % MINUTE;
    let seconds = decimal(after_minutes, SECOND, 9, "s");
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}")
    } else {
        seconds
    }
}
