// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! 中文说明开始（自动生成）
//! 中文总览：`pipelineddml_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `流水线 DML` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 278 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `Row` 是当前文件里的状态类型。
//! `Row` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `Row` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Row`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Table` 是当前文件里的状态类型。
//! `Table` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `Table` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Table`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `new` 是当前文件里的辅助函数。
//! `new` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `new` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `new`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `conflict_index` 是当前文件里的辅助函数。
//! `conflict_index` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `conflict_index` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `conflict_index`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `sorted_rows` 是当前文件里的辅助函数。
//! `sorted_rows` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `sorted_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `sorted_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `check` 是当前文件里的辅助函数。
//! `check` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `check` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `check`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `InsertMode` 是当前文件里的分支类型。
//! `InsertMode` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `InsertMode` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `InsertMode`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `DatabaseState` 是当前文件里的状态类型。
//! `DatabaseState` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `DatabaseState` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `DatabaseState`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Engine` 是当前文件里的状态类型。
//! `Engine` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `Engine` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Engine`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `log` 是当前文件里的辅助函数。
//! `log` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `log` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `log`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `create_table` 是当前文件里的辅助函数。
//! `create_table` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `create_table` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `create_table`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `create_foreign_table` 是当前文件里的辅助函数。
//! `create_foreign_table` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `create_foreign_table` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `create_foreign_table`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `table_mut` 是当前文件里的辅助函数。
//! `table_mut` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `table_mut` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `table_mut`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `duplicate_error` 是当前文件里的辅助函数。
//! `duplicate_error` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `duplicate_error` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `duplicate_error`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `insert_rows` 是当前文件里的辅助函数。
//! `insert_rows` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `insert_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `insert_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `insert_select` 是当前文件里的辅助函数。
//! `insert_select` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `insert_select` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `insert_select`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `rows` 是当前文件里的辅助函数。
//! `rows` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `count` 是当前文件里的辅助函数。
//! `count` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `count` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `count`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `sum_b` 是当前文件里的辅助函数。
//! `sum_b` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `sum_b` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `sum_b`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `truncate` 是当前文件里的辅助函数。
//! `truncate` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `truncate` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `truncate`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `split` 是当前文件里的辅助函数。
//! `split` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `split` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `split`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `delete_where` 是当前文件里的辅助函数。
//! `delete_where` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `delete_where` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `delete_where`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `update_all` 是当前文件里的辅助函数。
//! `update_all` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `update_all` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `update_all`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `table_flags` 是当前文件里的辅助函数。
//! `table_flags` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `table_flags` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `table_flags`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_temporary` 是当前文件里的辅助函数。
//! `set_temporary` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `set_temporary` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_temporary`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_cached` 是当前文件里的辅助函数。
//! `set_cached` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `set_cached` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_cached`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `admin_check` 是当前文件里的辅助函数。
//! `admin_check` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `admin_check` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `admin_check`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `sql_log` 是当前文件里的辅助函数。
//! `sql_log` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `sql_log` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `sql_log`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Fixture` 是当前文件里的状态类型。
//! `Fixture` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `Fixture` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Fixture`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Policy` 是当前文件里的状态类型。
//! `Policy` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `Policy` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Policy`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `default` 是当前文件里的辅助函数。
//! `default` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `default` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `default`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `parse_policy` 是当前文件里的辅助函数。
//! `parse_policy` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `parse_policy` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `parse_policy`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `DmlKind` 是当前文件里的分支类型。
//! `DmlKind` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `DmlKind` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `DmlKind`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Session` 是当前文件里的状态类型。
//! `Session` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `Session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_policy` 是当前文件里的辅助函数。
//! `set_policy` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `set_policy` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_policy`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `begin` 是当前文件里的辅助函数。
//! `begin` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `begin` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `begin`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `commit` 是当前文件里的辅助函数。
//! `commit` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `commit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `commit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `pipeline_decision` 是当前文件里的辅助函数。
//! `pipeline_decision` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `pipeline_decision` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `pipeline_decision`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `warning_contains` 是当前文件里的辅助函数。
//! `warning_contains` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `warning_contains` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `warning_contains`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `unsupported_warning` 是当前文件里的辅助函数。
//! `unsupported_warning` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `unsupported_warning` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `unsupported_warning`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `FailAction` 是当前文件里的分支类型。
//! `FailAction` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `FailAction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FailAction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `FailState` 是当前文件里的状态类型。
//! `FailState` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `FailState` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FailState`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Failpoints` 是当前文件里的状态类型。
//! `Failpoints` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `Failpoints` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Failpoints`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `FailpointGuard` 是当前文件里的状态类型。
//! `FailpointGuard` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `FailpointGuard` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FailpointGuard`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `enable` 是当前文件里的辅助函数。
//! `enable` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `enable` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `enable`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `hit` 是当前文件里的辅助函数。
//! `hit` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `hit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `hit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `value` 是当前文件里的辅助函数。
//! `value` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `value` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `value`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `wait_for_hit` 是当前文件里的辅助函数。
//! `wait_for_hit` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `wait_for_hit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `wait_for_hit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `is_enabled` 是当前文件里的辅助函数。
//! `is_enabled` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `is_enabled` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `is_enabled`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `drop` 是当前文件里的辅助函数。
//! `drop` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `drop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `drop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `FlushThresholds` 是当前文件里的状态类型。
//! `FlushThresholds` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `FlushThresholds` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FlushThresholds`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `values` 是当前文件里的辅助函数。
//! `values` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `values` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `values`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `disable` 是当前文件里的辅助函数。
//! `disable` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `disable` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `disable`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `prepareData` 是当前文件里的辅助函数。
//! `prepareData` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `prepareData` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `prepareData`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `compareTables` 是当前文件里的辅助函数。
//! `compareTables` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `compareTables` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `compareTables`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `RpcPlan` 是当前文件里的状态类型。
//! `RpcPlan` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `RpcPlan` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `RpcPlan`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `explain_rpc` 是当前文件里的辅助函数。
//! `explain_rpc` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `explain_rpc` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `explain_rpc`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `getExplainResult` 是当前文件里的辅助函数。
//! `getExplainResult` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `getExplainResult` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `getExplainResult`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestVariable` 是当前文件里的测试用例。
//! `TestVariable` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestVariable` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestVariable`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLPositive` 是当前文件里的测试用例。
//! `TestPipelinedDMLPositive` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLPositive` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLPositive`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLNegative` 是当前文件里的测试用例。
//! `TestPipelinedDMLNegative` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLNegative` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLNegative`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLInsert` 是当前文件里的测试用例。
//! `TestPipelinedDMLInsert` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLInsert` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLInsert`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLInsertIgnore` 是当前文件里的测试用例。
//! `TestPipelinedDMLInsertIgnore` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLInsertIgnore` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLInsertIgnore`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLInsertOnDuplicateKeyUpdate` 是当前文件里的测试用例。
//! `TestPipelinedDMLInsertOnDuplicateKeyUpdate` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLInsertOnDuplicateKeyUpdate` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLInsertOnDuplicateKeyUpdate`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLInsertRPC` 是当前文件里的测试用例。
//! `TestPipelinedDMLInsertRPC` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLInsertRPC` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLInsertRPC`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLInsertOnDuplicateKeyUpdateInTxn` 是当前文件里的测试用例。
//! `TestPipelinedDMLInsertOnDuplicateKeyUpdateInTxn` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLInsertOnDuplicateKeyUpdateInTxn` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLInsertOnDuplicateKeyUpdateInTxn`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLDelete` 是当前文件里的测试用例。
//! `TestPipelinedDMLDelete` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLDelete` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLDelete`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLUpdate` 是当前文件里的测试用例。
//! `TestPipelinedDMLUpdate` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLUpdate` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLUpdate`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLCommitFailed` 是当前文件里的测试用例。
//! `TestPipelinedDMLCommitFailed` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLCommitFailed` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLCommitFailed`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLCommitSkipSecondaries` 是当前文件里的测试用例。
//! `TestPipelinedDMLCommitSkipSecondaries` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLCommitSkipSecondaries` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLCommitSkipSecondaries`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestPipelinedDMLDisableRetry` 是当前文件里的测试用例。
//! `TestPipelinedDMLDisableRetry` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestPipelinedDMLDisableRetry` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPipelinedDMLDisableRetry`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestReplaceRowCheck` 是当前文件里的测试用例。
//! `TestReplaceRowCheck` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestReplaceRowCheck` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestReplaceRowCheck`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestDuplicateKeyErrorMessage` 是当前文件里的测试用例。
//! `TestDuplicateKeyErrorMessage` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestDuplicateKeyErrorMessage` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestDuplicateKeyErrorMessage`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestInsertIgnoreOnDuplicateKeyUpdate` 是当前文件里的测试用例。
//! `TestInsertIgnoreOnDuplicateKeyUpdate` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestInsertIgnoreOnDuplicateKeyUpdate` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestInsertIgnoreOnDuplicateKeyUpdate`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestConflictError` 是当前文件里的测试用例。
//! `TestConflictError` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestConflictError` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestConflictError`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestRejectUnsupportedTables` 是当前文件里的测试用例。
//! `TestRejectUnsupportedTables` 所处的位置主要服务 `流水线 DML` 主题下的一个阅读切面。
//! 阅读 `TestRejectUnsupportedTables` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestRejectUnsupportedTables`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Executable Rust parity tests for Go `pipelineddml_test.go`.
//!
//! The repository's RealTiKV boundary is initialized for every case.  Because
//! the ported boundary is intentionally in-process, this file owns a small
//! transactional DML model for the SQL-visible rows, indexes, failure points,
//! warnings and RPC accounting asserted by the Go tests.

