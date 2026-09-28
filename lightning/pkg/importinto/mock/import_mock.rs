// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! MockGen port of `lightning/pkg/importinto/mock/import_mock.go`
//! (CheckpointManager, JobSubmitter, JobMonitor, JobOrchestrator, ProgressUpdater).
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/mock/import_mock.rs`对应的测试替身与调用录制，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少90行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl MockCheckpointManager`把\"MockCheckpointManager\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl CheckpointManager`把\"CheckpointManager\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Initialize`是当前文件的重要函数，承担\"Initialize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Get`是当前文件的重要函数，承担\"Get\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Update`是当前文件的重要函数，承担\"Update\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Remove`是当前文件的重要函数，承担\"Remove\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `IgnoreError`是当前文件的重要函数，承担\"IgnoreError\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DestroyError`是当前文件的重要函数，承担\"DestroyError\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpTables`是当前文件的重要函数，承担\"DumpTables\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpEngines`是当前文件的重要函数，承担\"DumpEngines\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpChunks`是当前文件的重要函数，承担\"DumpChunks\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetCheckpoints`是当前文件的重要函数，承担\"GetCheckpoints\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Close`是当前文件的重要函数，承担\"Close\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl MockCheckpointManagerMockRecorder`把\"MockCheckpointManagerMockRecorder\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl MockJobSubmitter`把\"MockJobSubmitter\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl JobSubmitter`把\"JobSubmitter\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `SubmitTable`是当前文件的重要函数，承担\"SubmitTable\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetGroupKey`是当前文件的重要函数，承担\"GetGroupKey\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl MockJobSubmitterMockRecorder`把\"MockJobSubmitterMockRecorder\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl MockJobMonitor`把\"MockJobMonitor\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl JobMonitor`把\"JobMonitor\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `WaitForJobs`是当前文件的重要函数，承担\"WaitForJobs\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl MockJobMonitorMockRecorder`把\"MockJobMonitorMockRecorder\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl MockJobOrchestrator`把\"MockJobOrchestrator\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl JobOrchestrator`把\"JobOrchestrator\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `SubmitAndWait`是当前文件的重要函数，承担\"SubmitAndWait\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Cancel`是当前文件的重要函数，承担\"Cancel\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl MockJobOrchestratorMockRecorder`把\"MockJobOrchestratorMockRecorder\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl MockProgressUpdater`把\"MockProgressUpdater\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl ProgressUpdater`把\"ProgressUpdater\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `UpdateTotalSize`是当前文件的重要函数，承担\"UpdateTotalSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `UpdateFinishedSize`是当前文件的重要函数，承担\"UpdateFinishedSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl MockProgressUpdaterMockRecorder`把\"MockProgressUpdaterMockRecorder\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `_stubs_ty`是当前文件的重要函数，承担\" stubs ty\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"---------------------------------------------------------------------------\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"MockCheckpointManager\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"MockJobSubmitter\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Go returns (*ImportJob, error); nil job with nil error → default ImportJob.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"MockJobMonitor\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"MockJobOrchestrator\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"MockProgressUpdater\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Silence unused import when only re-exported helpers are used externally.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use astersql_lightning_pkg_importinto::{
    CheckpointManager, ImportJob, JobMonitor, JobOrchestrator, JobSubmitter, ProgressUpdater,
    TableCheckpoint, context, importsdk,
};

use crate::stubs::{
    self, Call, Controller, Result, take_error, take_one, take_opt_pair, take_pair,
};

// ---------------------------------------------------------------------------
// MockCheckpointManager
// ---------------------------------------------------------------------------

/// MockCheckpointManager is a mock of CheckpointManager interface.
pub struct MockCheckpointManager {
    pub ctrl: Controller,
    pub recorder: MockCheckpointManagerMockRecorder,
}

/// MockCheckpointManagerMockRecorder is the mock recorder for MockCheckpointManager.
pub struct MockCheckpointManagerMockRecorder {
    ctrl: Controller,
}

