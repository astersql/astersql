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

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

// Shared test harness for `pkg/sessiontxn/staleread`.
//
// The Go suite (`main_test.go`) is a one-line `TestMain` that only wires up
// `testkit/testsetup` and `goleak` for the whole package's real
// `testkit`/`mockstore`/`session` based tests (`externalts_test.go`,
// `processor_test.go`, `provider_test.go`). This crate's Rust port of the
// package (`util.rs`, `processor.rs`, `provider.rs`, `failpoint.rs`)
// intentionally decouples the stale-read evaluation/processor/provider
// logic from any concrete session/store implementation behind the
// `SessionBackend` trait, so there is no Rust `sessionctx`/`mockstore` to
// plug into these tests yet (`Cargo.toml` even keeps the would-be
// production dependencies on `astersql-sessionctx`/`astersql-kv`/... behind
// `cfg(any())` for the same reason).
//
// Per the porting instructions for this task we substitute a minimal but
// real `SessionBackend` implementation (`MockBackend`) that behaves like an
// in-memory session/store pair: it evaluates a tiny literal-expression
// syntax (`"datetime:<millis>"`, `"tso:<value>"`, `"str:<value>"`,
// `"null"`, `"err"`) instead of a real SQL expression, tracks a
// caller-configurable "now"/"safe ts" clock the same way a real
// `sessionctx` tracks statement/GC-safe timestamps, and records every call
// it received so tests can assert on side effects the same way the Go
// tests assert on `sessiontxn.AssertStmtStaleness` / TiKV request counts.
// This lets `externalts_test.rs`, `processor_test.rs` and
// `provider_test.rs` exercise the real `StaleReadProcessor`/
// `StalenessTxnContextProvider`/`calculate_as_of_ts_expr`/... state
// machines end to end, without inventing new production semantics.

//
// 中文概述：过期读包的共享测试夹具。
// 用内存 `MockBackend` 模拟会话/存储：迷你字面量表达式语法、可配置时钟、
// 以及调用计数，供 externalts/processor/provider 测试驱动真实状态机。

use std::sync::{Arc, Mutex};

use crate::*;

/// A `Cell<T>`-like container that is `Sync`, so `MockBackend` can satisfy
/// `SessionBackend: Send + Sync` the same way a real session/store would.
/// 可跨线程共享的 Cell 替代品，满足 SessionBackend: Send + Sync。
pub(crate) struct SyncCell<T>(Mutex<T>);

impl<T: Copy> SyncCell<T> {
    /// 以初值构造。
    fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    /// 读取当前值。
    pub(crate) fn get(&self) -> T {
        *self.0.lock().expect("sync cell lock poisoned")
    }

    /// 写入新值。
    pub(crate) fn set(&self, value: T) {
        *self.0.lock().expect("sync cell lock poisoned") = value;
    }
}

/// Every call `MockBackend` observed, in call order, plus how many times
/// each side-effecting method ran. Tests assert against this instead of a
/// real TiKV/PD round trip.
#[derive(Default)]
/// MockBackend 侧效应记录：调用顺序与各方法调用次数。
pub(crate) struct BackendCalls {
    pub(crate) evaluated_expressions: Vec<String>,
    pub(crate) validated_read_ts: Vec<u64>,
    pub(crate) snapshot_info_schema_ts: Vec<u64>,
    pub(crate) commit_before_enter_new_txn_calls: u32,
    pub(crate) create_transaction_ts: Vec<u64>,
    pub(crate) snapshot_with_ts_calls: Vec<u64>,
    pub(crate) external_timestamp_calls: u32,
    pub(crate) stale_timestamp_calls: u32,
}

/// A fake `SessionBackend`: an in-memory stand-in for the TiKV/PD-backed
/// session the real `github.com/pingcap/tidb/pkg/sessiontxn/staleread`
/// package binds against. `evaluate_expression` understands a tiny literal
/// syntax instead of real SQL expressions (this package never parses SQL
/// itself in Go either -- it always receives an already-evaluated
/// `expression.Expression`), and every other method is driven by
/// caller-configurable `Cell`s so tests can force each branch
/// `util.rs`/`processor.rs`/`provider.rs` implement.
/// 假会话后端：内存替代 TiKV/PD，用字面量语法替代真实 SQL 表达式求值。
pub(crate) struct MockBackend {
    pub(crate) now_millis: SyncCell<i64>,
    pub(crate) safe_millis: SyncCell<i64>,
    pub(crate) stale_ts: SyncCell<u64>,
    pub(crate) stale_timestamp_error: Mutex<Option<String>>,
    pub(crate) external_ts: SyncCell<u64>,
    pub(crate) external_timestamp_error: Mutex<Option<String>>,
    pub(crate) reject_read_ts_in_the_future: SyncCell<bool>,
    pub(crate) fail_commit_before_enter_new_txn: SyncCell<bool>,
    pub(crate) fail_create_transaction: SyncCell<bool>,
    pub(crate) fail_snapshot_with_ts: SyncCell<bool>,
    pub(crate) calls: Mutex<BackendCalls>,
}