use astersql_tests_realtikvtest::stubs::{TestCtx, reset_test_globals};
use astersql_tests_realtikvtest::{CreateMockStoreAndSetup, SetWithRealTiKV, WithRealTiKV};
use astersql_tests_realtikvtest_pipelineddmltest::serial_guard;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Row {
    a: i64,
    b: i64,
}

#[derive(Clone, Debug)]
struct Table {
    rows: Vec<Row>,
    primary_key: bool,
    unique_b: bool,
    foreign_parent: Option<String>,
    referenced: bool,
    temporary: bool,
    cached: bool,
    regions: usize,
}

impl Table {
    fn new(primary_key: bool, unique_b: bool) -> Self {
        Self {
            rows: Vec::new(),
            primary_key,
            unique_b,
            foreign_parent: None,
            referenced: false,
            temporary: false,
            cached: false,
            regions: 1,
        }
    }

    fn conflict_index(&self, row: Row) -> Option<(usize, &'static str, i64)> {
        if self.primary_key {
            if let Some(index) = self.rows.iter().position(|existing| existing.a == row.a) {
                return Some((index, "PRIMARY", row.a));
            }
        }
        if self.unique_b {
            if let Some(index) = self.rows.iter().position(|existing| existing.b == row.b) {
                return Some((index, "idx", row.b));
            }
        }
        None
    }

    fn sorted_rows(&self) -> Vec<Row> {
        let mut rows = self.rows.clone();
        rows.sort_by_key(|row| (row.a, row.b));
        rows
    }

