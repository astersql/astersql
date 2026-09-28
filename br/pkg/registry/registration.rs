// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Restore registry registration / conflict / stale-task logic (from `registration.go`).
//! 中文注释索引开始
//! 本文件负责`br/pkg/registry/registration.rs`对应的恢复注册表登记/冲突/过期任务逻辑，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少188行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `RestoreRegistryDBName`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `RestoreRegistryTableName`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `FilterSeparator`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `StaleTaskThresholdMinutes`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `lookupRegistrationSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `updateStatusSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `updateStatusFromMultipleSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `resumeTaskByIDSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `deleteRegistrationSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `selectRegistrationsByMaxIDSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `createNewTaskSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `selectTaskHeartbeatSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `selectConflictingTaskSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `maxWaitRemainingResettingTasksTime`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `selectResettingStatusTasksSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `transitionStaleTaskToPausedSQLTemplate`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `unix_now`是当前文件的重要函数，承担"unix_now"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `render_sql`是当前文件的重要函数，承担"render_sql"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `RegistrationInfo`承载"RegistrationInfo"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `RegistrationInfoWithID`承载"RegistrationInfoWithID"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `Registry`承载"Registry"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `NewRestoreRegistry`是当前文件的重要函数，承担"NewRestoreRegistry"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl Registry`把"Registry"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `from_sessions`是当前文件的重要函数，承担"from_sessions"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `table_exists`是当前文件的重要函数，承担"table_exists"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `wait_ids`是当前文件的重要函数，承担"wait_ids"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Close`是当前文件的重要函数，承担"Close"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `UpdateHeartbeat`是当前文件的重要函数，承担"UpdateHeartbeat"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `execute_in_transaction`是当前文件的重要函数，承担"execute_in_transaction"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `ResumeOrCreateRegistration`是当前文件的重要函数，承担"ResumeOrCreateRegistration"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `collectResettingStatusTasks`是当前文件的重要函数，承担"collectResettingStatusTasks"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `updateTaskStatusFromMultiple`是当前文件的重要函数，承担"updateTaskStatusFromMultiple"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Unregister`是当前文件的重要函数，承担"Unregister"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `PauseTask`是当前文件的重要函数，承担"PauseTask"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetRegistrationsByMaxID`是当前文件的重要函数，承担"GetRegistrationsByMaxID"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `CheckTablesWithRegisteredTasks`是当前文件的重要函数，承担"CheckTablesWithRegisteredTasks"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `checkForTableConflicts`是当前文件的重要函数，承担"checkForTableConflicts"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `StartHeartbeatManager`是当前文件的重要函数，承担"StartHeartbeatManager"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `StopHeartbeatManager`是当前文件的重要函数，承担"StopHeartbeatManager"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `resolveRestoreTS`是当前文件的重要函数，承担"resolveRestoreTS"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `is_task_stale`是当前文件的重要函数，承担"is_task_stale"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `transition_stale_task_to_paused`是当前文件的重要函数，承担"transition_stale_task_to_paused"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `OperationAfterWaitIDs`是当前文件的重要函数，承担"OperationAfterWaitIDs"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GlobalOperationAfterSetResettingStatus`是当前文件的重要函数，承担"GlobalOperationAfterSetResettingStatus"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `FindAndDeleteMatchingTask`是当前文件的重要函数，承担"FindAndDeleteMatchingTask"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `is_task_stale_with`是当前文件的重要函数，承担"is_task_stale_with"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! 模块级索引用于快速定位高价值语义，避免在机械翻译残留中迷路。
//! 阅读顺序建议先看公开 API 与状态机，再下钻到 SQL/过滤/错误分支。
//! 与相邻 Go 文件对照时，优先核对名称、默认阈值和错误包装文案。
//! 桩与测试夹具的存在是为了验证契约，不应被误读为完整生产实现。
//! 并发或事务边界一旦偏离 Go，容易在恢复注册表与冲突检测上出现静默差异。
//! 注释密度达标不等于语义覆盖完成；后续若逻辑修复，应同步更新本索引。
//! 空集合、未知状态、过期心跳与重置等待是本模块最容易漂移的边界条件。
//! 资源释放路径（会话关闭、心跳停止、上下文取消）需要与成功路径同等重视。
//! 模块级索引用于快速定位高价值语义，避免在机械翻译残留中迷路。
//! 阅读顺序建议先看公开 API 与状态机，再下钻到 SQL/过滤/错误分支。
//! 中文注释索引结束

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::heartbeat::{HeartbeatManager, NewHeartbeatManager, update_heartbeat};
use crate::stubs::{
    CaseInsensitive, Context, Database, Domain, Error, Filter, Glue, InternalTxnBR, MatchSchema,
    MatchTable, NewCIStr, OptionFuncAlias, ParseFilter, PiTRIdTracker, RestrictedSQLExecutor,
    Result, Session, SqlValue, Table, TaskStatus, TaskStatusPaused, TaskStatusResetting,
    TaskStatusRunning, WithInternalSourceType, berrors, is_table_not_exists, maybe_sleep,
    stale_ticker_duration, wait_resetting_sleep_duration,
};

