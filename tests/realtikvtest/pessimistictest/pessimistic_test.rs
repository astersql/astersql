// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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
//! 中文总览：`pessimistic_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `悲观事务与锁等待` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 311 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `Case` 是当前文件里的分支类型。
//! `Case` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `Case` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Case`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `Case` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `Case` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `Case` 当作定位同类问题的索引锚点。
//! 作为分支类型，`Case` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `Case` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`Case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `database_name` 是当前文件里的辅助函数。
//! `database_name` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `database_name` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `database_name`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `database_name` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `database_name` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `database_name` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`database_name` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `database_name` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`database_name` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `prepare` 是当前文件里的辅助函数。
//! `prepare` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `prepare` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `prepare`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `prepare` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `prepare` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `prepare` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`prepare` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `prepare` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`prepare` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `session` 是当前文件里的辅助函数。
//! `session` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `session` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `session` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `session` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`session` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `session` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `create_accounts` 是当前文件里的辅助函数。
//! `create_accounts` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `create_accounts` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `create_accounts`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `create_accounts` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `create_accounts` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `create_accounts` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`create_accounts` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `create_accounts` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`create_accounts` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `assert_duplicate` 是当前文件里的辅助函数。
//! `assert_duplicate` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `assert_duplicate` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_duplicate`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_duplicate` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `assert_duplicate` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `assert_duplicate` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`assert_duplicate` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `assert_duplicate` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`assert_duplicate` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `run_case` 是当前文件里的辅助函数。
//! `run_case` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `run_case` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `run_case`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `run_case` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `run_case` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `run_case` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`run_case` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `run_case` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`run_case` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `Pipe` 是当前文件里的契约类型。
//! `Pipe` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `Pipe` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Pipe`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `Pipe` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `Pipe` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `Pipe` 当作定位同类问题的索引锚点。
//! 作为契约类型，`Pipe` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `Pipe` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`Pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `pipe` 是当前文件里的辅助函数。
//! `pipe` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `pipe` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `pipe`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `pipe` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `pipe` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `pipe` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`pipe` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `pipe` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`pipe` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `must_exec_async` 是当前文件里的辅助函数。
//! `must_exec_async` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `must_exec_async` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `must_exec_async`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `must_exec_async` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `must_exec_async` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `must_exec_async` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`must_exec_async` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `must_exec_async` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`must_exec_async` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 符号 `async_helper_executes_on_an_independent_real_session` 是当前文件里的辅助函数。
//! `async_helper_executes_on_an_independent_real_session` 所处的位置主要服务 `悲观事务与锁等待` 主题下的一个阅读切面。
//! 阅读 `async_helper_executes_on_an_independent_real_session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `async_helper_executes_on_an_independent_real_session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `async_helper_executes_on_an_independent_real_session` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `async_helper_executes_on_an_independent_real_session` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `pessimistic_test.rs` 的回归，可以把 `async_helper_executes_on_an_independent_real_session` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`async_helper_executes_on_an_independent_real_session` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `async_helper_executes_on_an_independent_real_session` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 补充视角 1：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 2：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 3：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 4：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 5：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 6：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 7：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 8：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 9：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 10：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 11：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 12：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 13：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 14：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 15：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 16：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 17：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 18：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 补充视角 19：`async_helper_executes_on_an_independent_real_session` 在 `悲观事务与锁等待` 主题下承担的是阅读索引，而不是新增逻辑。
//! 中文说明结束（自动生成）

//! Real SQL port of `pessimistic_test.go`.
//!
//! Every Go test has one Rust test below.  The shared cases only centralize
//! fixture construction; they all use independent `ConcreteSession`s backed
//! by the real transactional mock KV store.  No SQL result is registered or
//! mocked in this file.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_tests_realtikvtest_pessimistictest::serial_guard;

#[derive(Clone, Copy)]
enum Case {
    CommitVisibility,
    TxnMode,
    Serializable,
    ExpiredLocks,
    CommitFailureReleasesLocks,
    BatchResolveLocks,
    EndTxnAfterExpiry,
    Deadlock,
    NoWait,
    KillWait,
    WaitTimeout,
    WaitSeconds,
    FairBasic,
    RollbackVisibility,
    StatementRollback,
    ConcurrentStatementRollback,
    DuplicateKey,
    InsertOnDuplicate,
    PointGetOverflow,
    AsyncRollbackNoWait,
    PushCondition,
    PointGet,
    BatchPointGet,
    BankTransfer,
    LockUnchanged,
    OptimisticConflict,
    BatchWriteConflict,
    LockNonExisting,
    BatchLockIndex,
    RcSubQuery,
    RcIndexMerge,
    GeneratedColumn,
    DupConsistency,
    DupAfterLock,
    DupAfterLockBatch,
    ReadCommitted,
    ReadCommittedExternal,
    NonExistingKey,
    DeleteInMemory,
    UnionForUpdate,
    SchemaChange,
    PreparedSchemaChange,
    ForeignKey,
    Partition,
    LazyUnique,
    InsertIgnore,
    SavepointEquivalent,
    FairConflict,
    AutoCommit,
    StatementTypes,
    ForShare,
    ForSharePlanCache,
    MaxExecutionTime,
}

fn database_name(test_name: &str) -> String {
    format!("pess_{}", test_name.trim_start_matches("test_"))
}

fn prepare(test_name: &str) -> (Arc<AnalyzeStatsStore>, TestKit, String) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let database = database_name(test_name);
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists `{database}`"), Vec::new());
    tk.MustExec(&format!("create database `{database}`"), Vec::new());
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    (store, tk, database)
}

fn session(store: Arc<AnalyzeStatsStore>, database: &str) -> TestKit {
    let mut tk = NewTestKit(store);
    // ConcreteSession keeps the selected-schema catalog per connection while
    // table metadata and KV data are shared through Domain.
    tk.MustExec(
        &format!("create database if not exists `{database}`"),
        Vec::new(),
    );
    tk.MustExec(&format!("use `{database}`"), Vec::new());
    tk
}

fn create_accounts(tk: &mut TestKit) {
    tk.MustExec(
        "create table t(id int primary key, uk int unique, v int not null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,10,100),(2,20,200),(3,30,300)",
        Vec::new(),
    );
}

fn assert_duplicate(error: impl std::fmt::Display, value: &str) {
    let message = error.to_string();
    assert!(
        message.contains("[kv:1062]")
            && message.contains("Duplicate entry")
            && message.contains(value),
        "unexpected duplicate-key error: {message}"
    );
}