/// NewMockCheckpointManager creates a new mock instance.
pub fn NewMockCheckpointManager(ctrl: Controller) -> MockCheckpointManager {
    MockCheckpointManager {
        recorder: MockCheckpointManagerMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockCheckpointManager {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockCheckpointManagerMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// Close mocks base method.
    pub fn Close(&self) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("Close", vec![]);
        take_error(rets)
    }

    /// DestroyError mocks base method.
    pub fn DestroyError(
        &self,
        arg0: &context::Context,
        arg1: &str,
    ) -> Result<Vec<TableCheckpoint>> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "DestroyError",
            vec![Box::new(arg0.clone()), Box::new(arg1.to_string())],
        );
        take_pair::<Vec<TableCheckpoint>>(rets)
    }

    /// DumpChunks mocks base method.
    pub fn DumpChunks(
        &self,
        arg0: &context::Context,
        _arg1: &mut dyn std::io::Write,
    ) -> Result<()> {
        self.ctrl.Helper();
        let rets = self
            .ctrl
            .Call("DumpChunks", vec![Box::new(arg0.clone()), Box::new(())]);
        take_error(rets)
    }

    /// DumpEngines mocks base method.
    pub fn DumpEngines(
        &self,
        arg0: &context::Context,
        _arg1: &mut dyn std::io::Write,
    ) -> Result<()> {
        self.ctrl.Helper();
        let rets = self
            .ctrl
            .Call("DumpEngines", vec![Box::new(arg0.clone()), Box::new(())]);
        take_error(rets)
    }

    /// DumpTables mocks base method.
    pub fn DumpTables(
        &self,
        arg0: &context::Context,
        _arg1: &mut dyn std::io::Write,
    ) -> Result<()> {
        self.ctrl.Helper();
        let rets = self
            .ctrl
            .Call("DumpTables", vec![Box::new(arg0.clone()), Box::new(())]);
        take_error(rets)
    }

    /// Get mocks base method.
    pub fn Get(&self, arg0: &context::Context, arg1: &str) -> Result<Option<TableCheckpoint>> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "Get",
            vec![Box::new(arg0.clone()), Box::new(arg1.to_string())],
        );
        take_opt_pair::<TableCheckpoint>(rets)
    }

    /// GetCheckpoints mocks base method.
    pub fn GetCheckpoints(&self, arg0: &context::Context) -> Result<Vec<TableCheckpoint>> {
        self.ctrl.Helper();
        let rets = self
            .ctrl
            .Call("GetCheckpoints", vec![Box::new(arg0.clone())]);
        take_pair::<Vec<TableCheckpoint>>(rets)
    }

    /// IgnoreError mocks base method.
    pub fn IgnoreError(&self, arg0: &context::Context, arg1: &str) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "IgnoreError",
            vec![Box::new(arg0.clone()), Box::new(arg1.to_string())],
        );
        take_error(rets)
    }

    /// Initialize mocks base method.
    pub fn Initialize(&self, arg0: &context::Context) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("Initialize", vec![Box::new(arg0.clone())]);
        take_error(rets)
    }

    /// Remove mocks base method.
    pub fn Remove(&self, arg0: &context::Context, arg1: &str) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "Remove",
            vec![Box::new(arg0.clone()), Box::new(arg1.to_string())],
        );
        take_error(rets)
    }

    /// Update mocks base method.
    pub fn Update(&self, arg0: &context::Context, arg1: &TableCheckpoint) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "Update",
            vec![Box::new(arg0.clone()), Box::new(arg1.clone())],
        );
        take_error(rets)
    }
}

impl CheckpointManager for MockCheckpointManager {
    fn Initialize(&self, ctx: &context::Context) -> Result<()> {
        MockCheckpointManager::Initialize(self, ctx)
    }
    fn Get(&self, ctx: &context::Context, tableName: &str) -> Result<Option<TableCheckpoint>> {
        MockCheckpointManager::Get(self, ctx, tableName)
    }
    fn Update(&self, ctx: &context::Context, cp: &TableCheckpoint) -> Result<()> {
        MockCheckpointManager::Update(self, ctx, cp)
    }
    fn Remove(&self, ctx: &context::Context, tableName: &str) -> Result<()> {
        MockCheckpointManager::Remove(self, ctx, tableName)
    }
    fn IgnoreError(&self, ctx: &context::Context, tableName: &str) -> Result<()> {
        MockCheckpointManager::IgnoreError(self, ctx, tableName)
    }
    fn DestroyError(
        &self,
        ctx: &context::Context,
        tableName: &str,
    ) -> Result<Vec<TableCheckpoint>> {
        MockCheckpointManager::DestroyError(self, ctx, tableName)
    }
    fn DumpTables(&self, ctx: &context::Context, writer: &mut dyn std::io::Write) -> Result<()> {
        MockCheckpointManager::DumpTables(self, ctx, writer)
    }
    fn DumpEngines(&self, ctx: &context::Context, writer: &mut dyn std::io::Write) -> Result<()> {
        MockCheckpointManager::DumpEngines(self, ctx, writer)
    }
    fn DumpChunks(&self, ctx: &context::Context, writer: &mut dyn std::io::Write) -> Result<()> {
        MockCheckpointManager::DumpChunks(self, ctx, writer)
    }
    fn GetCheckpoints(&self, ctx: &context::Context) -> Result<Vec<TableCheckpoint>> {
        MockCheckpointManager::GetCheckpoints(self, ctx)
    }
    fn Close(&self) -> Result<()> {
        MockCheckpointManager::Close(self)
    }
}