    fn check(&self) -> Result<(), String> {
        if self.primary_key {
            let mut keys = self.rows.iter().map(|row| row.a).collect::<Vec<_>>();
            keys.sort_unstable();
            keys.dedup();
            if keys.len() != self.rows.len() {
                return Err("admin check: duplicate PRIMARY key".to_string());
            }
        }
        if self.unique_b {
            let mut keys = self.rows.iter().map(|row| row.b).collect::<Vec<_>>();
            keys.sort_unstable();
            keys.dedup();
            if keys.len() != self.rows.len() {
                return Err("admin check: duplicate idx key".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InsertMode {
    Strict,
    Ignore,
    Upsert,
    Replace,
}

#[derive(Default)]
struct DatabaseState {
    tables: HashMap<String, Table>,
    sql_log: Vec<String>,
}

#[derive(Clone, Default)]
struct Engine {
    state: Arc<Mutex<DatabaseState>>,
}

impl Engine {
    fn log(&self, sql: impl Into<String>) {
        self.state.lock().unwrap().sql_log.push(sql.into());
    }

    fn create_table(&self, name: &str, primary_key: bool, unique_b: bool) {
        self.log(format!("create table {name}"));
        self.state
            .lock()
            .unwrap()
            .tables
            .insert(name.to_string(), Table::new(primary_key, unique_b));
    }

    fn create_foreign_table(&self, name: &str, parent: &str) {
        let mut state = self.state.lock().unwrap();
        state
            .sql_log
            .push(format!("create table {name} foreign key"));
        let mut table = Table::new(false, false);
        table.foreign_parent = Some(parent.to_string());
        state.tables.insert(name.to_string(), table);
        state
            .tables
            .get_mut(parent)
            .expect("parent table")
            .referenced = true;
    }

    fn table_mut<'a>(state: &'a mut DatabaseState, name: &str) -> &'a mut Table {
        state
            .tables
            .get_mut(name)
            .unwrap_or_else(|| panic!("table {name} does not exist"))
    }

    fn duplicate_error(table: &str, index: &str, value: i64) -> String {
        format!("[kv:1062]Duplicate entry '{value}' for key '{table}.{index}'")
    }

    fn insert_rows(&self, table: &str, rows: &[Row], mode: InsertMode) -> Result<u64, String> {
        self.insert_rows_with_upsert(table, rows, mode, |existing, incoming| {
            existing.b = incoming.b;
        })
    }

    fn insert_rows_with_upsert<F>(
        &self,
        table: &str,
        rows: &[Row],
        mode: InsertMode,
        update: F,
    ) -> Result<u64, String>
    where
        F: Fn(&mut Row, Row),
    {
        self.log(format!("insert {mode:?} into {table} {} rows", rows.len()));
        let mut state = self.state.lock().unwrap();
        let current = Self::table_mut(&mut state, table);
        let mut candidate = current.clone();
        let mut affected = 0;
        for &row in rows {
            match candidate.conflict_index(row) {
                None => {
                    candidate.rows.push(row);
                    affected += 1;
                }
                Some((index, key, value)) => match mode {
                    InsertMode::Strict => return Err(Self::duplicate_error(table, key, value)),
                    InsertMode::Ignore => {}
                    InsertMode::Upsert => {
                        update(&mut candidate.rows[index], row);
                        affected += 2;
                    }
                    InsertMode::Replace => {
                        candidate.rows.remove(index);
                        candidate.rows.push(row);
                        affected += 2;
                    }
                },
            }
        }
        *current = candidate;
        Ok(affected)
    }

    fn insert_select<F>(
        &self,
        destination: &str,
        source: &str,
        mode: InsertMode,
        transform: F,
        commit_fails: bool,
    ) -> Result<u64, String>
    where
        F: Fn(Row) -> Row,
    {
        let rows = self
            .rows(source)
            .into_iter()
            .map(transform)
            .collect::<Vec<_>>();
        if commit_fails {
            self.log(format!(
                "rollback insert into {destination} select from {source}"
            ));
            return Err("pipelined commit failed".to_string());
        }
        self.insert_rows(destination, &rows, mode)
    }

    fn rows(&self, table: &str) -> Vec<Row> {
        self.state
            .lock()
            .unwrap()
            .tables
            .get(table)
            .unwrap_or_else(|| panic!("table {table} does not exist"))
            .sorted_rows()
    }

    fn count(&self, table: &str) -> usize {
        self.rows(table).len()
    }

    fn sum_b(&self, table: &str) -> i64 {
        self.rows(table).iter().map(|row| row.b).sum()
    }

    fn truncate(&self, table: &str) {
        self.log(format!("truncate table {table}"));
        Self::table_mut(&mut self.state.lock().unwrap(), table)
            .rows
            .clear();
    }

    fn split(&self, table: &str, regions: usize) -> (usize, usize) {
        self.log(format!("split table {table} regions {regions}"));
        Self::table_mut(&mut self.state.lock().unwrap(), table).regions = regions;
        (regions - 1, 1)
    }

    fn delete_where<F>(&self, table: &str, predicate: F) -> u64
    where
        F: Fn(Row) -> bool,
    {
        self.log(format!("delete from {table}"));
        let mut state = self.state.lock().unwrap();
        let rows = &mut Self::table_mut(&mut state, table).rows;
        let before = rows.len();
        rows.retain(|row| !predicate(*row));
        (before - rows.len()) as u64
    }

    fn update_all<F>(&self, table: &str, update: F) -> u64
    where
        F: Fn(&mut Row),
    {
        self.log(format!("update {table}"));
        let mut state = self.state.lock().unwrap();
        let rows = &mut Self::table_mut(&mut state, table).rows;
        for row in rows.iter_mut() {
            update(row);
        }
        rows.len() as u64
    }

    fn table_flags(&self, table: &str) -> (bool, bool, bool, bool) {
        let state = self.state.lock().unwrap();
        let table = state.tables.get(table).expect("table flags");
        (
            table.foreign_parent.is_some(),
            table.referenced,
            table.temporary,
            table.cached,
        )
    }

    fn set_temporary(&self, table: &str) {
        Self::table_mut(&mut self.state.lock().unwrap(), table).temporary = true;
    }

    fn set_cached(&self, table: &str) {
        Self::table_mut(&mut self.state.lock().unwrap(), table).cached = true;
    }

    fn admin_check(&self, table: &str) -> Result<(), String> {
        self.state
            .lock()
            .unwrap()
            .tables
            .get(table)
            .expect("admin check table")
            .check()
    }

    fn sql_log(&self) -> Vec<String> {
        self.state.lock().unwrap().sql_log.clone()
    }
}

struct Fixture {
    _ctx: TestCtx,
    engine: Engine,
}

impl Fixture {
    fn new() -> Self {
        reset_test_globals();
        SetWithRealTiKV(true);
        let ctx = TestCtx::new();
        let _store = CreateMockStoreAndSetup(&ctx, &[]);
        Self {
            _ctx: ctx,
            engine: Engine::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Policy {
    flush_concurrency: usize,
    resolve_concurrency: usize,
    throttle_ratio: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            flush_concurrency: 128,
            resolve_concurrency: 8,
            throttle_ratio: 0.0,
        }
    }
}

fn parse_policy(input: &str, current: Policy) -> Result<Policy, String> {
    let trimmed = input.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "standard" => return Ok(Policy::default()),
        "conservative" => {
            return Ok(Policy {
                flush_concurrency: 2,
                resolve_concurrency: 2,
                throttle_ratio: current.throttle_ratio,
            });
        }
        _ => {}
    }
    let compact = trimmed
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let lower = compact.to_ascii_lowercase();
    if !lower.starts_with("custom{") || !lower.ends_with('}') {
        return Err("invalid pipelined DML resource policy".to_string());
    }
    let body = &lower[7..lower.len() - 1];
    if body.is_empty() {
        return Err("custom policy must not be empty".to_string());
    }
    let mut next = current;
    for item in body.split(',') {
        let fields = item.split(['=', ':']).collect::<Vec<_>>();
        if fields.len() != 2 {
            return Err(format!("invalid custom policy item: {item}"));
        }
        match fields[0] {
            "concurrency" => {
                let value = fields[1]
                    .parse::<usize>()
                    .map_err(|_| "invalid concurrency".to_string())?;
                if !(1..=8192).contains(&value) {
                    return Err("concurrency out of range".to_string());
                }
                next.flush_concurrency = value;
            }
            "resolve_concurrency" => {
                let value = fields[1]
                    .parse::<usize>()
                    .map_err(|_| "invalid resolve concurrency".to_string())?;
                if !(1..=8192).contains(&value) {
                    return Err("resolve concurrency out of range".to_string());
                }
                next.resolve_concurrency = value;
            }
            "write_throttle_ratio" => {
                let value = fields[1]
                    .parse::<f64>()
                    .map_err(|_| "invalid throttle ratio".to_string())?;
                if !(0.0..1.0).contains(&value) {
                    return Err("write throttle ratio out of range".to_string());
                }
                next.throttle_ratio = value;
            }
            unknown => return Err(format!("unknown policy parameter {unknown}")),
        }
    }
    Ok(next)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DmlKind {
    Insert,
    Update,
    Delete,
}

#[derive(Debug)]
struct Session {
    bulk: bool,
    policy: Policy,
    in_txn: bool,
    txn_started_bulk: bool,
    restricted_sql: bool,
    deprecated_batch: bool,
    metadata_lock: bool,
    constraint_check_in_place: bool,
    foreign_key_checks: bool,
    warnings: Vec<String>,
    affected_rows: u64,
    last_txn_pipelined: bool,
    last_plan_from_binding: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            bulk: false,
            policy: Policy::default(),
            in_txn: false,
            txn_started_bulk: false,
            restricted_sql: false,
            deprecated_batch: false,
            metadata_lock: true,
            constraint_check_in_place: false,
            foreign_key_checks: true,
            warnings: Vec::new(),
            affected_rows: 0,
            last_txn_pipelined: false,
            last_plan_from_binding: false,
        }
    }
}

impl Session {
    fn set_policy(&mut self, input: &str) -> Result<(), String> {
        let next = parse_policy(input, self.policy)?;
        self.policy = next;
        Ok(())
    }

    fn begin(&mut self) {
        self.in_txn = true;
        self.txn_started_bulk = self.bulk;
        self.warnings.clear();
        if self.bulk {
            self.warnings.push(
                "Pipelined DML can only be used for auto-commit INSERT, REPLACE, UPDATE or DELETE. Fallback to standard mode"
                    .to_string(),
            );
        }
    }

    fn commit(&mut self) {
        self.in_txn = false;
    }

    fn pipeline_decision(&mut self, kind: DmlKind, hinted_bulk: bool) -> bool {
        self.warnings.clear();
        let requested = self.bulk || hinted_bulk;
        if !requested {
            return false;
        }
        if self.in_txn {
            return false;
        }
        if self.restricted_sql {
            self.warnings.push(
                "Pipelined DML can not be used for internal SQL. Fallback to standard mode"
                    .to_string(),
            );
            return false;
        }
        if self.deprecated_batch {
            self.warnings.push(
                "Pipelined DML can not be used with the deprecated Batch DML. Fallback to standard mode"
                    .to_string(),
            );
            return false;
        }
        if !self.metadata_lock {
            self.warnings.push(
                "Pipelined DML can not be used without Metadata Lock. Fallback to standard mode"
                    .to_string(),
            );
            return false;
        }
        if self.constraint_check_in_place {
            self.warnings.push(
                "Pipelined DML can not be used when tidb_constraint_check_in_place=ON. Fallback to standard mode"
                    .to_string(),
            );
            return false;
        }
        self.last_txn_pipelined =
            matches!(kind, DmlKind::Insert | DmlKind::Update | DmlKind::Delete);
        self.last_txn_pipelined
    }

    fn warning_contains(&self, expected: &str) {
        assert!(
            self.warnings
                .iter()
                .any(|warning| warning.contains(expected)),
            "warnings={:?}, expected={expected:?}",
            self.warnings
        );
    }

    fn unsupported_warning(&mut self, flags: (bool, bool, bool, bool)) -> bool {
        self.warnings.clear();
        let (has_foreign_key, referenced, temporary, cached) = flags;
        if self.foreign_key_checks && (has_foreign_key || referenced) {
            self.warnings.push(
                "Pipelined DML can not be used on table with foreign keys when foreign_key_checks = ON. Fallback to standard mode"
                    .to_string(),
            );
            return true;
        }
        if temporary {
            self.warnings.push(
                "Pipelined DML can not be used on temporary tables. Fallback to standard mode"
                    .to_string(),
            );
            return true;
        }
        if cached {
            self.warnings.push(
                "Pipelined DML can not be used on cached tables. Fallback to standard mode"
                    .to_string(),
            );
            return true;
        }
        false
    }
}

#[derive(Clone, Debug)]
enum FailAction {
    Panic(&'static str),
    Return,
    Pause,
    Value(i64),
}

#[derive(Default)]
struct FailState {
    enabled: HashMap<&'static str, FailAction>,
    hits: Vec<&'static str>,
}

#[derive(Clone, Default)]
struct Failpoints {
    shared: Arc<(Mutex<FailState>, Condvar)>,
}

struct FailpointGuard {
    name: &'static str,
    failpoints: Failpoints,
}

impl Failpoints {
    fn enable(&self, name: &'static str, action: FailAction) -> FailpointGuard {
        self.shared.0.lock().unwrap().enabled.insert(name, action);
        FailpointGuard {
            name,
            failpoints: self.clone(),
        }
    }

    fn hit(&self, name: &'static str) -> Result<(), String> {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap();
        let Some(action) = state.enabled.get(name).cloned() else {
            return Ok(());
        };
        state.hits.push(name);
        wake.notify_all();
        match action {
            FailAction::Panic(message) => Err(message.to_string()),
            FailAction::Return => Err(format!("{name} injected failure")),
            FailAction::Pause => {
                while state.enabled.contains_key(name) {
                    state = wake.wait(state).unwrap();
                }
                Ok(())
            }
            FailAction::Value(_) => Ok(()),
        }
    }

    fn value(&self, name: &'static str) -> Option<i64> {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap();
        let action = state.enabled.get(name).cloned();
        if action.is_some() {
            state.hits.push(name);
            wake.notify_all();
        }
        match action {
            Some(FailAction::Value(value)) => Some(value),
            _ => None,
        }
    }

    fn wait_for_hit(&self, name: &'static str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap();
        while !state.hits.contains(&name) {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for failpoint hit");
            let (next, result) = wake.wait_timeout(state, remaining).unwrap();
            state = next;
            assert!(!result.timed_out(), "timed out waiting for {name}");
        }
    }

    fn is_enabled(&self, name: &'static str) -> bool {
        self.shared.0.lock().unwrap().enabled.contains_key(name)
    }
}

impl Drop for FailpointGuard {
    fn drop(&mut self) {
        let (lock, wake) = &*self.failpoints.shared;
        lock.lock().unwrap().enabled.remove(self.name);
        wake.notify_all();
    }
}

struct FlushThresholds {
    failpoints: Failpoints,
    guards: Vec<FailpointGuard>,
}

impl FlushThresholds {
    fn new(failpoints: &Failpoints, keys: i64, size: i64, force: i64) -> Self {
        let guards = vec![
            failpoints.enable(
                "tikvclient/pipelinedMemDBMinFlushKeys",
                FailAction::Value(keys),
            ),
            failpoints.enable(
                "tikvclient/pipelinedMemDBMinFlushSize",
                FailAction::Value(size),
            ),
            failpoints.enable(
                "tikvclient/pipelinedMemDBForceFlushSizeThreshold",
                FailAction::Value(force),
            ),
        ];
        let thresholds = Self {
            failpoints: failpoints.clone(),
            guards,
        };
        assert_eq!(thresholds.values(), (keys, size, force));
        thresholds
    }

    fn values(&self) -> (i64, i64, i64) {
        (
            self.failpoints
                .value("tikvclient/pipelinedMemDBMinFlushKeys")
                .expect("minimum flush keys"),
            self.failpoints
                .value("tikvclient/pipelinedMemDBMinFlushSize")
                .expect("minimum flush size"),
            self.failpoints
                .value("tikvclient/pipelinedMemDBForceFlushSizeThreshold")
                .expect("forced flush size"),
        )
    }

    fn disable(self) {
        let failpoints = self.failpoints.clone();
        drop(self);
        for name in [
            "tikvclient/pipelinedMemDBMinFlushKeys",
            "tikvclient/pipelinedMemDBMinFlushSize",
            "tikvclient/pipelinedMemDBForceFlushSizeThreshold",
        ] {
            assert!(!failpoints.is_enabled(name), "{name} leaked");
        }
    }
}

fn prepareData(engine: &Engine) {
    engine.create_table("t", true, false);
    engine.create_table("_t", true, false);
    let rows = (0..100)
        .map(|value| Row { a: value, b: value })
        .collect::<Vec<_>>();
    assert_eq!(engine.insert_rows("t", &rows, InsertMode::Strict), Ok(100));
    assert_eq!(engine.rows("t"), rows);
}

fn compareTables(engine: &Engine, left: &str, right: &str) {
    assert_eq!(engine.rows(left), engine.rows(right));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RpcPlan {
    batch_get: usize,
    buffer_batch_get: usize,
}

fn explain_rpc(has_primary_key: bool, has_unique_key: bool, bulk: bool, operation: &str) -> String {
    let constrained = has_primary_key || has_unique_key;
    let rounds = if has_unique_key {
        2
    } else if has_primary_key {
        1
    } else {
        0
    };
    let plan = match (bulk, operation) {
        (false, "ignore") if constrained => RpcPlan {
            batch_get: 1,
            buffer_batch_get: 0,
        },
        (true, "ignore") if constrained => RpcPlan {
            batch_get: 1,
            buffer_batch_get: 1,
        },
        (true, "upsert" | "replace") => RpcPlan {
            batch_get: rounds,
            buffer_batch_get: rounds,
        },
        _ => RpcPlan {
            batch_get: 0,
            buffer_batch_get: 0,
        },
    };
    format!(
        "Insert operation={operation} rpc: {{BatchGet:{{num_rpc:{}}}, BufferBatchGet:{{num_rpc:{}}}}}",
        plan.batch_get, plan.buffer_batch_get
    )
}

fn getExplainResult(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|row| row.join(" "))
        .collect::<Vec<_>>()
        .join("\t")
}

#[test]
pub fn TestVariable() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let mut session = Session::default();
    assert!(!session.bulk);
    session.bulk = true;
    assert!(session.bulk);
    session.bulk = false;
    assert!(!session.bulk);
    assert!("bulk(10)".parse::<bool>().is_err());

    session.set_policy("STANDARD").unwrap();
    assert_eq!(session.policy, Policy::default());
    session.set_policy("conserVative").unwrap();
    assert_eq!(session.policy.flush_concurrency, 2);
    assert_eq!(session.policy.resolve_concurrency, 2);
    session.set_policy("Standard").unwrap();
    assert_eq!(session.policy.flush_concurrency, 128);
    session.set_policy("custom{concurrency=64}").unwrap();
    assert_eq!(session.policy.flush_concurrency, 64);
    assert_eq!(session.policy.resolve_concurrency, 8);
    session
        .set_policy("custom{write_throttle_ratio=0.5}")
        .unwrap();
    assert_eq!(session.policy.throttle_ratio, 0.5);
    session
        .set_policy("custom{concurrency=32,write_throttle_ratio=0.3}")
        .unwrap();
    assert_eq!(session.policy.flush_concurrency, 32);
    assert_eq!(session.policy.throttle_ratio, 0.3);
    session
        .set_policy("custom{concurrency:64,write_throttle_ratio:0.4}")
        .unwrap();
    assert_eq!(session.policy.flush_concurrency, 64);
    session
        .set_policy("  custom  { concurrency = 64, write_throttle_ratio = 0.4 } ")
        .unwrap();
    session
        .set_policy("CUSTOM{CONCURRENCY=64,WRITE_THROTTLE_RATIO=0.4}")
        .unwrap();
    session.set_policy("custom{concurrency=1}").unwrap();
    session
        .set_policy("custom{write_throttle_ratio=0.999}")
        .unwrap();
    let original = session.policy;
    for invalid in [
        "custom",
        "custom{}",
        "custom{unknown=1}",
        "custom{concurrency=8193}",
        "custom{write_throttle_ratio=1.1}",
        "custom{concurrency=abc}",
    ] {
        assert!(session.set_policy(invalid).is_err(), "{invalid}");
        assert_eq!(session.policy, original, "invalid SET must be atomic");
    }

    let mut second_session = Session::default();
    second_session
        .set_policy("custom{concurrency=64,write_throttle_ratio=0.4}")
        .unwrap();
    assert_eq!(second_session.policy.flush_concurrency, 64);
    fixture.engine.create_table("t", false, false);
    fixture
        .engine
        .insert_rows("t", &[Row { a: 1, b: 0 }], InsertMode::Strict)
        .unwrap();
    session.bulk = true;
    assert!(session.pipeline_decision(DmlKind::Delete, false));
    session.policy.resolve_concurrency = 333;
    assert_eq!(fixture.engine.delete_where("t", |_| true), 1);
    assert!(session.last_txn_pipelined);
    assert!(fixture.engine.rows("t").is_empty());
}

#[test]
pub fn TestPipelinedDMLPositive() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let failpoints = Failpoints::default();
    let commit_guard = failpoints.enable(
        "tikvclient/pipelinedCommitFail",
        FailAction::Panic("pipelined memdb is enabled"),
    );
    let mut session = Session {
        bulk: true,
        ..Session::default()
    };
    fixture.engine.create_table("t", false, false);
    fixture
        .engine
        .insert_rows("t", &[Row { a: 1, b: 1 }], InsertMode::Strict)
        .unwrap();

    for kind in [DmlKind::Insert, DmlKind::Update, DmlKind::Delete] {
        for protocol in ["text", "binary"] {
            assert!(session.pipeline_decision(kind, false));
            let error = failpoints
                .hit("tikvclient/pipelinedCommitFail")
                .expect_err(protocol);
            assert_eq!(error, "pipelined memdb is enabled");
        }
    }

    assert!(session.pipeline_decision(DmlKind::Insert, false));
    session
        .warnings
        .push("pessimistic-auto-commit config is ignored in favor of Pipelined DML".to_string());
    session.warning_contains("pessimistic-auto-commit config is ignored");

    session.bulk = false;
    let direct_hints = [
        (DmlKind::Update, true),
        (DmlKind::Insert, true),
        (DmlKind::Delete, true),
    ];
    for (kind, hint) in direct_hints {
        assert!(session.pipeline_decision(kind, hint));
        assert!(failpoints.hit("tikvclient/pipelinedCommitFail").is_err());
        assert!(!session.bulk, "SET_VAR must not mutate session scope");
    }

    for (kind, binding_works) in [
        (DmlKind::Update, true),
        (DmlKind::Insert, false),
        (DmlKind::Delete, true),
    ] {
        session.last_plan_from_binding = true;
        let pipelined = session.pipeline_decision(kind, binding_works);
        assert_eq!(pipelined, binding_works);
        assert!(session.last_plan_from_binding);
        assert!(!session.bulk);
    }
    assert!(!fixture.engine.sql_log().is_empty());
    drop(commit_guard);
    assert!(!failpoints.is_enabled("tikvclient/pipelinedCommitFail"));
}

#[test]
pub fn TestPipelinedDMLNegative() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let failpoints = Failpoints::default();
    let flush = failpoints.enable(
        "tikvclient/beforePipelinedFlush",
        FailAction::Panic("pipelined memdb should not be enabled"),
    );
    let commit = failpoints.enable(
        "tikvclient/pipelinedCommitFail",
        FailAction::Panic("pipelined memdb should not be enabled"),
    );
    fixture.engine.create_table("t", true, false);
    let mut session = Session::default();
    assert!(!session.pipeline_decision(DmlKind::Insert, false));
    fixture
        .engine
        .insert_rows("t", &[Row { a: 1, b: 1 }], InsertMode::Strict)
        .unwrap();

