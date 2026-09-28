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

// ingest 测试辅助模块的 Rust 单元测试。
//
// 通过内存运行时模拟 Go 版 helper 操作的进程级状态，验证 mock 后端注入、幂等恢复，
// 以及测试进程退出前的资源泄漏检查顺序，避免测试依赖真实存储或直接终止进程。

use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::{Arc, Mutex};

use crate::{
    BackendRoots, CheckIngestLeakageForTest, IngestTestError, IngestTestRuntime,
    InjectMockBackendCtx,
};

#[derive(Clone, Debug)]
/// 可复制的运行时状态快照，供断言注入、恢复和泄漏报告产生的副作用。
struct RuntimeState {
    roots: BackendRoots,
    initialized: bool,
    mock_enabled: bool,
    tracker_count: usize,
    backend_count: usize,
    jobs: Vec<String>,
    reports: Vec<String>,
}

/// 用互斥锁串行化进程级状态变更的内存运行时替身。
struct MockRuntime {
    state: Mutex<RuntimeState>,
}

impl MockRuntime {
    fn new() -> Self {
        Self {
            state: Mutex::new(RuntimeState {
                roots: BackendRoots {
                    disk_path: "old-root".into(),
                    memory_limit: 64,
                },
                initialized: false,
                mock_enabled: false,
                tracker_count: 0,
                backend_count: 0,
                jobs: Vec::new(),
                reports: Vec::new(),
            }),
        }
    }

    fn state(&self) -> RuntimeState {
        self.state.lock().unwrap().clone()
    }
}

#[derive(Debug)]
/// 将不会返回的进程退出转换为可由测试捕获并检查的 panic 载荷。
struct Exit(i32);

impl IngestTestRuntime for MockRuntime {
    fn snapshot_roots(&self) -> Result<BackendRoots, IngestTestError> {
        Ok(self.state.lock().unwrap().roots.clone())
    }

    fn enable_mock_backend(&self, _store_id: &str) -> Result<(), IngestTestError> {
        self.state.lock().unwrap().mock_enabled = true;
        Ok(())
    }

    fn disable_mock_backend(&self) -> Result<(), IngestTestError> {
        self.state.lock().unwrap().mock_enabled = false;
        Ok(())
    }

    fn replace_roots(&self, roots: BackendRoots) -> Result<(), IngestTestError> {
        self.state.lock().unwrap().roots = roots;
        Ok(())
    }

    fn set_initialized(&self, initialized: bool) -> Result<(), IngestTestError> {
        self.state.lock().unwrap().initialized = initialized;
        Ok(())
    }

    fn tracker_count(&self) -> usize {
        self.state.lock().unwrap().tracker_count
    }

    fn backend_count(&self) -> usize {
        self.state.lock().unwrap().backend_count
    }

    fn registered_jobs(&self) -> Vec<String> {
        self.state.lock().unwrap().jobs.clone()
    }

    fn report_leak(&self, message: &str) {
        self.state.lock().unwrap().reports.push(message.into());
    }

    fn exit(&self, code: i32) -> ! {
        panic_any(Exit(code))
    }
}

/// 执行泄漏检查并捕获 mock `exit`，从而取得原本不会返回的退出码。
fn run_leak_check(runtime: &MockRuntime, exit_code: i32) -> i32 {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        CheckIngestLeakageForTest(runtime, exit_code)
    }))
    .unwrap_err();
    panic.downcast_ref::<Exit>().unwrap().0
}

#[test]
/// 验证注入后的全局状态、完整恢复结果，以及重复恢复的幂等性。
fn inject_and_restore_match_go_global_state_transitions() {
    let runtime = Arc::new(MockRuntime::new());
    let runtime_boundary: Arc<dyn IngestTestRuntime> = runtime.clone();
    let mut guard = InjectMockBackendCtx(runtime_boundary, "store", "temporary-root").unwrap();

    let injected = runtime.state();
    assert!(injected.initialized);
    assert!(injected.mock_enabled);
    assert_eq!(injected.roots.disk_path, "temporary-root");
    assert_eq!(injected.roots.memory_limit, i64::MAX as u64);

    guard.restore().unwrap();
    let restored = runtime.state();
    assert!(!restored.initialized);
    assert!(!restored.mock_enabled);
    assert_eq!(
        restored.roots,
        BackendRoots {
            disk_path: "old-root".into(),
            memory_limit: 64
        }
    );
    guard.restore().unwrap();
}

#[test]
/// 验证 Go helper 的泄漏报告优先级，并保留已有非零退出码。
fn leakage_check_matches_go_priority_exit_codes_and_messages() {
    let runtime = MockRuntime::new();
    {
        let mut state = runtime.state.lock().unwrap();
        state.tracker_count = 1;
        state.backend_count = 1;
        state.jobs = vec!["job-a".into()];
    }
    assert_eq!(run_leak_check(&runtime, 0), 1);
    assert_eq!(
        runtime.state().reports,
        vec!["add index leakage check failed: disk usage tracker leak"]
    );

    let runtime = MockRuntime::new();
    {
        let mut state = runtime.state.lock().unwrap();
        state.jobs = vec!["job-a".into(), "job-b".into()];
    }
    assert_eq!(run_leak_check(&runtime, 0), 1);
    assert_eq!(
        runtime.state().reports,
        vec!["add index metrics leakage: [job-a job-b]"]
    );

    let runtime = MockRuntime::new();
    runtime.state.lock().unwrap().backend_count = 1;
    assert_eq!(run_leak_check(&runtime, 0), 1);
    assert_eq!(
        runtime.state().reports,
        vec!["add index leakage check failed: backend context leak"]
    );

    let runtime = MockRuntime::new();
    assert_eq!(run_leak_check(&runtime, 0), 0);
    assert!(runtime.state().reports.is_empty());

    let runtime = MockRuntime::new();
    runtime.state.lock().unwrap().tracker_count = 1;
    assert_eq!(run_leak_check(&runtime, 7), 7);
    assert!(runtime.state().reports.is_empty());
}
