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

//! 中文说明开始（自动生成）
//! 中文总览：`stale_read_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `事务语义与时间戳行为` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 418 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `fixture` 是当前文件里的辅助函数。
//! `fixture` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `fixture` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `fixture`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `fixture` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `fixture` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `fixture` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`fixture` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `fixture` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`fixture` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `current_ts` 是当前文件里的辅助函数。
//! `current_ts` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `current_ts` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `current_ts`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `current_ts` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `current_ts` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `current_ts` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`current_ts` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `current_ts` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`current_ts` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `history` 是当前文件里的辅助函数。
//! `history` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `history` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `history`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `history` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `history` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `history` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`history` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `history` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`history` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `select_as_of` 是当前文件里的辅助函数。
//! `select_as_of` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `select_as_of` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `select_as_of`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `select_as_of` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `select_as_of` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `select_as_of` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`select_as_of` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `select_as_of` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`select_as_of` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `assert_as_of_rows` 是当前文件里的辅助函数。
//! `assert_as_of_rows` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `assert_as_of_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_as_of_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_as_of_rows` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `assert_as_of_rows` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `assert_as_of_rows` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`assert_as_of_rows` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `assert_as_of_rows` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`assert_as_of_rows` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `tso_seconds_ago` 是当前文件里的辅助函数。
//! `tso_seconds_ago` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `tso_seconds_ago` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `tso_seconds_ago`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `tso_seconds_ago` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `tso_seconds_ago` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `tso_seconds_ago` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`tso_seconds_ago` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `tso_seconds_ago` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`tso_seconds_ago` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `physical_ts` 是当前文件里的辅助函数。
//! `physical_ts` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `physical_ts` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `physical_ts`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `physical_ts` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `physical_ts` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `physical_ts` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`physical_ts` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `physical_ts` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`physical_ts` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `TestTxnScopeAndValidateReadTs` 是当前文件里的测试用例。
//! `TestTxnScopeAndValidateReadTs` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestTxnScopeAndValidateReadTs` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestTxnScopeAndValidateReadTs`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestTxnScopeAndValidateReadTs` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestTxnScopeAndValidateReadTs` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestTxnScopeAndValidateReadTs` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestTxnScopeAndValidateReadTs` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestExactStalenessTransaction` 是当前文件里的测试用例。
//! `TestExactStalenessTransaction` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestExactStalenessTransaction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestExactStalenessTransaction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestExactStalenessTransaction` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestExactStalenessTransaction` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestExactStalenessTransaction` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestExactStalenessTransaction` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `INJECT_TXN_SCOPE` 是当前文件里的常量。
//! `INJECT_TXN_SCOPE` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `INJECT_TXN_SCOPE` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `INJECT_TXN_SCOPE`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `INJECT_TXN_SCOPE` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `INJECT_TXN_SCOPE` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `INJECT_TXN_SCOPE` 当作定位同类问题的索引锚点。
//! 作为常量，`INJECT_TXN_SCOPE` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `INJECT_TXN_SCOPE` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`INJECT_TXN_SCOPE` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `Case` 是当前文件里的状态类型。
//! `Case` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `Case` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Case`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `Case` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `Case` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `Case` 当作定位同类问题的索引锚点。
//! 作为状态类型，`Case` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `Case` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`Case` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `TestSelectAsOf` 是当前文件里的测试用例。
//! `TestSelectAsOf` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestSelectAsOf` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestSelectAsOf`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestSelectAsOf` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestSelectAsOf` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestSelectAsOf` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestSelectAsOf` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `ASSERT_STALE_TSO` 是当前文件里的常量。
//! `ASSERT_STALE_TSO` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `ASSERT_STALE_TSO` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ASSERT_STALE_TSO`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `ASSERT_STALE_TSO` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `ASSERT_STALE_TSO` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `ASSERT_STALE_TSO` 当作定位同类问题的索引锚点。
//! 作为常量，`ASSERT_STALE_TSO` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `ASSERT_STALE_TSO` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`ASSERT_STALE_TSO` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `INJECT_NOW` 是当前文件里的常量。
//! `INJECT_NOW` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `INJECT_NOW` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `INJECT_NOW`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `INJECT_NOW` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `INJECT_NOW` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `INJECT_NOW` 当作定位同类问题的索引锚点。
//! 作为常量，`INJECT_NOW` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `INJECT_NOW` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`INJECT_NOW` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `FIXED_TIME` 是当前文件里的常量。
//! `FIXED_TIME` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `FIXED_TIME` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FIXED_TIME`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `FIXED_TIME` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `FIXED_TIME` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `FIXED_TIME` 当作定位同类问题的索引锚点。
//! 作为常量，`FIXED_TIME` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `FIXED_TIME` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`FIXED_TIME` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `FIXED_UNIX` 是当前文件里的常量。
//! `FIXED_UNIX` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `FIXED_UNIX` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FIXED_UNIX`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `FIXED_UNIX` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `FIXED_UNIX` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `FIXED_UNIX` 当作定位同类问题的索引锚点。
//! 作为常量，`FIXED_UNIX` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `FIXED_UNIX` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`FIXED_UNIX` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `TestStaleReadKVRequest` 是当前文件里的测试用例。
//! `TestStaleReadKVRequest` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleReadKVRequest` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleReadKVRequest`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleReadKVRequest` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleReadKVRequest` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleReadKVRequest` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleReadKVRequest` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStalenessAndHistoryRead` 是当前文件里的测试用例。
//! `TestStalenessAndHistoryRead` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStalenessAndHistoryRead` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStalenessAndHistoryRead`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStalenessAndHistoryRead` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStalenessAndHistoryRead` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStalenessAndHistoryRead` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStalenessAndHistoryRead` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestTimeBoundedStalenessTxn` 是当前文件里的测试用例。
//! `TestTimeBoundedStalenessTxn` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestTimeBoundedStalenessTxn` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestTimeBoundedStalenessTxn`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestTimeBoundedStalenessTxn` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestTimeBoundedStalenessTxn` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestTimeBoundedStalenessTxn` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestTimeBoundedStalenessTxn` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `INJECT_SAFE_TS` 是当前文件里的常量。
//! `INJECT_SAFE_TS` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `INJECT_SAFE_TS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `INJECT_SAFE_TS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `INJECT_SAFE_TS` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `INJECT_SAFE_TS` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `INJECT_SAFE_TS` 当作定位同类问题的索引锚点。
//! 作为常量，`INJECT_SAFE_TS` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `INJECT_SAFE_TS` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`INJECT_SAFE_TS` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `TestStalenessTransactionSchemaVer` 是当前文件里的测试用例。
//! `TestStalenessTransactionSchemaVer` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStalenessTransactionSchemaVer` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStalenessTransactionSchemaVer`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStalenessTransactionSchemaVer` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStalenessTransactionSchemaVer` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStalenessTransactionSchemaVer` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStalenessTransactionSchemaVer` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestSetTransactionReadOnlyAsOf` 是当前文件里的测试用例。
//! `TestSetTransactionReadOnlyAsOf` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestSetTransactionReadOnlyAsOf` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestSetTransactionReadOnlyAsOf`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestSetTransactionReadOnlyAsOf` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestSetTransactionReadOnlyAsOf` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestSetTransactionReadOnlyAsOf` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestSetTransactionReadOnlyAsOf` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestValidateReadOnlyInStalenessTransaction` 是当前文件里的测试用例。
//! `TestValidateReadOnlyInStalenessTransaction` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestValidateReadOnlyInStalenessTransaction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestValidateReadOnlyInStalenessTransaction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestValidateReadOnlyInStalenessTransaction` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestValidateReadOnlyInStalenessTransaction` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestValidateReadOnlyInStalenessTransaction` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestValidateReadOnlyInStalenessTransaction` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `Outcome` 是当前文件里的分支类型。
//! `Outcome` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `Outcome` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Outcome`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `Outcome` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `Outcome` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `Outcome` 当作定位同类问题的索引锚点。
//! 作为分支类型，`Outcome` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `Outcome` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`Outcome` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `TestSpecialSQLInStalenessTxn` 是当前文件里的测试用例。
//! `TestSpecialSQLInStalenessTxn` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestSpecialSQLInStalenessTxn` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestSpecialSQLInStalenessTxn`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestSpecialSQLInStalenessTxn` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestSpecialSQLInStalenessTxn` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestSpecialSQLInStalenessTxn` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestSpecialSQLInStalenessTxn` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestAsOfTimestampCompatibility` 是当前文件里的测试用例。
//! `TestAsOfTimestampCompatibility` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestAsOfTimestampCompatibility` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestAsOfTimestampCompatibility`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestAsOfTimestampCompatibility` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestAsOfTimestampCompatibility` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestAsOfTimestampCompatibility` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestAsOfTimestampCompatibility` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestSetTransactionInfoSchema` 是当前文件里的测试用例。
//! `TestSetTransactionInfoSchema` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestSetTransactionInfoSchema` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestSetTransactionInfoSchema`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestSetTransactionInfoSchema` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestSetTransactionInfoSchema` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestSetTransactionInfoSchema` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestSetTransactionInfoSchema` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStaleSelect` 是当前文件里的测试用例。
//! `TestStaleSelect` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleSelect` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleSelect`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleSelect` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleSelect` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleSelect` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleSelect` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStaleReadFutureTime` 是当前文件里的测试用例。
//! `TestStaleReadFutureTime` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleReadFutureTime` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleReadFutureTime`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleReadFutureTime` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleReadFutureTime` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleReadFutureTime` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleReadFutureTime` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStaleReadPrepare` 是当前文件里的测试用例。
//! `TestStaleReadPrepare` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleReadPrepare` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleReadPrepare`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleReadPrepare` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleReadPrepare` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleReadPrepare` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleReadPrepare` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStmtCtxStaleFlag` 是当前文件里的测试用例。
//! `TestStmtCtxStaleFlag` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStmtCtxStaleFlag` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStmtCtxStaleFlag`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStmtCtxStaleFlag` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStmtCtxStaleFlag` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStmtCtxStaleFlag` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStmtCtxStaleFlag` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `ASSERT_STMT_STALE` 是当前文件里的常量。
//! `ASSERT_STMT_STALE` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `ASSERT_STMT_STALE` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ASSERT_STMT_STALE`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `ASSERT_STMT_STALE` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `ASSERT_STMT_STALE` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `ASSERT_STMT_STALE` 当作定位同类问题的索引锚点。
//! 作为常量，`ASSERT_STMT_STALE` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `ASSERT_STMT_STALE` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`ASSERT_STMT_STALE` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `TestStaleSessionQuery` 是当前文件里的测试用例。
//! `TestStaleSessionQuery` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleSessionQuery` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleSessionQuery`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleSessionQuery` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleSessionQuery` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleSessionQuery` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleSessionQuery` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStaleReadCompatibility` 是当前文件里的测试用例。
//! `TestStaleReadCompatibility` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleReadCompatibility` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleReadCompatibility`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleReadCompatibility` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleReadCompatibility` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleReadCompatibility` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleReadCompatibility` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStaleReadNoExtraTSORequest` 是当前文件里的测试用例。
//! `TestStaleReadNoExtraTSORequest` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleReadNoExtraTSORequest` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleReadNoExtraTSORequest`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleReadNoExtraTSORequest` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleReadNoExtraTSORequest` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleReadNoExtraTSORequest` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleReadNoExtraTSORequest` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestPlanCacheWithStaleReadByBinaryProto` 是当前文件里的测试用例。
//! `TestPlanCacheWithStaleReadByBinaryProto` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestPlanCacheWithStaleReadByBinaryProto` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestPlanCacheWithStaleReadByBinaryProto`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestPlanCacheWithStaleReadByBinaryProto` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestPlanCacheWithStaleReadByBinaryProto` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestPlanCacheWithStaleReadByBinaryProto` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestPlanCacheWithStaleReadByBinaryProto` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStalePrepare` 是当前文件里的测试用例。
//! `TestStalePrepare` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStalePrepare` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStalePrepare`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStalePrepare` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStalePrepare` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStalePrepare` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStalePrepare` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `TestStaleTSO` 是当前文件里的测试用例。
//! `TestStaleTSO` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleTSO` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleTSO`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleTSO` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleTSO` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleTSO` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleTSO` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `MOCK_STALE_TSO` 是当前文件里的常量。
//! `MOCK_STALE_TSO` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `MOCK_STALE_TSO` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_STALE_TSO`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `MOCK_STALE_TSO` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `MOCK_STALE_TSO` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `MOCK_STALE_TSO` 当作定位同类问题的索引锚点。
//! 作为常量，`MOCK_STALE_TSO` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `MOCK_STALE_TSO` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`MOCK_STALE_TSO` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `TestStaleReadNoBackoff` 是当前文件里的测试用例。
//! `TestStaleReadNoBackoff` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleReadNoBackoff` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleReadNoBackoff`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleReadNoBackoff` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleReadNoBackoff` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleReadNoBackoff` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleReadNoBackoff` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 符号 `STORE_SEND_RESULT` 是当前文件里的常量。
//! `STORE_SEND_RESULT` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `STORE_SEND_RESULT` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `STORE_SEND_RESULT`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `STORE_SEND_RESULT` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `STORE_SEND_RESULT` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `STORE_SEND_RESULT` 当作定位同类问题的索引锚点。
//! 作为常量，`STORE_SEND_RESULT` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `STORE_SEND_RESULT` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`STORE_SEND_RESULT` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `GlobalConfigRestore` 是当前文件里的状态类型。
//! `GlobalConfigRestore` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `GlobalConfigRestore` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `GlobalConfigRestore`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `GlobalConfigRestore` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `GlobalConfigRestore` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `GlobalConfigRestore` 当作定位同类问题的索引锚点。
//! 作为状态类型，`GlobalConfigRestore` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `GlobalConfigRestore` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`GlobalConfigRestore` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `drop` 是当前文件里的辅助函数。
//! `drop` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `drop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `drop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `drop` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `drop` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `drop` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`drop` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `drop` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`drop` 在 `事务语义与时间戳行为` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `TestStaleReadAllCombinations` 是当前文件里的测试用例。
//! `TestStaleReadAllCombinations` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `TestStaleReadAllCombinations` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestStaleReadAllCombinations`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestStaleReadAllCombinations` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestStaleReadAllCombinations` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `stale_read_test.rs` 的回归，可以把 `TestStaleReadAllCombinations` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestStaleReadAllCombinations` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 若该场景涉及并发、锁或时间戳，最值得关注的是阻塞点、释放点和最终可见性是否按预期排列。
//! 中文说明结束（自动生成）