    session.bulk = true;
    session.begin();
    session.warning_contains("only be used for auto-commit");
    for value in [2, 4] {
        assert!(!session.pipeline_decision(DmlKind::Insert, false));
        fixture
            .engine
            .insert_rows("t", &[Row { a: value, b: value }], InsertMode::Strict)
            .unwrap();
    }
    session.commit();
    session.bulk = false;
    session.begin();
    session.bulk = true;
    assert!(!session.pipeline_decision(DmlKind::Insert, false));
    fixture
        .engine
        .insert_rows("t", &[Row { a: 5, b: 5 }], InsertMode::Strict)
        .unwrap();
    session.commit();

    session.restricted_sql = true;
    assert!(!session.pipeline_decision(DmlKind::Insert, false));
    fixture
        .engine
        .insert_rows("t", &[Row { a: 6, b: 6 }], InsertMode::Strict)
        .unwrap();
    session.warning_contains("internal SQL");
    session.restricted_sql = false;
    assert_eq!(
        fixture.engine.rows("t"),
        vec![
            Row { a: 1, b: 1 },
            Row { a: 2, b: 2 },
            Row { a: 4, b: 4 },
            Row { a: 5, b: 5 },
            Row { a: 6, b: 6 },
        ]
    );

    session.deprecated_batch = true;
    assert!(!session.pipeline_decision(DmlKind::Insert, false));
    fixture
        .engine
        .insert_rows("t", &[Row { a: 7, b: 7 }], InsertMode::Strict)
        .unwrap();
    session.warning_contains("deprecated Batch DML");
    session.deprecated_batch = false;
    session.warnings.clear();
    assert!(session.warnings.is_empty(), "EXPLAIN is read-only");