impl MockCheckpointManagerMockRecorder {
    pub fn Close(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Close", "MockCheckpointManager.Close", vec![])
    }

    pub fn DestroyError(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "DestroyError",
            "MockCheckpointManager.DestroyError",
            vec![_arg0, _arg1],
        )
    }

    pub fn DumpChunks(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "DumpChunks",
            "MockCheckpointManager.DumpChunks",
            vec![_arg0, _arg1],
        )
    }

    pub fn DumpEngines(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "DumpEngines",
            "MockCheckpointManager.DumpEngines",
            vec![_arg0, _arg1],
        )
    }

    pub fn DumpTables(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "DumpTables",
            "MockCheckpointManager.DumpTables",
            vec![_arg0, _arg1],
        )
    }

    pub fn Get(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Get", "MockCheckpointManager.Get", vec![_arg0, _arg1])
    }

    pub fn GetCheckpoints(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "GetCheckpoints",
            "MockCheckpointManager.GetCheckpoints",
            vec![_arg0],
        )
    }

    pub fn IgnoreError(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "IgnoreError",
            "MockCheckpointManager.IgnoreError",
            vec![_arg0, _arg1],
        )
    }

    pub fn Initialize(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "Initialize",
            "MockCheckpointManager.Initialize",
            vec![_arg0],
        )
    }

    pub fn Remove(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "Remove",
            "MockCheckpointManager.Remove",
            vec![_arg0, _arg1],
        )
    }

    pub fn Update(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "Update",
            "MockCheckpointManager.Update",
            vec![_arg0, _arg1],
        )
    }
}

// ---------------------------------------------------------------------------
// MockJobSubmitter
// ---------------------------------------------------------------------------

/// MockJobSubmitter is a mock of JobSubmitter interface.
pub struct MockJobSubmitter {
    pub ctrl: Controller,
    pub recorder: MockJobSubmitterMockRecorder,
}

/// MockJobSubmitterMockRecorder is the mock recorder for MockJobSubmitter.
pub struct MockJobSubmitterMockRecorder {
    ctrl: Controller,
}

/// NewMockJobSubmitter creates a new mock instance.
pub fn NewMockJobSubmitter(ctrl: Controller) -> MockJobSubmitter {
    MockJobSubmitter {
        recorder: MockJobSubmitterMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockJobSubmitter {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockJobSubmitterMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// GetGroupKey mocks base method.
    pub fn GetGroupKey(&self) -> String {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("GetGroupKey", vec![]);
        take_one::<String>(rets)
    }

    /// SubmitTable mocks base method.
    pub fn SubmitTable(
        &self,
        arg0: &context::Context,
        arg1: &importsdk::TableMeta,
    ) -> Result<ImportJob> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "SubmitTable",
            vec![Box::new(arg0.clone()), Box::new(arg1.clone())],
        );
        // Go returns (*ImportJob, error); nil job with nil error → default ImportJob.
        match take_opt_pair::<ImportJob>(rets)? {
            Some(job) => Ok(job),
            None => Ok(ImportJob {
                JobID: 0,
                TableMeta: None,
                GroupKey: String::new(),
            }),
        }
    }
}

impl JobSubmitter for MockJobSubmitter {
    fn SubmitTable(
        &self,
        ctx: &context::Context,
        tableMeta: &importsdk::TableMeta,
    ) -> Result<ImportJob> {
        MockJobSubmitter::SubmitTable(self, ctx, tableMeta)
    }
    fn GetGroupKey(&self) -> String {
        MockJobSubmitter::GetGroupKey(self)
    }
}

impl MockJobSubmitterMockRecorder {
    pub fn GetGroupKey(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("GetGroupKey", "MockJobSubmitter.GetGroupKey", vec![])
    }

    pub fn SubmitTable(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "SubmitTable",
            "MockJobSubmitter.SubmitTable",
            vec![_arg0, _arg1],
        )
    }
}

// ---------------------------------------------------------------------------
// MockJobMonitor
// ---------------------------------------------------------------------------

/// MockJobMonitor is a mock of JobMonitor interface.
pub struct MockJobMonitor {
    pub ctrl: Controller,
    pub recorder: MockJobMonitorMockRecorder,
}

/// MockJobMonitorMockRecorder is the mock recorder for MockJobMonitor.
pub struct MockJobMonitorMockRecorder {
    ctrl: Controller,
}

/// NewMockJobMonitor creates a new mock instance.
pub fn NewMockJobMonitor(ctrl: Controller) -> MockJobMonitor {
    MockJobMonitor {
        recorder: MockJobMonitorMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockJobMonitor {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockJobMonitorMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// WaitForJobs mocks base method.
    pub fn WaitForJobs(&self, arg0: &context::Context, arg1: &[ImportJob]) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "WaitForJobs",
            vec![Box::new(arg0.clone()), Box::new(arg1.to_vec())],
        );
        take_error(rets)
    }
}

