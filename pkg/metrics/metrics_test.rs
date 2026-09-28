// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// External-style smoke tests corresponding to Go `package metrics_test`.
//
// 对应 Go `package metrics_test` 的外部冒烟测试：覆盖 PanicCounter 自增、
// InitMetrics/RegisterMetrics 注册路径，以及执行错误到标签的映射。

use astersql_parser_terror::ErrResultUndetermined;

use crate::main_test::ensure_test_env;
use crate::metrics::{self, LabelDomain, PanicCounter};
use crate::server::ExecuteErrorToLabel;

/// TestMetrics corresponds to Go's TestMetrics: Inc must not panic.
/// 对应 Go TestMetrics：对 PanicCounter 自增不得 panic。
#[test]
fn test_metrics() {
    ensure_test_env();
    PanicCounter.with_label_values(&[LabelDomain]).inc();
}

/// TestRegisterMetrics corresponds to Go's TestRegisterMetrics.
/// 对应 Go TestRegisterMetrics：在独立注册表中完整初始化并注册。
#[test]
fn test_register_metrics() {
    if crate::main_test::run_in_isolated_process("metrics_test::test_register_metrics") {
        return;
    }
    ensure_test_env();
    unsafe {
        metrics::InitMetrics().expect("InitMetrics");
        crate::ddl::BatchAddIdxHistogram
            .as_ref()
            .expect("BatchAddIdxHistogram initialized")
            .with_label_values(&["add-index"])
            .observe(0.001);
        metrics::RegisterMetrics().expect("register all package metrics");
    }

    assert!(
        prometheus::gather()
            .iter()
            .any(|family| family.name() == "tidb_ddl_batch_add_idx_duration_seconds"),
        "RegisterMetrics must expose Go's BatchAddIdxHistogram collector"
    );
}

/// TestExecuteErrorToLabel corresponds to Go's TestExecuteErrorToLabel.
///
/// Go passes `errors.New` / `terror.ErrResultUndetermined` into
/// `ExecuteErrorToLabel(error)`; the Rust port takes the already-extracted RFC
/// code (`None` for plain errors).
///
/// 对应 Go TestExecuteErrorToLabel：普通错误映射 unknown，
/// ErrResultUndetermined 映射其 RFC code。
#[test]
fn test_execute_error_to_label() {
    ensure_test_env();
    assert_eq!("unknown", ExecuteErrorToLabel(None));
    assert_eq!(
        "global:2",
        ExecuteErrorToLabel(Some(ErrResultUndetermined.RFCCode().as_str()))
    );
}