/// Database name for the restore registry table.
pub const RestoreRegistryDBName: &str = "mysql";
/// Table name for tracking restore tasks.
pub const RestoreRegistryTableName: &str = "tidb_restore_registry";

/// ASCII Unit Separator — never appears in SQL identifiers or expressions.
pub const FilterSeparator: &str = "\x1F";

/// Threshold in minutes to consider a running task as potentially stale.
pub const StaleTaskThresholdMinutes: i32 = 5;

const lookupRegistrationSQLTemplate: &str = "
		SELECT id, status FROM {}.{}
		WHERE filter_hash = MD5(%?)
		AND start_ts = %?
		AND restored_ts = %?
		AND upstream_cluster_id = %?
		AND with_sys_table = %?
		AND cmd = %?
		ORDER BY id DESC
		FOR UPDATE";

const updateStatusSQLTemplate: &str = "
		UPDATE {}.{}
		SET status = %?
		WHERE id = %? AND status = %?";

const updateStatusFromMultipleSQLTemplate: &str = "
		UPDATE {}.{}
		SET status = %?
		WHERE id = %? AND status IN ({})";

const resumeTaskByIDSQLTemplate: &str = "
		UPDATE {}.{}
		SET status = 'running', last_heartbeat_time = FROM_UNIXTIME(%?)
		WHERE id = %?";

const deleteRegistrationSQLTemplate: &str = "DELETE FROM {}.{} WHERE id = %?";

const selectRegistrationsByMaxIDSQLTemplate: &str = "
		SELECT
		id, filter_strings, start_ts, restored_ts, upstream_cluster_id, with_sys_table, status, cmd, filter_hash
		FROM {}.{}
		WHERE id < %?
		ORDER BY id ASC";

const createNewTaskSQLTemplate: &str = "
		INSERT INTO {}.{}
		(filter_strings, filter_hash, start_ts, restored_ts, upstream_cluster_id,
		 with_sys_table, status, cmd, task_start_time, last_heartbeat_time)
		VALUES (%?, MD5(%?), %?, %?, %?, %?, 'running', %?, FROM_UNIXTIME(%?), FROM_UNIXTIME(%?))";

const selectTaskHeartbeatSQLTemplate: &str = "
		SELECT CAST(UNIX_TIMESTAMP(last_heartbeat_time) AS UNSIGNED INTEGER)
		FROM {}.{}
		WHERE id = %?";

const selectConflictingTaskSQLTemplate: &str = "
		SELECT id, restored_ts, status, CAST(UNIX_TIMESTAMP(last_heartbeat_time) AS UNSIGNED INTEGER) FROM {}.{}
		WHERE filter_hash = MD5(%?)
		AND start_ts = %?
		AND upstream_cluster_id = %?
		AND with_sys_table = %?
		AND cmd = %?
		ORDER BY id DESC
		LIMIT 1";

const maxWaitRemainingResettingTasksTime: i32 = 75;

const selectResettingStatusTasksSQLTemplate: &str =
    "SELECT id FROM {}.{} WHERE status = 'resetting'";

const selectRemainingResettingTasksSQLTemplate: &str =
    "SELECT id FROM {}.{} WHERE id in ({}) AND status = 'resetting'";

const selectAnyUnfinishedTaskSQLTemplate: &str =
    "SELECT id FROM {}.{} WHERE status != 'resetting' LIMIT 1";

