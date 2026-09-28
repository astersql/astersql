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

//! 中文说明开始（自动生成）
//! 中文总览：`txn_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `事务语义与时间戳行为` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 231 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `STORAGE_WAIT_TABLE` 是当前文件里的常量。
//! `STORAGE_WAIT_TABLE` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `STORAGE_WAIT_TABLE` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `STORAGE_WAIT_TABLE`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `STORAGE_WAIT_TABLE` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `STORAGE_WAIT_TABLE` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `BEFORE_PREWRITE` 是当前文件里的常量。
//! `BEFORE_PREWRITE` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `BEFORE_PREWRITE` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BEFORE_PREWRITE`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `BEFORE_PREWRITE` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `BEFORE_PREWRITE` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `PRIMARY_PREWRITE_HANDLED_RPC_ERROR` 是当前文件里的常量。
//! `PRIMARY_PREWRITE_HANDLED_RPC_ERROR` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `PRIMARY_PREWRITE_HANDLED_RPC_ERROR` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PRIMARY_PREWRITE_HANDLED_RPC_ERROR`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `PRIMARY_PREWRITE_HANDLED_RPC_ERROR` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `PRIMARY_PREWRITE_HANDLED_RPC_ERROR` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `PRIMARY_PREWRITE_RETRY` 是当前文件里的常量。
//! `PRIMARY_PREWRITE_RETRY` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `PRIMARY_PREWRITE_RETRY` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PRIMARY_PREWRITE_RETRY`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `PRIMARY_PREWRITE_RETRY` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `PRIMARY_PREWRITE_RETRY` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `AFTER_CHECK_FOREIGN_KEY` 是当前文件里的常量。
//! `AFTER_CHECK_FOREIGN_KEY` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `AFTER_CHECK_FOREIGN_KEY` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AFTER_CHECK_FOREIGN_KEY`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `AFTER_CHECK_FOREIGN_KEY` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `AFTER_CHECK_FOREIGN_KEY` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `ASYNC_COMMIT_PAUSE` 是当前文件里的常量。
//! `ASYNC_COMMIT_PAUSE` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `ASYNC_COMMIT_PAUSE` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ASYNC_COMMIT_PAUSE`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `ASYNC_COMMIT_PAUSE` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `ASYNC_COMMIT_PAUSE` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `PAUSE_TIMEOUT` 是当前文件里的常量。
//! `PAUSE_TIMEOUT` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `PAUSE_TIMEOUT` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PAUSE_TIMEOUT`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `PAUSE_TIMEOUT` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `PAUSE_TIMEOUT` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `fixture` 是当前文件里的辅助函数。
//! `fixture` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `fixture` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `fixture`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `fixture` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `fixture` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `peer` 是当前文件里的辅助函数。
//! `peer` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `peer` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `peer`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `peer` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `peer` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `contains_any` 是当前文件里的辅助函数。
//! `contains_any` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `contains_any` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `contains_any`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `contains_any` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `contains_any` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `assert_entry_limit` 是当前文件里的辅助函数。
//! `assert_entry_limit` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `assert_entry_limit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_entry_limit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_entry_limit` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `assert_entry_limit` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `EntryLimitGlobalReset` 是当前文件里的状态类型。
//! `EntryLimitGlobalReset` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `EntryLimitGlobalReset` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `EntryLimitGlobalReset`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `EntryLimitGlobalReset` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `EntryLimitGlobalReset` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `drop` 是当前文件里的辅助函数。
//! `drop` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `drop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `drop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `drop` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `drop` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `BoundedPause` 是当前文件里的状态类型。
//! `BoundedPause` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `BoundedPause` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BoundedPause`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `BoundedPause` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `BoundedPause` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `new` 是当前文件里的辅助函数。
//! `new` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `new` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `new`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `new` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `new` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `wait_until_reached` 是当前文件里的辅助函数。
//! `wait_until_reached` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `wait_until_reached` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `wait_until_reached`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `wait_until_reached` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `wait_until_reached` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `resume` 是当前文件里的辅助函数。
//! `resume` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `resume` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `resume`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `resume` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `resume` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `PrimaryPrewriteHookState` 是当前文件里的状态类型。
//! `PrimaryPrewriteHookState` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `PrimaryPrewriteHookState` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PrimaryPrewriteHookState`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `PrimaryPrewriteHookState` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `PrimaryPrewriteHookState` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `PrimaryPrewriteHook` 是当前文件里的状态类型。
//! `PrimaryPrewriteHook` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `PrimaryPrewriteHook` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PrimaryPrewriteHook`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `PrimaryPrewriteHook` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `PrimaryPrewriteHook` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `wait_for_calls` 是当前文件里的辅助函数。
//! `wait_for_calls` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `wait_for_calls` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `wait_for_calls`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `wait_for_calls` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `wait_for_calls` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `resume_first` 是当前文件里的辅助函数。
//! `resume_first` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `resume_first` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `resume_first`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `resume_first` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `resume_first` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `phases` 是当前文件里的辅助函数。
//! `phases` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `phases` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `phases`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `phases` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `phases` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `scalar` 是当前文件里的辅助函数。
//! `scalar` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `scalar` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `scalar`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `scalar` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `scalar` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `internal_scalar` 是当前文件里的辅助函数。
//! `internal_scalar` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `internal_scalar` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `internal_scalar`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `internal_scalar` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `internal_scalar` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `internal_last_commit_ts` 是当前文件里的辅助函数。
//! `internal_last_commit_ts` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `internal_last_commit_ts` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `internal_last_commit_ts`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `internal_last_commit_ts` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `internal_last_commit_ts` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestInTxnPSProtoPointGet` 是当前文件里的测试用例。
//! `TestInTxnPSProtoPointGet` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestInTxnPSProtoPointGet` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestInTxnPSProtoPointGet`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestInTxnPSProtoPointGet` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestInTxnPSProtoPointGet` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestTxnGoString` 是当前文件里的测试用例。
//! `TestTxnGoString` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestTxnGoString` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestTxnGoString`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestTxnGoString` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestTxnGoString` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestSetTransactionIsolationOneSho` 是当前文件里的测试用例。
//! `TestSetTransactionIsolationOneSho` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestSetTransactionIsolationOneSho` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestSetTransactionIsolationOneSho`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestSetTransactionIsolationOneSho` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestSetTransactionIsolationOneSho` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestStatementErrorInTransaction` 是当前文件里的测试用例。
//! `TestStatementErrorInTransaction` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStatementErrorInTransaction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStatementErrorInTransaction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStatementErrorInTransaction` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStatementErrorInTransaction` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestWriteConflictMessage` 是当前文件里的测试用例。
//! `TestWriteConflictMessage` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestWriteConflictMessage` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestWriteConflictMessage`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestWriteConflictMessage` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestWriteConflictMessage` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestDuplicateErrorMessage` 是当前文件里的测试用例。
//! `TestDuplicateErrorMessage` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestDuplicateErrorMessage` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestDuplicateErrorMessage`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestDuplicateErrorMessage` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestDuplicateErrorMessage` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestAssertionWhenPessimisticLockLost` 是当前文件里的测试用例。
//! `TestAssertionWhenPessimisticLockLost` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestAssertionWhenPessimisticLockLost` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestAssertionWhenPessimisticLockLost`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestAssertionWhenPessimisticLockLost` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestAssertionWhenPessimisticLockLost` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `assert_pessimistic_wait_visible` 是当前文件里的辅助函数。
//! `assert_pessimistic_wait_visible` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `assert_pessimistic_wait_visible` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_pessimistic_wait_visible`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_pessimistic_wait_visible` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `assert_pessimistic_wait_visible` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestPessimisticLockLockView` 是当前文件里的测试用例。
//! `TestPessimisticLockLockView` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestPessimisticLockLockView` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPessimisticLockLockView`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestPessimisticLockLockView` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestPessimisticLockLockView` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestPessimisticLockDataLockWaitsFromStorageWaitTable` 是当前文件里的测试用例。
//! `TestPessimisticLockDataLockWaitsFromStorageWaitTable` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestPessimisticLockDataLockWaitsFromStorageWaitTable` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPessimisticLockDataLockWaitsFromStorageWaitTable`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestPessimisticLockDataLockWaitsFromStorageWaitTable` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestPessimisticLockDataLockWaitsFromStorageWaitTable` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestSelectLockForPartitionTable` 是当前文件里的测试用例。
//! `TestSelectLockForPartitionTable` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestSelectLockForPartitionTable` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestSelectLockForPartitionTable`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestSelectLockForPartitionTable` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestSelectLockForPartitionTable` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestTxnEntrySizeLimit` 是当前文件里的测试用例。
//! `TestTxnEntrySizeLimit` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestTxnEntrySizeLimit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestTxnEntrySizeLimit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestTxnEntrySizeLimit` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestTxnEntrySizeLimit` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestCheckTxnStatusOnOptimisticTxnBreakConsistency` 是当前文件里的测试用例。
//! `TestCheckTxnStatusOnOptimisticTxnBreakConsistency` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestCheckTxnStatusOnOptimisticTxnBreakConsistency` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestCheckTxnStatusOnOptimisticTxnBreakConsistency`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestCheckTxnStatusOnOptimisticTxnBreakConsistency` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestCheckTxnStatusOnOptimisticTxnBreakConsistency` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestDMLWithAddForeignKey` 是当前文件里的测试用例。
//! `TestDMLWithAddForeignKey` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestDMLWithAddForeignKey` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestDMLWithAddForeignKey`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestDMLWithAddForeignKey` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestDMLWithAddForeignKey` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestLockKeysInDML` 是当前文件里的测试用例。
//! `TestLockKeysInDML` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestLockKeysInDML` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestLockKeysInDML`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestLockKeysInDML` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestLockKeysInDML` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestSelectForUpdateWriteConflict` 是当前文件里的测试用例。
//! `TestSelectForUpdateWriteConflict` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestSelectForUpdateWriteConflict` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestSelectForUpdateWriteConflict`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestSelectForUpdateWriteConflict` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestSelectForUpdateWriteConflict` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 符号 `TestIssue62775` 是当前文件里的测试用例。
//! `TestIssue62775` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestIssue62775` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestIssue62775`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestIssue62775` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestIssue62775` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 中文说明结束（自动生成）