impl Default for MockBackend {
    /// 默认时钟约 2023-11，safe point 略早，默认拒绝未来读 ts。
    fn default() -> Self {
        Self {
            now_millis: SyncCell::new(1_700_000_000_000),
            safe_millis: SyncCell::new(1_699_999_999_000),
            stale_ts: SyncCell::new(0),
            stale_timestamp_error: Mutex::new(None),
            external_ts: SyncCell::new(0),
            external_timestamp_error: Mutex::new(None),
            reject_read_ts_in_the_future: SyncCell::new(true),
            fail_commit_before_enter_new_txn: SyncCell::new(false),
            fail_create_transaction: SyncCell::new(false),
            fail_snapshot_with_ts: SyncCell::new(false),
            calls: Mutex::new(BackendCalls::default()),
        }
    }
}

impl SessionBackend for MockBackend {
    /// 解析测试字面量：`datetime:` / `tso:` / `str:` / `null` / `err`。
    fn evaluate_expression(&self, expression: &Expression) -> Result<Datum, Error> {
        self.calls
            .lock()
            .expect("mock backend calls lock poisoned")
            .evaluated_expressions
            .push(expression.0.clone());
        // 强制求值失败分支。
        if expression.0 == "err" {
            return Err(Error::as_of("mock backend: expression evaluation failed"));
        }
        if expression.0 == "null" {
            return Ok(Datum::Null);
        }
        // 毫秒日期时间字面量。
        if let Some(rest) = expression.0.strip_prefix("datetime:") {
            let millis: i64 = rest
                .parse()
                .expect("test datetime literal must be a valid i64 millisecond value");
            return Ok(Datum::DateTimeMillis(millis));
        }
        // 原始 TSO 字面量。
        if let Some(rest) = expression.0.strip_prefix("tso:") {
            let tso: u64 = rest
                .parse()
                .expect("test tso literal must be a valid u64 value");
            return Ok(Datum::Uint(tso));
        }
        if let Some(rest) = expression.0.strip_prefix("str:") {
            return Ok(Datum::String(rest.to_owned()));
        }
        // Unprefixed values represent string literals, matching the Go
        // parser's Datum for inputs such as `42`, `0`, or an invalid date.
        Ok(Datum::String(expression.0.clone()))
    }

    fn parse_datetime_millis(&self, datum: &Datum) -> Result<i64, Error> {
        match datum {
            Datum::DateTimeMillis(millis) => Ok(*millis),
            Datum::String(value) => parse_mysql_datetime_millis(value)
                .ok_or_else(|| Error::as_of("mock backend: value is not a datetime literal")),
            Datum::Bytes(value) => {
                let value = std::str::from_utf8(value)
                    .map_err(|_| Error::as_of("mock backend: value is not a datetime literal"))?;
                parse_mysql_datetime_millis(value)
                    .ok_or_else(|| Error::as_of("mock backend: value is not a datetime literal"))
            }
            _ => Err(Error::as_of(
                "mock backend: value is not a datetime literal",
            )),
        }
    }

    /// 可选拒绝物理时间晚于 now 的读 ts。
    fn validate_snapshot_read_ts(&self, ts: u64) -> Result<(), Error> {
        self.calls
            .lock()
            .expect("mock backend calls lock poisoned")
            .validated_read_ts
            .push(ts);
        if self.reject_read_ts_in_the_future.get() && extract_physical(ts) > self.now_millis.get() {
            return Err(Error::as_of(
                "mock backend: snapshot read ts is in the future",
            ));
        }
        Ok(())
    }

    /// 返回可注入的 stale_ts，或预设错误。
    fn stale_timestamp(&self) -> Result<u64, Error> {
        self.calls
            .lock()
            .expect("mock backend calls lock poisoned")
            .stale_timestamp_calls += 1;
        if let Some(message) = self
            .stale_timestamp_error
            .lock()
            .expect("stale timestamp error lock poisoned")
            .clone()
        {
            return Err(Error::backend(message));
        }
        Ok(self.stale_ts.get())
    }

    fn statement_timestamp_millis(&self) -> Result<i64, Error> {
        Ok(self.now_millis.get())
    }

    fn statement_min_safe_millis(&self) -> Result<i64, Error> {
        Ok(self.safe_millis.get())
    }

    /// 返回可注入的 external_ts，或预设错误。
    fn external_timestamp(&self) -> Result<u64, Error> {
        self.calls
            .lock()
            .expect("mock backend calls lock poisoned")
            .external_timestamp_calls += 1;
        if let Some(message) = self
            .external_timestamp_error
            .lock()
            .expect("external timestamp error lock poisoned")
            .clone()
        {
            return Err(Error::backend(message));
        }
        Ok(self.external_ts.get())
    }