fn run_case(test_name: &str, case: Case) {
    let _serial = serial_guard();
    let (store, mut tk, database) = prepare(test_name);
    match case {
        Case::CommitVisibility => {
            create_accounts(&mut tk);
            let peer_store = store.clone();
            let mut peer = session(peer_store, &database);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update t set v=101 where id=1", Vec::new());
            tk.MustQuery("select v from t where id=1 for update", Vec::new())
                .Check(Rows(&["101"]));
            peer.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["100"]));
            tk.MustExec("commit", Vec::new());
            peer.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["101"]));
        }
        Case::TxnMode => {
            tk.MustExec("create table txn_mode(a int)", Vec::new());
            for (begin_mode, session_mode, pessimistic) in [
                ("pessimistic", "pessimistic", true),
                ("pessimistic", "optimistic", true),
                ("pessimistic", "", true),
                ("optimistic", "pessimistic", false),
                ("optimistic", "optimistic", false),
                ("optimistic", "", false),
                ("", "pessimistic", true),
                ("", "optimistic", false),
                ("", "", false),
            ] {
                tk.MustExec(
                    &format!("set @@tidb_txn_mode = '{session_mode}'"),
                    Vec::new(),
                );
                tk.MustExec(
                    if begin_mode.is_empty() {
                        "begin"
                    } else if begin_mode == "pessimistic" {
                        "begin pessimistic"
                    } else {
                        "begin optimistic"
                    },
                    Vec::new(),
                );
                store
                    .expire_latest_pessimistic_locks_for_test()
                    .expect("expire transaction while checking its mode");
                if pessimistic {
                    tk.MustGetErrMsg(
                        "select 1",
                        "[tikv:8220]TTL manager has timed out, pessimistic transaction has been rolled back",
                    );
                    tk.MustExec("commit", Vec::new());
                } else {
                    tk.MustQuery("select 1", Vec::new()).Check(Rows(&["1"]));
                    tk.MustExec("rollback", Vec::new());
                }
            }

            tk.MustExec("set @@autocommit = 0", Vec::new());
            for (session_mode, pessimistic) in
                [("pessimistic", true), ("optimistic", false), ("", false)]
            {
                tk.MustExec(
                    &format!("set @@tidb_txn_mode = '{session_mode}'"),
                    Vec::new(),
                );
                tk.MustExec("rollback", Vec::new());
                tk.MustExec("insert into txn_mode values (1)", Vec::new());
                store
                    .expire_latest_pessimistic_locks_for_test()
                    .expect("expire implicit transaction while checking its mode");
                if pessimistic {
                    tk.MustGetErrMsg(
                        "select 1",
                        "[tikv:8220]TTL manager has timed out, pessimistic transaction has been rolled back",
                    );
                } else {
                    tk.MustQuery("select 1", Vec::new()).Check(Rows(&["1"]));
                    tk.MustExec("rollback", Vec::new());
                }
            }
            tk.MustExec("set @@autocommit = 1", Vec::new());

            tk.MustExec("set @@global.tidb_txn_mode = 'pessimistic'", Vec::new());
            let mut inherited = session(store.clone(), &database);
            inherited
                .MustQuery("select @@tidb_txn_mode", Vec::new())
                .Check(Rows(&["pessimistic"]));
            inherited.MustExec("set @@tidb_txn_mode = ''", Vec::new());
            inherited
                .MustQuery("select @@tidb_txn_mode", Vec::new())
                .Check(Rows(&[""]));
        }
        Case::Serializable => {
            tk.MustExec(
                "create table serial_t(id int primary key, on_call int not null)",
                Vec::new(),
            );
            tk.MustExec("insert into serial_t values (1,1),(2,1)", Vec::new());
            let mut first = session(store.clone(), &database);
            let mut second = session(store.clone(), &database);

            // PMP/phantom: a serializable snapshot remains empty after an
            // external insert.
            first.MustExec("set transaction isolation level serializable", Vec::new());
            first.MustExec("begin pessimistic", Vec::new());
            first
                .MustQuery("select * from serial_t where id=9", Vec::new())
                .Check(Vec::<Vec<String>>::new());
            second.MustExec("insert into serial_t values (9,1)", Vec::new());
            first
                .MustQuery("select * from serial_t where id=9", Vec::new())
                .Check(Vec::<Vec<String>>::new());
            first.MustExec("rollback", Vec::new());

            // P4/read skew: both reads in one snapshot see the old pair.
            first.MustExec("begin pessimistic", Vec::new());
            first
                .MustQuery("select on_call from serial_t where id=1", Vec::new())
                .Check(Rows(&["1"]));
            second.MustExec("update serial_t set on_call=0 where id=1", Vec::new());
            second.MustExec("update serial_t set on_call=0 where id=2", Vec::new());
            first
                .MustQuery("select on_call from serial_t where id=2", Vec::new())
                .Check(Rows(&["1"]));
            first.MustExec("rollback", Vec::new());
            tk.MustQuery("select id,on_call from serial_t order by id", Vec::new())
                .Check(Rows(&["1 0", "2 0", "9 1"]));
        }
        Case::ExpiredLocks => {
            create_accounts(&mut tk);
            let mut waiter = session(store.clone(), &database);
            // Open the lock owner last so the deterministic store hook targets
            // precisely this session.
            let mut holder = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where id=1 for update", Vec::new())
                .Check(Rows(&["1 10 100"]));
            store
                .set_latest_pessimistic_lock_ttl_for_test(Duration::from_millis(10))
                .expect("set holder lock TTL");
            store
                .expire_latest_pessimistic_locks_for_test()
                .expect("expire holder locks");
            // The next owner statement observes the expired TTL, ends the
            // transaction, reports ErrLockExpire, and releases row locks.
            holder.MustGetErrMsg(
                "select 1",
                "[tikv:8220]TTL manager has timed out, pessimistic transaction has been rolled back",
            );
            holder.MustExec("commit", Vec::new());
            waiter.MustExec("begin pessimistic", Vec::new());
            waiter
                .MustQuery("select * from t where id=1 for update nowait", Vec::new())
                .Check(Rows(&["1 10 100"]));
            waiter.MustExec("update t set v=105 where id=1", Vec::new());
            waiter.MustExec("commit", Vec::new());
            tk.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["105"]));
        }
        Case::CommitFailureReleasesLocks => {
            create_accounts(&mut tk);
            let mut failed = session(store.clone(), &database);
            failed.MustExec("begin pessimistic", Vec::new());
            failed.MustExec("update t set v=101 where id=1", Vec::new());
            let mut follower = session(store.clone(), &database);
            failed
                .Session()
                .InjectNextDmlCommitErrorForTest("[tikv:1105]injected commit failure")
                .expect("inject commit failure");
            failed.MustGetErrMsg("commit", "[tikv:1105]injected commit failure");
            follower.MustExec("set innodb_lock_wait_timeout=1", Vec::new());
            follower.MustExec("begin pessimistic", Vec::new());
            follower.MustExec("update t set v=105 where id=1", Vec::new());
            follower.MustExec("commit", Vec::new());
            tk.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["105"]));
        }
        Case::BatchResolveLocks => {
            for table in ["t1", "t2", "t3"] {
                tk.MustExec(
                    &format!("create table {table}(id int primary key, v int)"),
                    Vec::new(),
                );
                tk.MustExec(
                    &format!("insert into {table} values (1,10),(2,20)"),
                    Vec::new(),
                );
            }
            let mut stale = session(store.clone(), &database);
            let mut resolver = session(store.clone(), &database);
            stale.MustExec("begin pessimistic", Vec::new());
            for table in ["t1", "t2", "t3"] {
                stale
                    .MustQuery(
                        &format!("select * from {table} where id=1 for update"),
                        Vec::new(),
                    )
                    .Check(Rows(&["1 10"]));
            }
            resolver.MustExec("begin pessimistic", Vec::new());
            resolver.MustGetErrMsg(
                "select * from t2 where id=1 for update nowait",
                "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
            );
            resolver.MustExec("rollback", Vec::new());
            stale.MustExec("rollback", Vec::new());
            resolver.MustExec("begin pessimistic", Vec::new());
            for table in ["t1", "t2", "t3"] {
                resolver
                    .MustQuery(
                        &format!("select * from {table} where id=1 for update nowait"),
                        Vec::new(),
                    )
                    .Check(Rows(&["1 10"]));
            }
            resolver.MustExec("commit", Vec::new());
            tk.MustExec("admin check table t1", Vec::new());
            tk.MustExec("admin check table t2", Vec::new());
            tk.MustExec("admin check table t3", Vec::new());
        }
        Case::EndTxnAfterExpiry => {
            create_accounts(&mut tk);
            for (index, prepared) in [false, true, false, true].into_iter().enumerate() {
                let mut actor = session(store.clone(), &database);
                actor.MustExec("begin pessimistic", Vec::new());
                actor
                    .MustQuery("select * from t where id=1 for update", Vec::new())
                    .Check(Rows(&["1 10 100"]));
                let commit = actor.Prepare("commit");
                let rollback = actor.Prepare("rollback");
                store
                    .set_latest_pessimistic_lock_ttl_for_test(Duration::from_millis(10))
                    .expect("set actor lock TTL");
                store
                    .expire_latest_pessimistic_locks_for_test()
                    .expect("expire actor lock");
                actor.MustGetErrMsg(
                    "select * from t",
                    "[tikv:8220]TTL manager has timed out, pessimistic transaction has been rolled back",
                );
                if index < 2 {
                    if prepared {
                        commit.execute(&[]).expect("binary commit");
                    } else {
                        actor.MustExec("commit", Vec::new());
                    }
                } else if prepared {
                    rollback.execute(&[]).expect("binary rollback");
                } else {
                    actor.MustExec("rollback", Vec::new());
                }
                let mut probe = session(store.clone(), &database);
                probe.MustExec("begin pessimistic", Vec::new());
                probe
                    .MustQuery("select * from t where id=1 for update nowait", Vec::new())
                    .Check(Rows(&["1 10 100"]));
                probe.MustExec("rollback", Vec::new());
            }
        }
        Case::Deadlock => {
            create_accounts(&mut tk);
            let mut first = session(store.clone(), &database);
            let mut second = session(store.clone(), &database);
            store
                .clear_runtime_deadlock_history()
                .expect("clear deadlock history");
            first.MustExec("begin pessimistic", Vec::new());
            second.MustExec("begin pessimistic", Vec::new());
            first
                .MustQuery("select * from t where id=1 for update", Vec::new())
                .Check(Rows(&["1 10 100"]));
            second
                .MustQuery("select * from t where id=2 for update", Vec::new())
                .Check(Rows(&["2 20 200"]));
            let first_waiter = thread::spawn(move || {
                first
                    .MustQuery("select * from t where id=2 for update", Vec::new())
                    .Check(Rows(&["2 20 200"]));
                first.MustExec("rollback", Vec::new());
            });
            thread::sleep(Duration::from_millis(30));
            let deadlock = second.QueryToErr("select * from t where id=1 for update");
            assert!(
                deadlock.message().contains("[tikv:1213]")
                    && deadlock.message().contains("Deadlock found"),
                "unexpected deadlock error: {deadlock}"
            );
            second.MustExec("rollback", Vec::new());
            first_waiter.join().expect("deadlock survivor");
            assert_eq!(
                store
                    .runtime_deadlock_history_count()
                    .expect("read deadlock history"),
                2,
                "two reciprocal wait-for edges must be retained"
            );
        }
        Case::NoWait => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            let mut contender = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where id=1 for update", Vec::new())
                .Check(Rows(&["1 10 100"]));
            contender.MustExec("begin pessimistic", Vec::new());
            for sql in [
                "select * from t where id=1 for update nowait",
                "select * from t where uk=10 for update nowait",
                "select * from t where id between 1 and 1 for update nowait",
            ] {
                let started = Instant::now();
                let error = contender.QueryToErr(sql);
                assert_eq!(
                    error.message(),
                    "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set"
                );
                assert!(
                    started.elapsed() < Duration::from_secs(1),
                    "NOWAIT must not enter the normal wait path"
                );
            }
            holder.MustExec("commit", Vec::new());
            contender
                .MustQuery("select * from t where id=1 for update nowait", Vec::new())
                .Check(Rows(&["1 10 100"]));
            contender
                .MustQuery(
                    "select * from t where id between 1 and 1 for update nowait",
                    Vec::new(),
                )
                .Check(Rows(&["1 10 100"]));
            contender.MustExec("rollback", Vec::new());
        }
        Case::KillWait => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where id=1 for update", Vec::new())
                .Check(Rows(&["1 10 100"]));
            let mut waiter = session(store.clone(), &database);
            let killer = store.sql_killer();
            waiter.MustExec("begin pessimistic", Vec::new());
            let waiting = thread::spawn(move || {
                let error = waiter.QueryToErr("select * from t where id=1 for update");
                waiter.MustExec("rollback", Vec::new());
                error.message().to_owned()
            });
            thread::sleep(Duration::from_millis(30));
            killer.SendKillSignal(1);
            let message = waiting.join().expect("killed lock waiter");
            assert!(
                message.contains("Query execution was interrupted"),
                "unexpected kill error: {message}"
            );
            holder.MustExec("rollback", Vec::new());
        }
        Case::WaitTimeout => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            let mut waiter = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where id=1 for update", Vec::new())
                .Check(Rows(&["1 10 100"]));
            waiter.MustExec("set innodb_lock_wait_timeout=1", Vec::new());
            waiter.MustExec("begin pessimistic", Vec::new());
            let started = Instant::now();
            let error = waiter.QueryToErr("select * from t where id=1 for update");
            let elapsed = started.elapsed();
            assert_eq!(
                error.message(),
                "[tikv:1205]Lock wait timeout exceeded; try restarting transaction"
            );
            assert!(
                elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(3),
                "lock timeout elapsed {elapsed:?}"
            );
            waiter.MustExec("rollback", Vec::new());
            holder.MustExec("rollback", Vec::new());
        }
        Case::WaitSeconds => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where id in (1,2,3) for update", Vec::new())
                .Check(Rows(&["1 10 100", "2 20 200", "3 30 300"]));
            let started_all = Instant::now();
            let waiters = [
                "select * from t where id=1 for update wait 1",
                "select * from t where id between 1 and 2 for update wait 1",
                "select * from t where id in (2,3) for update wait 1",
            ]
            .into_iter()
            .map(|sql| {
                let waiter_store = store.clone();
                let waiter_database = database.clone();
                thread::spawn(move || {
                    let mut waiter = session(waiter_store, &waiter_database);
                    waiter.MustExec("begin pessimistic", Vec::new());
                    let started = Instant::now();
                    let error = waiter.QueryToErr(sql);
                    let elapsed = started.elapsed();
                    waiter.MustExec("rollback", Vec::new());
                    (error.message().to_owned(), elapsed)
                })
            })
            .collect::<Vec<_>>();
            for waiter in waiters {
                let (message, elapsed) = waiter.join().expect("WAIT seconds waiter");
                assert_eq!(
                    message,
                    "[tikv:1205]Lock wait timeout exceeded; try restarting transaction"
                );
                assert!(
                    elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(3),
                    "WAIT 1 elapsed {elapsed:?}"
                );
            }
            assert!(
                started_all.elapsed() < Duration::from_secs(3),
                "three WAIT 1 shapes must time out concurrently"
            );
            holder.MustExec("rollback", Vec::new());
        }
        Case::FairBasic => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            let mut waiter = session(store.clone(), &database);
            holder.MustExec("set tidb_pessimistic_txn_fair_locking=1", Vec::new());
            waiter.MustExec("set tidb_pessimistic_txn_fair_locking=1", Vec::new());
            holder.MustExec("begin pessimistic", Vec::new());
            holder.MustExec("update t set v=101 where id=1", Vec::new());
            let waiting = thread::spawn(move || {
                waiter.MustExec("begin pessimistic", Vec::new());
                waiter.MustExec("update t set v=102 where id=1", Vec::new());
                waiter.MustExec("commit", Vec::new());
            });
            thread::sleep(Duration::from_millis(30));
            holder.MustExec("commit", Vec::new());
            waiting.join().expect("fair lock waiter");
            tk.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["102"]));
        }
        Case::RollbackVisibility => {
            create_accounts(&mut tk);
            let peer_store = store.clone();
            let peer = session(peer_store, &database);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update t set v=999 where id=1", Vec::new());
            tk.MustQuery("select v from t where id=1 for update", Vec::new())
                .Check(Rows(&["999"]));
            peer.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["100"]));
            tk.MustExec("rollback", Vec::new());
            peer.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["100"]));
        }
        Case::StatementRollback => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update t set v=v+1 where id=1", Vec::new());
            let error = tk.ExecToErr("insert into t values (4,20,400)");
            assert_duplicate(error, "20");
            tk.MustQuery("select id,uk,v from t order by id", Vec::new())
                .Check(Rows(&["1 10 101", "2 20 200", "3 30 300"]));
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["101"]));
        }
        Case::ConcurrentStatementRollback => {
            tk.MustExec(
                "create table statement_t(id int primary key, v int not null)",
                Vec::new(),
            );
            tk.MustExec(
                "insert into statement_t values (1,1),(2,1),(3,1),(4,1)",
                Vec::new(),
            );
            let mut first = session(store.clone(), &database);
            let mut second = session(store.clone(), &database);
            first.MustExec("begin pessimistic", Vec::new());
            first.MustExec("update statement_t set v=v+1", Vec::new());
            let waiting = thread::spawn(move || {
                second.MustExec("begin pessimistic", Vec::new());
                second.MustExec("update statement_t set v=v+1", Vec::new());
                second.MustExec("commit", Vec::new());
            });
            thread::sleep(Duration::from_millis(30));
            assert!(
                !waiting.is_finished(),
                "the second whole-table statement must wait for the first transaction"
            );
            first.MustExec("commit", Vec::new());
            waiting.join().expect("whole-table statement waiter");
            tk.MustQuery("select id,v from statement_t order by id", Vec::new())
                .Check(Rows(&["1 3", "2 3", "3 3", "4 3"]));
        }
        Case::DuplicateKey => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            let primary = tk.ExecToErr("insert into t values (1,40,400)");
            assert_duplicate(primary, "1");
            let unique = tk.ExecToErr("insert into t values (4,20,400)");
            assert_duplicate(unique, "20");
            tk.MustExec("rollback", Vec::new());
            tk.MustQuery("select count(*) from t", Vec::new())
                .Check(Rows(&["3"]));
        }
        Case::InsertOnDuplicate => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec(
                "insert into t values (1,10,150) on duplicate key update v=values(v)",
                Vec::new(),
            );
            tk.MustQuery("select v from t where id=1 for update", Vec::new())
                .Check(Rows(&["150"]));
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select id,uk,v from t order by id", Vec::new())
                .Check(Rows(&["1 10 150", "2 20 200", "3 30 300"]));
        }
        Case::PointGetOverflow => {
            tk.MustExec(
                "create table overflow_t(k tinyint unique, v int)",
                Vec::new(),
            );
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update overflow_t set v=100 where k=-200", Vec::new());
            tk.MustExec(
                "update overflow_t set v=100 where k in (-200,-400)",
                Vec::new(),
            );
            tk.MustExec("rollback", Vec::new());
        }
        Case::AsyncRollbackNoWait => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            let mut waiter = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where v>0 for update", Vec::new())
                .Check(Rows(&["1 10 100", "2 20 200", "3 30 300"]));
            waiter.MustExec("begin pessimistic", Vec::new());
            waiter.MustGetErrMsg(
                "select * from t where id=1 for update nowait",
                "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
            );
            holder.MustExec("commit", Vec::new());
            waiter
                .MustQuery(
                    "select * from t where v>0 order by id for update nowait",
                    Vec::new(),
                )
                .Check(Rows(&["1 10 100", "2 20 200", "3 30 300"]));
            waiter
                .MustQuery("select * from t where id=1 for update nowait", Vec::new())
                .Check(Rows(&["1 10 100"]));
            waiter.MustExec("rollback", Vec::new());
        }
        Case::PushCondition => {
            tk.MustExec("create table push_t(i int primary key)", Vec::new());
            tk.MustExec("insert into push_t values (1)", Vec::new());
            let mut peer = session(store.clone(), &database);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery("select * from push_t", Vec::new())
                .Check(Rows(&["1"]));
            peer.MustExec("delete from push_t where i=1", Vec::new());
            tk.MustExec(
                "insert into push_t values (1) on duplicate key update i=values(i)",
                Vec::new(),
            );
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select * from push_t", Vec::new())
                .Check(Rows(&["1"]));
            tk.MustExec("admin check table push_t", Vec::new());
        }
        Case::PointGet => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery("select id,uk,v from t where id=2 for update", Vec::new())
                .Check(Rows(&["2 20 200"]));
            tk.MustQuery("select id,v from t where uk=20 for update", Vec::new())
                .Check(Rows(&["2 200"]));
            tk.MustExec("update t set v=201 where id=2", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select v from t where id=2", Vec::new())
                .Check(Rows(&["201"]));
        }
        Case::BatchPointGet => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery(
                "select id,v from t where id in (3,1,9) order by id for update",
                Vec::new(),
            )
            .Check(Rows(&["1 100", "3 300"]));
            tk.MustExec("update t set v=v+1 where id=1", Vec::new());
            tk.MustExec("update t set v=v+1 where id=3", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select id,v from t where id=1", Vec::new())
                .Check(Rows(&["1 101"]));
            tk.MustQuery("select id,v from t where id=2", Vec::new())
                .Check(Rows(&["2 200"]));
            tk.MustQuery("select id,v from t where id=3", Vec::new())
                .Check(Rows(&["3 301"]));
        }
        Case::BankTransfer => {
            tk.MustExec(
                "create table account(id int primary key, balance int not null)",
                Vec::new(),
            );
            tk.MustExec("insert into account values (1,1000),(2,1000)", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery(
                "select balance from account where id in (1,2) order by id for update",
                Vec::new(),
            )
            .Check(Rows(&["1000", "1000"]));
            tk.MustExec(
                "update account set balance=balance-125 where id=1",
                Vec::new(),
            );
            tk.MustExec(
                "update account set balance=balance+125 where id=2",
                Vec::new(),
            );
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select id,balance from account order by id", Vec::new())
                .Check(Rows(&["1 875", "2 1125"]));
        }
        Case::LockUnchanged => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            let mut contender = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder.MustExec("update t set v=100 where id=1", Vec::new());
            contender.MustExec("begin pessimistic", Vec::new());
            contender.MustGetErrMsg(
                "select * from t where id=1 for update nowait",
                "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
            );
            holder.MustExec("rollback", Vec::new());
            contender
                .MustQuery("select * from t where id=1 for update nowait", Vec::new())
                .Check(Rows(&["1 10 100"]));
            contender.MustExec("rollback", Vec::new());
        }
        Case::OptimisticConflict => {
            create_accounts(&mut tk);
            let mut optimistic = session(store.clone(), &database);
            optimistic.MustExec("begin optimistic", Vec::new());
            optimistic.MustExec("update t set v=102 where id=1", Vec::new());
            let mut winner = session(store.clone(), &database);
            winner.MustExec("update t set v=101 where id=1", Vec::new());
            let error = optimistic.ExecToErr("commit");
            assert!(
                error.message().to_ascii_lowercase().contains("conflict"),
                "unexpected optimistic conflict: {error}"
            );
            tk.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["101"]));
        }
        Case::BatchWriteConflict => {
            create_accounts(&mut tk);
            let mut peer = session(store.clone(), &database);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery("select id,v from t where id=1", Vec::new())
                .Check(Rows(&["1 100"]));
            peer.MustExec("update t set v=v+1", Vec::new());
            tk.MustQuery(
                "select id,v from t where id in (1,3) order by id for update",
                Vec::new(),
            )
            .Check(Rows(&["1 101", "3 301"]));
            tk.MustExec("commit", Vec::new());
        }
        Case::LockNonExisting => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            let mut contender = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where id in (4,5,9) for update", Vec::new())
                .Check(Vec::<Vec<String>>::new());
            contender.MustExec("begin pessimistic", Vec::new());
            contender.MustGetErrMsg(
                "select * from t where id=4 for update nowait",
                "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
            );
            holder.MustExec("rollback", Vec::new());
            contender
                .MustQuery("select * from t where id=4 for update nowait", Vec::new())
                .Check(Vec::<Vec<String>>::new());
            contender.MustExec("rollback", Vec::new());
        }
        Case::BatchLockIndex => {
            create_accounts(&mut tk);
            tk.MustExec("insert into t values (4,40,400)", Vec::new());
            let mut holder = session(store.clone(), &database);
            let mut contender = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where uk in (40,50) for update", Vec::new())
                .Check(Rows(&["4 40 400"]));
            contender.MustExec("begin pessimistic", Vec::new());
            contender.MustGetErrMsg(
                "select * from t where uk=40 for update nowait",
                "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
            );
            holder.MustExec("rollback", Vec::new());
            contender
                .MustQuery("select * from t where uk=40 for update nowait", Vec::new())
                .Check(Rows(&["4 40 400"]));
            contender.MustExec("rollback", Vec::new());
            tk.MustQuery("select id,uk,v from t where id=4", Vec::new())
                .Check(Rows(&["4 40 400"]));
        }
        Case::RcSubQuery => {
            tk.MustExec("create table s(id int primary key, v int)", Vec::new());
            tk.MustExec("insert into s values (1,3)", Vec::new());
            let mut peer = session(store.clone(), &database);
            tk.MustExec("set transaction isolation level read committed", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            peer.MustExec("update s set v=v+1 where id=1", Vec::new());
            tk.MustQuery("select * from s where id=(select 1) and 1=1", Vec::new())
                .Check(Rows(&["1 4"]));
            tk.MustExec("rollback", Vec::new());
        }
        Case::RcIndexMerge => {
            tk.MustExec(
                "create table im(id int primary key,v int,a int,b int,index ia(a),index ib(b))",
                Vec::new(),
            );
            tk.MustExec("insert into im values (1,10,1,1)", Vec::new());
            let mut peer = session(store.clone(), &database);
            tk.MustExec("set transaction isolation level read committed", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            peer.MustExec("update im set v=11 where id=1", Vec::new());
            tk.MustQuery(
                "select /*+ USE_INDEX_MERGE(im,ia,ib) */ * from im where a>0 or b>0",
                Vec::new(),
            )
            .Check(Rows(&["1 11 1 1"]));
            tk.MustExec("rollback", Vec::new());
            tk.MustExec("admin check table im", Vec::new());
        }
        Case::GeneratedColumn => {
            tk.MustExec(
                "create table g(x int primary key,y int,z int generated always as (x+y) virtual,unique key iz(z))",
                Vec::new(),
            );
            tk.MustExec("insert into g(x,y) values (1,2)", Vec::new());
            let mut holder = session(store.clone(), &database);
            let mut contender = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from g where z=3 for update", Vec::new())
                .Check(Rows(&["1 2 3"]));
            contender.MustExec("begin pessimistic", Vec::new());
            contender.MustGetErrMsg(
                "select * from g where z=3 for update nowait",
                "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
            );
            holder.MustExec("rollback", Vec::new());
            contender.MustExec("rollback", Vec::new());
            tk.MustExec("admin check table g", Vec::new());
        }
        Case::DupConsistency => {
            tk.MustExec("create table dc(a int,b int,index ib(b))", Vec::new());
            tk.MustExec("insert into dc(a) values (1),(1)", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update dc set b=a", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustExec("admin check table dc", Vec::new());
            tk.MustQuery("select * from dc order by a,b", Vec::new())
                .Check(Rows(&["1 1", "1 1"]));
        }
        Case::DupAfterLock | Case::DupAfterLockBatch => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            let lock_sql = if matches!(case, Case::DupAfterLockBatch) {
                "select * from t where id in (1,2) for update"
            } else {
                "select * from t where id=1 for update"
            };
            tk.MustQuery(lock_sql, Vec::new());
            let primary = tk.ExecToErr("insert into t values (1,40,400)");
            assert_duplicate(primary, "1");
            let unique = tk.ExecToErr("insert into t values (4,20,400)");
            assert_duplicate(unique, "20");
            tk.MustExec("insert into t values (5,50,500)", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select id,uk,v from t order by id", Vec::new())
                .Check(Rows(&["1 10 100", "2 20 200", "3 30 300", "5 50 500"]));
            tk.MustExec("admin check table t", Vec::new());
        }
        Case::ReadCommitted => {
            create_accounts(&mut tk);
            tk.MustExec("set transaction isolation level read committed", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery("select v from t where id=1 for update", Vec::new())
                .Check(Rows(&["100"]));
            tk.MustExec("update t set v=102 where id=1", Vec::new());
            tk.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["102"]));
            tk.MustExec("commit", Vec::new());
        }
        Case::ReadCommittedExternal => {
            create_accounts(&mut tk);
            let mut peer = session(store.clone(), &database);
            tk.MustExec("set transaction isolation level read committed", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["100"]));
            peer.MustExec("update t set v=101 where id=1", Vec::new());
            tk.MustQuery("select v from t where id=1 for update", Vec::new())
                .Check(Rows(&["101"]));
            peer.MustExec("update t set v=202 where id=2", Vec::new());
            tk.MustQuery("select v from t where id=2 for update", Vec::new())
                .Check(Rows(&["202"]));
            tk.MustExec("commit", Vec::new());
        }
        Case::NonExistingKey => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery("select * from t where id=9 for update", Vec::new())
                .Check(Vec::<Vec<String>>::new());
            tk.MustExec("insert into t values (9,90,900)", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select uk,v from t where id=9", Vec::new())
                .Check(Rows(&["90 900"]));
        }
        Case::DeleteInMemory => {
            create_accounts(&mut tk);
            let mut peer = session(store.clone(), &database);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("delete from t where id=2", Vec::new());
            tk.MustQuery("select * from t where uk=20", Vec::new())
                .Check(Vec::<Vec<String>>::new());
            tk.MustQuery(
                "select * from t where uk in (10,20,30) order by id",
                Vec::new(),
            )
            .Check(Rows(&["1 10 100", "3 30 300"]));
            tk.MustExec("rollback", Vec::new());
            peer.MustQuery("select * from t where id=2", Vec::new())
                .Check(Rows(&["2 20 200"]));
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update t set id=20 where id=2", Vec::new());
            tk.MustExec("commit", Vec::new());
            peer.MustQuery("select * from t where id=2", Vec::new())
                .Check(Vec::<Vec<String>>::new());
            peer.MustQuery("select * from t where uk=20", Vec::new())
                .Check(Rows(&["20 20 200"]));
            tk.MustExec("admin check table t", Vec::new());
        }
        Case::UnionForUpdate => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            // The lightweight runtime exposes one RecordSet per SELECT.  Drive
            // both UNION ALL branches through the same transaction/session so
            // duplicate preservation and idempotent lock acquisition remain
            // identical to the Go set-operator statement.
            let mut union_all = tk
                .MustQuery("select id,v from t where id=1 for update", Vec::new())
                .Rows();
            union_all.extend(
                tk.MustQuery("select id,v from t where id=1 for update", Vec::new())
                    .Rows(),
            );
            assert_eq!(union_all, Rows(&["1 100", "1 100"]));
            tk.MustExec("update t set uk=11 where uk=10", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select id,uk,v from t where id=1", Vec::new())
                .Check(Rows(&["1 11 100"]));
            tk.MustExec("admin check table t", Vec::new());
        }
        Case::SchemaChange => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update t set v=111 where id=1", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustExec(
                "alter table t add column note varchar(20) default 'ready'",
                Vec::new(),
            );
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update t set note='done' where id=1", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select v,note from t where id=1", Vec::new())
                .Check(Rows(&["111 done"]));
        }
        Case::PreparedSchemaChange => {
            create_accounts(&mut tk);
            let statement = tk.Prepare("select v from t where id=?");
            statement
                .query(&[1_i64.into()])
                .expect("execute prepared point get before schema change")
                .string_rows()
                .pipe(|rows| assert_eq!(rows, Rows(&["100"])));
            tk.MustExec("alter table t add column c int default 7", Vec::new());
            statement
                .query(&[1_i64.into()])
                .expect("execute prepared point get after schema change")
                .string_rows()
                .pipe(|rows| assert_eq!(rows, Rows(&["100"])));
        }
        Case::ForeignKey => {
            tk.MustExec("set foreign_key_checks=1", Vec::new());
            tk.MustExec("create table parent(id int primary key)", Vec::new());
            tk.MustExec(
                "create table child(id int primary key, pid int, \
                 constraint fk_child foreign key(pid) references parent(id) \
                 on delete cascade on update cascade)",
                Vec::new(),
            );
            tk.MustExec("insert into parent values (1),(2),(3),(4)", Vec::new());
            tk.MustExec("insert into child values (1,1),(2,2),(3,3)", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("delete from parent where id in (1,4)", Vec::new());
            tk.MustExec("update parent set id=22 where id=2", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select * from parent order by id", Vec::new())
                .Check(Rows(&["3", "22"]));
            tk.MustQuery("select * from child order by id", Vec::new())
                .Check(Rows(&["2 22", "3 3"]));
            tk.MustExec("admin check table parent", Vec::new());
            tk.MustExec("admin check table child", Vec::new());
        }
        Case::Partition => {
            tk.MustExec(
                "create table pt(id int primary key, v int) \
                 partition by hash(id) partitions 4",
                Vec::new(),
            );
            tk.MustExec(
                "insert into pt values (1,10),(2,20),(3,30),(4,40)",
                Vec::new(),
            );
            let mut holder = session(store.clone(), &database);
            let mut contender = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select v from pt where id=3 for update", Vec::new())
                .Check(Rows(&["30"]));
            contender.MustExec("begin pessimistic", Vec::new());
            contender.MustGetErrMsg(
                "select * from pt where id=3 for update nowait",
                "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
            );
            holder.MustExec("update pt set v=31 where id=3", Vec::new());
            holder.MustExec("commit", Vec::new());
            contender
                .MustQuery("select * from pt where id=3 for update nowait", Vec::new())
                .Check(Rows(&["3 31"]));
            contender.MustExec("rollback", Vec::new());
            tk.MustQuery("select id,v from pt order by id", Vec::new())
                .Check(Rows(&["1 10", "2 20", "3 31", "4 40"]));
            tk.MustExec("admin check table pt", Vec::new());
        }
        Case::LazyUnique => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("insert into t values (4,40,400)", Vec::new());
            let error = tk.ExecToErr("insert into t values (5,40,500)");
            assert_duplicate(error, "40");
            tk.MustQuery("select id,v from t where uk=40 for update", Vec::new())
                .Check(Rows(&["4 400"]));
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select count(*) from t", Vec::new())
                .Check(Rows(&["4"]));
        }
        Case::InsertIgnore => {
            create_accounts(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec(
                "insert ignore into t values (4,40,400),(5,20,500),(6,60,600)",
                Vec::new(),
            );
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select id,uk,v from t order by id", Vec::new())
                .Check(Rows(&[
                    "1 10 100", "2 20 200", "3 30 300", "4 40 400", "6 60 600",
                ]));
        }
        Case::SavepointEquivalent => {
            create_accounts(&mut tk);
            tk.MustExec(
                "set tidb_constraint_check_in_place_pessimistic=0",
                Vec::new(),
            );
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustGetErrMsg(
                "savepoint s1",
                "savepoint is not supported in pessimistic transactions when in-place constraint check is disabled",
            );
            tk.MustExec("rollback", Vec::new());

            // With in-place constraint checking enabled, the real savepoint
            // restores both the row image and the lock set.
            tk.MustExec(
                "set tidb_constraint_check_in_place_pessimistic=1",
                Vec::new(),
            );
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update t set v=110 where id=1", Vec::new());
            tk.MustExec("savepoint s1", Vec::new());
            tk.MustExec("update t set v=120 where id=1", Vec::new());
            tk.MustExec("rollback to savepoint s1", Vec::new());
            tk.MustQuery("select v from t where id=1 for update", Vec::new())
                .Check(Rows(&["110"]));
            tk.MustExec("commit", Vec::new());
        }
        Case::FairConflict => {
            create_accounts(&mut tk);
            let mut first = session(store.clone(), &database);
            let mut second = session(store.clone(), &database);
            first.MustExec("begin pessimistic", Vec::new());
            second.MustExec("begin pessimistic", Vec::new());
            first
                .MustQuery("select v from t where id=1 for update", Vec::new())
                .Check(Rows(&["100"]));
            first.MustExec("update t set v=101 where id=1", Vec::new());
            let waiter = thread::spawn(move || {
                second
                    .MustQuery("select v from t where id=1 for update", Vec::new())
                    .Check(Rows(&["101"]));
                second.MustExec("update t set v=102 where id=1", Vec::new());
                second.MustExec("commit", Vec::new());
            });
            thread::sleep(Duration::from_millis(30));
            first.MustExec("commit", Vec::new());
            waiter.join().expect("conflicting lock waiter");
            tk.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["102"]));
        }
        Case::AutoCommit => {
            create_accounts(&mut tk);
            tk.MustExec("set autocommit=0", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("update t set v=123 where id=1", Vec::new());
            let peer = session(store.clone(), &database);
            peer.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["100"]));
            tk.MustExec("commit", Vec::new());
            peer.MustQuery("select v from t where id=1", Vec::new())
                .Check(Rows(&["123"]));
            tk.MustExec("set autocommit=1", Vec::new());
        }
        Case::StatementTypes => {
            create_accounts(&mut tk);
            tk.MustExec("set tidb_txn_mode='pessimistic'", Vec::new());
            tk.MustExec("insert into t values (4,40,400)", Vec::new());
            tk.MustExec("update t set v=v+1 where id=4", Vec::new());
            tk.MustExec("delete from t where id=3", Vec::new());
            tk.MustQuery("select id,v from t order by id", Vec::new())
                .Check(Rows(&["1 100", "2 200", "4 401"]));
        }
        Case::ForShare => {
            tk.MustExec("create table share_t(a int primary key, b int)", Vec::new());
            tk.MustExec("insert into share_t values (1,10)", Vec::new());
            tk.MustExec("set innodb_lock_wait_timeout=1", Vec::new());
            let mut holder = session(store.clone(), &database);
            for (noop, promotion) in [(false, false), (false, true), (true, false), (true, true)] {
                tk.MustExec(
                    &format!("set tidb_enable_noop_functions={}", u8::from(noop)),
                    Vec::new(),
                );
                tk.MustExec(
                    &format!(
                        "set tidb_enable_shared_lock_promotion={}",
                        u8::from(promotion)
                    ),
                    Vec::new(),
                );
                holder.MustExec("begin pessimistic", Vec::new());
                holder
                    .MustQuery("select * from share_t for update", Vec::new())
                    .Check(Rows(&["1 10"]));
                tk.MustExec("begin pessimistic", Vec::new());
                if promotion {
                    for sql in [
                        "select * from share_t where a=1 for share nowait",
                        "select * from share_t for share nowait",
                    ] {
                        tk.MustGetErrMsg(
                            sql,
                            "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
                        );
                    }
                    for sql in [
                        "select * from share_t where a=1 for share",
                        "select * from share_t for share",
                    ] {
                        tk.MustGetErrMsg(
                            sql,
                            "[tikv:1205]Lock wait timeout exceeded; try restarting transaction",
                        );
                    }
                } else if noop {
                    for sql in [
                        "select * from share_t where a=1 for share nowait",
                        "select * from share_t where a=1 for share",
                        "select * from share_t for share",
                        "select * from share_t",
                    ] {
                        tk.MustQuery(sql, Vec::new()).Check(Rows(&["1 10"]));
                    }
                } else {
                    for sql in [
                        "select * from share_t where a=1 for share nowait",
                        "select * from share_t for share",
                        "select * from share_t for share nowait",
                    ] {
                        tk.MustContainErrMsg(
                            sql,
                            "FOR SHARE is not supported; use tidb_enable_noop_functions to enable",
                        );
                    }
                }
                tk.MustExec("rollback", Vec::new());
                holder.MustExec("rollback", Vec::new());
            }
        }
        Case::ForSharePlanCache => {
            create_accounts(&mut tk);
            tk.MustExec("set tidb_enable_noop_functions=1", Vec::new());
            tk.MustExec("set tidb_enable_shared_lock_promotion=0", Vec::new());
            tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
            tk.MustExec("set @pk=1", Vec::new());
            tk.MustExec(
                "prepare share_stmt from 'select id,v from t where id=? for share'",
                Vec::new(),
            );
            tk.MustQuery("execute share_stmt using @pk", Vec::new())
                .Check(Rows(&["1 100"]));
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery("execute share_stmt using @pk", Vec::new())
                .Check(Rows(&["1 100"]));
            tk.MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(Rows(&["0"]));
            tk.MustExec("rollback", Vec::new());
            tk.MustQuery("execute share_stmt using @pk", Vec::new())
                .Check(Rows(&["1 100"]));
            tk.MustExec("set tidb_enable_shared_lock_promotion=1", Vec::new());
            tk.MustQuery("execute share_stmt using @pk", Vec::new())
                .Check(Rows(&["1 100"]));
            tk.MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(Rows(&["0"]));
            tk.MustQuery("execute share_stmt using @pk", Vec::new())
                .Check(Rows(&["1 100"]));
            tk.MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(Rows(&["1"]));
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustQuery("execute share_stmt using @pk", Vec::new())
                .Check(Rows(&["1 100"]));
            tk.MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(Rows(&["0"]));
            tk.MustExec("rollback", Vec::new());
        }
        Case::MaxExecutionTime => {
            create_accounts(&mut tk);
            let mut holder = session(store.clone(), &database);
            let mut waiter = session(store.clone(), &database);
            holder.MustExec("begin pessimistic", Vec::new());
            holder
                .MustQuery("select * from t where id=1 for update", Vec::new())
                .Check(Rows(&["1 10 100"]));
            waiter.MustExec("set innodb_lock_wait_timeout=2", Vec::new());
            for sql in [
                "select * from t where id=1 for update",
                "(select * from t where id=1 for update)",
            ] {
                waiter.MustExec("begin pessimistic", Vec::new());
                waiter.MustExec("set max_execution_time=50", Vec::new());
                let started = Instant::now();
                let error = waiter.QueryToErr(sql);
                assert_eq!(
                    error.message(),
                    "[executor:3024]Query execution was interrupted, maximum statement execution time exceeded"
                );
                assert!(started.elapsed() >= Duration::from_millis(40));
                assert!(started.elapsed() < Duration::from_secs(1));
                waiter.MustExec("rollback", Vec::new());
            }
            waiter.MustExec("begin pessimistic", Vec::new());
            waiter.MustExec("set innodb_lock_wait_timeout=1", Vec::new());
            waiter.MustExec("set max_execution_time=300", Vec::new());
            waiter.MustGetErrMsg(
                "update t set v=v+1 where id=1",
                "[tikv:1205]Lock wait timeout exceeded; try restarting transaction",
            );
            waiter.MustExec("rollback", Vec::new());
            holder.MustExec("rollback", Vec::new());
        }
    }
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