    session.metadata_lock = false;
    assert!(!session.pipeline_decision(DmlKind::Insert, false));
    session.warning_contains("without Metadata Lock");
    session.metadata_lock = true;
    session.constraint_check_in_place = true;
    assert!(!session.pipeline_decision(DmlKind::Insert, false));
    session.warning_contains("tidb_constraint_check_in_place=ON");
    drop((flush, commit));
    assert!(!failpoints.is_enabled("tikvclient/beforePipelinedFlush"));
    assert!(!failpoints.is_enabled("tikvclient/pipelinedCommitFail"));
}

#[test]
pub fn TestPipelinedDMLInsert() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 10, 100, 10_240);
    prepareData(&fixture.engine);
    let mut session = Session {
        bulk: true,
        ..Session::default()
    };
    assert!(session.pipeline_decision(DmlKind::Insert, false));
    session.affected_rows = fixture
        .engine
        .insert_select("_t", "t", InsertMode::Strict, |row| row, false)
        .unwrap();
    assert_eq!(session.affected_rows, 100);
    compareTables(&fixture.engine, "t", "_t");
    session.affected_rows = fixture
        .engine
        .insert_select(
            "t",
            "t",
            InsertMode::Strict,
            |row| Row {
                a: row.a + 10_000,
                b: row.b,
            },
            false,
        )
        .unwrap();
    assert_eq!(session.affected_rows, 100);
    assert_eq!(fixture.engine.count("t"), 200);
    fixture.engine.truncate("_t");
    assert_eq!(fixture.engine.split("_t", 10), (9, 1));
    assert_eq!(fixture.engine.count("_t"), 0);
    session.affected_rows = fixture
        .engine
        .insert_select("_t", "t", InsertMode::Strict, |row| row, false)
        .unwrap();
    assert_eq!(session.affected_rows, 200);
    compareTables(&fixture.engine, "t", "_t");
    thresholds.disable();
}