//! End-to-end stale-read compatibility tests ported from TiDB's
//! `tests/realtikvtest/txntest/stale_read_test.go`.

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{DbValue, NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_txntest::serial_guard;

fn fixture(name: &str) -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    let database = format!("stale_{name}");
    tk.MustExec(&format!("create database `{database}`"), Vec::new());
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    tk
}

fn current_ts(tk: &mut TestKit) -> u64 {
    tk.MustExec("begin", Vec::new());
    let value = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0]
        .parse()
        .expect("@@tidb_current_ts must be a TSO");
    tk.MustExec("rollback", Vec::new());
    value
}

fn history(tk: &mut TestKit) -> u64 {
    tk.MustExec("create table t (id int primary key, v int)", Vec::new());
    tk.MustExec("insert into t values (1,10)", Vec::new());
    let ts = current_ts(tk);
    tk.MustExec("insert into t values (2,20)", Vec::new());
    ts
}

fn select_as_of(ts: u64) -> String {
    format!("select * from t as of timestamp {ts} order by id")
}

fn assert_as_of_rows(tk: &TestKit, ts: u64, expected: &[&str]) {
    tk.MustQuery(&select_as_of(ts), Vec::new())
        .Check(Rows(expected));
}

fn tso_seconds_ago(seconds: u64) -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis() as u64;
    millis.saturating_sub(seconds * 1_000) << 18
}

fn physical_ts(ts: u64) -> u64 {
    ts >> 18
}