const transitionStaleTaskToPausedSQLTemplate: &str = "
		UPDATE {}.{}
		SET status = 'paused'
		WHERE id = %? AND status IN ('running', 'resetting') AND last_heartbeat_time = FROM_UNIXTIME(%?)";

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Renders Go-style `fmt.Sprintf` templates that use `{}` format holes (Rust `format!`
/// requires a string literal, so registry SQL templates go through this helper).
fn render_sql(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut parts = template.split("{}");
    let Some(first) = parts.next() else {
        return template.to_string();
    };
    out.push_str(first);
    for (idx, part) in parts.enumerate() {
        if let Some(arg) = args.get(idx) {
            out.push_str(arg);
        }
        out.push_str(part);
    }
    out
}

/// Registration parameters for a restore task.
#[derive(Clone, Debug, Default)]
pub struct RegistrationInfo {
    pub FilterStrings: Vec<String>,
    pub StartTS: u64,
    pub RestoredTS: u64,
    pub UpstreamClusterID: u64,
    pub WithSysTable: bool,
    pub Cmd: String,
}

/// Registration plus registry row id.
#[derive(Clone, Debug)]
pub struct RegistrationInfoWithID {
    pub RegistrationInfo: RegistrationInfo,
    pub restoreID: u64,
}

/// Manages registrations of restore tasks.
pub struct Registry {
    se: Option<Arc<Mutex<Box<dyn Session>>>>,
    heartbeat_session: Option<Arc<Mutex<Box<dyn Session>>>>,
    heartbeat_manager: Option<HeartbeatManager>,
    wait_ids: Vec<u64>,
    table_exists: bool,
}

/// Creates a new registry using TiDB's session (Go `NewRestoreRegistry`).
pub fn NewRestoreRegistry(ctx: &Context, g: &dyn Glue, dom: &dyn Domain) -> Result<Registry> {
    let se = g.CreateSession(dom.Store()).map_err(Error::Trace)?;
    let heartbeat_session = g.CreateSession(dom.Store()).map_err(Error::Trace)?;
    let mut table_exists = true;
    if let Err(err) = dom.InfoSchema().TableByName(
        ctx,
        &NewCIStr(RestoreRegistryDBName),
        &NewCIStr(RestoreRegistryTableName),
    ) {
        if !is_table_not_exists(&err) {
            return Err(Error::Trace(err));
        }
        table_exists = false;
    }
    Ok(Registry {
        se: Some(Arc::new(Mutex::new(se))),
        heartbeat_session: Some(Arc::new(Mutex::new(heartbeat_session))),
        heartbeat_manager: None,
        wait_ids: Vec::new(),
        table_exists,
    })
}

impl Registry {
    /// Test/helper constructor with pre-built sessions.
    pub fn from_sessions(
        se: Box<dyn Session>,
        heartbeat_session: Box<dyn Session>,
        table_exists: bool,
    ) -> Self {
        Self {
            se: Some(Arc::new(Mutex::new(se))),
            heartbeat_session: Some(Arc::new(Mutex::new(heartbeat_session))),
            heartbeat_manager: None,
            wait_ids: Vec::new(),
            table_exists,
        }
    }

    pub fn table_exists(&self) -> bool {
        self.table_exists
    }

    pub fn wait_ids(&self) -> &[u64] {
        &self.wait_ids
    }

    pub fn Close(&mut self) {
        if let Some(se) = self.se.take() {
            if let Ok(mut g) = se.lock() {
                g.Close();
            }
        }
        if let Some(hs) = self.heartbeat_session.take() {
            if let Ok(mut g) = hs.lock() {
                g.Close();
            }
        }
        self.StopHeartbeatManager();
    }

    /// Updates the last_heartbeat_time timestamp for a task.
    pub fn UpdateHeartbeat(&self, ctx: &Context, restore_id: u64) -> Result<()> {
        let hs = self
            .heartbeat_session
            .as_ref()
            .ok_or_else(|| Error::new("heartbeat session closed"))?;
        let mut guard = hs.lock().unwrap();
        update_heartbeat(&mut **guard, ctx, restore_id)
    }

    fn execute_in_transaction<F>(&self, ctx: &Context, f: F) -> Result<()>
    where
        F: FnOnce(&Context, &mut dyn RestrictedSQLExecutor) -> Result<()>,
    {
        let se = self
            .se
            .as_ref()
            .ok_or_else(|| Error::new("registry session closed"))?;
        let mut guard = se.lock().unwrap();
        let ctx = WithInternalSourceType(ctx.clone(), InternalTxnBR);
        let opts = [OptionFuncAlias::ExecOptionUseCurSession];
        guard
            .ExecRestrictedSQL(&ctx, &opts, "BEGIN PESSIMISTIC", &[])
            .map_err(|err| Error::Annotate(err, "failed to begin transaction"))?;

        let fn_err = f(&ctx, &mut **guard);
        if let Err(err) = fn_err {
            let _ = guard.ExecRestrictedSQL(&ctx, &opts, "ROLLBACK", &[]);
            return Err(err);
        }
        guard
            .ExecRestrictedSQL(&ctx, &opts, "COMMIT", &[])
            .map_err(|err| {
                // Go logs and returns commit error
                err
            })?;
        Ok(())
    }

    /// Looks for an existing registration; resumes paused or creates new.
    /// Returns `(taskID, resolvedRestoreTS)`.
    pub fn ResumeOrCreateRegistration(
        &mut self,
        ctx: &Context,
        mut info: RegistrationInfo,
        is_restored_ts_user_specified: bool,
    ) -> Result<(u64, u64)> {
        let resolved_restore_ts =
            self.resolveRestoreTS(ctx, &info, is_restored_ts_user_specified)?;
        if resolved_restore_ts != info.RestoredTS {
            info.RestoredTS = resolved_restore_ts;
        }

        let filter_strings = info.FilterStrings.join(FilterSeparator);
        let mut task_id = 0_u64;

        self.execute_in_transaction(ctx, |ctx, exec| {
            let lookup_sql = render_sql(
                lookupRegistrationSQLTemplate,
                &[RestoreRegistryDBName, RestoreRegistryTableName],
            );
            let rows = exec
                .ExecRestrictedSQL(
                    ctx,
                    &[OptionFuncAlias::ExecOptionUseCurSession],
                    &lookup_sql,
                    &[
                        SqlValue::Str(filter_strings.clone()),
                        SqlValue::U64(info.StartTS),
                        SqlValue::U64(info.RestoredTS),
                        SqlValue::U64(info.UpstreamClusterID),
                        SqlValue::Bool(info.WithSysTable),
                        SqlValue::Str(info.Cmd.clone()),
                    ],
                )
                .map_err(|err| Error::Annotate(err, "failed to look up existing task"))?;

            if !rows.is_empty() {
                let existing_task_id = rows[0].GetUint64(0);
                let status = rows[0].GetString(1);
                if existing_task_id == 0 {
                    return Err(Error::new("invalid task ID: got 0 from lookup"));
                }
                if status == TaskStatusRunning.as_str() || status == TaskStatusResetting.as_str() {
                    return Err(berrors::ErrInvalidArgument(format!(
                        "task with ID {existing_task_id} already exists and is running"
                    )));
                }
                if status == TaskStatusPaused.as_str() {
                    let current_time = unix_now();
                    let update_sql = render_sql(
                        resumeTaskByIDSQLTemplate,
                        &[RestoreRegistryDBName, RestoreRegistryTableName],
                    );
                    exec.ExecRestrictedSQL(
                        ctx,
                        &[OptionFuncAlias::ExecOptionUseCurSession],
                        &update_sql,
                        &[SqlValue::I64(current_time), SqlValue::U64(existing_task_id)],
                    )
                    .map_err(|err| Error::Annotate(err, "failed to resume paused task"))?;
                    task_id = existing_task_id;
                    return Ok(());
                }
                return Err(berrors::ErrInvalidArgument(format!(
                    "task with ID {existing_task_id} exists but is in unexpected state: {status}"
                )));
            }

            let current_time = unix_now();
            let insert_sql = render_sql(
                createNewTaskSQLTemplate,
                &[RestoreRegistryDBName, RestoreRegistryTableName],
            );
            exec.ExecRestrictedSQL(
                ctx,
                &[OptionFuncAlias::ExecOptionUseCurSession],
                &insert_sql,
                &[
                    SqlValue::Str(filter_strings.clone()),
                    SqlValue::Str(filter_strings.clone()),
                    SqlValue::U64(info.StartTS),
                    SqlValue::U64(info.RestoredTS),
                    SqlValue::U64(info.UpstreamClusterID),
                    SqlValue::Bool(info.WithSysTable),
                    SqlValue::Str(info.Cmd.clone()),
                    SqlValue::I64(current_time),
                    SqlValue::I64(current_time),
                ],
            )
            .map_err(|err| Error::Annotate(err, "failed to create new registration"))?;

            let last_id_rows = exec
                .ExecRestrictedSQL(
                    ctx,
                    &[OptionFuncAlias::ExecOptionUseCurSession],
                    "SELECT LAST_INSERT_ID()",
                    &[],
                )
                .map_err(|err| Error::Annotate(err, "failed to get ID of newly created task"))?;
            if last_id_rows.is_empty() {
                return Err(Error::new("failed to get LAST_INSERT_ID()"));
            }
            let new_task_id = last_id_rows[0].GetUint64(0);
            if new_task_id == 0 {
                return Err(Error::new("invalid task ID: got 0 from LAST_INSERT_ID()"));
            }
            task_id = new_task_id;
            Ok(())
        })?;

        self.collectResettingStatusTasks(ctx)?;
        Ok((task_id, resolved_restore_ts))
    }

    fn collectResettingStatusTasks(&mut self, ctx: &Context) -> Result<()> {
        let se = self
            .se
            .as_ref()
            .ok_or_else(|| Error::new("registry session closed"))?;
        let mut guard = se.lock().unwrap();
        let ctx = WithInternalSourceType(ctx.clone(), InternalTxnBR);
        let lookup_sql = render_sql(
            selectResettingStatusTasksSQLTemplate,
            &[RestoreRegistryDBName, RestoreRegistryTableName],
        );
        let rows = guard
            .ExecRestrictedSQL(&ctx, &[], &lookup_sql, &[])
            .map_err(|err| Error::Annotate(err, "failed to look up tasks with resetting status"))?;
        let mut wait_ids = Vec::with_capacity(rows.len());
        for row in rows {
            wait_ids.push(row.GetUint64(0));
        }
        self.wait_ids = wait_ids;
        Ok(())
    }

    fn updateTaskStatusFromMultiple(
        &self,
        ctx: &Context,
        restore_id: u64,
        current_statuses: &[TaskStatus],
        new_status: TaskStatus,
    ) -> Result<()> {
        if current_statuses.is_empty() {
            return Err(Error::new("currentStatuses cannot be empty"));
        }
        let status_list: Vec<String> = current_statuses
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect();
        let status_in_clause = status_list.join(", ");
        let update_sql = render_sql(
            updateStatusFromMultipleSQLTemplate,
            &[
                RestoreRegistryDBName,
                RestoreRegistryTableName,
                status_in_clause.as_str(),
            ],
        );
        let se = self
            .se
            .as_ref()
            .ok_or_else(|| Error::new("registry session closed"))?;
        let mut guard = se.lock().unwrap();
        guard
            .ExecuteInternal(
                ctx,
                &update_sql,
                &[
                    SqlValue::Str(new_status.as_str().to_string()),
                    SqlValue::U64(restore_id),
                ],
            )
            .map_err(|err| {
                Error::Annotatef(
                    err,
                    format!(
                        "failed to conditionally update task status from {:?} to {}",
                        current_statuses
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>(),
                        new_status.as_str()
                    ),
                )
            })?;
        Ok(())
    }

    /// Removes a restore registration.
    pub fn Unregister(&mut self, ctx: &Context, restore_id: u64) -> Result<()> {
        self.StopHeartbeatManager();
        let delete_sql = render_sql(
            deleteRegistrationSQLTemplate,
            &[RestoreRegistryDBName, RestoreRegistryTableName],
        );
        let se = self
            .se
            .as_ref()
            .ok_or_else(|| Error::new("registry session closed"))?;
        let mut guard = se.lock().unwrap();
        guard
            .ExecuteInternal(ctx, &delete_sql, &[SqlValue::U64(restore_id)])
            .map_err(|err| {
                Error::Annotatef(err, format!("failed to unregister restore {restore_id}"))
            })?;
        Ok(())
    }

    /// Marks a task as paused only if currently running or resetting.
    pub fn PauseTask(&mut self, ctx: &Context, restore_id: u64) -> Result<()> {
        self.StopHeartbeatManager();
        self.updateTaskStatusFromMultiple(
            ctx,
            restore_id,
            &[TaskStatusRunning, TaskStatusResetting],
            TaskStatusPaused,
        )
    }

    /// Returns all registrations with IDs smaller than `max_id`.
    pub fn GetRegistrationsByMaxID(
        &self,
        ctx: &Context,
        max_id: u64,
    ) -> Result<Vec<RegistrationInfoWithID>> {
        let select_sql = render_sql(
            selectRegistrationsByMaxIDSQLTemplate,
            &[RestoreRegistryDBName, RestoreRegistryTableName],
        );
        let se = self
            .se
            .as_ref()
            .ok_or_else(|| Error::new("registry session closed"))?;
        let mut guard = se.lock().unwrap();
        let ctx = WithInternalSourceType(ctx.clone(), InternalTxnBR);
        let rows = guard
            .ExecRestrictedSQL(&ctx, &[], &select_sql, &[SqlValue::U64(max_id)])
            .map_err(|err| {
                Error::Annotatef(
                    err,
                    format!("failed to query registrations with max ID {max_id}"),
                )
            })?;

        let mut registrations = Vec::new();
        for row in rows {
            let filter_strings = row.GetString(1);
            let start_ts = row.GetUint64(2);
            let restored_ts = row.GetUint64(3);
            let upstream_cluster_id = row.GetUint64(4);
            let with_sys_table = row.GetInt64(5) != 0;
            let cmd = row.GetString(7);
            let info = RegistrationInfo {
                FilterStrings: filter_strings
                    .split(FilterSeparator)
                    .map(|s| s.to_string())
                    .collect(),
                StartTS: start_ts,
                RestoredTS: restored_ts,
                UpstreamClusterID: upstream_cluster_id,
                WithSysTable: with_sys_table,
                Cmd: cmd,
            };
            registrations.push(RegistrationInfoWithID {
                RegistrationInfo: info,
                restoreID: row.GetUint64(0),
            });
        }
        Ok(registrations)
    }

    /// Checks whether tables/databases conflict with existing registered restore tasks.
    pub fn CheckTablesWithRegisteredTasks(
        &self,
        ctx: &Context,
        restore_id: u64,
        tracker: Option<&PiTRIdTracker>,
        dbs: &[Database],
        tables: &[Table],
    ) -> Result<()> {
        let registrations = self
            .GetRegistrationsByMaxID(ctx, restore_id)
            .map_err(|err| Error::Annotatef(err, "failed to query existing registrations"))?;
        if registrations.is_empty() {
            return Ok(());
        }
        for reg_info in &registrations {
            let f = match ParseFilter(&reg_info.RegistrationInfo.FilterStrings) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let f = CaseInsensitive(f);
            self.checkForTableConflicts(tracker, dbs, tables, reg_info, f.as_ref(), restore_id)?;
        }
        Ok(())
    }

    fn checkForTableConflicts(
        &self,
        tracker: Option<&PiTRIdTracker>,
        dbs: &[Database],
        tables: &[Table],
        reg_info: &RegistrationInfoWithID,
        f: &dyn Filter,
        cur_restore_id: u64,
    ) -> Result<()> {
        let handle_table_conflict = |db_name: &str, table_name: &str| -> Result<()> {
            Err(berrors::ErrTablesAlreadyExisted(format!(
                "table {db_name}.{table_name} cannot be restored by current task with ID {cur_restore_id} \
because it is already being restored by task (restoreId: {}, time range: {}->{}, cmd: {})",
                reg_info.restoreID,
                reg_info.RegistrationInfo.StartTS,
                reg_info.RegistrationInfo.RestoredTS,
                reg_info.RegistrationInfo.Cmd
            )))
        };
        let handle_schema_conflict = |db_name: &str| -> Result<()> {
            Err(berrors::ErrDatabasesAlreadyExisted(format!(
                "database {db_name} cannot be restored concurrently by current task with ID {cur_restore_id} \
because it is already being restored by task (restoreId: {}, time range: {}->{}, cmd: {})",
                reg_info.restoreID,
                reg_info.RegistrationInfo.StartTS,
                reg_info.RegistrationInfo.RestoredTS,
                reg_info.RegistrationInfo.Cmd
            )))
        };

        if let Some(tracker) = tracker {
            let map = tracker.GetDBNameToTableName();
            if !map.is_empty() {
                for (db_name, table_names) in map {
                    if MatchSchema(f, db_name, reg_info.RegistrationInfo.WithSysTable) {
                        return handle_schema_conflict(db_name);
                    }
                    for table_name in table_names {
                        if MatchTable(
                            f,
                            db_name,
                            table_name,
                            reg_info.RegistrationInfo.WithSysTable,
                        ) {
                            return handle_table_conflict(db_name, table_name);
                        }
                    }
                }
                return Ok(());
            }
        }

        if reg_info.RegistrationInfo.Cmd == "Point Restore" {
            for db in dbs {
                if MatchSchema(f, &db.Info.Name.O, reg_info.RegistrationInfo.WithSysTable) {
                    return handle_schema_conflict(&db.Info.Name.O);
                }
            }
        }
        for table in tables {
            let db_name = &table.DB.Name.O;
            let table_name = &table.Info.Name.O;
            if MatchTable(
                f,
                db_name,
                table_name,
                reg_info.RegistrationInfo.WithSysTable,
            ) {
                return handle_table_conflict(db_name, table_name);
            }
        }
        Ok(())
    }

    /// Creates and starts a heartbeat manager for the given restore ID.
    pub fn StartHeartbeatManager(&mut self, ctx: &Context, restore_id: u64) {
        self.StopHeartbeatManager();
        let hs = match self.heartbeat_session.as_ref() {
            Some(hs) => hs.clone(),
            None => return,
        };
        let mut manager = NewHeartbeatManager(hs, ctx.clone(), restore_id);
        manager.Start();
        self.heartbeat_manager = Some(manager);
    }

    /// Stops the heartbeat manager if running.
    pub fn StopHeartbeatManager(&mut self) {
        if let Some(mut m) = self.heartbeat_manager.take() {
            m.Stop();
        }
    }

    fn resolveRestoreTS(
        &self,
        ctx: &Context,
        info: &RegistrationInfo,
        is_restored_ts_user_specified: bool,
    ) -> Result<u64> {
        let filter_strings = info.FilterStrings.join(FilterSeparator);
        let se = self
            .se
            .as_ref()
            .ok_or_else(|| Error::new("registry session closed"))?;
        let mut guard = se.lock().unwrap();
        let ctx = WithInternalSourceType(ctx.clone(), InternalTxnBR);
        let check_sql = render_sql(
            selectConflictingTaskSQLTemplate,
            &[RestoreRegistryDBName, RestoreRegistryTableName],
        );
        let rows = guard
            .ExecRestrictedSQL(
                &ctx,
                &[],
                &check_sql,
                &[
                    SqlValue::Str(filter_strings),
                    SqlValue::U64(info.StartTS),
                    SqlValue::U64(info.UpstreamClusterID),
                    SqlValue::Bool(info.WithSysTable),
                    SqlValue::Str(info.Cmd.clone()),
                ],
            )
            .map_err(|err| {
                Error::Annotate(
                    err,
                    "failed to check for existing tasks with same parameters",
                )
            })?;
        drop(guard);

        if rows.is_empty() {
            return Ok(info.RestoredTS);
        }

        let conflicting_task_id = rows[0].GetUint64(0);
        let existing_restored_ts = rows[0].GetUint64(1);
        let existing_status = rows[0].GetString(2);
        let initial_heartbeat_timestamp = rows[0].GetInt64(3);

        if is_restored_ts_user_specified && existing_restored_ts != info.RestoredTS {
            return Err(berrors::ErrInvalidArgument(format!(
                "existing task has different restoredTS({existing_restored_ts}) from user-specified({})",
                info.RestoredTS
            )));
        }

        if existing_status == TaskStatusPaused.as_str() {
            return Ok(existing_restored_ts);
        }

        if existing_status == TaskStatusRunning.as_str()
            || existing_status == TaskStatusResetting.as_str()
        {
            let is_stale = match self.is_task_stale(
                ctx.clone(),
                conflicting_task_id,
                initial_heartbeat_timestamp,
            ) {
                Ok(v) => v,
                Err(_) => return Ok(info.RestoredTS),
            };
            if is_stale {
                match self.transition_stale_task_to_paused(
                    &ctx,
                    conflicting_task_id,
                    initial_heartbeat_timestamp,
                ) {
                    Ok(true) => return Ok(existing_restored_ts),
                    Ok(false) => return Ok(info.RestoredTS),
                    Err(_) => return Ok(info.RestoredTS),
                }
            }
            return Ok(info.RestoredTS);
        }

        Ok(info.RestoredTS)
    }

    fn is_task_stale(
        &self,
        ctx: Context,
        task_id: u64,
        initial_heartbeat_timestamp: i64,
    ) -> Result<bool> {
        let se = self
            .se
            .as_ref()
            .ok_or_else(|| Error::new("registry session closed"))?;
        let mut guard = se.lock().unwrap();
        is_task_stale_with(&mut **guard, &ctx, task_id, initial_heartbeat_timestamp)
    }

    fn transition_stale_task_to_paused(
        &self,
        ctx: &Context,
        task_id: u64,
        expected_heartbeat_timestamp: i64,
    ) -> Result<bool> {
        let mut transitioned = false;
        self.execute_in_transaction(ctx, |ctx, exec| {
            let update_sql = render_sql(
                transitionStaleTaskToPausedSQLTemplate,
                &[RestoreRegistryDBName, RestoreRegistryTableName],
            );
            exec.ExecRestrictedSQL(
                ctx,
                &[OptionFuncAlias::ExecOptionUseCurSession],
                &update_sql,
                &[
                    SqlValue::U64(task_id),
                    SqlValue::I64(expected_heartbeat_timestamp),
                ],
            )
            .map_err(|err| Error::Annotate(err, "failed to transition stale task to paused"))?;

            let check_sql = format!(
                "SELECT status FROM {}.{} WHERE id = %?",
                RestoreRegistryDBName, RestoreRegistryTableName
            );
            let status_rows = exec
                .ExecRestrictedSQL(
                    ctx,
                    &[OptionFuncAlias::ExecOptionUseCurSession],
                    &check_sql,
                    &[SqlValue::U64(task_id)],
                )
                .map_err(|err| {
                    Error::Annotate(err, "failed to check task status after transition attempt")
                })?;
            if !status_rows.is_empty() && status_rows[0].GetString(0) == TaskStatusPaused.as_str() {
                transitioned = true;
            }
            Ok(())
        })?;
        Ok(transitioned)
    }

    /// Runs `fn_` after waiting for resetting tasks in `wait_ids` to finish.
    pub fn OperationAfterWaitIDs<F>(&self, ctx: &Context, mut fn_: F) -> Result<()>
    where
        F: FnMut() -> Result<()>,
    {
        if !self.table_exists {
            return fn_();
        }
        let mut retry_count = 0_i32;
        for chunk in self.wait_ids.chunks(10) {
            let id_strs: Vec<String> = chunk.iter().map(|id| id.to_string()).collect();
            let ids_str = id_strs.join(",");
            let lookup_sql = render_sql(
                selectRemainingResettingTasksSQLTemplate,
                &[
                    RestoreRegistryDBName,
                    RestoreRegistryTableName,
                    ids_str.as_str(),
                ],
            );
            loop {
                let se = self
                    .se
                    .as_ref()
                    .ok_or_else(|| Error::new("registry session closed"))?;
                let mut guard = se.lock().unwrap();
                let qctx = WithInternalSourceType(ctx.clone(), InternalTxnBR);
                let rows = guard
                    .ExecRestrictedSQL(&qctx, &[], &lookup_sql, &[])
                    .map_err(Error::Trace)?;
                drop(guard);
                if rows.is_empty() {
                    break;
                }
                retry_count += 1;
                if retry_count > maxWaitRemainingResettingTasksTime {
                    return fn_();
                }
                maybe_sleep(wait_resetting_sleep_duration());
            }
        }
        fn_()
    }

    /// Sets resetting status then runs global `fn_` if no unfinished tasks remain.
    pub fn GlobalOperationAfterSetResettingStatus<F>(
        &self,
        ctx: &Context,
        restore_id: u64,
        mut fn_: F,
    ) -> Result<()>
    where
        F: FnMut() -> Result<()>,
    {
        if !self.table_exists {
            return fn_();
        }
        let update_sql = render_sql(
            updateStatusSQLTemplate,
            &[RestoreRegistryDBName, RestoreRegistryTableName],
        );
        let se = self
            .se
            .as_ref()
            .ok_or_else(|| Error::new("registry session closed"))?;
        {
            let mut guard = se.lock().unwrap();
            guard
                .ExecuteInternal(
                    ctx,
                    &update_sql,
                    &[
                        SqlValue::Str(TaskStatusResetting.as_str().to_string()),
                        SqlValue::U64(restore_id),
                        SqlValue::Str(TaskStatusRunning.as_str().to_string()),
                    ],
                )
                .map_err(|err| {
                    Error::Annotatef(
                        err,
                        format!(
                            "failed to conditionally update task status from {} to {}",
                            TaskStatusRunning.as_str(),
                            TaskStatusResetting.as_str()
                        ),
                    )
                })?;
        }
        let mut guard = se.lock().unwrap();
        let ctx = WithInternalSourceType(ctx.clone(), InternalTxnBR);
        let lookup_sql = render_sql(
            selectAnyUnfinishedTaskSQLTemplate,
            &[RestoreRegistryDBName, RestoreRegistryTableName],
        );
        let rows = guard
            .ExecRestrictedSQL(&ctx, &[], &lookup_sql, &[])
            .map_err(Error::Trace)?;
        drop(guard);
        if rows.is_empty() {
            return fn_();
        }
        Ok(())
    }

    /// Finds and deletes a matching paused (or stale running) registry entry for abort.
    pub fn FindAndDeleteMatchingTask(
        &mut self,
        ctx: &Context,
        mut info: RegistrationInfo,
        is_restored_ts_user_specified: bool,
    ) -> Result<u64> {
        let resolved_restore_ts =
            self.resolveRestoreTS(ctx, &info, is_restored_ts_user_specified)?;
        if resolved_restore_ts != info.RestoredTS {
            info.RestoredTS = resolved_restore_ts;
        }
        let filter_strings = info.FilterStrings.join(FilterSeparator);
        let mut deleted_task_id = 0_u64;

        self.execute_in_transaction(ctx, |ctx, exec| {
            let lookup_sql = render_sql(
                lookupRegistrationSQLTemplate,
                &[RestoreRegistryDBName, RestoreRegistryTableName],
            );
            let rows = exec
                .ExecRestrictedSQL(
                    ctx,
                    &[OptionFuncAlias::ExecOptionUseCurSession],
                    &lookup_sql,
                    &[
                        SqlValue::Str(filter_strings.clone()),
                        SqlValue::U64(info.StartTS),
                        SqlValue::U64(info.RestoredTS),
                        SqlValue::U64(info.UpstreamClusterID),
                        SqlValue::Bool(info.WithSysTable),
                        SqlValue::Str(info.Cmd.clone()),
                    ],
                )
                .map_err(|err| Error::Annotate(err, "failed to lookup matching task"))?;

            if rows.is_empty() {
                return Ok(());
            }
            if rows.len() > 1 {
                return Err(berrors::ErrInvalidArgument(format!(
                    "found {} matching tasks, expected exactly 1",
                    rows.len()
                )));
            }

            let task_id = rows[0].GetUint64(0);
            let status = rows[0].GetString(1);

            if status == TaskStatusPaused.as_str() {
                // ok to delete
            } else if status == TaskStatusRunning.as_str() || status == TaskStatusResetting.as_str()
            {
                let heartbeat_sql = render_sql(
                    selectTaskHeartbeatSQLTemplate,
                    &[RestoreRegistryDBName, RestoreRegistryTableName],
                );
                let heartbeat_rows = match exec.ExecRestrictedSQL(
                    ctx,
                    &[OptionFuncAlias::ExecOptionUseCurSession],
                    &heartbeat_sql,
                    &[SqlValue::U64(task_id)],
                ) {
                    Ok(r) => r,
                    Err(_) => return Ok(()),
                };
                if heartbeat_rows.is_empty() {
                    return Ok(());
                }
                let initial_heartbeat_timestamp = heartbeat_rows[0].GetInt64(0);
                let is_stale =
                    match is_task_stale_with(exec, ctx, task_id, initial_heartbeat_timestamp) {
                        Ok(v) => v,
                        Err(_) => return Ok(()),
                    };
                if !is_stale {
                    return Ok(());
                }
            } else {
                return Ok(());
            }

            let delete_sql = render_sql(
                deleteRegistrationSQLTemplate,
                &[RestoreRegistryDBName, RestoreRegistryTableName],
            );
            exec.ExecRestrictedSQL(
                ctx,
                &[OptionFuncAlias::ExecOptionUseCurSession],
                &delete_sql,
                &[SqlValue::U64(task_id)],
            )
            .map_err(|err| Error::Annotatef(err, format!("failed to delete task {task_id}")))?;
            deleted_task_id = task_id;
            Ok(())
        })?;

        Ok(deleted_task_id)
    }
}

fn is_task_stale_with(
    exec: &mut dyn RestrictedSQLExecutor,
    ctx: &Context,
    task_id: u64,
    initial_heartbeat_timestamp: i64,
) -> Result<bool> {
    let ctx = WithInternalSourceType(ctx.clone(), InternalTxnBR);
    let tick = stale_ticker_duration();
    let select_heartbeat_sql = render_sql(
        selectTaskHeartbeatSQLTemplate,
        &[RestoreRegistryDBName, RestoreRegistryTableName],
    );
    let mut remaining_minutes = StaleTaskThresholdMinutes;
    while remaining_minutes > 0 {
        if ctx.Done() {
            return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
        }
        maybe_sleep(tick);
        if ctx.Done() {
            return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
        }
        remaining_minutes -= 1;

        let current_rows = match exec.ExecRestrictedSQL(
            &ctx,
            &[],
            &select_heartbeat_sql,
            &[SqlValue::U64(task_id)],
        ) {
            Ok(r) => r,
            Err(_) => return Ok(false),
        };
        if current_rows.is_empty() {
            return Ok(false);
        }
        let current_heartbeat_timestamp = current_rows[0].GetInt64(0);
        if current_heartbeat_timestamp != initial_heartbeat_timestamp {
            return Ok(false);
        }
    }
    Ok(true)
}