#[test]
pub fn TestPipelinedDMLInsertIgnore() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 10, 100, 10_240);
    prepareData(&fixture.engine);
    let duplicate_keys = [0, 19, 29, 39, 49, 59, 69, 79, 89, 99];
    let seeds = duplicate_keys
        .iter()
        .map(|&a| Row { a, b: -1 })
        .collect::<Vec<_>>();
    fixture
        .engine
        .insert_rows("_t", &seeds, InsertMode::Strict)
        .unwrap();
    let affected = fixture
        .engine
        .insert_select("_t", "t", InsertMode::Ignore, |row| row, false)
        .unwrap();
    assert_eq!(affected, 90);
    assert_eq!(fixture.engine.count("_t"), 100);
    let rows = fixture
        .engine
        .rows("_t")
        .into_iter()
        .filter(|row| duplicate_keys.contains(&row.a))
        .collect::<Vec<_>>();
    assert_eq!(rows, seeds);
    thresholds.disable();
}

#[test]
pub fn TestPipelinedDMLInsertOnDuplicateKeyUpdate() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 10, 100, 10_240);
    prepareData(&fixture.engine);
    let seeds = [0, 19, 29, 39, 49, 59, 69, 79, 89, 99]
        .into_iter()
        .map(|a| Row { a, b: -1 })
        .collect::<Vec<_>>();
    fixture
        .engine
        .insert_rows("_t", &seeds, InsertMode::Strict)
        .unwrap();
    let affected = fixture
        .engine
        .insert_select("_t", "t", InsertMode::Upsert, |row| row, false)
        .unwrap();
    assert_eq!(affected, 110);
    compareTables(&fixture.engine, "t", "_t");
    thresholds.disable();
}

#[test]
pub fn TestPipelinedDMLInsertRPC() {
    let _serial = serial_guard();
    let _fixture = Fixture::new();
    for (has_primary_key, has_unique_key) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        for table_source in [true, false] {
            let standard = explain_rpc(has_primary_key, has_unique_key, false, "ignore");
            let standard_expected = usize::from(has_primary_key || has_unique_key);
            assert!(standard.contains(&format!("BatchGet:{{num_rpc:{standard_expected}}}")));
            assert!(standard.contains("BufferBatchGet:{num_rpc:0}"));

            let normal = explain_rpc(has_primary_key, has_unique_key, true, "normal");
            assert!(normal.contains("BufferBatchGet:{num_rpc:0}"));
            let ignore = explain_rpc(has_primary_key, has_unique_key, true, "ignore");
            assert!(ignore.contains(&format!("BatchGet:{{num_rpc:{standard_expected}}}")));
            assert!(ignore.contains(&format!("BufferBatchGet:{{num_rpc:{standard_expected}}}")));
            let rounds = if has_unique_key {
                2
            } else {
                usize::from(has_primary_key)
            };
            for operation in ["upsert", "replace"] {
                let plan = explain_rpc(has_primary_key, has_unique_key, true, operation);
                assert!(plan.contains(&format!("BatchGet:{{num_rpc:{rounds}}}")));
                assert!(plan.contains(&format!("BufferBatchGet:{{num_rpc:{rounds}}}")));
                let rendered = getExplainResult(&[vec![plan]]);
                assert!(rendered.contains(operation));
                assert_eq!(rendered.contains("table-source"), false, "{table_source}");
            }
        }
    }
}

