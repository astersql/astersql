// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 昂贵查询模块的公共测试入口。
//
//（Rust 无 goroutine 泄漏检测，但常量内容需与 Go 保持一致）。

use std::sync::{Arc, Mutex};

use testsetup::SetupForCommonTest;

use crate::{EventLogger, Handle, LogLevel, ProcessInfo, SessionManager, set_log_level};
use std::time::Duration;

// explicit while exercising the same common-test initialization entry point.

#[test]
fn TestMain() {
    SetupForCommonTest();
}

#[derive(Default)]
struct RecordingLogger {
    warnings: Mutex<Vec<String>>,
}

impl EventLogger for RecordingLogger {
    fn warn(&self, message: &str, _process: &ProcessInfo, _cost: Duration, _detail: &str) {
        self.warnings.lock().unwrap().push(message.to_owned());
    }

    fn info(&self, _message: &str, _connection_id: u64) {}
}

struct SingleProcessManager {
    process: Arc<ProcessInfo>,
}

impl SessionManager for SingleProcessManager {
    fn show_process_list(&self) -> Vec<Arc<ProcessInfo>> {
        vec![Arc::clone(&self.process)]
    }

    fn get_process_info(&self, connection_id: u64) -> Option<Arc<ProcessInfo>> {
        (self.process.id == connection_id).then(|| Arc::clone(&self.process))
    }

    fn kill(&self, _connection_id: u64, _query: bool, _connection: bool, _runaway: bool) {}
}

#[test]
fn log_on_query_exceed_mem_quota_keeps_empty_sql_process() {
    set_log_level(LogLevel::Warn);

    let process = Arc::new(ProcessInfo::new(7, ""));
    let manager = Arc::new(SingleProcessManager {
        process: Arc::clone(&process),
    });
    let logger = Arc::new(RecordingLogger::default());
    let (_exit_sender, exit_receiver) = std::sync::mpsc::channel();
    let handle = Handle::new(exit_receiver).with_observers(
        Arc::new(crate::NoopHistogram),
        Arc::new(crate::NoopHistogram),
        Arc::clone(&logger) as Arc<dyn EventLogger>,
    );
    handle.set_session_manager(manager);

    handle.log_on_query_exceed_mem_quota(7);

    assert_eq!(
        logger.warnings.lock().unwrap().as_slice(),
        ["memory exceeds quota"]
    );
    set_log_level(LogLevel::Info);
}