//! Transaction integration tests ported from TiDB's
//! `tests/realtikvtest/txntest/txn_test.go`.

#![allow(non_snake_case)]

use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use astersql_kv::{Context, InternalTxnGC, WithInternalSourceType};
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{DbValue, NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_txntest::serial_guard;

const STORAGE_WAIT_TABLE: &str =
    "github.com/pingcap/tidb/pkg/executor/dataLockWaitsSkipResolvingLocks";
const BEFORE_PREWRITE: &str = "tikvclient/beforePrewrite";
const PRIMARY_PREWRITE_HANDLED_RPC_ERROR: &str = "primary-prewrite-handled-rpc-error";
const PRIMARY_PREWRITE_RETRY: &str = "primary-prewrite-retry";
const AFTER_CHECK_FOREIGN_KEY: &str =
    "github.com/pingcap/tidb/pkg/ddl/afterCheckForeignKeyConstrain";
const ASYNC_COMMIT_PAUSE: &str = "tikvclient/asyncCommitDoNothing";
const PAUSE_TIMEOUT: Duration = Duration::from_secs(3);

fn fixture(name: &str) -> (Arc<AnalyzeStatsStore>, TestKit, String) {
    let (store, _domain) = match std::env::var("ASTERSQL_TXN_TIKV_PATH") {
        Ok(path) => astersql_testkit::mockstore::CreateTiKVStoreAndDomain(&path)
            .expect("connect canonical SQL harness to TiKV"),
        Err(std::env::VarError::NotPresent) => CreateMockStoreAndDomain(),
        Err(error) => panic!("invalid ASTERSQL_TXN_TIKV_PATH: {error}"),
    };
    let database = format!(
        "txn_{name}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    );
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("create database `{database}`"), Vec::new());
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    (store, tk, database)
}

fn peer(store: Arc<AnalyzeStatsStore>, database: &str) -> TestKit {
    let mut tk = NewTestKit(store);
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    tk
}

fn contains_any(message: &str, fragments: &[&str]) -> bool {
    fragments.iter().any(|fragment| message.contains(fragment))
}

fn assert_entry_limit(tk: &mut TestKit, sql: &str, limit: usize) {
    let error = tk.ExecToErr(sql);
    let expected = format!("[kv:8025]entry too large, the max entry size is {limit}");
    assert!(
        error.message().contains(&expected),
        "unexpected entry-size error for `{sql}`: {error}"
    );
}

struct EntryLimitGlobalReset(TestKit);

impl Drop for EntryLimitGlobalReset {
    fn drop(&mut self) {
        let _ = self
            .0
            .Exec("set global tidb_txn_entry_size_limit=0", Vec::new());
    }
}

struct GlobalVariablesRestore {
    session: TestKit,
    values: Vec<(&'static str, String)>,
}

impl GlobalVariablesRestore {
    fn set(mut session: TestKit, variables: &[(&'static str, &str)]) -> Self {
        let mut guard = Self {
            session: session.clone(),
            values: Vec::new(),
        };
        for &(name, value) in variables {
            guard
                .values
                .push((name, scalar(&session, &format!("select @@global.{name}"))));
            session.MustExec(&format!("set global {name}={value}"), Vec::new());
        }
        guard
    }
}

impl Drop for GlobalVariablesRestore {
    fn drop(&mut self) {
        for (name, value) in self.values.iter().rev() {
            let escaped = value.replace("'", "''");
            let _ = self
                .session
                .Exec(&format!("set global {name}='{escaped}'"), Vec::new());
        }
    }
}

struct GlobalConfigRestore(astersql_config::Config);

impl Drop for GlobalConfigRestore {
    fn drop(&mut self) {
        astersql_config::store_global_config(self.0.clone());
    }
}

struct BoundedPause(testfailpoint::PauseGuard);

impl BoundedPause {
    fn new(name: &str) -> Self {
        Self(testfailpoint::enable_pause(name))
    }

    fn wait_until_reached(&self, operation: &str) {
        if !self.0.wait_until_reached_timeout(PAUSE_TIMEOUT) {
            self.0.resume();
            panic!("{operation} did not reach its failpoint within {PAUSE_TIMEOUT:?}");
        }
    }

    fn resume(&self) {
        self.0.resume();
    }
}

impl Drop for BoundedPause {
    fn drop(&mut self) {
        self.0.resume();
    }
}

#[derive(Default)]
struct PrimaryPrewriteHookState {
    phases: Vec<String>,
    first_resumed: bool,
}

struct PrimaryPrewriteHook {
    state: Arc<(Mutex<PrimaryPrewriteHookState>, Condvar)>,
    _interceptor: tikv_client::rpc_interceptor::InterceptorGuard,
}

struct PrimaryPrewriteInterceptor {
    state: Arc<(Mutex<PrimaryPrewriteHookState>, Condvar)>,
    start_ts: u64,
}

impl tikv_client::rpc_interceptor::RpcInterceptor for PrimaryPrewriteInterceptor {
    fn before(
        &self,
        request: &dyn std::any::Any,
        _timeout: &mut Duration,
    ) -> tikv_client::Result<()> {
        let Some(request) = request.downcast_ref::<tikv_client::proto::kvrpcpb::PrewriteRequest>()
        else {
            return Ok(());
        };
        if request.start_version != self.start_ts {
            return Ok(());
        }
        assert_eq!(
            request.mutations.len(),
            1,
            "the real prewrite must contain one mutation"
        );
        if request.mutations[0].key == request.primary_lock {
            let (lock, changed) = &*self.state;
            let mut state = lock.lock().expect("primary hook state");
            if !state.phases.is_empty() {
                state.phases.push(format!(
                    "{PRIMARY_PREWRITE_RETRY};start_ts={};mutations={};primary=true",
                    request.start_version,
                    request.mutations.len()
                ));
                changed.notify_all();
            }
        }
        Ok(())
    }

    fn after(
        &self,
        request: &dyn std::any::Any,
        result: &mut tikv_client::Result<Box<dyn std::any::Any>>,
    ) {
        let Some(request) = request.downcast_ref::<tikv_client::proto::kvrpcpb::PrewriteRequest>()
        else {
            return;
        };
        if request.start_version != self.start_ts
            || request.mutations.len() != 1
            || request.mutations[0].key != request.primary_lock
        {
            return;
        }
        let handled = result
            .as_ref()
            .ok()
            .and_then(|response| {
                response.downcast_ref::<tikv_client::proto::kvrpcpb::PrewriteResponse>()
            })
            .is_some_and(|response| response.region_error.is_none() && response.errors.is_empty());
        if !handled {
            return;
        }
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().expect("primary hook state");
        if !state.phases.is_empty() {
            return;
        }
        state.phases.push(format!(
            "{PRIMARY_PREWRITE_HANDLED_RPC_ERROR};start_ts={};mutations={};primary=true",
            request.start_version,
            request.mutations.len()
        ));
        changed.notify_all();
        drop(
            changed
                .wait_while(state, |state| !state.first_resumed)
                .expect("primary hook state"),
        );
        *result = Err(tikv_client::Error::GrpcAPI(tonic::Status::unavailable(
            "injected RPC response loss after TiKV handled primary prewrite",
        )));
    }

    fn prewrite_batch_size(&self, start_ts: u64) -> Option<usize> {
        (start_ts == self.start_ts).then_some(1)
    }
    fn lock_ttl(&self, start_ts: u64) -> Option<u64> {
        (start_ts == self.start_ts).then_some(100)
    }
    fn disable_heartbeat(&self, start_ts: u64) -> bool {
        start_ts == self.start_ts
    }
}

impl PrimaryPrewriteHook {
    fn new(start_ts: u64) -> Self {
        let state = Arc::new((
            Mutex::new(PrimaryPrewriteHookState::default()),
            Condvar::new(),
        ));
        let interceptor =
            tikv_client::rpc_interceptor::register(Arc::new(PrimaryPrewriteInterceptor {
                state: Arc::clone(&state),
                start_ts,
            }));
        Self {
            state,
            _interceptor: interceptor,
        }
    }

    fn wait_for_calls(&self, calls: usize, timeout: Duration) -> bool {
        let (lock, changed) = &*self.state;
        let state = lock.lock().expect("primary prewrite hook lock poisoned");
        let (state, _) = changed
            .wait_timeout_while(state, timeout, |state| state.phases.len() < calls)
            .expect("primary prewrite hook lock poisoned");
        state.phases.len() >= calls
    }

    fn resume_first(&self) {
        let (lock, changed) = &*self.state;
        lock.lock()
            .expect("primary prewrite hook lock poisoned")
            .first_resumed = true;
        changed.notify_all();
    }

    fn phases(&self) -> Vec<String> {
        self.state
            .0
            .lock()
            .expect("primary prewrite hook lock poisoned")
            .phases
            .clone()
    }
}

impl Drop for PrimaryPrewriteHook {
    fn drop(&mut self) {
        self.resume_first();
    }
}

fn scalar(tk: &TestKit, sql: &str) -> String {
    let rows = tk.MustQuery(sql, Vec::new()).Rows();
    assert_eq!(rows.len(), 1, "sql={sql:?}, rows={rows:?}");
    assert_eq!(rows[0].len(), 1, "sql={sql:?}, rows={rows:?}");
    rows[0][0].clone()
}

fn internal_scalar(
    session: &astersql_testkit::TestSession,
    context: &Context,
    sql: &str,
) -> String {
    let rows = session
        .QueryInternal(context, sql, &[])
        .unwrap_or_else(|error| panic!("internal query {sql:?}: {error}"))
        .string_rows();
    assert_eq!(rows.len(), 1, "sql={sql:?}, rows={rows:?}");
    assert_eq!(rows[0].len(), 1, "sql={sql:?}, rows={rows:?}");
    rows[0][0].clone()
}

fn internal_last_commit_ts(session: &astersql_testkit::TestSession, context: &Context) -> u64 {
    internal_scalar(
        session,
        context,
        "select json_extract(@@tidb_last_txn_info,'$.commit_ts')",
    )
    .trim_matches('"')
    .parse()
    .expect("internal last commit TS")
}

#[test]
fn TestInTxnPSProtoPointGet() {
    let _serial = serial_guard();
    let (_store, mut tk, _database) = fixture("prepared_point_get");
    tk.MustExec(
        "create table t1(c1 int primary key,c2 int,c3 int)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values(1,10,100)", Vec::new());

    let point_get = tk.Prepare("select c1,c2 from t1 where c1=?");
    let point_lock = tk.Prepare("select c1,c2 from t1 where c1=? for update");
    let args = [DbValue::from(1_i64)];
    for _ in 0..2 {
        assert_eq!(
            point_get
                .query(&args)
                .expect("prepared point get")
                .string_rows(),
            Rows(&["1 10"])
        );
        assert_eq!(
            point_lock
                .query(&args)
                .expect("prepared point lock")
                .string_rows(),
            Rows(&["1 10"])
        );
    }

    tk.MustExec("start transaction", Vec::new());
    assert_eq!(
        point_get
            .query(&args)
            .expect("in-txn point get")
            .string_rows(),
        Rows(&["1 10"])
    );
    assert!(
        tk.Session()
            .TransactionDebugStringForTest()
            .is_some_and(|state| state.starts_with("Txn{state=valid,")),
        "prepared statement must preserve a valid transaction"
    );
    assert_eq!(
        point_lock
            .query(&args)
            .expect("in-txn point lock")
            .string_rows(),
        Rows(&["1 10"])
    );
    assert!(
        tk.Session()
            .TransactionDebugStringForTest()
            .is_some_and(|state| state.starts_with("Txn{state=valid,")),
        "prepared statement must preserve a valid transaction"
    );
    tk.MustExec("update t1 set c2=c2+1", Vec::new());
    assert_eq!(
        point_get
            .query(&args)
            .expect("read own write through cached plan")
            .string_rows(),
        Rows(&["1 11"])
    );
    assert_eq!(
        point_lock
            .query(&args)
            .expect("lock read own write through cached plan")
            .string_rows(),
        Rows(&["1 11"])
    );
    assert!(
        tk.Session()
            .TransactionDebugStringForTest()
            .is_some_and(|state| state.starts_with("Txn{state=valid,")),
        "prepared statement must preserve a valid transaction"
    );
    tk.MustExec("commit", Vec::new());
}

#[test]
fn TestTxnGoString() {
    let _serial = serial_guard();
    let (_store, mut tk, _database) = fixture("txn_state");
    tk.MustExec("create table gostr(id int)", Vec::new());
    assert_eq!(
        tk.Session().TransactionDebugStringForTest().as_deref(),
        Some("Txn{state=invalid}")
    );

    tk.MustExec("begin", Vec::new());
    let start_ts = scalar(&tk, "select @@tidb_current_ts");
    assert_eq!(
        tk.Session().TransactionDebugStringForTest(),
        Some(format!("Txn{{state=valid, txnStartTS={start_ts}}}"))
    );
    tk.MustExec("insert into gostr values(1)", Vec::new());
    assert_eq!(
        tk.Session().TransactionDebugStringForTest(),
        Some(format!("Txn{{state=valid, txnStartTS={start_ts}}}"))
    );
    tk.MustExec("rollback", Vec::new());
    assert_eq!(
        tk.Session().TransactionDebugStringForTest().as_deref(),
        Some("Txn{state=invalid}")
    );
}

#[test]
fn TestSetTransactionIsolationOneSho() {
    let _serial = serial_guard();
    let (store, mut tk, _database) = fixture("one_shot_isolation");
    tk.MustExec("create table t(k int,v int)", Vec::new());
    tk.MustExec("insert into t values(1,42)", Vec::new());
    tk.MustExec("set tx_isolation='read-committed'", Vec::new());
    tk.MustQuery("select @@tx_isolation", Vec::new())
        .Check(Rows(&["READ-COMMITTED"]));
    tk.MustExec("set tx_isolation='repeatable-read'", Vec::new());
    tk.MustExec("set transaction isolation level read committed", Vec::new());
    tk.MustQuery("select @@tx_isolation_one_shot", Vec::new())
        .Check(Rows(&["READ-COMMITTED"]));
    tk.MustQuery("select @@tx_isolation", Vec::new())
        .Check(Rows(&["REPEATABLE-READ"]));
    // Go checks the requests from two separate autocommit SELECTs.
    for _ in 0..2 {
        store
            .clear_select_request_for_test()
            .expect("clear SELECT observation");
        tk.MustQuery("select * from t where k=1", Vec::new())
            .Check(Rows(&["1 42"]));
        let observed = store
            .last_select_request_for_test()
            .expect("read SELECT request")
            .expect("SELECT request was dispatched");
        assert_eq!(observed.request.IsolationLevel, astersql_kv::IsoLevel::SI);
        for request in observed.auxiliary_requests {
            assert_eq!(request.IsolationLevel, astersql_kv::IsoLevel::SI);
        }
    }
    tk.MustExec("set transaction isolation level read committed", Vec::new());
    tk.MustExec("begin", Vec::new());
    assert_eq!(
        tk.Session().TransactionIsolationForTest().as_deref(),
        Some("READ-COMMITTED"),
        "one-shot isolation was not installed on the active transaction"
    );
    tk.MustQuery("select * from t where k=1", Vec::new())
        .Check(Rows(&["1 42"]));
    tk.MustExec("commit", Vec::new());
    assert_eq!(
        tk.Session().TransactionIsolationForTest().as_deref(),
        Some("REPEATABLE-READ"),
        "one-shot isolation was not restored after the transaction"
    );
    tk.MustQuery("select * from t where k=1", Vec::new())
        .Check(Rows(&["1 42"]));
    tk.MustExec("begin", Vec::new());
    let error = tk.ExecToErr("set transaction isolation level read committed");
    assert!(
        error.message().to_ascii_lowercase().contains("transaction"),
        "unexpected one-shot isolation error: {error}"
    );
    tk.MustExec("rollback", Vec::new());
}

#[test]
fn TestStatementErrorInTransaction() {
    let _serial = serial_guard();
    let (_store, mut tk, _database) = fixture("statement_error");
    tk.MustExec(
        "create table statement_side_effect(c int primary key)",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into statement_side_effect values(1)", Vec::new());
    let duplicate = tk.ExecToErr("insert into statement_side_effect values(2),(3),(4),(1)");
    assert!(
        duplicate.message().contains("Duplicate entry"),
        "unexpected duplicate error: {duplicate}"
    );
    tk.MustQuery("select * from statement_side_effect", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select * from statement_side_effect", Vec::new())
        .Check(Rows(&["1"]));

    tk.MustExec("create table t(a int,b int)", Vec::new());
    tk.MustExec("insert into t values(1,2),(1,2),(1,1),(1,1)", Vec::new());
    tk.MustExec("start transaction", Vec::new());
    let missing = tk.ExecToErr("update missing_table set b=11 where a=1");
    assert!(
        missing
            .message()
            .to_ascii_lowercase()
            .contains("doesn't exist")
            || missing
                .message()
                .to_ascii_lowercase()
                .contains("unknown table")
            || missing
                .message()
                .to_ascii_lowercase()
                .contains("unknown dml table"),
        "unexpected missing-table error: {missing}"
    );
    tk.MustExec("update t set b=11 where a=1 and b=2", Vec::new());
    tk.MustExec("rollback", Vec::new());
    tk.MustQuery("select * from t where b=11", Vec::new())
        .Check(Rows(&[]));
}

#[test]
fn TestWriteConflictMessage() {
    let _serial = serial_guard();
    let (store, mut first, database) = fixture("write_conflict");
    let mut second = peer(store, &database);
    first.MustExec("create table t(c int primary key)", Vec::new());
    first.MustExec("begin optimistic", Vec::new());
    second.MustExec("insert into t values(1)", Vec::new());
    first.MustExec("insert into t values(1)", Vec::new());
    let error = first.ExecToErr("commit");
    let message = error.message();
    assert!(message.contains("Write conflict"));
    assert!(
        message.contains(&format!("tableName={database}.t, handle=1}}")),
        "missing table and handle: {message}"
    );
    assert!(
        message.contains("reason=Optimistic"),
        "missing conflict reason: {message}"
    );

    first.MustExec(
        "create table t2(id varchar(30) primary key clustered)",
        Vec::new(),
    );
    first.MustExec("begin optimistic", Vec::new());
    second.MustExec("insert into t2 values('hello')", Vec::new());
    first.MustExec("insert into t2 values('hello')", Vec::new());
    let common_handle = first.ExecToErr("commit");
    assert!(common_handle.message().contains("Write conflict"));
    assert!(
        common_handle
            .message()
            .contains(&format!("tableName={database}.t2, handle={{hello}}")),
        "missing common table and handle: {common_handle}"
    );
    assert!(
        common_handle.message().contains("reason=Optimistic"),
        "missing common-handle conflict reason: {common_handle}"
    );
}

#[test]
fn TestDuplicateErrorMessage() {
    let _serial = serial_guard();
    let (store, mut first, database) = fixture("duplicate_message");
    let mut second = peer(store, &database);
    first.MustExec("set @@tx_isolation='read-committed'", Vec::new());
    first.MustExec(
        "set @@tidb_constraint_check_in_place_pessimistic=off",
        Vec::new(),
    );
    first.MustExec("create table t(c int primary key,v int)", Vec::new());
    first.MustExec("create table t2(c int primary key,v int)", Vec::new());
    first.MustExec("begin pessimistic", Vec::new());
    first.MustExec("insert into t values(1,1)", Vec::new());
    let (done_tx, done_rx) = mpsc::channel();
    let peer_insert = thread::spawn(move || {
        let result = second
            .Exec("insert into t values(1,1)", Vec::new())
            .and_then(|_| second.Exec("insert into t2 values(1,2)", Vec::new()))
            .map(|_| ())
            .map_err(|error| error.to_string());
        done_tx
            .send((second, result))
            .expect("send primary duplicate setup");
    });
    let (returned_second, peer_result) = match done_rx.recv_timeout(Duration::from_millis(200)) {
        Ok(result) => result,
        Err(_) => {
            first.MustExec("rollback", Vec::new());
            if done_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
                peer_insert.join().expect("primary duplicate setup thread");
            } else {
                drop(peer_insert);
            }
            panic!("constraint-check-off insert unexpectedly blocked");
        }
    };
    peer_insert.join().expect("primary duplicate setup thread");
    peer_result.expect("peer primary insert succeeds");
    second = returned_second;
    let primary = first.ExecToErr("update t set v=v+1 where c=1");
    assert!(
        primary
            .message()
            .contains("Duplicate entry '1' for key 't.PRIMARY'"),
        "unexpected primary duplicate: {primary}"
    );
    first.MustExec("create table t3(c int,v int,unique key i1(v))", Vec::new());
    first.MustExec("create table t4(c int,v int,unique key i1(v))", Vec::new());
    first.MustExec("begin pessimistic", Vec::new());
    first.MustExec("insert into t3 values(1,1)", Vec::new());
    let (done_tx, done_rx) = mpsc::channel();
    let peer_insert = thread::spawn(move || {
        let result = second
            .Exec("insert into t3 values(1,1)", Vec::new())
            .and_then(|_| second.Exec("insert into t4 values(1,2)", Vec::new()))
            .map(|_| ())
            .map_err(|error| error.to_string());
        done_tx.send(result).expect("send unique duplicate setup");
    });
    let peer_result = match done_rx.recv_timeout(Duration::from_millis(200)) {
        Ok(result) => result,
        Err(_) => {
            first.MustExec("rollback", Vec::new());
            if done_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
                peer_insert.join().expect("unique duplicate setup thread");
            } else {
                drop(peer_insert);
            }
            panic!("constraint-check-off unique insert unexpectedly blocked");
        }
    };
    peer_insert.join().expect("unique duplicate setup thread");
    peer_result.expect("peer unique insert succeeds");
    let unique = first.ExecToErr("update t3 set c=c+1 where v=1");
    assert!(
        unique
            .message()
            .contains("Duplicate entry '1' for key 't3.i1'"),
        "unexpected unique duplicate: {unique}"
    );
}

#[test]
fn TestAssertionWhenPessimisticLockLost() {
    let _serial = serial_guard();
    let (store, mut first, database) = fixture("lock_lost");
    let mut second = peer(store, &database);
    for tk in [&mut first, &mut second] {
        tk.MustExec(
            "set @@tidb_constraint_check_in_place_pessimistic=0",
            Vec::new(),
        );
        tk.MustExec("set @@tidb_txn_assertion_level='strict'", Vec::new());
    }
    first.MustExec("create table t(id int primary key,val text)", Vec::new());
    first.MustExec("begin pessimistic", Vec::new());
    first
        .MustQuery("select * from t where id=1 for update", Vec::new())
        .Check(Rows(&[]));
    let (done_tx, done_rx) = mpsc::channel();
    let concurrent_insert = thread::spawn(move || {
        let result = second
            .Exec("begin pessimistic", Vec::new())
            .and_then(|_| second.Exec("insert into t values(1,'b')", Vec::new()))
            .and_then(|_| second.Exec("insert into t values(2,'b')", Vec::new()))
            .and_then(|_| second.Exec("commit", Vec::new()))
            .map(|_| ())
            .map_err(|error| error.to_string());
        done_tx.send(result).expect("send concurrent insert");
    });
    let insert_result = match done_rx.recv_timeout(Duration::from_millis(200)) {
        Ok(result) => result,
        Err(_) => {
            first.MustExec("rollback", Vec::new());
            if done_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
                concurrent_insert.join().expect("concurrent insert thread");
            } else {
                drop(concurrent_insert);
            }
            panic!(
                "tidb_constraint_check_in_place_pessimistic=0 did not prevent \
                 the missing-key FOR UPDATE lock from blocking the concurrent insert"
            );
        }
    };
    insert_result.expect("concurrent insert succeeds after lost lock");
    concurrent_insert.join().expect("concurrent insert thread");
    first
        .MustQuery("select * from t where id=2 for update", Vec::new())
        .Check(Rows(&["2 b"]));
    first.MustExec(
        "insert into t values(1,'a') on duplicate key update val=concat(val,'a')",
        Vec::new(),
    );
    let error = first.ExecToErr("commit");
    assert!(
        !error.message().to_ascii_lowercase().contains("assertion"),
        "lost pessimistic lock must not surface assertion error: {error}"
    );
}

fn assert_pessimistic_wait_visible(name: &str, storage_wait_table: bool) {
    let _storage_wait_guard =
        storage_wait_table.then(|| testfailpoint::enable(STORAGE_WAIT_TABLE, "return(true)"));
    assert_eq!(
        testfailpoint::is_active(STORAGE_WAIT_TABLE),
        storage_wait_table,
        "storage-wait-table case did not select its Go failpoint path"
    );
    let (store, mut holder, database) = fixture(name);
    holder.MustExec(
        "create table ordinary_lock_view(id int primary key,v int)",
        Vec::new(),
    );
    holder.MustExec("insert into ordinary_lock_view values(1,10)", Vec::new());
    holder.MustExec("begin pessimistic", Vec::new());
    holder.MustQuery(
        "select * from ordinary_lock_view where id=1 for update",
        Vec::new(),
    );

    let waiter_store = store.clone();
    let waiter_database = database.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let mut tk = peer(waiter_store, &waiter_database);
        tk.MustExec("begin pessimistic", Vec::new());
        let connection_id = tk.MustQuery("select connection_id()", Vec::new()).Rows()[0][0]
            .parse::<u64>()
            .expect("connection_id() is numeric");
        let start_ts = scalar(&tk, "select @@tidb_current_ts");
        started_tx
            .send((connection_id, start_ts))
            .expect("signal waiter");
        let result = tk.Query(
            "select * from ordinary_lock_view where id=1 for update",
            Vec::new(),
        );
        if result.is_ok() {
            tk.MustExec("commit", Vec::new());
        }
        let _ = done_tx.send(result.map(|_| ()));
    });
    let (waiter_connection, waiter_transaction) = started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("waiter started");
    assert!(
        done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "select for update unexpectedly completed before holder commit"
    );

    let inspector = peer(store, &database);
    let raw_transactions = inspector
        .MustQuery("select * from information_schema.tidb_trx", Vec::new())
        .Rows();
    let sql = format!(
        "select trx_id,session_id \
         from information_schema.data_lock_waits as l \
         left join information_schema.tidb_trx as trx on l.trx_id=trx.id \
         where l.trx_id={waiter_transaction} and trx.session_id={waiter_connection}"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let rows = loop {
        match done_rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {}
            result => panic!("waiter completed before wait row was observed: {result:?}"),
        }
        let rows = inspector.MustQuery(&sql, Vec::new()).Rows();
        if !rows.is_empty() {
            break rows;
        }
        assert!(
            Instant::now() < deadline,
            "missing wait row for transaction {waiter_transaction}"
        );
        thread::sleep(Duration::from_millis(100));
    };
    let raw_waits = inspector
        .MustQuery(
            "select * from information_schema.data_lock_waits",
            Vec::new(),
        )
        .Rows();
    assert!(
        !rows.is_empty(),
        "missing DATA_LOCK_WAITS join row for session {waiter_connection}; \
         waits={raw_waits:?}, transactions={raw_transactions:?}"
    );
    assert_eq!(rows.len(), 1, "unexpected wait rows: {rows:?}");
    assert_eq!(rows[0][0], waiter_transaction);
    assert_eq!(rows[0][1], waiter_connection.to_string());
    inspector
        .MustQuery(
            &format!(
                "select l.trx_id,trx.session_id \
                 from information_schema.data_lock_waits as l \
                 left join information_schema.tidb_trx as trx on l.trx_id=trx.id \
                 where l.trx_id={waiter_transaction} and trx.session_id={waiter_connection}"
            ),
            Vec::new(),
        )
        .Check(Rows(&[&format!(
            "{waiter_transaction} {waiter_connection}"
        )]));
    inspector
        .MustQuery(
            &format!(
                "select l.trx_id,trx.session_id \
                 from information_schema.data_lock_waits as l \
                 left join information_schema.tidb_trx as trx \
                 on l.trx_id=trx.id and trx.session_id=-1 \
                 where l.trx_id={waiter_transaction}"
            ),
            Vec::new(),
        )
        .Check(Rows(&[&format!("{waiter_transaction} <nil>")]));
    let ambiguous = inspector
        .Query(
            &format!(
                "select id from information_schema.tidb_trx as left_trx \
                 left join information_schema.tidb_trx as right_trx \
                 on left_trx.id=right_trx.id where left_trx.id={waiter_transaction}"
            ),
            Vec::new(),
        )
        .expect_err("an unqualified duplicate column must be ambiguous");
    assert!(
        ambiguous
            .to_string()
            .to_ascii_lowercase()
            .contains("ambiguous"),
        "unexpected ambiguous-column error: {ambiguous}"
    );
    holder.MustExec("commit", Vec::new());
    // Go waits for both SELECT and COMMIT here without a two-second deadline.
    // Allow the real storage lock retry and commit to finish under load.
    done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("waiter resumes")
        .expect("waiter select succeeds");
    waiter.join().expect("waiter thread");
}

#[test]
fn TestPessimisticLockLockView() {
    let _serial = serial_guard();
    assert_pessimistic_wait_visible("lock_view", false);
}

#[test]
fn TestPessimisticLockDataLockWaitsFromStorageWaitTable() {
    let _serial = serial_guard();
    assert_pessimistic_wait_visible("storage_wait_view", true);
}

#[test]
fn TestSelectLockForPartitionTable() {
    let _serial = serial_guard();
    let (store, mut holder, database) = fixture("partition_lock");
    holder.MustExec(
        "create table t(a int,b int,c int,key idx(a,b,c)) \
         partition by hash(c) partitions 10",
        Vec::new(),
    );
    holder.MustExec("insert into t values(1,1,1),(2,2,2),(3,3,3)", Vec::new());
    holder.MustExec("analyze table t", Vec::new());
    let locking_select =
        "select * from t use index(idx) where a=1 and b=1 order by a limit 1 for update";
    holder.MustExec("begin", Vec::new());
    let plan = holder.MustQuery(&format!("explain {locking_select}"), Vec::new());
    assert!(
        plan.Rows().iter().any(|row| row[0].contains("IndexReader")),
        "partition locking query must use IndexReader: {:?}",
        plan.Rows()
    );
    holder
        .MustQuery(locking_select, Vec::new())
        .Check(Rows(&["1 1 1"]));

    let (begun_tx, begun_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let mut tk = peer(store, &database);
        tk.MustExec("begin", Vec::new());
        begun_tx.send(()).expect("signal partition waiter");
        let result = tk.Query(locking_select, Vec::new());
        done_tx.send(result.map(|_| ())).expect("send lock result");
    });
    begun_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("partition waiter begun");
    assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
    holder.MustExec("commit", Vec::new());
    done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("partition waiter resumes")
        .expect("partition lock succeeds");
    waiter.join().expect("partition waiter");
}