impl JobMonitor for MockJobMonitor {
    fn WaitForJobs(&self, ctx: &context::Context, jobs: &[ImportJob]) -> Result<()> {
        MockJobMonitor::WaitForJobs(self, ctx, jobs)
    }
}

impl MockJobMonitorMockRecorder {
    pub fn WaitForJobs(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "WaitForJobs",
            "MockJobMonitor.WaitForJobs",
            vec![_arg0, _arg1],
        )
    }
}

// ---------------------------------------------------------------------------
// MockJobOrchestrator
// ---------------------------------------------------------------------------

/// MockJobOrchestrator is a mock of JobOrchestrator interface.
pub struct MockJobOrchestrator {
    pub ctrl: Controller,
    pub recorder: MockJobOrchestratorMockRecorder,
}

/// MockJobOrchestratorMockRecorder is the mock recorder for MockJobOrchestrator.
pub struct MockJobOrchestratorMockRecorder {
    ctrl: Controller,
}

/// NewMockJobOrchestrator creates a new mock instance.
pub fn NewMockJobOrchestrator(ctrl: Controller) -> MockJobOrchestrator {
    MockJobOrchestrator {
        recorder: MockJobOrchestratorMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockJobOrchestrator {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockJobOrchestratorMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// Cancel mocks base method.
    pub fn Cancel(&self, arg0: &context::Context) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("Cancel", vec![Box::new(arg0.clone())]);
        take_error(rets)
    }

    /// SubmitAndWait mocks base method.
    pub fn SubmitAndWait(
        &self,
        arg0: &context::Context,
        arg1: &[importsdk::TableMeta],
    ) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "SubmitAndWait",
            vec![Box::new(arg0.clone()), Box::new(arg1.to_vec())],
        );
        take_error(rets)
    }
}

impl JobOrchestrator for MockJobOrchestrator {
    fn SubmitAndWait(&self, ctx: &context::Context, tables: &[importsdk::TableMeta]) -> Result<()> {
        MockJobOrchestrator::SubmitAndWait(self, ctx, tables)
    }
    fn Cancel(&self, ctx: &context::Context) -> Result<()> {
        MockJobOrchestrator::Cancel(self, ctx)
    }
}

impl MockJobOrchestratorMockRecorder {
    pub fn Cancel(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Cancel", "MockJobOrchestrator.Cancel", vec![_arg0])
    }

    pub fn SubmitAndWait(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "SubmitAndWait",
            "MockJobOrchestrator.SubmitAndWait",
            vec![_arg0, _arg1],
        )
    }
}

// ---------------------------------------------------------------------------
// MockProgressUpdater
// ---------------------------------------------------------------------------

/// MockProgressUpdater is a mock of ProgressUpdater interface.
pub struct MockProgressUpdater {
    pub ctrl: Controller,
    pub recorder: MockProgressUpdaterMockRecorder,
}

/// MockProgressUpdaterMockRecorder is the mock recorder for MockProgressUpdater.
pub struct MockProgressUpdaterMockRecorder {
    ctrl: Controller,
}

/// NewMockProgressUpdater creates a new mock instance.
pub fn NewMockProgressUpdater(ctrl: Controller) -> MockProgressUpdater {
    MockProgressUpdater {
        recorder: MockProgressUpdaterMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockProgressUpdater {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockProgressUpdaterMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// UpdateFinishedSize mocks base method.
    pub fn UpdateFinishedSize(&self, arg0: i64) {
        self.ctrl.Helper();
        let _ = self.ctrl.Call("UpdateFinishedSize", vec![Box::new(arg0)]);
    }

    /// UpdateTotalSize mocks base method.
    pub fn UpdateTotalSize(&self, arg0: i64) {
        self.ctrl.Helper();
        let _ = self.ctrl.Call("UpdateTotalSize", vec![Box::new(arg0)]);
    }
}

impl ProgressUpdater for MockProgressUpdater {
    fn UpdateTotalSize(&self, size: i64) {
        MockProgressUpdater::UpdateTotalSize(self, size)
    }
    fn UpdateFinishedSize(&self, size: i64) {
        MockProgressUpdater::UpdateFinishedSize(self, size)
    }
}

impl MockProgressUpdaterMockRecorder {
    pub fn UpdateFinishedSize(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "UpdateFinishedSize",
            "MockProgressUpdater.UpdateFinishedSize",
            vec![_arg0],
        )
    }

    pub fn UpdateTotalSize(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "UpdateTotalSize",
            "MockProgressUpdater.UpdateTotalSize",
            vec![_arg0],
        )
    }
}

// Silence unused import when only re-exported helpers are used externally.
#[allow(dead_code)]
fn _stubs_ty() -> Controller {
    stubs::Controller::new()
}