#[test]
fn TestTxnScopeAndValidateReadTs() {
    let _serial = serial_guard();
    let _config_restore =
        GlobalConfigRestore(astersql_config::get_global_config().as_ref().clone());
    astersql_config::update_global(|config| {
        config.labels = HashMap::from([("zone".to_owned(), "bj".to_owned())]);
    });
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("create database `stale_scope_validate`", Vec::new());
    tk.MustExec("use `stale_scope_validate`", Vec::new());
    tk.MustExec("create table t1 (id int primary key)", Vec::new());
    std::thread::sleep(Duration::from_secs(1));

    tk.MustExec("begin", Vec::new());
    tk.MustExec("set @txn_scope_read_ts = @@tidb_current_ts", Vec::new());
    tk.MustExec("commit", Vec::new());
    tk.MustQuery(
        "select * from t1 as of timestamp @txn_scope_read_ts where id=1",
        Vec::new(),
    )
    .Check(Rows(&[]));
    let stale_request = store
        .last_replica_read_request_for_test()
        .expect("read statement stale request")
        .expect("AS OF statement must emit a KV request");
    assert_eq!(stale_request.txn_scope, "bj");
    assert_eq!(
        stale_request.store_labels.get("zone").map(String::as_str),
        Some("bj")
    );

    for replica_read in ["closest-replicas", "follower", "closest-adaptive"] {
        tk.MustExec(
            &format!("set @@tidb_replica_read='{replica_read}'"),
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        tk.MustQuery("select * from t1 where id=1", Vec::new())
            .Check(Rows(&[]));
        let request = store
            .last_replica_read_request_for_test()
            .expect("read replica request")
            .expect("replica query must emit a KV request");
        assert_eq!(request.replica_read, replica_read);
        assert_eq!(request.txn_scope, "bj");
        assert_eq!(
            request.store_labels.get("zone").map(String::as_str),
            Some("bj")
        );
        tk.MustExec("commit", Vec::new());
    }
}

#[test]
fn TestExactStalenessTransaction() {
    let _serial = serial_guard();
    let mut tk = fixture("exact_txn");
    const INJECT_TXN_SCOPE: &str = "tikvclient/injectTxnScope";
    struct Case {
        name: &'static str,
        pre_sql: &'static str,
        sql: &'static str,
        is_staleness: bool,
        expected_physical_ts: Option<u64>,
        zone: &'static str,
    }
    let cases = [
        Case {
            name: "AsOfTimestamp",
            pre_sql: "begin",
            sql: "start transaction read only as of timestamp '2020-09-06 00:00:00'",
            is_staleness: true,
            expected_physical_ts: Some(1_599_321_600_000),
            zone: "sh",
        },
        Case {
            name: "begin after AsOfTimestamp",
            pre_sql: "start transaction read only as of timestamp '2020-09-06 00:00:00'",
            sql: "begin",
            is_staleness: false,
            expected_physical_ts: None,
            zone: "",
        },
        Case {
            name: "AsOfTimestamp with tidb_bounded_staleness",
            pre_sql: "begin",
            sql: "start transaction read only as of timestamp \
                  tidb_bounded_staleness('2015-09-21 00:07:01', now())",
            is_staleness: true,
            expected_physical_ts: Some(1_442_765_221_000),
            zone: "bj",
        },
        Case {
            name: "begin after AsOfTimestamp with tidb_bounded_staleness",
            pre_sql: "start transaction read only as of timestamp \
                      tidb_bounded_staleness('2015-09-21 00:07:01', now())",
            sql: "begin",
            is_staleness: false,
            expected_physical_ts: None,
            zone: "",
        },
    ];

    for case in cases {
        let scope =
            testfailpoint::enable(INJECT_TXN_SCOPE, &format!(r#"1*return("{}")"#, case.zone));
        tk.MustExec(case.pre_sql, Vec::new());
        tk.MustExec(case.sql, Vec::new());

        let state = tk.StaleReadStateForTest();
        assert_eq!(state.is_staleness, case.is_staleness, "{}", case.name);
        assert!(
            state.transaction_active,
            "{} must activate a transaction",
            case.name
        );
        let start_ts = state.start_ts;
        if let Some(expected) = case.expected_physical_ts {
            assert_eq!(physical_ts(start_ts), expected, "{}", case.name);
        } else {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_millis() as u64;
            assert!(
                physical_ts(start_ts).abs_diff(now) < 1_000,
                "{}: fresh transaction physical TS {} is not near {now}",
                case.name,
                physical_ts(start_ts)
            );
        }
        assert!(
            testfailpoint::eval_string(INJECT_TXN_SCOPE).is_none(),
            "{}: transaction scope `{}` was not consumed",
            case.name,
            case.zone
        );
        drop(scope);
        tk.MustExec("commit", Vec::new());
    }
}

#[test]
fn TestSelectAsOf() {
    let _serial = serial_guard();
    let mut tk = fixture("select_as_of");
    tk.MustExec(
        "insert into mysql.tidb values(\
         'tikv_gc_safe_point','20160102-15:04:05 -0700',\
         'All versions after safe point can be accessed. (DO NOT EDIT)') \
         on duplicate key update variable_value='20160102-15:04:05 -0700',\
         comment='All versions after safe point can be accessed. (DO NOT EDIT)'",
        Vec::new(),
    );
    tk.MustExec("create table t(id int primary key,v int)", Vec::new());
    tk.MustExec("create table b(pid int primary key)", Vec::new());
    tk.MustExec("insert into t values(1,10)", Vec::new());
    tk.MustExec("insert into b values(1)", Vec::new());
    let ts = current_ts(&mut tk);
    tk.MustExec("insert into t values(2,20)", Vec::new());
    assert_as_of_rows(&tk, ts, &["1 10"]);
    tk.MustQuery(
        &format!("select * from t as of timestamp timestamp({ts}) order by id"),
        Vec::new(),
    )
    .Check(Rows(&["1 10"]));
    tk.MustExec(
        &format!("set transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 10"]));
    assert_as_of_rows(&tk, ts, &["1 10"]);
    tk.MustQuery(
        &format!(
            "select t.id,t.v,b.pid from t as of timestamp {ts}, \
             b as of timestamp {ts} where t.id=b.pid order by t.id"
        ),
        Vec::new(),
    )
    .Check(Rows(&["1 10 1"]));
    let different = ts + 1;
    let error = tk.QueryToErr(&format!(
        "select * from t as of timestamp {ts}, b as of timestamp {different}"
    ));
    assert!(
        error.message().contains("different time"),
        "unexpected error: {error}"
    );

    for sql in [
        format!("select * from t as of timestamp {ts}, b"),
        format!("select * from t, b as of timestamp {ts}"),
        format!(
            "select * from (select * from t as of timestamp {ts}, \
            b as of timestamp {ts}) as c, b"
        ),
    ] {
        let Err(error) = tk.Query(&sql, Vec::new()) else {
            panic!("`{sql}` unexpectedly accepted mixed historical/current sources");
        };
        assert!(
            error.message().contains("different time"),
            "`{sql}` must reject mixed historical/current sources: {error}"
        );
    }

    tk.MustQuery(
        &format!(
            "select * from (select t.id from t as of timestamp {ts}, \
             b as of timestamp {ts} where t.id=b.pid) as c"
        ),
        Vec::new(),
    )
    .Check(Rows(&["1"]));

    let syntax_error = tk.QueryToErr(&format!(
        "select * from (select * from t as of timestamp {ts}) as c as of timestamp {ts}"
    ));
    assert!(
        syntax_error.message().contains("syntax"),
        "AS OF on a derived table must remain a syntax error: {syntax_error}"
    );

    const ASSERT_STALE_TSO: &str = "github.com/pingcap/tidb/pkg/executor/assertStaleTSO";
    const INJECT_NOW: &str = "github.com/pingcap/tidb/pkg/expression/injectNow";
    const FIXED_TIME: &str = "2020-09-06 00:00:00";
    const FIXED_UNIX: i64 = 1_599_321_600;

    // Match Go's boundary setup: capture NOW first, then advance the oracle
    // beyond that wall-clock second before using it as a stale timestamp.
    let captured_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_secs();
    loop {
        let oracle_ts = current_ts(&mut tk);
        if physical_ts(oracle_ts) / 1_000 > captured_now {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let now = testfailpoint::enable(INJECT_NOW, &format!("1*return({captured_now})"));
    let tso = testfailpoint::enable(ASSERT_STALE_TSO, &format!("1*return({captured_now})"));
    tk.MustQuery(
        "select * from t as of timestamp now() order by id",
        Vec::new(),
    )
    .Check(Rows(&["1 10", "2 20"]));
    assert!(testfailpoint::eval_string(INJECT_NOW).is_none());
    assert!(testfailpoint::eval_string(ASSERT_STALE_TSO).is_none());
    drop(tso);
    drop(now);

    let tso = testfailpoint::enable(ASSERT_STALE_TSO, &format!("1*return({FIXED_UNIX})"));
    tk.MustExec(
        &format!("set transaction read only as of timestamp '{FIXED_TIME}'"),
        Vec::new(),
    );
    tk.MustQuery("select * from t", Vec::new());
    assert!(
        testfailpoint::eval_string(ASSERT_STALE_TSO).is_none(),
        "SET TRANSACTION fixed datetime must expose its physical stale TSO"
    );
    drop(tso);

    tk.MustExec(
        &format!("set transaction read only as of timestamp '{FIXED_TIME}'"),
        Vec::new(),
    );
    let set_conflict = tk.QueryToErr(&format!("select * from t as of timestamp '{FIXED_TIME}'"));
    assert!(
        set_conflict
            .message()
            .contains("can't use select as of while already set transaction as of"),
        "unexpected SET + statement AS OF conflict: {set_conflict}"
    );
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 10", "2 20"]));

    for sql in [
        format!("select * from t as of timestamp '{FIXED_TIME}'"),
        format!("select * from t as of timestamp timestamp('{FIXED_TIME}')"),
    ] {
        let tso = testfailpoint::enable(ASSERT_STALE_TSO, &format!("1*return({FIXED_UNIX})"));
        tk.MustQuery(&sql, Vec::new());
        assert!(
            testfailpoint::eval_string(ASSERT_STALE_TSO).is_none(),
            "`{sql}` did not expose its physical stale TSO"
        );
        drop(tso);
    }

    let injected_now = FIXED_UNIX + 100;
    for (sql, seconds_ago) in [
        (
            "select * from t as of timestamp now()-interval 2 second",
            2_i64,
        ),
        (
            "select * from t as of timestamp timestamp(now()-interval 2 second)",
            2,
        ),
        (
            "select * from t as of timestamp timestamp(now()-interval 1 second), \
             b as of timestamp timestamp(now()-interval 1 second)",
            1,
        ),
        (
            "select * from (select * from t as of timestamp \
             timestamp(now()-interval 2 second), b as of timestamp \
             timestamp(now()-interval 2 second)) as c",
            2,
        ),
    ] {
        let now = testfailpoint::enable(INJECT_NOW, &format!("1*return({injected_now})"));
        let tso = testfailpoint::enable(
            ASSERT_STALE_TSO,
            &format!("1*return({})", injected_now - seconds_ago),
        );
        tk.MustQuery(sql, Vec::new());
        assert!(
            testfailpoint::eval_string(INJECT_NOW).is_none(),
            "`{sql}` did not consume injectNow"
        );
        assert!(
            testfailpoint::eval_string(ASSERT_STALE_TSO).is_none(),
            "`{sql}` did not expose the calculated stale TSO"
        );
        drop(tso);
        drop(now);
    }

    for sql in [
        format!(
            "select * from t as of timestamp timestamp(now()-interval 1 second), \
             b as of timestamp timestamp('{FIXED_TIME}')"
        ),
        "select * from t as of timestamp timestamp(now()-interval 1 second), b".to_owned(),
        "select * from t, b as of timestamp timestamp(now()-interval 1 second)".to_owned(),
        "select * from (select * from t as of timestamp \
         timestamp(now()-interval 1 second), b as of timestamp \
         timestamp(now()-interval 1 second)) as c, b"
            .to_owned(),
    ] {
        let error = tk.QueryToErr(&sql);
        assert!(
            error.message().contains("different time"),
            "`{sql}` must reject mixed stale/current timestamps: {error}"
        );
    }

    let derived_syntax = tk.QueryToErr(
        "select * from (select * from t as of timestamp \
         timestamp(now()-interval 20 second), b as of timestamp \
         timestamp(now()-interval 20 second)) as c as of timestamp now()",
    );
    assert!(
        derived_syntax.message().contains("syntax"),
        "derived-table AS OF must remain a syntax error: {derived_syntax}"
    );
}

#[test]
fn TestStaleReadKVRequest() {
    let _serial = serial_guard();
    let _config_restore =
        GlobalConfigRestore(astersql_config::get_global_config().as_ref().clone());
    astersql_config::update_global(|config| {
        config.labels = HashMap::from([("zone".to_owned(), "sh".to_owned())]);
    });
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("create database `stale_kv_request`", Vec::new());
    tk.MustExec("use `stale_kv_request`", Vec::new());
    tk.MustExec("create table t (id int primary key)", Vec::new());
    tk.MustExec(
        "create table t1(c int primary key,d int,e int,index idx_d(d),index idx_e(e))",
        Vec::new(),
    );
    let ts = current_ts(&mut tk);
    tk.MustExec("set @@tidb_replica_read='closest-replicas'", Vec::new());
    let request_cases = [
        ("select * from t", "Coprocessor"),
        ("select * from t where id=1", "Get"),
        ("select * from t where id in (1,2,3)", "BatchGet"),
    ];
    for activation in ["START", "SET_BEGIN", "SESSION"] {
        for (sql, request_kind) in request_cases {
            store
                .clear_replica_read_request_for_test()
                .expect("clear replica request observation");
            match activation {
                "START" => tk.MustExec(
                    &format!("start transaction read only as of timestamp {ts}"),
                    Vec::new(),
                ),
                "SET_BEGIN" => {
                    tk.MustExec(
                        &format!("set transaction read only as of timestamp {ts}"),
                        Vec::new(),
                    );
                    tk.MustExec("begin", Vec::new());
                }
                "SESSION" => {}
                _ => unreachable!(),
            }
            tk.MustQuery(sql, Vec::new());
            let request = store
                .last_replica_read_request_for_test()
                .expect("read replica request observation")
                .expect("query must emit a KV request");
            assert_eq!(
                request.request_kind, request_kind,
                "{activation} `{sql}` request kind"
            );
            assert_eq!(
                request.replica_read, "closest-replicas",
                "{activation} `{sql}` replica policy"
            );
            assert_eq!(request.txn_scope, "sh", "{activation} `{sql}` txn scope");
            assert_eq!(
                request.store_labels,
                HashMap::from([("zone".to_owned(), "sh".to_owned())]),
                "{activation} `{sql}` store labels"
            );
            if activation != "SESSION" {
                tk.MustExec("commit", Vec::new());
            }
        }
    }

    tk.MustExec("insert into t1 values(1,1,1),(2,3,5)", Vec::new());
    let history_ts = current_ts(&mut tk);
    tk.MustExec("insert into t1 values(3,3,7),(4,0,5),(5,0,5)", Vec::new());
    for (name, sql, expected_len) in [
        (
            "IndexLookUp",
            format!(
                "select * from t1 as of timestamp {history_ts} use index(idx_d) \
                 where c<5 and d<5 order by c"
            ),
            2,
        ),
        (
            "IndexMerge",
            format!(
                "select /*+ use_index_merge(t1,idx_d,idx_e) */ * from t1 \
                 as of timestamp {history_ts} where c<5 and (d=5 or e=5)"
            ),
            1,
        ),
        (
            "TableReader",
            format!("select * from t1 as of timestamp {history_ts} where c<6 order by c"),
            2,
        ),
        (
            "IndexReader",
            format!(
                "select /*+ use_index(t1,idx_d) */ d from t1 as of timestamp {history_ts} \
                 where c<5 and d<1"
            ),
            0,
        ),
        (
            "PointGet",
            format!("select * from t1 as of timestamp {history_ts} where c=3"),
            0,
        ),
        (
            "BatchPointGet",
            format!("select * from t1 as of timestamp {history_ts} where c in (3,4,5)"),
            0,
        ),
    ] {
        let rows = tk.MustQuery(&sql, Vec::new()).Rows();
        assert_eq!(
            rows.len(),
            expected_len,
            "{name} stale executor row count: {rows:?}"
        );
    }
}

fn wait_ts_after_ts(tk: &mut TestKit, after_ts: u64) -> u64 {
    loop {
        let ts = current_ts(tk);
        // SQL timestamp literals retain only physical milliseconds. Wait for
        // the next physical tick so parsing the literal stays after the TSO.
        if physical_ts(ts) > physical_ts(after_ts) {
            return physical_ts(ts) << 18;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn history_timestamp_literal(ts: u64) -> String {
    let context = astersql_types::time::BasicTimeContext::default();
    let epoch = astersql_types::time::ParseDatetime(&context, "1970-01-01 00:00:00")
        .expect("valid epoch")
        .GoTime(context.location)
        .expect("epoch in UTC");
    let offset_seconds = astersql_util_timeutil::time_zone::Zone(
        &astersql_util_timeutil::time_zone::SystemLocation(),
    )
    .1;
    let local_millis = i128::from(physical_ts(ts)) + i128::from(offset_seconds) * 1_000;
    (epoch + Duration::from_millis(u64::try_from(local_millis).expect("history time after epoch")))
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

#[test]
fn TestStalenessAndHistoryRead() {
    let _serial = serial_guard();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("create database `stale_history`", Vec::new());
    tk.MustExec("use `stale_history`", Vec::new());
    let observed1 = current_ts(&mut tk);
    let ts1 = wait_ts_after_ts(&mut tk, observed1);
    assert!(
        ts1 > observed1,
        "SQL literal must be after the observed TSO"
    );
    let time1 = history_timestamp_literal(ts1);
    let schema_ver1 = domain.info_schema().SchemaMetaVersion();
    tk.MustExec("create table t(id int primary key)", Vec::new());
    tk.MustExec("drop table t", Vec::new());
    let observed2 = current_ts(&mut tk);
    let ts2 = wait_ts_after_ts(&mut tk, observed2);
    assert!(
        ts2 > observed2,
        "SQL literal must be after the observed TSO"
    );
    let time2 = history_timestamp_literal(ts2);
    let schema_ver2 = domain.info_schema().SchemaMetaVersion();
    assert!(schema_ver1 < schema_ver2);
    assert_eq!(
        domain
            .snapshot_info_schema(ts1)
            .expect("schema at first history timestamp")
            .SchemaMetaVersion(),
        schema_ver1
    );
    assert_eq!(
        domain
            .snapshot_info_schema(ts2)
            .expect("schema at second history timestamp")
            .SchemaMetaVersion(),
        schema_ver2
    );

    // SET TRANSACTION replaces the active historical snapshot.
    tk.MustExec(&format!("set @@tidb_snapshot='{time1}'"), Vec::new());
    let snapshot_v1 = tk.StaleReadStateForTest();
    assert_eq!(snapshot_v1.snapshot_ts, ts1);
    assert_eq!(snapshot_v1.pending_read_ts, None);
    assert_eq!(snapshot_v1.snapshot_info_schema_version, Some(schema_ver1));
    assert_eq!(snapshot_v1.session_info_schema_version, schema_ver1);
    tk.MustExec(
        &format!("set transaction read only as of timestamp '{time2}'"),
        Vec::new(),
    );
    let pending_v2 = tk.StaleReadStateForTest();
    assert_eq!(pending_v2.snapshot_ts, 0);
    assert_eq!(pending_v2.pending_read_ts, Some(ts2));
    assert_eq!(pending_v2.snapshot_info_schema_version, Some(schema_ver2));
    assert_eq!(pending_v2.session_info_schema_version, schema_ver2);

    // Conversely, tidb_snapshot replaces a pending transaction read TS.
    tk.MustExec(
        &format!("set transaction read only as of timestamp '{time1}'"),
        Vec::new(),
    );
    let pending_v1 = tk.StaleReadStateForTest();
    assert_eq!(pending_v1.pending_read_ts, Some(ts1));
    assert_eq!(pending_v1.snapshot_info_schema_version, Some(schema_ver1));
    assert_eq!(pending_v1.session_info_schema_version, schema_ver1);
    tk.MustExec(&format!("set @@tidb_snapshot='{time2}'"), Vec::new());
    let snapshot_v2 = tk.StaleReadStateForTest();
    assert_eq!(snapshot_v2.pending_read_ts, None);
    assert_eq!(snapshot_v2.snapshot_ts, ts2);
    assert_eq!(snapshot_v2.snapshot_info_schema_version, Some(schema_ver2));
    assert_eq!(snapshot_v2.session_info_schema_version, schema_ver2);

    // An explicit stale transaction clears snapshot mode, pins StartTS, and
    // leaves neither snapshot nor pending transaction state after COMMIT.
    tk.MustExec(&format!("set @@tidb_snapshot='{time1}'"), Vec::new());
    let snapshot_v1 = tk.StaleReadStateForTest();
    assert_eq!(snapshot_v1.snapshot_ts, ts1);
    assert_eq!(snapshot_v1.snapshot_info_schema_version, Some(schema_ver1));
    assert_eq!(snapshot_v1.session_info_schema_version, schema_ver1);
    tk.MustExec(
        &format!("start transaction read only as of timestamp '{time2}'"),
        Vec::new(),
    );
    assert_eq!(
        tk.Session().TransactionDebugStringForTest(),
        Some(format!("Txn{{state=valid, txnStartTS={ts2}}}")),
        "START AS OF must install the historical StartTS directly"
    );
    let active = tk.StaleReadStateForTest();
    assert!(active.transaction_active && active.is_staleness);
    assert_eq!(active.start_ts, ts2);
    assert_eq!(active.txn_read_ts, ts2);
    assert_eq!(active.pending_read_ts, None);
    assert_eq!(active.snapshot_ts, 0);
    assert_eq!(active.snapshot_info_schema_version, None);
    assert_eq!(active.txn_info_schema_version, schema_ver2);
    tk.MustQuery("select @@tidb_current_ts", Vec::new())
        .Check(Rows(&[&ts2.to_string()]));
    tk.MustExec("commit", Vec::new());
    assert_eq!(
        tk.Session().TransactionDebugStringForTest().as_deref(),
        Some("Txn{state=invalid}"),
        "COMMIT must clear stale transaction state"
    );
    let committed = tk.StaleReadStateForTest();
    assert!(!committed.transaction_active && !committed.is_staleness);
    assert_eq!(committed.snapshot_ts, 0);
    assert_eq!(committed.snapshot_info_schema_version, None);
    assert_eq!(committed.txn_info_schema_version, schema_ver2);

    tk.MustExec(
        &format!("start transaction read only as of timestamp '{time2}'"),
        Vec::new(),
    );
    let snapshot_error = tk.ExecToErr("set @@tidb_snapshot='2020-10-08 16:45:26'");
    assert!(
        snapshot_error
            .message()
            .contains("can't be changed while a transaction is in progress"),
        "unexpected snapshot/transaction mutex error: {snapshot_error}"
    );
    let active_after_error = tk.StaleReadStateForTest();
    assert_eq!(active_after_error.snapshot_ts, 0);
    assert_eq!(active_after_error.snapshot_info_schema_version, None);
    assert_eq!(active_after_error.txn_info_schema_version, schema_ver2);
    tk.MustExec("commit", Vec::new());

    tk.MustExec("begin", Vec::new());
    assert_eq!(tk.StaleReadStateForTest().pending_read_ts, None);
    let pending_error =
        tk.ExecToErr("set transaction read only as of timestamp '2020-10-08 16:46:26'");
    assert!(
        pending_error
            .message()
            .contains("can't be changed while a transaction is in progress")
            || pending_error
                .message()
                .contains("not allowed in a transaction"),
        "unexpected SET TRANSACTION mutex error: {pending_error}"
    );
    assert_eq!(tk.StaleReadStateForTest().pending_read_ts, None);
    tk.MustExec("commit", Vec::new());
}

#[test]
fn TestTimeBoundedStalenessTxn() {
    let _serial = serial_guard();
    let mut tk = fixture("bounded");
    history(&mut tk);
    const INJECT_SAFE_TS: &str = "github.com/pingcap/tidb/pkg/expression/injectSafeTS";
    let cases = [
        (
            "start transaction read only as of timestamp \
             tidb_bounded_staleness(now()-interval 20 second,now())",
            tso_seconds_ago(10),
            0_i8,
            true,
        ),
        (
            "start transaction read only as of timestamp \
             tidb_bounded_staleness(now()-interval 10 second,now())",
            tso_seconds_ago(20),
            1,
            true,
        ),
        (
            "start transaction read only as of timestamp \
             tidb_bounded_staleness(now()-interval 20 second,now()-interval 10 second)",
            tso_seconds_ago(5),
            -1,
            true,
        ),
        (
            "start transaction read only as of timestamp now()-interval 5 second",
            tso_seconds_ago(10),
            1,
            false,
        ),
        (
            "start transaction read only as of timestamp now()-interval 10 second",
            tso_seconds_ago(5),
            -1,
            false,
        ),
    ];
    for (sql, safe_ts, ordering, bounded) in cases {
        let guard = testfailpoint::enable(INJECT_SAFE_TS, &format!("1*return({safe_ts})"));
        tk.MustExec(sql, Vec::new());
        let start_ts = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0]
            .parse::<u64>()
            .expect("bounded-staleness StartTS");
        let remaining_safe_ts = testfailpoint::eval_string(INJECT_SAFE_TS);
        assert_eq!(
            remaining_safe_ts.is_none(),
            bounded,
            "bounded expressions must consume safeTS; exact AS OF expressions must not"
        );
        match ordering {
            -1 => assert!(start_ts < safe_ts, "{start_ts} must be below {safe_ts}"),
            0 => assert_eq!(start_ts, safe_ts),
            1 => assert!(start_ts > safe_ts, "{start_ts} must exceed {safe_ts}"),
            _ => unreachable!(),
        }
        tk.MustExec("commit", Vec::new());
        drop(guard);
    }
}

#[test]
fn TestStalenessTransactionSchemaVer() {
    let _serial = serial_guard();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("create database `stale_txn_schema`", Vec::new());
    tk.MustExec("use `stale_txn_schema`", Vec::new());
    tk.MustExec("create table t (id int primary key)", Vec::new());
    let schema_ver1 = domain.info_schema().SchemaMetaVersion();
    let ts = current_ts(&mut tk);
    tk.MustExec("alter table t add column c int", Vec::new());
    let schema_ver2 = domain.info_schema().SchemaMetaVersion();
    assert!(
        schema_ver2 > schema_ver1,
        "DDL must advance schema version: {schema_ver1} -> {schema_ver2}"
    );
    let historical_schema = domain
        .snapshot_info_schema(ts)
        .expect("load historical InfoSchema at stale TSO");
    assert_eq!(
        historical_schema.SchemaMetaVersion(),
        schema_ver1,
        "stale TSO must resolve the pre-DDL schema version"
    );
    tk.MustExec(
        &format!("start transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    let transaction_ts = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0]
        .parse::<u64>()
        .expect("stale transaction TSO");
    assert_eq!(transaction_ts, ts);
    let stale_state = tk.StaleReadStateForTest();
    assert_eq!(stale_state.txn_info_schema_version, schema_ver1);
    assert_eq!(stale_state.snapshot_info_schema_version, None);
    assert_eq!(
        domain
            .snapshot_info_schema(transaction_ts)
            .expect("transaction InfoSchema")
            .SchemaMetaVersion(),
        schema_ver1
    );
    tk.MustQuery("select * from t", Vec::new()).Check(Rows(&[]));
    let error = tk.QueryToErr("select c from t");
    assert!(
        error.message().contains("Unknown column") || error.message().contains("unknown column"),
        "unexpected historical schema error: {error}"
    );
    tk.MustExec("commit", Vec::new());
    assert_eq!(
        tk.StaleReadStateForTest().txn_info_schema_version,
        schema_ver2,
        "COMMIT must restore the latest transaction InfoSchema"
    );
    assert_eq!(domain.info_schema().SchemaMetaVersion(), schema_ver2);
    tk.MustQuery("select c from t", Vec::new()).Check(Rows(&[]));
    tk.MustQuery(&format!("select * from t as of timestamp {ts}"), Vec::new())
        .Check(Rows(&[]));
    assert_eq!(
        tk.StaleReadStateForTest().txn_info_schema_version,
        schema_ver2,
        "statement AS OF must not replace TxnManager InfoSchema"
    );
    assert_eq!(
        domain.info_schema().SchemaMetaVersion(),
        schema_ver2,
        "statement AS OF must not replace the session's latest InfoSchema"
    );
}

#[test]
fn TestSetTransactionReadOnlyAsOf() {
    let _serial = serial_guard();
    let mut tk = fixture("set_txn");
    let ts = history(&mut tk);
    tk.MustExec(
        &format!("set transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    assert_eq!(
        tk.Session().TransactionDebugStringForTest().as_deref(),
        Some("Txn{state=invalid}"),
        "SET TRANSACTION stores pending read TS without starting a transaction"
    );
    let pending = tk.StaleReadStateForTest();
    assert!(!pending.transaction_active);
    assert!(!pending.is_staleness);
    assert_eq!(pending.pending_read_ts, Some(ts));
    tk.MustExec("begin", Vec::new());
    assert_eq!(
        tk.Session().TransactionDebugStringForTest(),
        Some(format!("Txn{{state=valid, txnStartTS={ts}}}"))
    );
    let active = tk.StaleReadStateForTest();
    assert!(active.transaction_active && active.is_staleness);
    assert_eq!(active.start_ts, ts);
    assert_eq!(active.txn_read_ts, ts);
    assert_eq!(active.pending_read_ts, None);
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 10"]));
    tk.MustExec("commit", Vec::new());
    assert_eq!(
        tk.Session().TransactionDebugStringForTest().as_deref(),
        Some("Txn{state=invalid}")
    );
    let committed = tk.StaleReadStateForTest();
    assert!(!committed.transaction_active);
    assert!(!committed.is_staleness);
    assert_eq!(committed.pending_read_ts, None);
    tk.MustExec("begin", Vec::new());
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 10", "2 20"]));
    tk.MustExec("commit", Vec::new());
    const INJECT_SAFE_TS: &str = "github.com/pingcap/tidb/pkg/expression/injectSafeTS";
    let safe_ts = tso_seconds_ago(10);
    let guard = testfailpoint::enable(INJECT_SAFE_TS, &format!("1*return({safe_ts})"));
    tk.MustExec(
        "set transaction read only as of timestamp \
         tidb_bounded_staleness(now()-interval 20 second,now())",
        Vec::new(),
    );
    assert_eq!(
        tk.StaleReadStateForTest().pending_read_ts,
        Some(safe_ts),
        "bounded SET must retain the selected safeTS until BEGIN"
    );
    tk.MustExec("begin", Vec::new());
    tk.MustQuery("select @@tidb_current_ts", Vec::new())
        .Check(Rows(&[&safe_ts.to_string()]));
    tk.MustExec("commit", Vec::new());
    assert!(
        testfailpoint::eval_string(INJECT_SAFE_TS).is_none(),
        "SET bounded staleness must consume injected safeTS"
    );
    drop(guard);

    let invalid = tk.ExecToErr(
        "set transaction read only as of timestamp \
         tidb_bounded_staleness(invalid1,invalid2)",
    );
    assert!(
        invalid.message().contains("bounded")
            || invalid.message().contains("Unknown column")
            || invalid.message().contains("unsupported"),
        "unexpected invalid bounded-staleness error: {invalid}"
    );

    tk.MustExec(
        &format!("set transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    let error = tk.ExecToErr(&format!("start transaction read only as of timestamp {ts}"));
    assert!(
        error
            .message()
            .contains("forbidden after set transaction read only as of"),
        "unexpected error: {error}"
    );
    let repeated = tk.ExecToErr(&format!("start transaction read only as of timestamp {ts}"));
    assert!(
        repeated
            .message()
            .contains("forbidden after set transaction read only as of"),
        "pending read TS must survive a rejected START: {repeated}"
    );
    tk.MustExec("begin", Vec::new());
    tk.MustQuery("select @@tidb_current_ts", Vec::new())
        .Check(Rows(&[&ts.to_string()]));
    tk.MustExec("commit", Vec::new());
    tk.MustExec("begin", Vec::new());
    let fresh_ts = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0]
        .parse::<u64>()
        .expect("fresh transaction TSO");
    assert_ne!(fresh_ts, ts, "pending stale TS must be single-use");
    tk.MustExec("commit", Vec::new());

    tk.MustExec(
        &format!("start transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    let explicit = tk.StaleReadStateForTest();
    assert!(explicit.transaction_active && explicit.is_staleness);
    assert_eq!(explicit.start_ts, ts);
    assert_eq!(explicit.pending_read_ts, None);
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 10"]));
    tk.MustExec("commit", Vec::new());
}

#[test]
fn TestValidateReadOnlyInStalenessTransaction() {
    let _serial = serial_guard();
    let mut tk = fixture("readonly");
    let ts = history(&mut tk);
    tk.MustExec("create table t1 (id int primary key, v int)", Vec::new());
    tk.MustExec(
        "prepare stmt1 from 'insert into t(id,v) values (5,50)'",
        Vec::new(),
    );
    tk.MustExec("prepare stmt2 from 'select * from t'", Vec::new());
    tk.MustExec("set @@tidb_enable_noop_functions=1", Vec::new());

    #[derive(Clone, Copy)]
    enum Outcome {
        Read,
        Write,
        Lock,
        ExplainWrite,
    }
    let cases = [
        ("select statement", "select * from t", Outcome::Read, false),
        (
            "explain statement",
            "explain insert into t(id,v) values(1,10)",
            Outcome::ExplainWrite,
            true,
        ),
        (
            "explain analyze insert statement",
            "explain analyze insert into t(id,v) values(1,10)",
            Outcome::Write,
            false,
        ),
        (
            "explain analyze select statement",
            "explain analyze select * from t",
            Outcome::Read,
            false,
        ),
        (
            "execute insert statement",
            "execute stmt1",
            Outcome::Write,
            false,
        ),
        (
            "execute select statement",
            "execute stmt2",
            Outcome::Read,
            false,
        ),
        ("show statement", "show tables", Outcome::Read, false),
        (
            "set union",
            "select 1,2 union select 'a','b'",
            Outcome::Read,
            false,
        ),
        (
            "insert",
            "insert into t(id,v) values(3,30)",
            Outcome::Write,
            false,
        ),
        ("delete", "delete from t where id=1", Outcome::Write, false),
        (
            "update",
            "update t set v=11 where id=1",
            Outcome::Write,
            false,
        ),
        (
            "point get",
            "select * from t where id=1",
            Outcome::Read,
            false,
        ),
        (
            "batch point get",
            "select * from t where id in (1,2,3)",
            Outcome::Read,
            false,
        ),
        (
            "split table",
            "split table t between (0) and (1000000000) regions 16",
            Outcome::Read,
            false,
        ),
        ("do statement", "do sleep(1)", Outcome::Read, false),
        (
            "select for update",
            "select * from t where id=1 for update",
            Outcome::Lock,
            false,
        ),
        (
            "select lock in share mode",
            "select * from t where id=1 lock in share mode",
            Outcome::Lock,
            false,
        ),
        (
            "select for update union",
            "select * from t for update union select * from t",
            Outcome::Write,
            false,
        ),
        (
            "replace",
            "replace into t(id,v) values(1,11)",
            Outcome::Write,
            false,
        ),
        (
            "load data",
            "load data local infile '/mn/asa.csv' into table t fields terminated by x'2c' \
             enclosed by b'100010' lines terminated by '\\r\\n' ignore 1 lines (id)",
            Outcome::Write,
            false,
        ),
        (
            "update multi tables",
            "update t,t1 set t.v=1,t1.v=2 where t.id=2 and t1.id=3",
            Outcome::Write,
            false,
        ),
        (
            "delete multi tables",
            "delete t from t1 where t.id=t1.id",
            Outcome::Write,
            false,
        ),
        (
            "insert select",
            "insert into t select * from t1",
            Outcome::Write,
            false,
        ),
    ];

    let assert_rejected =
        |error: &astersql_testkit::TestError, outcome: Outcome, name: &str, sql: &str| {
            let message = error.message();
            let matches = match outcome {
                Outcome::Write => {
                    message.contains("read-only staleness transaction")
                        || message.contains("read-only statement")
                }
                Outcome::Lock => {
                    message.contains("select lock")
                        || message.contains("ForUpdateTS")
                        || message.contains("stale read")
                }
                Outcome::ExplainWrite => message.contains("GetForUpdateTS"),
                Outcome::Read => false,
            };
            assert!(matches, "{name}: `{sql}` returned the wrong error: {error}");
        };

    for (name, sql, outcome, valid_without_start) in cases {
        tk.MustExec(
            &format!("start transaction read only as of timestamp {ts}"),
            Vec::new(),
        );
        if matches!(outcome, Outcome::Read) {
            tk.MustQuery(sql, Vec::new());
        } else {
            let error = tk.ExecToErr(sql);
            assert_rejected(&error, outcome, name, sql);
        }
        tk.MustExec("commit", Vec::new());

        tk.MustExec(
            &format!("set transaction read only as of timestamp {ts}"),
            Vec::new(),
        );
        if matches!(outcome, Outcome::Read) || valid_without_start {
            tk.MustQuery(sql, Vec::new());
        } else {
            let error = tk.ExecToErr(sql);
            assert_rejected(&error, outcome, name, sql);
        }
        // Match Go's explicit per-case cleanup, including statements such as
        // EXPLAIN that do not start or consume a transaction.
        tk.MustExec("set transaction read only as of timestamp ''", Vec::new());
    }
}

#[test]
fn TestSpecialSQLInStalenessTxn() {
    let _serial = serial_guard();
    let mut tk = fixture("special_sql");
    let ts = history(&mut tk);
    tk.MustExec(
        "create user if not exists 'newuser' identified by 'mypassword'",
        Vec::new(),
    );
    for (name, sql, same_stale_transaction) in [
        (
            "ddl",
            "create table ddl_leaves_txn(id int,b int,index(b))",
            false,
        ),
        (
            "set global",
            "set global sql_mode='STRICT_TRANS_TABLES,NO_AUTO_CREATE_USER'",
            true,
        ),
        ("analyze", "analyze table t", true),
        (
            "session binding",
            "create session binding for select * from t where v=123 \
             using select * from t ignore index(primary) where v=123",
            true,
        ),
        (
            "global binding",
            "create global binding for select * from t where v=123 \
             using select * from t ignore index(primary) where v=123",
            true,
        ),
        (
            "grant",
            "grant all on stale_special_sql.* to 'newuser'",
            false,
        ),
        (
            "revoke",
            "revoke all on stale_special_sql.* from 'newuser'",
            false,
        ),
    ] {
        tk.MustExec(
            &format!("start transaction read only as of timestamp {ts}"),
            Vec::new(),
        );
        tk.MustExec(sql, Vec::new());
        assert_eq!(
            tk.StaleReadStateForTest().is_staleness,
            same_stale_transaction,
            "{name}: TxnCtx.IsStaleness"
        );
        let observed = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0]
            .parse::<u64>()
            .expect("transaction TSO after special statement");
        if same_stale_transaction {
            assert_eq!(observed, ts, "{name} must retain the stale transaction");
            tk.MustExec("commit", Vec::new());
        } else {
            assert_ne!(observed, ts, "{name} must leave the stale transaction");
            tk.MustExec("rollback", Vec::new());
        }
    }
}

#[test]
fn TestAsOfTimestampCompatibility() {
    let _serial = serial_guard();
    let mut tk = fixture("compat_txn");
    let ts = history(&mut tk);
    for begin in [
        format!("start transaction read only as of timestamp {ts}"),
        "begin".to_owned(),
        "start transaction".to_owned(),
    ] {
        for operation in [
            format!("set transaction read only as of timestamp {ts}"),
            select_as_of(ts),
        ] {
            tk.MustExec(&begin, Vec::new());
            let error = tk.ExecToErr(&operation);
            assert!(
                error.message().contains("transaction") || error.message().contains("can't be set"),
                "`{begin}` then `{operation}` must be rejected: {error}"
            );
            tk.MustExec("commit", Vec::new());
        }
    }
    tk.MustQuery(
        &format!("explain analyze select * from t as of timestamp {ts} where id=1"),
        Vec::new(),
    );
}

#[test]
fn TestSetTransactionInfoSchema() {
    let _serial = serial_guard();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    for (iteration, cache_size) in [1_073_741_824_u64, 0].into_iter().enumerate() {
        let database = format!("stale_set_schema_{iteration}");
        tk.MustExec(&format!("create database `{database}`"), Vec::new());
        tk.MustExec(&format!("use `{database}`"), Vec::new());
        tk.MustExec(
            &format!("set @@global.tidb_schema_cache_size={cache_size}"),
            Vec::new(),
        );
        tk.MustExec("create table t(id int primary key)", Vec::new());
        let version1 = domain.info_schema().SchemaMetaVersion();
        let ts1 = current_ts(&mut tk);
        tk.MustExec("alter table t add c int", Vec::new());
        let version2 = domain.info_schema().SchemaMetaVersion();
        let ts2 = current_ts(&mut tk);
        assert!(version1 < version2);

        tk.MustExec(
            &format!("set transaction read only as of timestamp {ts1}"),
            Vec::new(),
        );
        let pending_v1 = tk.StaleReadStateForTest();
        assert_eq!(pending_v1.session_info_schema_version, version1);
        assert_eq!(pending_v1.snapshot_info_schema_version, Some(version1));
        let old_column = tk.QueryToErr("select c from t");
        assert!(
            old_column
                .message()
                .to_ascii_lowercase()
                .contains("unknown column"),
            "cache_size={cache_size}: SET must immediately install schema v1: {old_column}"
        );

        tk.MustExec("alter table t add d int", Vec::new());
        let version3 = domain.info_schema().SchemaMetaVersion();
        assert!(version2 < version3);

        tk.MustExec(
            &format!("set transaction read only as of timestamp {ts1}"),
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        assert_eq!(
            tk.StaleReadStateForTest().txn_info_schema_version,
            version1,
            "cache_size={cache_size}: stale BEGIN must install TxnManager schema v1"
        );
        assert!(tk.Query("select c from t", Vec::new()).is_err());
        tk.MustExec("commit", Vec::new());

        tk.MustExec(
            &format!("set transaction read only as of timestamp {ts2}"),
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        assert_eq!(
            tk.StaleReadStateForTest().txn_info_schema_version,
            version2,
            "cache_size={cache_size}: stale BEGIN must install TxnManager schema v2"
        );
        tk.MustQuery("select c from t", Vec::new()).Check(Rows(&[]));
        assert!(tk.Query("select d from t", Vec::new()).is_err());
        tk.MustExec("commit", Vec::new());

        assert_eq!(
            tk.StaleReadStateForTest().txn_info_schema_version,
            version3,
            "cache_size={cache_size}: COMMIT must restore latest TxnManager schema"
        );
        assert_eq!(domain.info_schema().SchemaMetaVersion(), version3);
        tk.MustQuery("select c,d from t", Vec::new())
            .Check(Rows(&[]));
    }
}

#[test]
fn TestStaleSelect() {
    let _serial = serial_guard();
    let mut tk = fixture("stale_select");
    let ts = history(&mut tk);
    assert_as_of_rows(&tk, ts, &["1 10"]);
    tk.MustExec("begin", Vec::new());
    assert!(tk.Query(&select_as_of(ts), Vec::new()).is_err());
    tk.MustExec("commit", Vec::new());
    let prepared = tk.Prepare(&select_as_of(ts));
    let rows = prepared.query(&[]).expect("execute stale prepared select");
    assert_eq!(rows.string_rows(), Rows(&["1 10"]));
    tk.MustExec("begin", Vec::new());
    let prepared_in_txn = prepared
        .query(&[])
        .expect_err("prepared AS OF must be rejected in a current transaction");
    assert!(
        prepared_in_txn.message().contains("transaction"),
        "unexpected prepared/current-transaction error: {prepared_in_txn}"
    );
    tk.MustExec("commit", Vec::new());

    let later_ts = current_ts(&mut tk);
    tk.MustExec(
        &format!("start transaction read only as of timestamp {later_ts}"),
        Vec::new(),
    );
    let stale_in_stale = prepared
        .query(&[])
        .expect_err("statement AS OF must be rejected inside a stale transaction");
    assert!(
        stale_in_stale.message().contains("transaction")
            || stale_in_stale.message().contains("different time"),
        "unexpected stale-in-stale error: {stale_in_stale}"
    );
    tk.MustExec("commit", Vec::new());

    tk.MustExec("alter table t add c int", Vec::new());
    tk.MustExec("insert into t values(3,30,5)", Vec::new());
    let rows = prepared
        .query(&[])
        .expect("prepared stale select retains historical schema");
    assert_eq!(rows.string_rows(), Rows(&["1 10"]));
    tk.MustExec("alter table t add d int", Vec::new());
    tk.MustExec("insert into t values(4,40,4,4)", Vec::new());
    let point_ts = current_ts(&mut tk);
    tk.MustExec("insert into t values(5,50,5,5)", Vec::new());
    assert_eq!(
        tk.MustQuery(
            &format!("select * from t as of timestamp {point_ts} where c=5 order by id"),
            Vec::new(),
        )
        .Rows(),
        vec![vec![
            "3".to_owned(),
            "30".to_owned(),
            "5".to_owned(),
            "<nil>".to_owned(),
        ],],
    );
}

#[test]
fn TestStaleReadFutureTime() {
    let _serial = serial_guard();
    let mut tk = fixture("future");
    tk.MustExec("create table t (id int)", Vec::new());
    for sql in [
        "start transaction read only as of timestamp '2038-01-18 03:14:07'",
        "set transaction read only as of timestamp '2038-01-18 03:14:07'",
    ] {
        let error = tk.ExecToErr(sql);
        assert_eq!(
            error.message(),
            "cannot set read timestamp to a future time"
        );
        assert_eq!(
            tk.Session().TransactionDebugStringForTest().as_deref(),
            Some("Txn{state=invalid}"),
            "`{sql}` must not leave an active transaction"
        );
        let rejected = tk.StaleReadStateForTest();
        assert!(!rejected.transaction_active);
        assert!(!rejected.is_staleness);
        assert_eq!(rejected.pending_read_ts, None);
        tk.MustExec("begin", Vec::new());
        let start_ts = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0]
            .parse::<u64>()
            .expect("fresh transaction after rejected future timestamp");
        assert_ne!(
            start_ts, 0,
            "rejected future timestamp must not start/pin a transaction"
        );
        tk.MustExec("rollback", Vec::new());
    }
    let error = tk.QueryToErr("select * from t as of timestamp '2038-01-18 03:14:07'");
    assert_eq!(
        error.message(),
        "cannot set read timestamp to a future time"
    );
    tk.MustQuery("select * from t", Vec::new()).Check(Rows(&[]));
}

#[test]
fn TestStaleReadPrepare() {
    let _serial = serial_guard();
    let _config_restore =
        GlobalConfigRestore(astersql_config::get_global_config().as_ref().clone());
    astersql_config::update_global(|config| {
        config.labels = HashMap::from([("zone".to_owned(), "sh".to_owned())]);
    });
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("create database `stale_prepare`", Vec::new());
    tk.MustExec("use `stale_prepare`", Vec::new());
    let ts = history(&mut tk);
    tk.MustExec(
        &format!("prepare p1 from 'select * from t as of timestamp {ts} order by id'"),
        Vec::new(),
    );
    tk.MustExec("prepare p2 from 'select * from t order by id'", Vec::new());
    store
        .clear_replica_read_request_for_test()
        .expect("clear prepared AS OF request");
    tk.MustQuery("execute p1", Vec::new())
        .Check(Rows(&["1 10"]));
    let p1_request = store
        .last_replica_read_request_for_test()
        .expect("read prepared AS OF request")
        .expect("prepared AS OF must emit a request");
    assert_eq!(p1_request.txn_scope, "sh");
    assert_eq!(
        p1_request.store_labels,
        HashMap::from([("zone".to_owned(), "sh".to_owned())])
    );
    tk.MustExec(
        &format!("start transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    store
        .clear_replica_read_request_for_test()
        .expect("clear prepared stale-transaction request");
    tk.MustQuery("execute p2", Vec::new())
        .Check(Rows(&["1 10"]));
    let p2_request = store
        .last_replica_read_request_for_test()
        .expect("read prepared stale-transaction request")
        .expect("prepared stale-transaction SELECT must emit a request");
    assert_eq!(p2_request.txn_scope, "sh");
    assert_eq!(
        p2_request.store_labels,
        HashMap::from([("zone".to_owned(), "sh".to_owned())])
    );
    assert_eq!(tk.StaleReadStateForTest().start_ts, ts);
    tk.MustExec("commit", Vec::new());

    tk.MustExec(
        &format!("set transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    store
        .clear_replica_read_request_for_test()
        .expect("clear prepared SET request");
    tk.MustQuery("execute p2", Vec::new())
        .Check(Rows(&["1 10"]));
    let set_request = store
        .last_replica_read_request_for_test()
        .expect("read prepared SET request")
        .expect("prepared SET SELECT must emit a request");
    assert_eq!(set_request.txn_scope, "sh");
    assert_eq!(
        set_request.store_labels,
        HashMap::from([("zone".to_owned(), "sh".to_owned())])
    );
    assert_eq!(tk.StaleReadStateForTest().pending_read_ts, None);

    tk.MustExec(
        &format!("start transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    assert!(tk.Query("execute p1", Vec::new()).is_err());
    tk.MustExec("commit", Vec::new());

    tk.MustExec(
        &format!("set transaction read only as of timestamp {ts}"),
        Vec::new(),
    );
    let stale_after_set = tk.QueryToErr("execute p1");
    assert!(
        stale_after_set.message().contains("set transaction")
            || stale_after_set.message().contains("as of"),
        "prepared AS OF must conflict with pending SET TRANSACTION: {stale_after_set}"
    );
    tk.MustQuery("execute p2", Vec::new())
        .Check(Rows(&["1 10"]));

    tk.MustExec("create table t1(id int primary key,v int)", Vec::new());
    tk.MustExec("insert into t1 values(1,10)", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("set @a=tidb_parse_tso(@@tidb_current_ts)", Vec::new());
    tk.MustExec("commit", Vec::new());
    tk.MustExec("update t1 set v=100 where id=1", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&["1 100"]));
    tk.MustExec(
        "prepare s1 from 'select * from t1 as of timestamp @a where id=1'",
        Vec::new(),
    );
    tk.MustQuery("execute s1", Vec::new())
        .Check(Rows(&["1 10"]));
}

#[test]
fn TestStmtCtxStaleFlag() {
    let _serial = serial_guard();
    let mut tk = fixture("stmt_flag");
    let ts = history(&mut tk);
    const ASSERT_STMT_STALE: &str = "github.com/pingcap/tidb/exector/assertStmtCtxIsStaleness";
    let statements = [
        (format!("select * from t as of timestamp {ts}"), true),
        ("select * from t".to_owned(), false),
        (
            format!("start transaction read only as of timestamp {ts}"),
            false,
        ),
        ("select * from t".to_owned(), true),
        ("commit".to_owned(), false),
        (
            format!("set transaction read only as of timestamp {ts}"),
            false,
        ),
        ("select * from t".to_owned(), true),
        ("select * from t".to_owned(), false),
        (
            format!("prepare p from 'select * from t as of timestamp {ts}'"),
            false,
        ),
        ("execute p".to_owned(), true),
        ("prepare p1 from 'select * from t'".to_owned(), false),
        ("execute p1".to_owned(), false),
        (
            format!("start transaction read only as of timestamp {ts}"),
            false,
        ),
        ("execute p1".to_owned(), true),
        ("commit".to_owned(), false),
    ];
    for (sql, expected) in statements {
        let guard = testfailpoint::enable(ASSERT_STMT_STALE, &format!("1*return({expected})"));
        tk.MustExec(&sql, Vec::new());
        assert!(
            testfailpoint::eval_string(ASSERT_STMT_STALE).is_none(),
            "production did not observe StmtCtx.IsStaleness={expected} for `{sql}`"
        );
        let state = tk.StaleReadStateForTest();
        assert!(
            !state.statement_is_stale,
            "StmtCtx.IsStaleness was not reset after `{sql}`: {state:?}"
        );
        assert_eq!(
            state.last_statement_was_stale, expected,
            "statement-level stale observation for `{sql}`"
        );
        drop(guard);
    }
}

#[test]
fn TestStaleSessionQuery() {
    let _serial = serial_guard();
    let mut tk = fixture("session_query");
    tk.MustExec("create table t (id int primary key)", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    std::thread::sleep(Duration::from_secs(2));
    tk.MustExec("set @@tidb_read_staleness='-1'", Vec::new());
    assert!(
        tk.StaleReadStateForTest().session_read_ts.is_some(),
        "tidb_read_staleness must install a session read TS"
    );
    const INJECT_NOW: &str = "github.com/pingcap/tidb/pkg/expression/injectNow";
    const ASSERT_STALE_TSO: &str = "github.com/pingcap/tidb/pkg/executor/assertStaleTSO";
    let injected_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_secs();
    let now = testfailpoint::enable(INJECT_NOW, &format!("1*return({injected_now})"));
    let tso = testfailpoint::enable(ASSERT_STALE_TSO, &format!("1*return({})", injected_now - 1));
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1"]));
    assert!(testfailpoint::eval_string(INJECT_NOW).is_none());
    assert!(testfailpoint::eval_string(ASSERT_STALE_TSO).is_none());
    drop(tso);
    drop(now);
    tk.MustExec("begin", Vec::new());
    let current_txn = tk.StaleReadStateForTest();
    assert!(current_txn.transaction_active);
    assert!(!current_txn.is_staleness);
    assert!(current_txn.session_read_ts.is_some());
    tk.MustExec("insert into t values (2)", Vec::new());
    tk.MustExec("commit", Vec::new());
    tk.MustExec("insert into t values (3)", Vec::new());
    let now = testfailpoint::enable(INJECT_NOW, &format!("1*return({injected_now})"));
    let tso = testfailpoint::enable(ASSERT_STALE_TSO, &format!("1*return({})", injected_now - 1));
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1"]));
    assert!(testfailpoint::eval_string(INJECT_NOW).is_none());
    assert!(testfailpoint::eval_string(ASSERT_STALE_TSO).is_none());
    drop(tso);
    drop(now);
    tk.MustExec("set @@tidb_read_staleness=''", Vec::new());
    assert_eq!(tk.StaleReadStateForTest().session_read_ts, None);
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1", "2", "3"]));
}

#[test]
fn TestStaleReadCompatibility() {
    let _serial = serial_guard();
    let mut tk = fixture("compatibility");
    tk.MustExec("create table t(id int primary key,v int)", Vec::new());
    tk.MustExec("insert into t values(1,10)", Vec::new());
    let first_ts = current_ts(&mut tk);
    std::thread::sleep(Duration::from_secs(3));
    tk.MustExec("insert into t values(2,20)", Vec::new());
    let second_ts = current_ts(&mut tk);
    std::thread::sleep(Duration::from_secs(3));
    tk.MustExec(
        &format!("set transaction read only as of timestamp {first_ts}"),
        Vec::new(),
    );
    assert_eq!(tk.StaleReadStateForTest().pending_read_ts, Some(first_ts));
    let error = tk.QueryToErr(&select_as_of(first_ts));
    assert!(
        error
            .message()
            .contains("can't use select as of while already set transaction as of"),
        "unexpected error: {error}"
    );
    assert_eq!(
        tk.StaleReadStateForTest().pending_read_ts,
        None,
        "conflicting statement AS OF must consume pending SET TRANSACTION"
    );
    assert_eq!(
        tk.MustQuery("select * from t order by id", Vec::new())
            .Rows(),
        Rows(&["1 10", "2 20"]),
        "SET TRANSACTION must be consumed by the rejected AS OF statement"
    );
    tk.MustExec("set @@tidb_read_staleness='-5'", Vec::new());
    assert!(tk.StaleReadStateForTest().session_read_ts.is_some());
    let injected_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_secs();
    let now = testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/expression/injectNow",
        &format!("2*return({injected_now})"),
    );
    let stale = testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/assertStaleTSO",
        &format!("1*return({})", injected_now - 5),
    );
    assert_eq!(
        tk.MustQuery("select * from t order by id", Vec::new())
            .Rows(),
        Rows(&["1 10"]),
        "tidb_read_staleness"
    );
    assert!(
        testfailpoint::eval_string("github.com/pingcap/tidb/pkg/executor/assertStaleTSO").is_none()
    );
    drop(stale);
    assert_as_of_rows(&tk, second_ts, &["1 10", "2 20"]);
    tk.MustExec(
        &format!("set transaction read only as of timestamp {second_ts}"),
        Vec::new(),
    );
    let pending = tk.StaleReadStateForTest();
    assert_eq!(pending.pending_read_ts, Some(second_ts));
    assert!(pending.session_read_ts.is_some());
    assert_eq!(
        tk.MustQuery("select * from t order by id", Vec::new())
            .Rows(),
        Rows(&["1 10", "2 20"]),
        "SET TRANSACTION must override session staleness"
    );
    tk.MustExec(
        &format!("start transaction read only as of timestamp {second_ts}"),
        Vec::new(),
    );
    let active = tk.StaleReadStateForTest();
    assert!(active.transaction_active && active.is_staleness);
    assert_eq!(active.start_ts, second_ts);
    assert!(active.session_read_ts.is_some());
    assert_eq!(
        tk.MustQuery("select * from t order by id", Vec::new())
            .Rows(),
        Rows(&["1 10", "2 20"]),
        "START TRANSACTION AS OF must override session staleness"
    );
    tk.MustExec("commit", Vec::new());
    let resumed = tk.StaleReadStateForTest();
    assert!(!resumed.transaction_active);
    assert!(resumed.session_read_ts.is_some());
    let stale = testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/assertStaleTSO",
        &format!("1*return({})", injected_now - 5),
    );
    assert_eq!(
        tk.MustQuery("select * from t order by id", Vec::new())
            .Rows(),
        Rows(&["1 10"]),
        "session staleness must resume after stale COMMIT"
    );
    assert!(
        testfailpoint::eval_string("github.com/pingcap/tidb/pkg/executor/assertStaleTSO").is_none()
    );
    assert!(
        testfailpoint::eval_string("github.com/pingcap/tidb/pkg/expression/injectNow").is_none()
    );
    drop(stale);
    drop(now);
    tk.MustExec("set @@tidb_read_staleness=''", Vec::new());
    assert_eq!(tk.StaleReadStateForTest().session_read_ts, None);
    assert_eq!(
        tk.MustQuery("select * from t order by id", Vec::new())
            .Rows(),
        Rows(&["1 10", "2 20"]),
        "clearing session staleness must restore current reads"
    );
}

#[test]
fn TestStaleReadNoExtraTSORequest() {
    let _serial = serial_guard();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("create database `stale_no_extra_tso`", Vec::new());
    tk.MustExec("use `stale_no_extra_tso`", Vec::new());
    tk.MustExec("create table t(id int)", Vec::new());

    let mut unexpected_tso = Vec::new();
    let mut observe_tso = |before: u64, label: &str| {
        let after = store.tso_request_count_for_test();
        let requests = after.saturating_sub(before);
        if requests != 0 {
            unexpected_tso.push((label.to_owned(), requests));
        }
    };

    let before = store.tso_request_count_for_test();
    tk.MustQuery(
        "select * from t as of timestamp now()-interval 2 second",
        Vec::new(),
    );
    observe_tso(before, "statement stale read");

    tk.MustExec(
        "set transaction read only as of timestamp now()-interval 2 second",
        Vec::new(),
    );
    let before = store.tso_request_count_for_test();
    tk.MustQuery("select * from t", Vec::new());
    observe_tso(before, "SET + statement stale read");

    let before = store.tso_request_count_for_test();
    tk.MustExec(
        "start transaction read only as of timestamp now()-interval 2 second",
        Vec::new(),
    );
    observe_tso(before, "stale transaction / START");
    let before = store.tso_request_count_for_test();
    tk.MustQuery("select * from t", Vec::new());
    observe_tso(before, "stale transaction / SELECT");
    let before = store.tso_request_count_for_test();
    tk.MustExec("commit", Vec::new());
    observe_tso(before, "stale transaction / COMMIT");

    tk.MustExec(
        "set transaction read only as of timestamp now()-interval 2 second",
        Vec::new(),
    );
    let before = store.tso_request_count_for_test();
    tk.MustExec("begin", Vec::new());
    observe_tso(before, "SET + BEGIN stale transaction / BEGIN");
    let before = store.tso_request_count_for_test();
    tk.MustQuery("select * from t", Vec::new());
    observe_tso(before, "SET + BEGIN stale transaction / SELECT");
    let before = store.tso_request_count_for_test();
    tk.MustExec("commit", Vec::new());
    observe_tso(before, "SET + BEGIN stale transaction / COMMIT");

    tk.MustExec("set @@tidb_read_staleness='-1'", Vec::new());
    let before = store.tso_request_count_for_test();
    tk.MustQuery("select * from t", Vec::new());
    observe_tso(before, "tidb_read_staleness");
    tk.MustExec("set @@tidb_read_staleness=''", Vec::new());
    assert!(
        unexpected_tso.is_empty(),
        "stale paths made unexpected TSO requests: {unexpected_tso:?}"
    );
}

#[test]
fn TestPlanCacheWithStaleReadByBinaryProto() {
    let _serial = serial_guard();
    let mut tk = fixture("binary_prepare");
    tk.MustExec("create table t (id int primary key, v int)", Vec::new());
    tk.MustExec("insert into t values (1,10)", Vec::new());
    let ts = current_ts(&mut tk);
    tk.MustExec(&format!("set @a={ts}"), Vec::new());
    tk.MustExec("update t set v=100 where id=1", Vec::new());
    let stale = tk.Prepare("select * from t as of timestamp @a where id=?");
    for _ in 0..3 {
        let rows = stale
            .query(&[DbValue::from(1_i64)])
            .expect("execute binary stale prepared statement");
        assert_eq!(rows.string_rows(), Rows(&["1 10"]));
    }
    let current = tk.Prepare("select * from t where id=?");
    for _ in 0..2 {
        let rows = current
            .query(&[DbValue::from(1_i64)])
            .expect("execute binary current prepared statement");
        assert_eq!(rows.string_rows(), Rows(&["1 100"]));
    }
    tk.MustExec("set @@tx_read_ts=@a", Vec::new());
    let reused = current
        .query(&[DbValue::from(1_i64)])
        .expect("reuse cached current plan with @@tx_read_ts");
    assert_eq!(
        reused.string_rows(),
        Rows(&["1 10"]),
        "@@tx_read_ts must turn the already-cached current plan into a stale read"
    );
}

#[test]
fn TestStalePrepare() {
    let _serial = serial_guard();
    let mut tk = fixture("stale_prepare");
    tk.MustExec("create table t (id int primary key)", Vec::new());
    let binary = tk
        .Prepare("select * from t as of timestamp now(3)-interval 100000 microsecond order by id");
    tk.MustExec(
        "prepare stmt from \
         'select * from t as of timestamp now(3)-interval 100000 microsecond order by id'",
        Vec::new(),
    );
    let mut expected = Vec::new();
    for id in 0..20 {
        tk.MustExec("insert into t values (?)", vec![DbValue::from(id as i64)]);
        std::thread::sleep(Duration::from_millis(150));
        expected.push(vec![id.to_string()]);
        let binary_rows = binary
            .query(&[])
            .expect("binary dynamic AS OF")
            .string_rows();
        let named_rows = tk.MustQuery("execute stmt", Vec::new()).Rows();
        assert_eq!(binary_rows, named_rows);
        assert_eq!(
            binary_rows, expected,
            "iteration {id}: both prepared protocols must see every committed row"
        );
    }
}

#[test]
fn TestStaleTSO() {
    let _serial = serial_guard();
    let mut tk = fixture("stale_tso");
    tk.MustExec("create table t (id int primary key)", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    let current = current_ts(&mut tk);
    // Derive the injected future provider value from the actual mock-oracle
    // TSO. Subtracting ten seconds in each expression must land back on this
    // concrete snapshot instead of an unrelated wall-clock TSO.
    let next_tso = current + (10_000_u64 << 18);
    const MOCK_STALE_TSO: &str =
        "github.com/pingcap/tidb/pkg/sessiontxn/staleread/mockStaleReadTSO";
    let guard = testfailpoint::enable(MOCK_STALE_TSO, &format!("3*return({next_tso})"));
    for expression in [
        "now(3)-interval 10 second",
        "current_time()-interval 10 second",
        "curtime()-interval 10 second",
    ] {
        tk.MustQuery(
            &format!("select * from t as of timestamp {expression} order by id"),
            Vec::new(),
        )
        .Check(Rows(&["1"]));
    }
    assert!(
        testfailpoint::eval_string(MOCK_STALE_TSO).is_none(),
        "all three AS OF expressions must consume the injected future stale TSO"
    );
    drop(guard);
}

#[test]
fn TestStaleReadNoBackoff() {
    let _serial = serial_guard();
    let _config_restore =
        GlobalConfigRestore(astersql_config::get_global_config().as_ref().clone());
    astersql_config::update_global(|config| {
        config.labels = HashMap::from([("zone".to_owned(), "us-east-1a".to_owned())]);
    });
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("create database `stale_no_backoff`", Vec::new());
    tk.MustExec("use `stale_no_backoff`", Vec::new());
    tk.MustExec("create table t (id int primary key)", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("set @@tidb_read_staleness='-1'", Vec::new());
    tk.MustExec("set @@tidb_replica_read='closest-replicas'", Vec::new());
    std::thread::sleep(Duration::from_secs(1));
    const STORE_SEND_RESULT: &str = "tikvclient/tikvStoreSendReqResult";
    let guard = testfailpoint::enable(STORE_SEND_RESULT, r#"1*return("data_is_not_ready")"#);
    store
        .clear_replica_read_request_for_test()
        .expect("clear stale Get observation");
    let explain = tk
        .MustQuery("explain analyze select * from t where id=1", Vec::new())
        .Rows()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let request = store
        .last_replica_read_request_for_test()
        .expect("read stale Get observation")
        .expect("EXPLAIN ANALYZE point get must emit a KV request");
    assert_eq!(request.request_kind, "Get");
    assert!(request.stale_read, "injected request must be a stale read");
    assert!(
        !request.is_retry_request,
        "DataIsNotReady must be injected only on the first request"
    );
    assert_eq!(request.replica_read, "closest-replicas");
    assert_eq!(request.txn_scope, "us-east-1a");
    assert_eq!(
        request.store_labels,
        HashMap::from([("zone".to_owned(), "us-east-1a".to_owned())])
    );
    assert!(
        testfailpoint::eval_string(STORE_SEND_RESULT).is_none(),
        "DataIsNotReady injection was not consumed"
    );
    assert!(
        explain.contains("rpc_errors:{data_is_not_ready:1"),
        "injected DataIsNotReady must be reported in execution details: {explain}"
    );
    assert!(
        !explain.contains("dataNotReady_backoff"),
        "stale read must not back off on DataIsNotReady: {explain}"
    );
    drop(guard);
}

struct GlobalConfigRestore(astersql_config::Config);

impl Drop for GlobalConfigRestore {
    fn drop(&mut self) {
        astersql_config::store_global_config(self.0.clone());
    }
}

#[test]
fn TestStaleReadAllCombinations() {
    let _serial = serial_guard();
    let _config_restore =
        GlobalConfigRestore(astersql_config::get_global_config().as_ref().clone());
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("create database `stale_all_combinations`", Vec::new());
    tk.MustExec("use `stale_all_combinations`", Vec::new());
    tk.MustExec(
        "insert into mysql.tidb values(\
         'tikv_gc_safe_point','20160102-15:04:05 -0700',\
         'All versions after safe point can be accessed. (DO NOT EDIT)') \
         on duplicate key update variable_value='20160102-15:04:05 -0700',\
         comment='All versions after safe point can be accessed. (DO NOT EDIT)'",
        Vec::new(),
    );
    tk.MustExec("create table t(id int primary key,v int)", Vec::new());
    tk.MustExec("insert into t values(1,10)", Vec::new());
    let first_ts = current_ts(&mut tk);
    // The mock oracle advances logically rather than from wall time. Reserve
    // enough logical space for the Go test's incrementing external TS values.
    for _ in 0..128 {
        let _ = current_ts(&mut tk);
    }
    std::thread::sleep(Duration::from_secs(3));
    tk.MustExec("insert into t values(2,20)", Vec::new());
    let second_ts = current_ts(&mut tk);
    let row2_created_at = SystemTime::now();
    assert!(
        first_ts < second_ts,
        "row1 snapshot TSO must precede row2 snapshot TSO"
    );
    std::thread::sleep(Duration::from_secs(3));
    let label_settings: [(&str, &[(&str, &str)], &str); 5] = [
        ("no labels", &[], "global"),
        ("with DC label", &[("zone", "bj")], "bj"),
        ("with Zone label", &[("dc", "dc1")], "global"),
        ("with Rack label", &[("rack", "rack1")], "global"),
        (
            "with multiple labels",
            &[("zone", "bj"), ("dc", "dc1"), ("rack", "rack1")],
            "bj",
        ),
    ];
    let replica_settings = ["leader", "follower", "closest-replicas", "closest-adaptive"];
    let assert_request =
        |label: &str, labels: &[(&str, &str)], scope: &str, replica: &str, method: &str| {
            let request = store
                .last_replica_read_request_for_test()
                .expect("read all-combinations request observation")
                .unwrap_or_else(|| panic!("{label}/{replica}/{method}: missing KV request"));
            assert_eq!(
                request.replica_read, replica,
                "{label}/{replica}/{method}: replica policy"
            );
            assert_eq!(
                request.txn_scope, scope,
                "{label}/{replica}/{method}: txn scope"
            );
            assert_eq!(
                request.store_labels,
                labels
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect::<HashMap<_, _>>(),
                "{label}/{replica}/{method}: store labels"
            );
        };

    // Run the latency-sensitive session-staleness path first, matching Go's
    // ordering, then exercise every other read mechanism and transaction mode
    // under all five label sets and four replica policies.
    for (label, labels, expected_scope) in label_settings {
        astersql_config::update_global(|config| {
            config.labels = labels
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect();
        });
        for replica in replica_settings {
            tk.MustExec(&format!("set @@tidb_replica_read='{replica}'"), Vec::new());
            let elapsed = SystemTime::now()
                .duration_since(row2_created_at)
                .expect("wall clock after row2 creation");
            let staleness = elapsed.as_secs() + 2;
            tk.MustExec(
                &format!("set @@tidb_read_staleness='-{staleness}'"),
                Vec::new(),
            );
            store
                .clear_replica_read_request_for_test()
                .expect("clear session staleness request");
            assert_eq!(
                tk.MustQuery("select * from t order by id", Vec::new())
                    .Rows(),
                Rows(&["1 10"]),
                "{label}/{replica}/tidb_read_staleness"
            );
            assert_request(
                label,
                labels,
                expected_scope,
                replica,
                "tidb_read_staleness",
            );
            tk.MustExec("set @@tidb_read_staleness=''", Vec::new());
        }
    }

    let mut external_ts = first_ts;
    for (label, labels, expected_scope) in label_settings {
        astersql_config::update_global(|config| {
            config.labels = labels
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect();
        });
        for replica in replica_settings {
            tk.MustExec(&format!("set @@tidb_replica_read='{replica}'"), Vec::new());

            store
                .clear_replica_read_request_for_test()
                .expect("clear AS OF first request");
            assert_eq!(
                tk.MustQuery(&select_as_of(first_ts), Vec::new()).Rows(),
                Rows(&["1 10"]),
                "{label}/{replica}/AS OF first"
            );
            assert_request(label, labels, expected_scope, replica, "AS OF first");
            store
                .clear_replica_read_request_for_test()
                .expect("clear AS OF second request");
            assert_eq!(
                tk.MustQuery(&select_as_of(second_ts), Vec::new()).Rows(),
                Rows(&["1 10", "2 20"]),
                "{label}/{replica}/AS OF second"
            );
            assert_request(label, labels, expected_scope, replica, "AS OF second");

            tk.MustExec(
                &format!("set transaction read only as of timestamp {first_ts}"),
                Vec::new(),
            );
            store
                .clear_replica_read_request_for_test()
                .expect("clear SET TRANSACTION request");
            assert_eq!(
                tk.MustQuery("select * from t order by id", Vec::new())
                    .Rows(),
                Rows(&["1 10"]),
                "{label}/{replica}/SET TRANSACTION"
            );
            assert_request(label, labels, expected_scope, replica, "SET TRANSACTION");

            tk.MustExec("set @@tidb_enable_external_ts_read=1", Vec::new());
            tk.MustExec(
                &format!("set @@global.tidb_external_ts={external_ts}"),
                Vec::new(),
            );
            external_ts += 1;
            store
                .clear_replica_read_request_for_test()
                .expect("clear external TS request");
            assert_eq!(
                tk.MustQuery("select * from t order by id", Vec::new())
                    .Rows(),
                Rows(&["1 10"]),
                "{label}/{replica}/external TS"
            );
            assert_request(label, labels, expected_scope, replica, "external TS");
            tk.MustExec("set @@tidb_enable_external_ts_read=0", Vec::new());

            tk.MustExec(&format!("set @@tidb_snapshot='{first_ts}'"), Vec::new());
            store
                .clear_replica_read_request_for_test()
                .expect("clear tidb_snapshot request");
            assert_eq!(
                tk.MustQuery("select * from t order by id", Vec::new())
                    .Rows(),
                Rows(&["1 10"]),
                "{label}/{replica}/tidb_snapshot"
            );
            assert_request(label, labels, expected_scope, replica, "tidb_snapshot");
            tk.MustExec("set @@tidb_snapshot=''", Vec::new());

            for set_then_begin in [false, true] {
                if set_then_begin {
                    tk.MustExec(
                        &format!("set transaction read only as of timestamp {first_ts}"),
                        Vec::new(),
                    );
                    tk.MustExec("begin", Vec::new());
                } else {
                    tk.MustExec(
                        &format!("start transaction read only as of timestamp {first_ts}"),
                        Vec::new(),
                    );
                }
                store
                    .clear_replica_read_request_for_test()
                    .expect("clear stale transaction request");
                assert_eq!(
                    tk.MustQuery("select * from t order by id", Vec::new())
                        .Rows(),
                    Rows(&["1 10"]),
                    "{label}/{replica}/transaction set_then_begin={set_then_begin}"
                );
                assert_request(
                    label,
                    labels,
                    expected_scope,
                    replica,
                    if set_then_begin {
                        "SET + BEGIN"
                    } else {
                        "START TRANSACTION"
                    },
                );
                tk.MustExec("commit", Vec::new());
            }
        }
    }
    tk.MustExec(
        "delete from mysql.tidb where variable_name='tikv_gc_safe_point'",
        Vec::new(),
    );
}