#[test]
fn TestTxnEntrySizeLimit() {
    let _serial = serial_guard();
    let (store, mut first, database) = fixture("entry_limit");
    let mut second = peer(store.clone(), &database);
    let _global_reset = EntryLimitGlobalReset(peer(store.clone(), &database));
    first.MustExec("create table t(a int,b longtext)", Vec::new());
    assert_entry_limit(
        &mut first,
        "insert into t values(1,repeat('a',7340032))",
        6_291_456,
    );

    first.MustExec("set session tidb_txn_entry_size_limit=8388608", Vec::new());
    first.MustExec("insert into t values(1,repeat('a',7340032))", Vec::new());
    assert_entry_limit(
        &mut first,
        "insert into t values(1,repeat('a',9427968))",
        8_388_608,
    );

    assert_entry_limit(
        &mut second,
        "insert into t values(1,repeat('a',7340032))",
        6_291_456,
    );
    let mut third = peer(store.clone(), &database);
    assert_entry_limit(
        &mut third,
        "insert into t values(1,repeat('a',7340032))",
        6_291_456,
    );

    assert_entry_limit(
        &mut first,
        "alter table t modify column a varchar(255)",
        6_291_456,
    );

    first.MustExec("set global tidb_txn_entry_size_limit=8388608", Vec::new());
    first.MustExec("alter table t modify column a varchar(255)", Vec::new());
    second.MustExec("alter table t modify column a int", Vec::new());

    assert_entry_limit(
        &mut second,
        "insert into t values(1,repeat('a',7340032))",
        6_291_456,
    );
    assert_entry_limit(
        &mut third,
        "insert into t values(1,repeat('a',7340032))",
        6_291_456,
    );

    let mut fourth = peer(store, &database);
    fourth.MustExec("insert into t values(2,repeat('b',7340032))", Vec::new());

    first.MustExec("set global tidb_txn_entry_size_limit=0", Vec::new());
    for tk in [&mut first, &mut second, &mut third, &mut fourth] {
        assert_entry_limit(tk, "alter table t modify column a varchar(255)", 6_291_456);
    }

    first.MustExec("insert into t values(3,repeat('c',7340032))", Vec::new());
    first.MustExec("set session tidb_txn_entry_size_limit=0", Vec::new());
    assert_entry_limit(
        &mut first,
        "insert into t values(1,repeat('a',7340032))",
        6_291_456,
    );
}

