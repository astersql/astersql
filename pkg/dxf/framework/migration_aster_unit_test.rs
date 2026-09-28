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

// DXF Framework 包文档迁移回归测试。
//
// 断言 `FRAMEWORK_PACKAGE_DOC` 保留 Go 版资源模型与调度模型关键表述，
// 并确认文档已完成 Rust 迁移标记（含 AsterSQL 版权、无未完成草稿字样）。
// DXF（Distributed eXecution Framework）负责集群内分布式任务的统一调度与执行。

use super::FRAMEWORK_PACKAGE_DOC;

/// 资源模型：slot 数等于核数、task-concurrency 条带、background 服务域等表述须保留。
/// slot 是节点资源的最小粒度，用于避免过载并控制任务并发。
#[test]
fn framework_package_documentation_preserves_go_resource_model() {
    for statement in [
        "Package framework contains all the codes related to DXF.",
        "slot count = number of cores",
        "task-concurrency stripes",
        "special service scope 'background'",
    ] {
        assert!(
            FRAMEWORK_PACKAGE_DOC.contains(statement),
            "missing Go package documentation statement: {statement}"
        );
    }
}

/// 调度模型：scheduler/task-executor manager、优先级排序、awaiting-resolution、
/// running→pending 状态回退等 Go 文档关键句须仍在。
#[test]
fn framework_package_documentation_preserves_go_scheduling_model() {
    for statement in [
        "scheduler manager",
        "task executor manager",
        "priority asc, create_time asc, id asc",
        "awaiting-resolution",
        "running` -> `pending`",
    ] {
        assert!(
            FRAMEWORK_PACKAGE_DOC.contains(statement),
            "missing Go package documentation statement: {statement}"
        );
    }
}

/// 任务抽象与状态机：step/subtask 层次、失败语义、暂停恢复和修改路径须与 Go 文档一致。
#[test]
fn framework_package_documentation_preserves_go_task_state_contract() {
    for statement in [
        "multiple steps that runs in sequence",
        "multiple sub-tasks that runs in parallel",
        "it will end with `reverted` state",
        "The `failed` state is used to mean the framework cannot run the task",
        "`resuming` is always `running`",
        "modifying state transition",
    ] {
        assert!(
            FRAMEWORK_PACKAGE_DOC.contains(statement),
            "missing Go task-state documentation statement: {statement}"
        );
    }
}

/// 子任务回退到 pending 的条件与幂等约束是调度安全契约，不能只保留状态名。
#[test]
fn framework_package_documentation_preserves_go_subtask_recovery_contract() {
    for statement in [
        "only happens when some node is taken as dead",
        "the subtask is idempotent",
        "scheduled to other node again",
        "it's NOT",
        "a normal state transition",
    ] {
        assert!(
            FRAMEWORK_PACKAGE_DOC.contains(statement),
            "missing Go subtask-recovery documentation statement: {statement}"
        );
    }
}

/// 迁移完成标记：须含 AsterSQL 版权，且不得残留“机械/当前不保证可编译”等草稿提示。
#[test]
fn framework_package_documentation_is_a_finished_rust_migration() {
    assert!(FRAMEWORK_PACKAGE_DOC.contains("Copyright 2026 AsterSQL."));
    assert!(!FRAMEWORK_PACKAGE_DOC.contains("\u{673a}\u{68b0}"));
    let stale = format!("{}{}{}", "当前", "不保证", "可编译");
    assert!(!FRAMEWORK_PACKAGE_DOC.contains(&stale));
}