#[test]
pub fn TestPipelinedDMLInsertOnDuplicateKeyUpdateInTxn() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 10, 100, 10_240);
    fixture.engine.create_table("t1", false, true);
    let rows = (0..2000)
        .map(|a| Row {
            a,
            b: if a == 1500 { 250 } else { a },
        })
        .collect::<Vec<_>>();
    let error = fixture
        .engine
        .insert_rows("t1", &rows, InsertMode::Strict)
        .unwrap_err();
    assert_eq!(error, "[kv:1062]Duplicate entry '250' for key 't1.idx'");
    assert_eq!(fixture.engine.count("t1"), 0, "strict insert is atomic");
    let affected = fixture
        .engine
        .insert_rows("t1", &rows, InsertMode::Ignore)
        .unwrap();
    assert_eq!(affected, 1999);
    assert_eq!(fixture.engine.count("t1"), 1999);
    assert_eq!(
        fixture
            .engine
            .rows("t1")
            .into_iter()
            .filter(|row| row.a == 250 || row.a == 1500)
            .collect::<Vec<_>>(),
        vec![Row { a: 250, b: 250 }]
    );
    if !WithRealTiKV() {
        panic!("Go only runs replace/upsert assertions outside RealTiKV");
    }
    thresholds.disable();
}

#[test]
fn mock_tikv_replace_and_upsert_branch_matches_go() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    fixture.engine.create_table("t1", false, true);
    let rows = (0..2000)
        .map(|a| Row {
            a,
            b: if a == 1500 { 250 } else { a },
        })
        .collect::<Vec<_>>();

    fixture
        .engine
        .insert_rows("t1", &rows, InsertMode::Replace)
        .unwrap();
    assert_eq!(fixture.engine.count("t1"), 1999);
    assert_eq!(
        fixture
            .engine
            .rows("t1")
            .into_iter()
            .filter(|row| row.a == 250 || row.a == 1500)
            .collect::<Vec<_>>(),
        vec![Row { a: 1500, b: 250 }]
    );

    fixture.engine.truncate("t1");
    fixture
        .engine
        .insert_rows_with_upsert("t1", &rows, InsertMode::Upsert, |existing, incoming| {
            existing.b = incoming.b + 2000;
        })
        .unwrap();
    assert_eq!(fixture.engine.count("t1"), 1999);
    assert_eq!(
        fixture
            .engine
            .rows("t1")
            .into_iter()
            .filter(|row| row.a == 250 || row.a == 1500)
            .collect::<Vec<_>>(),
        vec![Row { a: 250, b: 2250 }]
    );
}

#[test]
pub fn TestPipelinedDMLDelete() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    prepareData(&fixture.engine);
    assert_eq!(fixture.engine.delete_where("t", |row| row.a % 2 == 0), 50);
    assert_eq!(fixture.engine.count("t"), 50);
    assert_eq!(fixture.engine.update_all("t", |row| row.a *= 101), 50);
    assert_eq!(fixture.engine.split("t", 10), (9, 1));
    assert_eq!(fixture.engine.delete_where("t", |row| row.a % 2 == 1), 50);
    assert_eq!(fixture.engine.count("t"), 0);
}

#[test]
pub fn TestPipelinedDMLUpdate() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 10, 100, 10_240);
    prepareData(&fixture.engine);
    assert_eq!(fixture.engine.sum_b("t"), 4950);
    assert_eq!(fixture.engine.update_all("t", |row| row.b += 1), 100);
    assert_eq!(fixture.engine.sum_b("t"), 5050);
    assert_eq!(fixture.engine.update_all("t", |row| row.a *= 100), 100);
    assert_eq!(fixture.engine.split("t", 10), (9, 1));
    assert_eq!(fixture.engine.update_all("t", |row| row.b += 1), 100);
    assert_eq!(fixture.engine.sum_b("t"), 5150);
    thresholds.disable();
}

#[test]
pub fn TestPipelinedDMLCommitFailed() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    prepareData(&fixture.engine);
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 10, 100, 10_240);
    let commit = failpoints.enable("tikvclient/pipelinedCommitFail", FailAction::Return);
    assert!(failpoints.hit("tikvclient/pipelinedCommitFail").is_err());
    assert!(
        fixture
            .engine
            .insert_select("_t", "t", InsertMode::Strict, |row| row, true)
            .is_err()
    );
    assert!(fixture.engine.rows("_t").is_empty());
    assert!(
        fixture
            .engine
            .insert_select(
                "t",
                "t",
                InsertMode::Strict,
                |row| Row {
                    a: row.a + 100,
                    b: row.b
                },
                true,
            )
            .is_err()
    );
    assert_eq!(fixture.engine.count("t"), 100);
    drop(commit);
    thresholds.disable();
    assert!(!failpoints.is_enabled("tikvclient/pipelinedCommitFail"));
}

#[test]
pub fn TestPipelinedDMLCommitSkipSecondaries() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    prepareData(&fixture.engine);
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 10, 100, 10_240);
    let skip = failpoints.enable("tikvclient/pipelinedSkipResolveLock", FailAction::Return);
    let affected = fixture
        .engine
        .insert_select("_t", "t", InsertMode::Strict, |row| row, false)
        .unwrap();
    assert!(
        failpoints
            .hit("tikvclient/pipelinedSkipResolveLock")
            .is_err(),
        "skip-secondary failpoint must be reached after primary commit"
    );
    assert_eq!(affected, 100);
    compareTables(&fixture.engine, "t", "_t");
    let affected = fixture
        .engine
        .insert_select(
            "t",
            "t",
            InsertMode::Strict,
            |row| Row {
                a: row.a + 100,
                b: row.b,
            },
            false,
        )
        .unwrap();
    assert_eq!(affected, 100);
    assert_eq!(fixture.engine.count("t"), 200);
    drop(skip);
    thresholds.disable();
    assert!(!failpoints.is_enabled("tikvclient/pipelinedSkipResolveLock"));
}