#[test]
fn TestCheckTxnStatusOnOptimisticTxnBreakConsistency() {
    let _serial = serial_guard();
    // Go skips this scenario on mock storage: it requires real prewrite locks.
    if std::env::var_os("ASTERSQL_TXN_TIKV_PATH").is_none() {
        return;
    }
    let _config_restore =
        GlobalConfigRestore(astersql_config::get_global_config().as_ref().clone());
    astersql_config::update_global(|config| {
        config.tikv_client.async_commit.safe_window = 500_000_000;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
    });
    let (store, mut first, database) = fixture("optimistic_consistency");
    let mut prepare_writer = peer(store.clone(), &database);
    let mut prepare_reader = peer(store.clone(), &database);
    let mut second = peer(store, &database);
    first.MustExec("create table t(id int primary key,v int)", Vec::new());
    first.MustExec("insert into t values(1,10),(2,20)", Vec::new());
    first.MustExec(
        "create table t2(id int primary key,v int unique)",
        Vec::new(),
    );
    first.MustExec("insert into t2 values(1,10)", Vec::new());
    prepare_writer.MustExec("set @@tidb_enable_async_commit=1", Vec::new());
    first.MustExec("set @@tidb_enable_async_commit=0", Vec::new());

    // Construct the same timestamp collision as Go:
    // first.StartTS == prepare_writer.LastCommitTS on row 1.
    let mut construction_attempt = 0;
    let collision_ts = loop {
        prepare_writer.MustExec("update t set v=10 where id=1", Vec::new());
        let prepare_pause = BoundedPause::new(BEFORE_PREWRITE);
        let (prepare_tx, prepare_rx) = mpsc::channel();
        let prepare_worker = thread::spawn(move || {
            let result = prepare_writer
                .Exec("update t set v=v+1 where id=1", Vec::new())
                .map(|_| ())
                .map_err(|error| error.to_string());
            prepare_tx
                .send((prepare_writer, result))
                .expect("send async prepare result");
        });
        prepare_pause.wait_until_reached("async collision writer");
        prepare_reader
            .MustQuery("select * from t where id=1", Vec::new())
            .Check(Rows(&["1 10"]));
        first.MustExec("begin optimistic", Vec::new());
        prepare_pause.resume();
        let (returned_writer, prepare_result) = prepare_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("async collision writer resumes");
        prepare_writer = returned_writer;
        prepare_result.expect("async collision writer commits");
        prepare_worker.join().expect("async collision writer");
        drop(prepare_pause);

        let last_commit_ts: u64 = scalar(
            &prepare_writer,
            "select json_extract(@@tidb_last_txn_info,'$.commit_ts')",
        )
        .trim_matches('"')
        .parse()
        .expect("prepare writer LastCommitTS");
        let current_start_ts: u64 = scalar(&first, "select @@tidb_current_ts")
            .parse()
            .expect("optimistic transaction StartTS");
        if current_start_ts == last_commit_ts {
            break last_commit_ts;
        }
        first.MustExec("rollback", Vec::new());
        assert!(
            construction_attempt < 1000,
            "failed to construct async-commit timestamp collision: \
             current StartTS={current_start_ts}, prior LastCommitTS={last_commit_ts}"
        );
        construction_attempt += 1;
    };
    assert_eq!(
        scalar(&first, "select @@tidb_current_ts")
            .parse::<u64>()
            .expect("current transaction StartTS"),
        collision_ts,
        "the regression requires StartTS to collide with the prior CommitTS"
    );

    first.MustExec("update t set v=v+100 where id=1", Vec::new());
    first.MustExec("update t set v=v+100 where id=2", Vec::new());
    first.MustExec("update t2 set v=v+1 where id=1", Vec::new());

    let _short_ttl = testfailpoint::enable("tikvclient/twoPCShortLockTTL", "return");
    let _no_keepalive = testfailpoint::enable("tikvclient/doNotKeepAlive", "return");
    let _single_mutation = testfailpoint::enable("tikvclient/twoPCRequestBatchSizeLimit", "return");
    let primary_hook = PrimaryPrewriteHook::new(collision_ts);
    let (commit_tx, commit_rx) = mpsc::channel();
    let commit_worker = thread::spawn(move || {
        let result = first
            .Exec("commit", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        commit_tx.send(result).expect("send consistency commit");
    });
    if !primary_hook.wait_for_calls(1, PAUSE_TIMEOUT) {
        primary_hook.resume_first();
        match commit_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => {
                commit_worker.join().expect("consistency commit worker");
                panic!(
                    "commit never reported a successfully handled primary prewrite whose RPC \
                     response was replaced by an injected error; commit result={result:?}"
                );
            }
            Err(error) => {
                drop(commit_worker);
                panic!(
                    "commit never reported a successfully handled primary prewrite and did not \
                     terminate during bounded cleanup: {error}"
                );
            }
        }
    }
    match commit_rx.recv_timeout(Duration::from_millis(50)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Ok(result) => {
            commit_worker.join().expect("consistency commit worker");
            panic!("commit escaped the first primary-prewrite RPC hook: {result:?}");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            commit_worker.join().expect("consistency commit worker");
            panic!("commit worker disconnected at the first primary-prewrite RPC hook");
        }
    }

    // While the first, successfully handled primary prewrite is hidden behind
    // an injected RPC error, tk2 must resolve the short-lived locks.
    let (resolver_tx, resolver_rx) = mpsc::channel();
    let resolver = thread::spawn(move || {
        let result = second
            .Exec("update t set v=v+1 where id=2", Vec::new())
            .and_then(|_| second.Exec("insert into t2 values(2,11)", Vec::new()))
            .map(|_| ())
            .map_err(|error| error.to_string());
        resolver_tx
            .send((second, result))
            .expect("send resolve-lock result");
    });
    let (mut second, resolve_result) = match resolver_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(_) => {
            primary_hook.resume_first();
            if commit_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
                commit_worker.join().expect("consistency commit worker");
            } else {
                drop(commit_worker);
            }
            if resolver_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
                resolver.join().expect("resolve-lock worker");
            } else {
                drop(resolver);
            }
            panic!("tk2 did not resolve the prewritten transaction's short-lived locks");
        }
    };
    resolver.join().expect("resolve-lock worker");
    resolve_result.expect("conflicting session resolves the optimistic transaction");
    match commit_rx.recv_timeout(Duration::from_millis(50)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Ok(result) => {
            commit_worker.join().expect("consistency commit worker");
            panic!("commit resumed before the test released its injected RPC failure: {result:?}");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            commit_worker.join().expect("consistency commit worker");
            panic!("commit worker disconnected before the injected RPC failure was released");
        }
    }

    primary_hook.resume_first();
    let conflict = match commit_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => {
            commit_worker.join().expect("consistency commit worker");
            result.expect_err("colliding optimistic transaction must fail")
        }
        Err(error) => {
            drop(commit_worker);
            panic!("consistency commit did not finish after bounded resume: {error}");
        }
    };
    assert!(
        primary_hook.wait_for_calls(2, Duration::from_millis(50)),
        "primary prewrite was not retried after the injected RPC failure"
    );
    let primary_phases = primary_hook.phases();
    assert_eq!(
        primary_phases.len(),
        2,
        "the primary must be prewritten exactly twice"
    );
    assert_eq!(
        primary_phases,
        [
            format!(
                "{PRIMARY_PREWRITE_HANDLED_RPC_ERROR};start_ts={collision_ts};mutations=1;primary=true"
            ),
            format!("{PRIMARY_PREWRITE_RETRY};start_ts={collision_ts};mutations=1;primary=true"),
        ],
        "the hook must expose two single-mutation primary prewrites for the colliding StartTS"
    );
    assert!(
        conflict.contains("[kv:9007]")
            && contains_any(&conflict, &["Write conflict", "write conflict"]),
        "unexpected optimistic consistency result: {conflict}"
    );
    second
        .MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 11", "2 21"]));
    second.MustExec("admin check table t2", Vec::new());
    second
        .MustQuery("select * from t2 order by id", Vec::new())
        .Check(Rows(&["1 10", "2 11"]));
}