    /// 构造带 snapshot_ts 的桩 InfoSchema。
    fn snapshot_info_schema(&self, ts: u64) -> Result<InfoSchema, Error> {
        self.calls
            .lock()
            .expect("mock backend calls lock poisoned")
            .snapshot_info_schema_ts
            .push(ts);
        Ok(InfoSchema {
            snapshot_ts: ts,
            local_temporary_tables_attached: false,
        })
    }

    /// 模拟进入新事务前的提交；可强制失败。
    fn commit_before_enter_new_txn(&self) -> Result<(), Error> {
        self.calls
            .lock()
            .expect("mock backend calls lock poisoned")
            .commit_before_enter_new_txn_calls += 1;
        if self.fail_commit_before_enter_new_txn.get() {
            return Err(Error::backend("mock backend: commit before enter failed"));
        }
        Ok(())
    }

    /// 以给定 start_ts 创建桩事务。
    fn create_transaction(&self, ts: u64) -> Result<Transaction, Error> {
        self.calls
            .lock()
            .expect("mock backend calls lock poisoned")
            .create_transaction_ts
            .push(ts);
        if self.fail_create_transaction.get() {
            return Err(Error::backend("mock backend: create transaction failed"));
        }
        Ok(Transaction {
            start_ts: ts,
            valid: true,
            ..Transaction::default()
        })
    }

    /// 以给定 ts 创建桩快照。
    fn snapshot_with_ts(&self, ts: u64) -> Result<Snapshot, Error> {
        self.calls
            .lock()
            .expect("mock backend calls lock poisoned")
            .snapshot_with_ts_calls
            .push(ts);
        if self.fail_snapshot_with_ts.get() {
            return Err(Error::backend("mock backend: snapshot with ts failed"));
        }
        Ok(Snapshot {
            ts,
            ..Snapshot::default()
        })
    }
}

/// Builds a `SessionRef` (the type every `staleread` production function
/// takes) wrapping a fresh `MockBackend`, mirroring the Go tests' use of
/// `testkit.NewTestKit(t, store).Session()`.
/// 构造绑定新鲜 MockBackend 的 SessionRef（对应 testkit Session）。
pub(crate) fn mock_session() -> (SessionRef, Arc<MockBackend>) {
    let backend = Arc::new(MockBackend::default());
    let session = Arc::new(Mutex::new(Session::new(backend.clone())));
    (session, backend)
}

/// 便捷构造 Expression 字面量规格。
pub(crate) fn expr(spec: impl Into<String>) -> Expression {
    Expression(spec.into())
}

/// Parse the UTC MySQL datetime forms used by the Go stale-read tests.
fn parse_mysql_datetime_millis(value: &str) -> Option<i64> {
    let compact_datetime = value.len() == 14 && value.bytes().all(|byte| byte.is_ascii_digit());
    let (date, time) = if compact_datetime {
        (&value[..8], &value[8..])
    } else {
        value.split_once(' ').or_else(|| value.split_once('T'))?
    };
    let date_parts: Vec<_> = date.split('-').collect();
    let (year, month, day) = if date_parts.len() == 3 {
        (
            date_parts[0].parse::<i64>().ok()?,
            date_parts[1].parse::<i64>().ok()?,
            date_parts[2].parse::<i64>().ok()?,
        )
    } else if date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit()) {
        (
            date[0..4].parse::<i64>().ok()?,
            date[4..6].parse::<i64>().ok()?,
            date[6..8].parse::<i64>().ok()?,
        )
    } else {
        return None;
    };

    let (hour, minute, second, fraction) = if compact_datetime {
        (
            time[0..2].parse::<i64>().ok()?,
            time[2..4].parse::<i64>().ok()?,
            time[4..6].parse::<i64>().ok()?,
            "",
        )
    } else {
        let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
        let clock_parts: Vec<_> = clock.split(':').collect();
        if clock_parts.len() != 3 {
            return None;
        }
        (
            clock_parts[0].parse::<i64>().ok()?,
            clock_parts[1].parse::<i64>().ok()?,
            clock_parts[2].parse::<i64>().ok()?,
            fraction,
        )
    };
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return None;
    }
    let millis = match fraction.len() {
        0 => 0,
        1..=3 if fraction.bytes().all(|byte| byte.is_ascii_digit()) => {
            fraction.parse::<i64>().ok()? * 10_i64.pow((3 - fraction.len()) as u32)
        }
        _ => return None,
    };
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1_000 + millis)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's civil-date conversion, yielding days since 1970-01-01.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 {
        year / 400
    } else {
        (year - 399) / 400
    };
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
