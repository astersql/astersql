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

// 隔离级别 metrics 迁移期单元测试。
//
// 验证 `init_metrics_vars` 能按 Go 标签值绑定计数器，以及源 `CounterVec` 更换后
// 句柄会重绑到新源（旧源不再收到 inc）。

use std::sync::Mutex;

/// 串行化访问全局 `static mut` 指标，避免并行测试互相覆盖。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 构造与 Go 描述符一致的 `rc_check_ts_conflict_total{type}` CounterVec。
fn fresh_source_counter() -> prometheus::CounterVec {
    prometheus::CounterVec::new(
        prometheus::Opts::new(
            "rc_check_ts_conflict_total",
            "Counter of WriteConflict caused by RCCheckTS.",
        ),
        &["type"],
    )
    .expect("the Go metric descriptor is valid")
}

/// 注入源计数器后初始化，确认 read/write 标签各自独立累加。
#[test]
fn init_metrics_vars_binds_the_two_go_label_values() {
    let _guard = TEST_LOCK.lock().expect("test lock is not poisoned");
    let source = fresh_source_counter();
    unsafe {
        astersql_sessiontxn_isolation_metrics::metrics::RCCheckTSWriteConfilictCounter =
            Some(source.clone());
    }

    astersql_sessiontxn_isolation_metrics::isolation_metrics::init_metrics_vars();

    unsafe {
        astersql_sessiontxn_isolation_metrics::isolation_metrics::RC_READ_CHECK_TS_WRITE_CONFLICT_COUNTER
            .as_ref()
            .expect("read counter is initialized")
            .inc_by(2.0);
        astersql_sessiontxn_isolation_metrics::isolation_metrics::RC_WRITE_CHECK_TS_WRITE_CONFLICT_COUNTER
            .as_ref()
            .expect("write counter is initialized")
            .inc_by(3.0);
    }

    assert_eq!(
        source
            .with_label_values(&[astersql_sessiontxn_isolation_metrics::metrics::LblRCReadCheckTS])
            .get(),
        2.0
    );
    assert_eq!(
        source
            .with_label_values(&[astersql_sessiontxn_isolation_metrics::metrics::LblRCWriteCheckTS])
            .get(),
        3.0
    );
}

/// 更换上游 CounterVec 并再次 init 后，inc 应落到新源而非旧源。
#[test]
fn init_metrics_vars_rebinds_handles_after_the_source_changes() {
    let _guard = TEST_LOCK.lock().expect("test lock is not poisoned");
    let first = fresh_source_counter();
    unsafe {
        astersql_sessiontxn_isolation_metrics::metrics::RCCheckTSWriteConfilictCounter =
            Some(first.clone());
    }
    astersql_sessiontxn_isolation_metrics::isolation_metrics::init_metrics_vars();

    let second = fresh_source_counter();
    unsafe {
        astersql_sessiontxn_isolation_metrics::metrics::RCCheckTSWriteConfilictCounter =
            Some(second.clone());
    }
    astersql_sessiontxn_isolation_metrics::isolation_metrics::init_metrics_vars();
    unsafe {
        astersql_sessiontxn_isolation_metrics::isolation_metrics::RC_READ_CHECK_TS_WRITE_CONFLICT_COUNTER
            .as_ref()
            .expect("read counter is rebound")
            .inc();
    }

    assert_eq!(
        first
            .with_label_values(&[astersql_sessiontxn_isolation_metrics::metrics::LblRCReadCheckTS])
            .get(),
        0.0
    );
    assert_eq!(
        second
            .with_label_values(&[astersql_sessiontxn_isolation_metrics::metrics::LblRCReadCheckTS])
            .get(),
        1.0
    );
}