#[test]
fn TestDMLWithAddForeignKey() {
    let _serial = serial_guard();
    if astersql_config_kerneltype::IsNextGen() {
        // Same Go precondition: NextGen cannot disable MDL for this race.
        return;
    }
    eprintln!(
        "foreign-key race: kernel=classic, storage={}",
        if std::env::var_os("ASTERSQL_TXN_TIKV_PATH").is_some() {
            "tikv"
        } else {
            "mock"
        }
    );
    let _config_restore =
        GlobalConfigRestore(astersql_config::get_global_config().as_ref().clone());
    astersql_config::update_global(|config| {
        config.tikv_client.async_commit.safe_window = 10_000_000_000;
        config.tikv_client.async_commit.allowed_clock_drift = 500_000_000;
    });
    let (store, mut setup, database) = fixture("fk_race");
    let _globals = GlobalVariablesRestore::set(
        peer(store.clone(), &database),
        &[
            ("tidb_enable_1pc", "'OFF'"),
            ("tidb_enable_metadata_lock", "'OFF'"),
            ("tidb_enable_async_commit", "'ON'"),
        ],
    );
    setup.MustExec(
        "create table parent(id int primary key,val int,index(val))",
        Vec::new(),
    );
    setup.MustExec(
        "create table child(id int primary key,val int,index(val))",
        Vec::new(),
    );

    let before_prewrite = BoundedPause::new(BEFORE_PREWRITE);
    let after_check = BoundedPause::new(AFTER_CHECK_FOREIGN_KEY);
    let async_commit = BoundedPause::new(ASYNC_COMMIT_PAUSE);
    let dml_store = store.clone();
    let dml_database = database.clone();
    let ddl_store = store;
    let ddl_database = database;
    let (dml_tx, dml_rx) = mpsc::channel();
    let dml = thread::spawn(move || {
        let result = peer(dml_store, &dml_database)
            .Exec("insert into child values(1,1)", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        dml_tx.send(result).expect("send orphan DML result");
    });
    let deadline = Instant::now() + PAUSE_TIMEOUT;
    loop {
        if let Ok(result) = dml_rx.try_recv() {
            panic!("orphan DML returned before reaching prewrite: {result:?}");
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            before_prewrite.resume();
            panic!("orphan DML prewrite did not reach its failpoint within {PAUSE_TIMEOUT:?}");
        }
        if before_prewrite
            .0
            .wait_until_reached_timeout(remaining.min(Duration::from_millis(25)))
        {
            break;
        }
    }
    eprintln!("foreign-key race: orphan DML reached beforePrewrite");
    let (ddl_tx, ddl_rx) = mpsc::channel();
    let add_fk = thread::spawn(move || {
        let result = peer(ddl_store, &ddl_database)
            .Exec(
                "alter table child add foreign key fk(val) references parent(val)",
                Vec::new(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string());
        ddl_tx.send(result).expect("send ADD FOREIGN KEY result");
    });
    after_check.wait_until_reached("ADD FOREIGN KEY constraint check");
    eprintln!("foreign-key race: ADD FOREIGN KEY reached afterCheckForeignKeyConstrain");
    before_prewrite.resume();
    async_commit.wait_until_reached("orphan DML async commit");
    eprintln!("foreign-key race: orphan DML reached asyncCommitDoNothing");
    after_check.resume();
    let ddl_result = ddl_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("ADD FOREIGN KEY finishes");
    add_fk.join().expect("DDL thread");
    async_commit.resume();
    let dml_result = dml_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("orphan DML finishes");
    dml.join().expect("DML thread");
    eprintln!("foreign-key race: DML={dml_result:?}, DDL={ddl_result:?}");
    let errors = [dml_result.as_ref().err(), ddl_result.as_ref().err()]
        .into_iter()
        .flatten()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(
        !errors.is_empty(),
        "concurrent orphan DML and ADD FOREIGN KEY cannot both succeed"
    );
    assert!(
        errors.iter().all(|error| {
            !error.contains("dropped its response")
                && !error.contains("session is closed")
                && !error.contains("worker panicked")
        }),
        "race must fail through SQL constraint semantics, not a session-worker panic: {errors:?}"
    );
}

#[test]
fn TestLockKeysInDML() {
    let _serial = serial_guard();
    let (store, mut first, database) = fixture("fk_lock");
    first.MustExec("create table t1(id int primary key)", Vec::new());
    first.MustExec(
        "create table t2(id int primary key,foreign key fk(id) references t1(id))",
        Vec::new(),
    );
    first.MustExec("insert into t1 values(1)", Vec::new());
    first.MustExec("begin", Vec::new());
    first.MustExec("insert into t2 values(1)", Vec::new());

    let started = Instant::now();
    let (done_tx, done_rx) = mpsc::channel();
    let updater = thread::spawn(move || {
        let mut second = peer(store, &database);
        second.MustExec("begin", Vec::new());
        let result = second.Exec("update t1 set id=2 where id=1", Vec::new());
        second.MustExec("commit", Vec::new());
        done_tx.send(result).expect("send FK update result");
    });
    assert!(done_rx.recv_timeout(Duration::from_millis(500)).is_err());
    first.MustExec("commit", Vec::new());
    let update = done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("FK updater resumes");
    assert!(started.elapsed() >= Duration::from_millis(500));
    assert!(update.is_err(), "parent-key update must violate FK");
    updater.join().expect("FK updater");
    first
        .MustQuery("select * from t1", Vec::new())
        .Check(Rows(&["1"]));
    first
        .MustQuery("select * from t2", Vec::new())
        .Check(Rows(&["1"]));
}

#[test]
fn TestSelectForUpdateWriteConflict() {
    let _serial = serial_guard();
    let (store, mut first, database) = fixture("select_for_update_conflict");
    let mut second = peer(store, &database);
    first.MustExec("create table t(id int primary key,val int)", Vec::new());
    first.MustExec("insert into t values(1,100)", Vec::new());
    first.MustExec("begin optimistic", Vec::new());
    first
        .MustQuery("select * from t where id=1 for update", Vec::new())
        .Check(Rows(&["1 100"]));
    second.MustExec("begin optimistic", Vec::new());
    let (done_tx, done_rx) = mpsc::channel();
    let update = thread::spawn(move || {
        let result = second
            .Exec("update t set val=200 where id=1", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        done_tx
            .send((second, result))
            .expect("send optimistic update");
    });
    let (mut second, update_result) = match done_rx.recv_timeout(Duration::from_millis(200)) {
        Ok(result) => result,
        Err(_) => {
            first.MustExec("rollback", Vec::new());
            if done_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
                update.join().expect("optimistic update thread");
            } else {
                drop(update);
            }
            panic!("optimistic update unexpectedly blocked by SELECT FOR UPDATE");
        }
    };
    update.join().expect("optimistic update thread");
    update_result.expect("optimistic update succeeds before either transaction commits");
    first.MustExec("commit", Vec::new());
    let error = second.ExecToErr("commit");
    assert!(
        contains_any(error.message(), &["Write conflict", "write conflict"]),
        "unexpected SELECT FOR UPDATE conflict: {error}"
    );
    first
        .MustQuery("select * from t where id=1", Vec::new())
        .Check(Rows(&["1 100"]));
}

#[test]
fn TestIssue62775() {
    let _serial = serial_guard();
    let _config_restore =
        GlobalConfigRestore(astersql_config::get_global_config().as_ref().clone());
    astersql_config::update_global(|config| {
        config.pessimistic_txn.pessimistic_auto_commit.store(true)
    });
    let (store, mut tk, database) = fixture("issue_62775");
    let cases = [
        (
            "create table t(id int primary key,v int)",
            "insert into t values(1,10)",
            "select v from t where id=1 for update",
            Rows(&["10"]),
        ),
        (
            "create table t(id varchar(64) primary key clustered,v int)",
            "insert into t values('a',10)",
            "select v from t where id='a' for update",
            Rows(&["10"]),
        ),
        (
            "create table t(id varchar(64) primary key nonclustered,v int)",
            "insert into t values('a',10)",
            "select v from t where id='a' for update",
            Rows(&["10"]),
        ),
        (
            "create table t(id int primary key,v int)",
            "insert into t values(1,10),(2,20)",
            "select v from t for update",
            Rows(&["10", "20"]),
        ),
        (
            "create table t(id int primary key,v int)",
            "insert into t values(1,10)",
            "select (v) from t where id=1 for update",
            Rows(&["10"]),
        ),
        (
            "create table t(id varchar(64) primary key clustered,v int)",
            "insert into t values('a',10)",
            "select (v) from t where id='a' for update",
            Rows(&["10"]),
        ),
        (
            "create table t(id varchar(64) primary key nonclustered,v int)",
            "insert into t values('a',10)",
            "select (v) from t where id='a' for update",
            Rows(&["10"]),
        ),
        (
            "create table t(id int primary key,v int)",
            "insert into t values(1,10),(2,20)",
            "select (v) from t for update",
            Rows(&["10", "20"]),
        ),
    ];
    let internal_context: Context = WithInternalSourceType(Context::default(), InternalTxnGC);
    for (create, prepare, query, expected) in cases {
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec(create, Vec::new());
        tk.MustExec(prepare, Vec::new());
        let internal_tk = peer(store.clone(), &database);
        let session = internal_tk.Session();
        assert_eq!(
            internal_last_commit_ts(&session, &internal_context),
            0,
            "fresh internal session must start with LastCommitTS=0"
        );
        let rows = session
            .QueryInternal(&internal_context, query, &[])
            .expect("internal SELECT FOR UPDATE");
        assert_eq!(rows.string_rows(), expected);
        assert_eq!(
            internal_last_commit_ts(&session, &internal_context),
            0,
            "read-only internal SELECT FOR UPDATE must not allocate commit TS"
        );
        session
            .ExecuteInternal(&internal_context, "update t set v=v+1", &[])
            .expect("internal update");
        assert!(
            internal_last_commit_ts(&session, &internal_context) > 0,
            "internal write must publish LastCommitTS"
        );
    }
}

// A timing failpoint must not manufacture a foreign-key violation for valid data.
#[test]
fn TestAsyncCommitPausePreservesValidForeignKey() {
    let _serial = serial_guard();
    let (store, mut setup, database) = fixture("valid_fk_pause");
    setup.MustExec("create table parent(id int primary key)", Vec::new());
    setup.MustExec(
        "create table child(id int primary key, foreign key fk(id) references parent(id))",
        Vec::new(),
    );
    setup.MustExec("insert into parent values(1)", Vec::new());
    let pause = BoundedPause::new(ASYNC_COMMIT_PAUSE);
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut writer = peer(store, &database);
        writer.MustExec("set tidb_enable_async_commit=1", Vec::new());
        let result = writer
            .Exec("insert into child values(1)", Vec::new())
            .map(|_| ())
            .map_err(|error| error.to_string());
        done_tx.send(result).expect("send valid FK insert result");
    });
    pause.wait_until_reached("valid FK async commit");
    pause.resume();
    let result = done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("valid FK insert resumes");
    worker.join().expect("valid FK insert worker");
    result.expect("timing failpoint must preserve valid FK insert");
    setup
        .MustQuery("select * from child", Vec::new())
        .Check(Rows(&["1"]));
}