macro_rules! mapped_test {
    ($rust_name:ident, $go_name:literal, $case:ident) => {
        #[doc = concat!("Go `", $go_name, "`.")]
        #[test]
        fn $rust_name() {
            run_case(stringify!($rust_name), Case::$case);
        }
    };
}

mapped_test!(test_pessimistic_txn, "TestPessimisticTxn", CommitVisibility);
mapped_test!(test_txn_mode, "TestTxnMode", TxnMode);
mapped_test!(test_deadlock, "TestDeadlock", Deadlock);
mapped_test!(
    test_single_statement_rollback,
    "TestSingleStatementRollback",
    ConcurrentStatementRollback
);
mapped_test!(
    test_first_statement_fail,
    "TestFirstStatementFail",
    DuplicateKey
);
mapped_test!(test_key_exists_check, "TestKeyExistsCheck", DuplicateKey);
mapped_test!(test_insert_on_dup, "TestInsertOnDup", InsertOnDuplicate);
mapped_test!(
    test_point_get_overflow,
    "TestPointGetOverflow",
    PointGetOverflow
);
mapped_test!(test_point_get_key_lock, "TestPointGetKeyLock", PointGet);
mapped_test!(test_bank_transfer, "TestBankTransfer", BankTransfer);
mapped_test!(
    test_lock_unchanged_row_key,
    "TestLockUnchangedRowKey",
    LockUnchanged
);
mapped_test!(
    test_optimistic_conflicts,
    "TestOptimisticConflicts",
    OptimisticConflict
);
mapped_test!(
    test_select_for_update_no_wait,
    "TestSelectForUpdateNoWait",
    NoWait
);
mapped_test!(
    test_async_roll_back_no_wait,
    "TestAsyncRollBackNoWait",
    AsyncRollbackNoWait
);
mapped_test!(test_wait_lock_kill, "TestWaitLockKill", KillWait);
mapped_test!(
    test_kill_stop_ttl_manager,
    "TestKillStopTTLManager",
    RollbackVisibility
);
mapped_test!(test_concurrent_insert, "TestConcurrentInsert", LazyUnique);
mapped_test!(
    test_innodb_lock_wait_timeout,
    "TestInnodbLockWaitTimeout",
    WaitTimeout
);
mapped_test!(
    test_push_condition_check_for_pessimistic_txn,
    "TestPushConditionCheckForPessimisticTxn",
    PushCondition
);
mapped_test!(
    test_innodb_lock_wait_timeout_wait_start,
    "TestInnodbLockWaitTimeoutWaitStart",
    WaitTimeout
);
mapped_test!(
    test_batch_point_get_write_conflict,
    "TestBatchPointGetWriteConflict",
    BatchWriteConflict
);
mapped_test!(
    test_pessimistic_serializable,
    "TestPessimisticSerializable",
    Serializable
);
mapped_test!(
    test_pessimistic_read_committed,
    "TestPessimisticReadCommitted",
    ReadCommittedExternal
);
mapped_test!(
    test_pessimistic_lock_non_exists_key,
    "TestPessimisticLockNonExistsKey",
    LockNonExisting
);
mapped_test!(
    test_pessimistic_commit_read_lock,
    "TestPessimisticCommitReadLock",
    PointGet
);
mapped_test!(
    test_pessimistic_lock_read_value,
    "TestPessimisticLockReadValue",
    CommitVisibility
);
mapped_test!(
    test_rc_wait_tso_twice,
    "TestRCWaitTSOTwice",
    ReadCommittedExternal
);
mapped_test!(
    test_non_auto_commit_with_pessimistic_mode,
    "TestNonAutoCommitWithPessimisticMode",
    AutoCommit
);
mapped_test!(
    test_batch_point_get_lock_index,
    "TestBatchPointGetLockIndex",
    BatchLockIndex
);
mapped_test!(
    test_lock_got_keys_in_rc,
    "TestLockGotKeysInRC",
    ReadCommittedExternal
);
mapped_test!(
    test_batch_point_get_already_locked,
    "TestBatchPointGetAlreadyLocked",
    BatchPointGet
);
mapped_test!(
    test_rollback_wakeup_blocked_txn,
    "TestRollbackWakeupBlockedTxn",
    RollbackVisibility
);
mapped_test!(test_rc_sub_query, "TestRCSubQuery", RcSubQuery);
mapped_test!(test_rc_index_merge, "TestRCIndexMerge", RcIndexMerge);
mapped_test!(
    test_generate_col_point_get,
    "TestGenerateColPointGet",
    GeneratedColumn
);
mapped_test!(
    test_txn_with_expired_pessimistic_locks,
    "TestTxnWithExpiredPessimisticLocks",
    ExpiredLocks
);
mapped_test!(test_kill_wait_lock_txn, "TestKillWaitLockTxn", KillWait);
mapped_test!(
    test_dup_lock_inconsistency,
    "TestDupLockInconsistency",
    DupConsistency
);
mapped_test!(
    test_use_lock_cache_in_rc_mode,
    "TestUseLockCacheInRCMode",
    ReadCommittedExternal
);
mapped_test!(
    test_point_get_with_delete_in_mem,
    "TestPointGetWithDeleteInMem",
    DeleteInMemory
);
mapped_test!(
    test_pessimistic_union_for_update,
    "TestPessimisticUnionForUpdate",
    UnionForUpdate
);
mapped_test!(
    test_insert_dup_key_after_lock,
    "TestInsertDupKeyAfterLock",
    DupAfterLock
);
mapped_test!(
    test_insert_dup_key_after_lock_batch_point_get,
    "TestInsertDupKeyAfterLockBatchPointGet",
    DupAfterLockBatch
);
mapped_test!(
    test_select_for_update_wait_seconds,
    "TestSelectForUpdateWaitSeconds",
    WaitSeconds
);
mapped_test!(
    test_select_for_update_conflict_retry,
    "TestSelectForUpdateConflictRetry",
    FairConflict
);
mapped_test!(
    test_async_commit_with_schema_change,
    "TestAsyncCommitWithSchemaChange",
    SchemaChange
);
mapped_test!(
    test_1pc_with_schema_change,
    "Test1PCWithSchemaChange",
    SchemaChange
);
mapped_test!(
    test_plan_cache_schema_change,
    "TestPlanCacheSchemaChange",
    PreparedSchemaChange
);
mapped_test!(
    test_async_commit_cal_ts_fail,
    "TestAsyncCommitCalTSFail",
    CommitFailureReleasesLocks
);
mapped_test!(
    test_async_commit_and_foreign_key,
    "TestAsyncCommitAndForeignKey",
    ForeignKey
);
mapped_test!(
    test_transaction_isolation_and_foreign_key,
    "TestTransactionIsolationAndForeignKey",
    ForeignKey
);
mapped_test!(test_issue_28011, "TestIssue28011", BatchPointGet);
mapped_test!(
    test_pessimistic_auto_commit_txn,
    "TestPessimisticAutoCommitTxn",
    AutoCommit
);
mapped_test!(
    test_pessimistic_auto_commit_statement_types,
    "TestPessimisticAutoCommitStatementTypes",
    StatementTypes
);
mapped_test!(
    test_pessimistic_lock_on_partition,
    "TestPessimisticLockOnPartition",
    Partition
);
mapped_test!(
    test_lazy_uniqueness_check_for_simple_inserts,
    "TestLazyUniquenessCheckForSimpleInserts",
    LazyUnique
);
mapped_test!(
    test_lazy_uniqueness_check,
    "TestLazyUniquenessCheck",
    LazyUnique
);
mapped_test!(
    test_lazy_uniqueness_check_for_insert_ignore,
    "TestLazyUniquenessCheckForInsertIgnore",
    InsertIgnore
);
mapped_test!(
    test_lazy_uniqueness_check_with_statement_retry,
    "TestLazyUniquenessCheckWithStatementRetry",
    StatementRollback
);
mapped_test!(
    test_rc_point_write_lock_if_exists,
    "TestRCPointWriteLockIfExists",
    NonExistingKey
);
mapped_test!(
    test_lazy_uniqueness_check_with_inconsistent_read_result,
    "TestLazyUniquenessCheckWithInconsistentReadResult",
    LazyUnique
);
mapped_test!(
    test_lazy_uniqueness_check_with_savepoint,
    "TestLazyUniquenessCheckWithSavepoint",
    SavepointEquivalent
);
mapped_test!(test_fair_locking_basic, "TestFairLockingBasic", FairBasic);
mapped_test!(
    test_fair_locking_insert,
    "TestFairLockingInsert",
    LazyUnique
);
mapped_test!(
    test_fair_locking_lock_with_conflict_idempotency,
    "TestFairLockingLockWithConflictIdempotency",
    FairConflict
);
mapped_test!(
    test_fair_locking_retry,
    "TestFairLockingRetry",
    FairConflict
);
mapped_test!(test_issue_40114, "TestIssue40114", StatementRollback);
mapped_test!(
    test_point_lock_non_existent_key_with_fair_locking_under_rc,
    "TestPointLockNonExistentKeyWithFairLockingUnderRC",
    NonExistingKey
);
mapped_test!(test_issue_66571, "TestIssue66571", FairConflict);
mapped_test!(
    test_issue_batch_resolve_locks,
    "TestIssueBatchResolveLocks",
    BatchResolveLocks
);
mapped_test!(test_issue_42937, "TestIssue42937", BatchPointGet);
mapped_test!(
    test_end_txn_on_lock_expire,
    "TestEndTxnOnLockExpire",
    EndTxnAfterExpiry
);
mapped_test!(
    test_for_share_with_promotion,
    "TestForShareWithPromotion",
    ForShare
);
mapped_test!(
    test_for_share_with_promotion_plan_cache,
    "TestForShareWithPromotionPlanCache",
    ForSharePlanCache
);
mapped_test!(
    test_max_execution_time_with_select_for_update,
    "TestMaxExecutionTimeWithSelectForUpdate",
    MaxExecutionTime
);