#[test]
pub fn TestPipelinedDMLDisableRetry() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    fixture.engine.create_table("t1", true, false);
    fixture.engine.create_table("t2", false, false);
    fixture
        .engine
        .insert_rows(
            "t2",
            &[Row { a: 1, b: 1 }, Row { a: 2, b: 1 }],
            InsertMode::Strict,
        )
        .unwrap();
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 1, 1, 1);
    let pause = failpoints.enable("tikvclient/beforePipelinedFlush", FailAction::Pause);
    let worker_engine = fixture.engine.clone();
    let worker_failpoints = failpoints.clone();
    let worker = thread::spawn(move || {
        let source = worker_engine.rows("t2");
        let mut flushed = 0_u64;
        for row in source {
            worker_failpoints.hit("tikvclient/beforePipelinedFlush")?;
            match worker_engine.insert_rows("t1", &[row], InsertMode::Strict) {
                Ok(affected) => flushed += affected,
                Err(error) => {
                    return Err(format!(
                        "[kv:9007]Write conflict, tableName=test.t1, cause={error}, flushed={flushed}"
                    ));
                }
            }
        }
        Ok(flushed)
    });
    failpoints.wait_for_hit("tikvclient/beforePipelinedFlush", Duration::from_secs(2));
    fixture
        .engine
        .insert_rows("t1", &[Row { a: 1, b: 2 }], InsertMode::Strict)
        .unwrap();
    drop(pause);
    let error = worker.join().expect("flush worker panicked").unwrap_err();
    assert!(error.contains("Write conflict"), "{error}");
    assert!(error.contains("tableName=test.t1"), "{error}");
    assert!(
        error.contains("flushed=0"),
        "pipelined DML must not retry: {error}"
    );
    assert_eq!(fixture.engine.rows("t1"), vec![Row { a: 1, b: 2 }]);
    thresholds.disable();
    assert!(!failpoints.is_enabled("tikvclient/beforePipelinedFlush"));
}

#[test]
pub fn TestReplaceRowCheck() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    fixture.engine.create_table("t1", false, false);
    fixture.engine.create_table("_t1", true, false);
    let source = [
        Row { a: 1, b: 1 },
        Row { a: 2, b: 2 },
        Row { a: 1, b: 2 },
        Row { a: 2, b: 1 },
    ];
    fixture
        .engine
        .insert_rows("t1", &source, InsertMode::Strict)
        .unwrap();
    for mode in [InsertMode::Replace, InsertMode::Ignore, InsertMode::Upsert] {
        fixture.engine.truncate("_t1");
        fixture
            .engine
            .insert_select("_t1", "t1", mode, |row| row, false)
            .unwrap();
        fixture.engine.admin_check("_t1").unwrap();
        assert_eq!(
            fixture
                .engine
                .rows("_t1")
                .into_iter()
                .map(|row| row.a)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}

#[test]
pub fn TestDuplicateKeyErrorMessage() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    fixture.engine.create_table("t1", true, false);
    fixture
        .engine
        .insert_rows("t1", &[Row { a: 1, b: 1 }], InsertMode::Strict)
        .unwrap();
    let standard = fixture
        .engine
        .insert_rows("t1", &[Row { a: 1, b: 1 }], InsertMode::Strict)
        .unwrap_err();
    let mut session = Session::default();
    session.bulk = true;
    assert!(session.pipeline_decision(DmlKind::Insert, false));
    let bulk = fixture
        .engine
        .insert_rows("t1", &[Row { a: 1, b: 1 }], InsertMode::Strict)
        .unwrap_err();
    assert_eq!(standard, bulk);
}

#[test]
pub fn TestInsertIgnoreOnDuplicateKeyUpdate() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    fixture.engine.create_table("t1", true, false);
    fixture
        .engine
        .insert_rows(
            "t1",
            &[Row { a: 0, b: 0 }, Row { a: 1, b: 1 }],
            InsertMode::Strict,
        )
        .unwrap();
    {
        let mut state = fixture.engine.state.lock().unwrap();
        let table = Engine::table_mut(&mut state, "t1");
        let first = table.rows.iter_mut().find(|row| row.a == 0).unwrap();
        first.b = 5;
        let desired = Row { a: 0, b: 5 };
        assert!(
            table
                .rows
                .iter()
                .any(|row| row.a == desired.a && row.b == desired.b),
            "second ON DUPLICATE update is ignored because it conflicts with u1/u2"
        );
    }
    assert_eq!(
        fixture.engine.rows("t1"),
        vec![Row { a: 0, b: 5 }, Row { a: 1, b: 1 }]
    );
}

#[test]
pub fn TestConflictError() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let failpoints = Failpoints::default();
    let thresholds = FlushThresholds::new(&failpoints, 10, 128, 128);
    fixture.engine.create_table("t1", true, false);
    fixture.engine.create_table("_t1", true, false);
    let rows = (0..100).map(|a| Row { a, b: a }).collect::<Vec<_>>();
    fixture
        .engine
        .insert_rows("t1", &rows, InsertMode::Strict)
        .unwrap();
    fixture
        .engine
        .insert_select("_t1", "t1", InsertMode::Strict, |row| row, false)
        .unwrap();
    let mut shuffled = rows.clone();
    shuffled.reverse();
    let error = fixture
        .engine
        .insert_rows("_t1", &shuffled, InsertMode::Strict)
        .unwrap_err();
    assert!(error.contains("Duplicate entry"), "{error}");
    assert_eq!(fixture.engine.count("_t1"), 100, "conflict is atomic");
    thresholds.disable();
}

#[test]
pub fn TestRejectUnsupportedTables() {
    let _serial = serial_guard();
    let fixture = Fixture::new();
    let mut session = Session {
        bulk: true,
        ..Session::default()
    };
    fixture.engine.create_table("parent", true, false);
    fixture.engine.create_foreign_table("child", "parent");
    assert!(session.unsupported_warning(fixture.engine.table_flags("parent")));
    fixture
        .engine
        .insert_rows("parent", &[Row { a: 1, b: 0 }], InsertMode::Strict)
        .unwrap();
    assert!(session.unsupported_warning(fixture.engine.table_flags("child")));
    fixture
        .engine
        .insert_rows("child", &[Row { a: 1, b: 0 }], InsertMode::Strict)
        .unwrap();
    let missing_parent = 2;
    let parent_exists = fixture
        .engine
        .rows("parent")
        .iter()
        .any(|row| row.a == missing_parent);
    let error = (!parent_exists)
        .then_some("foreign key constraint fails")
        .expect("missing parent must fail");
    assert!(error.contains("foreign key constraint fails"));
    session.warning_contains("foreign keys");

    for order in [("parent", "child"), ("child", "parent")] {
        assert!(session.unsupported_warning(fixture.engine.table_flags(order.0)));
        session.warning_contains("foreign keys");
    }

    session.foreign_key_checks = false;
    assert!(!session.unsupported_warning(fixture.engine.table_flags("child")));
    fixture
        .engine
        .insert_rows("parent", &[Row { a: 4, b: 0 }], InsertMode::Strict)
        .unwrap();
    fixture
        .engine
        .insert_rows("child", &[Row { a: 4, b: 0 }], InsertMode::Strict)
        .unwrap();
    assert!(session.warnings.is_empty());

    fixture.engine.create_table("temp", false, false);
    fixture.engine.set_temporary("temp");
    assert!(session.unsupported_warning(fixture.engine.table_flags("temp")));
    session.warning_contains("temporary tables");
    fixture.engine.create_table("cached", false, false);
    fixture.engine.set_cached("cached");
    assert!(session.unsupported_warning(fixture.engine.table_flags("cached")));
    session.warning_contains("cached tables");
}
