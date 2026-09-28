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

// 测试环境辅助：并发上限与下一代（nextgen）配置注入。
//
// 对应 Go `testenv`：在 Rust 侧用进程级静态量模拟 `GOMAXPROCS`，
// 并为 nextgen 测试切换 keyspace（键空间，逻辑隔离的数据命名空间）与 DXF 服务作用域；
// 通过 [`TestContext::cleanup`] 在测试结束时恢复全局配置。

use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use astersql_config as config;
use astersql_dxf_framework_handle as handle;
use astersql_keyspace as keyspace;

/// 测试辅助允许的最大并行度上限。
const MAX_TEST_PROCS: usize = 16;

/// 进程级测试并行度上限，对应 Go 的 `GOMAXPROCS` 在测试中的角色。
static TEST_MAX_PROCS: LazyLock<AtomicUsize> = LazyLock::new(|| {
    AtomicUsize::new(
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(MAX_TEST_PROCS),
    )
});
/// The small part of Go's `testing.TB` contract needed by this package.
///
/// Keeping cleanup registration explicit ensures that callers restore global
/// configuration even when a test returns early or panics.
///
/// Go `testing.TB` 契约的最小子集：`helper` 标记辅助栈帧，`cleanup` 注册清理回调，
/// 确保测试提前返回或 panic 时仍能恢复全局配置。
pub trait TestContext {
    /// 标记当前函数为测试辅助，便于失败栈回溯跳过该帧。
    fn helper(&self);
    /// 注册测试结束时执行的清理回调（Send 以便跨线程调度）。
    fn cleanup(&self, cleanup: Box<dyn FnOnce() + Send + 'static>);
}

/// Caps the parallelism value used by AsterSQL test helpers at 16.
///
/// Rust has no mutable process-wide scheduler setting equivalent to Go's
/// `GOMAXPROCS`. Test helpers that choose worker counts use
/// [`MaxProcsForTest`] as their process-wide source of truth.
///
/// 将测试并行度上限设为 `min(可用 CPU 数, 16)`，供选择 worker 数的测试辅助读取。
pub fn SetGOMAXPROCSForTest() {
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    TEST_MAX_PROCS.store(available.min(MAX_TEST_PROCS), Ordering::SeqCst);
}

/// Returns the parallelism cap installed by [`SetGOMAXPROCSForTest`].
///
/// 返回 [`SetGOMAXPROCSForTest`] 安装的并行度上限。
pub fn MaxProcsForTest() -> usize {
    TEST_MAX_PROCS.load(Ordering::SeqCst)
}

/// Runs next-generation tests in the SYSTEM keyspace and DXF service scope.
///
/// 将全局配置切到 SYSTEM keyspace 与 nextgen DXF 服务作用域，并注册 cleanup 以恢复原值。
pub fn UpdateConfigForNextgen(t: &dyn TestContext) {
    t.helper();

    // 保存当前全局配置，供 cleanup 还原。
    let previous_config = config::get_global_config().as_ref().clone();
    t.cleanup(Box::new(move || {
        config::store_global_config(previous_config);
    }));

    // 切换到 SYSTEM 键空间与 nextgen 目标作用域。
    config::update_global(|conf| {
        conf.keyspace_name = keyspace::System.to_owned();
        conf.instance.tidb_service_scope = handle::NEXT_GEN_TARGET_SCOPE.to_owned();
    });
}

/// Returns the service scope installed for the active next-generation test.
///
/// 返回当前 nextgen 测试安装的服务作用域字符串。
pub fn ServiceScopeForTest() -> String {
    config::get_global_config()
        .instance
        .tidb_service_scope
        .clone()
}

/// [`SetGOMAXPROCSForTest`] 的 snake_case 别名。
pub fn set_gomaxprocs_for_test() {
    SetGOMAXPROCSForTest();
}

/// [`MaxProcsForTest`] 的 snake_case 别名。
pub fn max_procs_for_test() -> usize {
    MaxProcsForTest()
}

/// [`ServiceScopeForTest`] 的 snake_case 别名。
pub fn service_scope_for_test() -> String {
    ServiceScopeForTest()
}

/// [`UpdateConfigForNextgen`] 的 snake_case 别名。
pub fn update_config_for_nextgen(t: &dyn TestContext) {
    UpdateConfigForNextgen(t);
}
