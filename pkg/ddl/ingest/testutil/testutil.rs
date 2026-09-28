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

// Ingest 测试辅助：注入 mock 后端、恢复全局状态，并在测试结束时检查资源泄漏。
//
// Ingest（索引批量写入）会维护磁盘路径、内存限额、后端上下文等进程级状态；
// 本模块提供可替换的运行时边界，便于单元测试隔离副作用。

use std::fmt;
use std::sync::Arc;

/// ingest 测试辅助操作失败时的错误包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestTestError(pub String);

impl fmt::Display for IngestTestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for IngestTestError {}

/// 后端根配置：磁盘路径与内存上限。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendRoots {
    /// 本地磁盘存储根路径。
    pub disk_path: String,
    /// 内存使用上限（字节）。
    pub memory_limit: u64,
}

/// Runtime boundary for the global ingest state changed by the Go helper.
/// Implementations must serialize mutations of the global roots and failpoint.
///
/// 全局 ingest 状态变更的运行时边界（对应 Go 侧 helper）。
/// 实现需串行化对全局 roots 与 failpoint（故障注入点）的修改。
pub trait IngestTestRuntime: Send + Sync {
    /// 快照当前后端根配置，供后续恢复。
    fn snapshot_roots(&self) -> Result<BackendRoots, IngestTestError>;
    /// 按 store_id 启用 mock 后端。
    fn enable_mock_backend(&self, store_id: &str) -> Result<(), IngestTestError>;
    /// 关闭 mock 后端。
    fn disable_mock_backend(&self) -> Result<(), IngestTestError>;
    /// 用给定 roots 替换全局后端根配置。
    fn replace_roots(&self, roots: BackendRoots) -> Result<(), IngestTestError>;
    /// 标记 ingest 全局状态是否已初始化。
    fn set_initialized(&self, initialized: bool) -> Result<(), IngestTestError>;
    /// 当前磁盘用量追踪器数量。
    fn tracker_count(&self) -> usize;
    /// 当前后端上下文数量。
    fn backend_count(&self) -> usize;
    /// 已注册的 ingest 任务标识列表。
    fn registered_jobs(&self) -> Vec<String>;
    /// 报告泄漏信息（通常写入日志或测试输出）。
    fn report_leak(&self, message: &str);
    /// 以给定退出码结束进程（对应测试 harness 的退出路径）。
    fn exit(&self, code: i32) -> !;
}

/// Restores every process-global mutation performed by `InjectMockBackendCtx`.
///
/// 在退出作用域时恢复 `InjectMockBackendCtx` 对进程全局状态的全部修改。
pub struct MockBackendGuard {
    runtime: Arc<dyn IngestTestRuntime>,
    old_roots: Option<BackendRoots>,
}

impl MockBackendGuard {
    /// 手动恢复：关闭初始化标志、还原 roots、禁用 mock 后端。
    pub fn restore(&mut self) -> Result<(), IngestTestError> {
        let Some(old_roots) = self.old_roots.take() else {
            return Ok(());
        };
        // 按与注入相反的顺序回滚：先取消初始化，再还原 roots，最后关闭 mock。
        let initialized = self.runtime.set_initialized(false);
        let roots = self.runtime.replace_roots(old_roots);
        let failpoint = self.runtime.disable_mock_backend();
        initialized.and(roots).and(failpoint)
    }
}

impl Drop for MockBackendGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// 注入 mock 后端上下文：启用 mock、标记已初始化，并将 roots 指向临时目录。
///
/// 返回的 `MockBackendGuard` 在 drop 时自动恢复旧状态；任一步失败会尽力回滚已做变更。
#[allow(non_snake_case)]
pub fn InjectMockBackendCtx(
    runtime: Arc<dyn IngestTestRuntime>,
    store_id: &str,
    temporary_directory: impl Into<String>,
) -> Result<MockBackendGuard, IngestTestError> {
    let old_roots = runtime.snapshot_roots()?;
    runtime.enable_mock_backend(store_id)?;
    // 初始化失败时关闭刚启用的 mock，避免留下半开状态。
    if let Err(error) = runtime.set_initialized(true) {
        let _ = runtime.disable_mock_backend();
        return Err(error);
    }
    // 替换 roots 失败时同时撤销初始化与 mock。
    if let Err(error) = runtime.replace_roots(BackendRoots {
        disk_path: temporary_directory.into(),
        memory_limit: i64::MAX as u64,
    }) {
        let _ = runtime.set_initialized(false);
        let _ = runtime.disable_mock_backend();
        return Err(error);
    }
    Ok(MockBackendGuard {
        runtime,
        old_roots: Some(old_roots),
    })
}

/// 测试结束泄漏检查：成功退出码下若仍有 tracker/backend/已注册任务则报泄漏并以 1 退出。
#[allow(non_snake_case)]
pub fn CheckIngestLeakageForTest(runtime: &dyn IngestTestRuntime, exit_code: i32) -> ! {
    if exit_code == 0 {
        // 优先检查磁盘追踪器与后端上下文是否未释放。
        let leak = if runtime.tracker_count() != 0 {
            Some("disk usage tracker")
        } else if runtime.backend_count() != 0 {
            Some("backend context")
        } else {
            None
        };
        if let Some(leak) = leak {
            runtime.report_leak(&format!("add index leakage check failed: {leak} leak"));
            runtime.exit(1);
        }
        // 仍有已注册 add-index 任务指标时同样视为泄漏。
        let jobs = runtime.registered_jobs();
        if !jobs.is_empty() {
            runtime.report_leak(&format!("add index metrics leakage: [{}]", jobs.join(" ")));
            runtime.exit(1);
        }
    }
    runtime.exit(exit_code)
}