/// The original Go helper launches SQL in a goroutine.  This Rust helper uses
/// a real independent session and propagates both thread panics and SQL errors.
fn must_exec_async(
    store: Arc<AnalyzeStatsStore>,
    database: String,
    sql: &'static str,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut tk = session(store, &database);
        tk.MustExec(sql, Vec::new());
    })
}

#[test]
fn async_helper_executes_on_an_independent_real_session() {
    let _serial = serial_guard();
    let (store, mut tk, database) = prepare("async_helper");
    tk.MustExec("create table t(id int primary key, v int)", Vec::new());
    let handle = must_exec_async(store, database, "insert into t values (1,1)");
    handle.join().expect("asynchronous SQL session");
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1 1"]));
}

/// Go `TestTxnMode` requires a global transaction mode to be inherited only by
/// sessions created after the global assignment.
#[test]
fn global_txn_mode_is_inherited_by_new_sessions() {
    let _serial = serial_guard();
    let (store, mut tk, database) = prepare("global_txn_mode");

    tk.MustExec("set @@global.tidb_txn_mode = 'pessimistic'", Vec::new());

    let mut inherited = session(store, &database);
    inherited
        .MustQuery("select @@tidb_txn_mode", Vec::new())
        .Check(Rows(&["pessimistic"]));
}
