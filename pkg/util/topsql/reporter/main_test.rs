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

// reporter 包级测试入口：对照 Go `TestMain` 的公共初始化与资源收尾。
//
// Rust 测试框架自行管理进程退出；此处启动并关闭真实 reporter，验证 worker
// 可同步回收且 DataSink 收到关闭通知，避免把包级测试退化为默认值占位。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::datasink::{DataSink, DataSinkError, ReportData};

/// 记录 reporter 关闭回调。
struct ClosingSink(AtomicBool);

impl DataSink for ClosingSink {
    fn try_send(&self, _data: Arc<ReportData>, _deadline: Instant) -> Result<(), DataSinkError> {
        Ok(())
    }

    fn on_reporter_closing(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// 默认载荷为空；启动/关闭 reporter 后 worker 已 join 且 sink 收到关闭通知。
#[test]
#[serial_test::serial]
fn test_main() {
    let data = ReportData::default();
    assert!(data.data_records.is_empty());
    assert!(data.ru_records.is_empty());

    let reporter =
        crate::NewRemoteTopSQLReporter(|plan| Ok(plan.to_owned()), |plan| Ok(plan.to_owned()));
    let sink = Arc::new(ClosingSink(AtomicBool::new(false)));
    reporter.Register(sink.clone()).unwrap();
    reporter.Start();
    reporter.Close();
    assert!(sink.0.load(Ordering::SeqCst));
}
