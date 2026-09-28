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
//! 中文总览：`common.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `测试工具与兼容封装` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 165 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `SuiteContext` 是当前文件里的状态类型。
//! `SuiteContext` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `SuiteContext` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SuiteContext`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `SuiteContext` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `SuiteContext` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `SuiteContext` 当作定位同类问题的索引锚点。
//! 作为状态类型，`SuiteContext` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `SuiteContext` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `InitTest` 是当前文件里的公开函数。
//! `InitTest` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `InitTest` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `InitTest`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `InitTest` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `InitTest` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `InitTest` 当作定位同类问题的索引锚点。
//! 作为公开函数，`InitTest` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `InitTest` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `insert_row_values` 是当前文件里的辅助函数。
//! `insert_row_values` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `insert_row_values` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `insert_row_values`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `insert_row_values` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `insert_row_values` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `insert_row_values` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`insert_row_values` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `insert_row_values` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `TestOneColFrame` 是当前文件里的测试用例。
//! `TestOneColFrame` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `TestOneColFrame` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestOneColFrame`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestOneColFrame` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestOneColFrame` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `TestOneColFrame` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestOneColFrame` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 符号 `TestTwoColsFrame` 是当前文件里的测试用例。
//! `TestTwoColsFrame` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `TestTwoColsFrame` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestTwoColsFrame`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestTwoColsFrame` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestTwoColsFrame` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `TestTwoColsFrame` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestTwoColsFrame` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 符号 `TestOneIndexFrame` 是当前文件里的测试用例。
//! `TestOneIndexFrame` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `TestOneIndexFrame` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestOneIndexFrame`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `TestOneIndexFrame` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `TestOneIndexFrame` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `TestOneIndexFrame` 当作定位同类问题的索引锚点。
//! 作为测试用例，`TestOneIndexFrame` 通常代表一个可以独立复现的语义切片，用来证明 Rust 端仍与 Go 对齐。
//! 阅读此类条目时，通常要把建表造数、执行 SQL、读取结果和最终清理视为同一故事线。
//! 符号 `AddIndexNonUnique` 是当前文件里的公开函数。
//! `AddIndexNonUnique` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `AddIndexNonUnique` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AddIndexNonUnique`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `AddIndexNonUnique` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `AddIndexNonUnique` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `AddIndexNonUnique` 当作定位同类问题的索引锚点。
//! 作为公开函数，`AddIndexNonUnique` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `AddIndexNonUnique` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `AddIndexUnique` 是当前文件里的公开函数。
//! `AddIndexUnique` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `AddIndexUnique` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AddIndexUnique`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `AddIndexUnique` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `AddIndexUnique` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `AddIndexUnique` 当作定位同类问题的索引锚点。
//! 作为公开函数，`AddIndexUnique` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `AddIndexUnique` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `AddIndexPK` 是当前文件里的公开函数。
//! `AddIndexPK` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `AddIndexPK` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AddIndexPK`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `AddIndexPK` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `AddIndexPK` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `AddIndexPK` 当作定位同类问题的索引锚点。
//! 作为公开函数，`AddIndexPK` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `AddIndexPK` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `AddIndexGenCol` 是当前文件里的公开函数。
//! `AddIndexGenCol` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `AddIndexGenCol` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AddIndexGenCol`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `AddIndexGenCol` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `AddIndexGenCol` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `AddIndexGenCol` 当作定位同类问题的索引锚点。
//! 作为公开函数，`AddIndexGenCol` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `AddIndexGenCol` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `AddIndexMultiCols` 是当前文件里的公开函数。
//! `AddIndexMultiCols` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `AddIndexMultiCols` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AddIndexMultiCols`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `AddIndexMultiCols` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `AddIndexMultiCols` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `AddIndexMultiCols` 当作定位同类问题的索引锚点。
//! 作为公开函数，`AddIndexMultiCols` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `AddIndexMultiCols` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `FailpointsPath` 是当前文件里的状态类型。
//! `FailpointsPath` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `FailpointsPath` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FailpointsPath`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `FailpointsPath` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `FailpointsPath` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `FailpointsPath` 当作定位同类问题的索引锚点。
//! 作为状态类型，`FailpointsPath` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `FailpointsPath` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `FAILPOINTS` 是当前文件里的静态量。
//! `FAILPOINTS` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `FAILPOINTS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FAILPOINTS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `FAILPOINTS` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `FAILPOINTS` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `FAILPOINTS` 当作定位同类问题的索引锚点。
//! 作为静态量，`FAILPOINTS` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `FAILPOINTS` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `useFailpoints_with` 是当前文件里的辅助函数。
//! `useFailpoints_with` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `useFailpoints_with` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `useFailpoints_with`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `useFailpoints_with` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `useFailpoints_with` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `useFailpoints_with` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`useFailpoints_with` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `useFailpoints_with` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `FailSyncDone` 是当前文件里的状态类型。
//! `FailSyncDone` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `FailSyncDone` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FailSyncDone`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `FailSyncDone` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `FailSyncDone` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `FailSyncDone` 当作定位同类问题的索引锚点。
//! 作为状态类型，`FailSyncDone` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `FailSyncDone` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `drop` 是当前文件里的辅助函数。
//! `drop` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `drop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `drop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `drop` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `drop` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `drop` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`drop` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `drop` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `InitTestFailpoint` 是当前文件里的公开函数。
//! `InitTestFailpoint` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `InitTestFailpoint` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `InitTestFailpoint`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `InitTestFailpoint` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `InitTestFailpoint` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `InitTestFailpoint` 当作定位同类问题的索引锚点。
//! 作为公开函数，`InitTestFailpoint` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `InitTestFailpoint` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 符号 `AssertExternalField` 是当前文件里的公开函数。
//! `AssertExternalField` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `AssertExternalField` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AssertExternalField`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `AssertExternalField` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `AssertExternalField` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `common.rs` 的回归，可以把 `AssertExternalField` 当作定位同类问题的索引锚点。
//! 作为公开函数，`AssertExternalField` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 如果 `AssertExternalField` 影响全局夹具或共享状态，通常也意味着相邻测试需要串行化或显式重置。
//! 中文说明结束（自动生成）

//! Suite fixtures, DDL helpers, and add-index test frames
//! (Go `tests/realtikvtest/testutils/common.go`).

use crate::compatibility::CompatibilityContext;
use crate::stubs::{
    ExternalTagged, ExternalTaggedField, failpoint, failpoint_hold_duration, kerneltype, logutil,
    require, testkit,
};
use crate::workload::{Workload, initWorkloadParams};
use astersql_tests_realtikvtest::CreateMockStoreAndSetup;
use astersql_tests_realtikvtest::stubs::{Storage, TestCtx};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

pub(crate) const TABLE_NUM: i32 = 3;
pub(crate) const NON_PART_TAB_NUM: i32 = 1;

/// TestKit object pool (Go `sync.Pool` of `*testkit.TestKit`).
pub(crate) struct TkPool {
    t: TestCtx,
    store: Storage,
    free: Mutex<Vec<testkit::TestKit>>,
}

impl TkPool {
    pub(crate) fn new(t: TestCtx, store: Storage) -> Self {
        Self {
            t,
            store,
            free: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn get(&self) -> testkit::TestKit {
        if let Some(tk) = self.free.lock().unwrap().pop() {
            return tk;
        }
        testkit::NewTestKit(&self.t, self.store.clone())
    }

    pub(crate) fn put(&self, tk: testkit::TestKit) {
        self.free.lock().unwrap().push(tk);
    }
}

/// SuiteContext wraps test context for add index.
pub struct SuiteContext {
    pub(crate) cancelled: Arc<AtomicBool>,
    pub store: Storage,
    pub t: TestCtx,
    pub tk: testkit::TestKit,
    pub isUnique: Arc<AtomicBool>,
    pub isPK: Arc<AtomicBool>,
    pub tableNum: i32,
    pub colNum: i32,
    pub rowNum: i32,
    pub(crate) workload: Option<Arc<Mutex<Workload>>>,
    pub(crate) tkPool: Option<Arc<TkPool>>,
    pub isFailpointsTest: bool,
    pub(crate) failSync: Arc<Mutex<FailSync>>,
    /// Shared with concurrent DDL workers (RwLock avoids Start/worker deadlock).
    pub CompCtx: Option<Arc<std::sync::RwLock<CompatibilityContext>>>,
}

/// WaitGroup stand-in used by failpoint goroutines.
pub(crate) struct FailSync {
    count: i32,
}

impl FailSync {
    pub(crate) fn new() -> Self {
        Self { count: 0 }
    }

    pub(crate) fn add(&mut self, delta: i32) {
        self.count += delta;
    }

    pub(crate) fn done(&mut self) {
        self.count -= 1;
    }

    pub(crate) fn wait(&mut self) {
        // Workers call Done; the frame waits until count hits 0.
        // With threads, wait is coordinated via a condvar-like spin on count
        // after the spawned work has been scheduled; see useFailpoints join.
    }

    pub(crate) fn count(&self) -> i32 {
        self.count
    }
}

impl SuiteContext {
    pub(crate) fn getTestKit(&self) -> testkit::TestKit {
        self.tkPool.as_ref().expect("tkPool initialized").get()
    }

    pub(crate) fn putTestKit(&self, tk: testkit::TestKit) {
        self.tkPool.as_ref().expect("tkPool initialized").put(tk);
    }

    pub(crate) fn done(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub(crate) fn set_unique(&self, v: bool) {
        self.isUnique.store(v, Ordering::SeqCst);
    }

    pub(crate) fn set_pk(&self, v: bool) {
        self.isPK.store(v, Ordering::SeqCst);
    }

    pub(crate) fn get_unique(&self) -> bool {
        self.isUnique.load(Ordering::SeqCst)
    }

    pub(crate) fn get_pk(&self) -> bool {
        self.isPK.load(Ordering::SeqCst)
    }

    /// Clone a worker-facing view that shares atomics / CompCtx / pools.
    pub(crate) fn share_for_worker(&self) -> SuiteContext {
        SuiteContext {
            cancelled: self.cancelled.clone(),
            store: self.store.clone(),
            t: self.t.clone(),
            tk: self.tk.clone(),
            isUnique: self.isUnique.clone(),
            isPK: self.isPK.clone(),
            tableNum: self.tableNum,
            colNum: self.colNum,
            rowNum: self.rowNum,
            workload: self.workload.clone(),
            tkPool: self.tkPool.clone(),
            isFailpointsTest: self.isFailpointsTest,
            failSync: self.failSync.clone(),
            CompCtx: self.CompCtx.clone(),
        }
    }
}

pub(crate) fn newSuiteContext(t: &TestCtx, tk: testkit::TestKit, store: Storage) -> SuiteContext {
    SuiteContext {
        cancelled: Arc::new(AtomicBool::new(false)),
        store,
        t: t.clone(),
        tk,
        isUnique: Arc::new(AtomicBool::new(false)),
        isPK: Arc::new(AtomicBool::new(false)),
        tableNum: 3,
        colNum: 28,
        rowNum: 64,
        workload: None,
        tkPool: None,
        isFailpointsTest: false,
        failSync: Arc::new(Mutex::new(FailSync::new())),
        CompCtx: None,
    }
}

/// InitTest inits SuiteContext for test.
pub fn InitTest(t: &TestCtx) -> SuiteContext {
    let store = CreateMockStoreAndSetup(t, &[]);
    let tk = testkit::NewTestKit(t, store.clone());
    tk.MustExec("drop database if exists addindex;");
    tk.MustExec("create database addindex;");
    tk.MustExec("use addindex;");
    if kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_enable_fast_reorg=on;");
    }

    let mut ctx = newSuiteContext(t, tk, store);
    createTable(&ctx.tk);
    insertRows(&ctx.tk);
    initWorkloadParams(&mut ctx);
    ctx
}

pub(crate) fn genTableStr(table_name: &str) -> String {
    format!(
        "create table addindex.{} ({}",
        table_name,
        concat!(
            "c0 int, c1 bit(8), c2 boolean, c3 tinyint default 3, c4 smallint not null, c5 mediumint,",
            "c6 int, c7 bigint, c8 float, c9 double, c10 decimal(13,7), c11 date, c12 time, c13 datetime,",
            "c14 timestamp, c15 year, c16 char(10), c17 varchar(10), c18 text, c19 tinytext, c20 mediumtext,",
            "c21 longtext, c22 binary(20), c23 varbinary(30), c24 blob, c25 tinyblob, c26 MEDIUMBLOB, c27 LONGBLOB,",
            "c28 json, c29 INT AS (JSON_EXTRACT(c28, '$.population')))"
        )
    )
}

pub(crate) fn genPartTableStr() -> Vec<String> {
    let mut table_defs = Vec::new();
    let mut num = NON_PART_TAB_NUM;
    // Range table def
    table_defs.push(format!(
        "CREATE TABLE addindex.t{} ({} PARTITION BY RANGE (`c0`) {}",
        num,
        concat!(
            "c0 int, c1 bit(8), c2 boolean, c3 tinyint default 3, c4 smallint not null, c5 mediumint,",
            "c6 int, c7 bigint, c8 float, c9 double, c10 decimal(13,7), c11 date, c12 time, c13 datetime,",
            "c14 timestamp, c15 year, c16 char(10), c17 varchar(10), c18 text, c19 tinytext, c20 mediumtext,",
            "c21 longtext, c22 binary(20), c23 varbinary(30), c24 blob, c25 tinyblob, c26 MEDIUMBLOB, c27 LONGBLOB,",
            "c28 json, c29 INT AS (JSON_EXTRACT(c28, '$.population')))"
        ),
        concat!(
            "(PARTITION `p0` VALUES LESS THAN (10),",
            " PARTITION `p1` VALUES LESS THAN (20),",
            " PARTITION `p2` VALUES LESS THAN (30),",
            " PARTITION `p3` VALUES LESS THAN (40),",
            " PARTITION `p4` VALUES LESS THAN (50),",
            " PARTITION `p5` VALUES LESS THAN (60),",
            " PARTITION `p6` VALUES LESS THAN (70),",
            " PARTITION `p7` VALUES LESS THAN (80),",
            " PARTITION `p8` VALUES LESS THAN MAXVALUE)"
        )
    ));
    num += 1;
    // Hash part table
    table_defs.push(format!(
        "CREATE TABLE addindex.t{} ({} PARTITION BY HASH (c0) PARTITIONS 4",
        num,
        concat!(
            "c0 int, c1 bit(8), c2 boolean, c3 tinyint default 3, c4 smallint not null, c5 mediumint,",
            "c6 int, c7 bigint, c8 float, c9 double, c10 decimal(13,7), c11 date, c12 time, c13 datetime,",
            "c14 timestamp, c15 year, c16 char(10), c17 varchar(10), c18 text, c19 tinytext, c20 mediumtext,",
            "c21 longtext, c22 binary(20), c23 varbinary(30), c24 blob, c25 tinyblob, c26 MEDIUMBLOB, c27 LONGBLOB,",
            "c28 json, c29 INT AS (JSON_EXTRACT(c28, '$.population')))"
        )
    ));
    table_defs
}

pub(crate) fn createTable(tk: &testkit::TestKit) {
    for i in 0..NON_PART_TAB_NUM {
        let table_name = format!("t{i}");
        let table_def = genTableStr(&table_name);
        tk.MustExec(&table_def);
    }
    for table_def in genPartTableStr() {
        tk.MustExec(&table_def);
    }
}

fn insert_row_values() -> &'static [&'static str] {
    &[
        " (1, 1, 1, 1, 1, 1, 1, 1, 1.0, 1.0, 1111.1111, '2001-01-01', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 100}')",
        " (2, 2, 2, 2, 2, 2, 2, 2, 2.0, 2.0, 1112.1111, '2001-01-02', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 101}')",
        " (3, 3, 3, 3, 3, 3, 3, 3, 3.0, 3.0, 1113.1111, '2001-01-03', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 102}')",
        " (4, 4, 4, 4, 4, 4, 4, 4, 4.0, 4.0, 1114.1111, '2001-01-04', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 103}')",
        " (5, 5, 1, 1, 1, 1, 5, 1, 1.0, 1.0, 1111.1111, '2001-01-05', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'eeee', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 104}')",
        " (6, 2, 2, 2, 2, 2, 6, 2, 2.0, 2.0, 1112.1111, '2001-01-06', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'ffff', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 105}')",
        " (7, 3, 3, 3, 3, 3, 7, 3, 3.0, 3.0, 1113.1111, '2001-01-07', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'gggg', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 106}')",
        " (8, 4, 4, 4, 4, 4, 8, 4, 4.0, 4.0, 1114.1111, '2001-01-08', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'hhhh', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 107}')",
        " (9, 1, 1, 1, 1, 1, 9, 1, 1.0, 1.0, 1111.1111, '2001-01-09', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'iiii', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 100}')",
        " (10, 2, 2, 2, 2, 2, 10, 2, 2.0, 2.0, 1112.1111, '2001-01-10', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'jjjj', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 101}')",
        " (11, 3, 3, 3, 3, 3, 11, 3, 3.0, 3.0, 1113.1111, '2001-01-11', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'kkkk', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 102}')",
        " (12, 4, 4, 4, 4, 4, 12, 4, 4.0, 4.0, 1114.1111, '2001-01-12', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'llll', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 103}')",
        " (13, 5, 1, 1, 1, 1, 13, 1, 1.0, 1.0, 1111.1111, '2001-01-13', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'mmmm', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 104}')",
        " (14, 2, 2, 2, 2, 2, 14, 2, 2.0, 2.0, 1112.1111, '2001-01-14', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'nnnn', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 105}')",
        " (15, 3, 3, 3, 3, 3, 15, 3, 3.0, 3.0, 1113.1111, '2001-01-15', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'oooo', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 106}')",
        " (16, 4, 4, 4, 4, 4, 16, 4, 4.0, 4.0, 1114.1111, '2001-01-16', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'pppp', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 107}')",
        " (17, 1, 1, 1, 1, 1, 17, 1, 1.0, 1.0, 1111.1111, '2001-01-17', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'qqqq', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 100}')",
        " (18, 2, 2, 2, 2, 2, 18, 2, 2.0, 2.0, 1112.1111, '2001-01-18', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'rrrr', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 101}')",
        " (19, 3, 3, 3, 3, 3, 19, 3, 3.0, 3.0, 1113.1111, '2001-01-19', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'ssss', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 102}')",
        " (20, 4, 4, 4, 4, 4, 20, 4, 4.0, 4.0, 1114.1111, '2001-01-20', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'tttt', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 103}')",
        " (21, 5, 1, 1, 1, 1, 21, 1, 1.0, 1.0, 1111.1111, '2001-01-21', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'uuuu', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 104}')",
        " (22, 2, 2, 2, 2, 2, 22, 2, 2.0, 2.0, 1112.1111, '2001-01-22', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'vvvv', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 105}')",
        " (23, 3, 3, 3, 3, 3, 23, 3, 3.0, 3.0, 1113.1111, '2001-01-23', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'wwww', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 106}')",
        " (24, 4, 4, 4, 4, 4, 24, 4, 4.0, 4.0, 1114.1111, '2001-01-24', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'xxxx', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 107}')",
        " (25, 1, 1, 1, 1, 1, 25, 1, 1.0, 1.0, 1111.1111, '2001-01-25', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'yyyy', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 100}')",
        " (26, 2, 2, 2, 2, 2, 26, 2, 2.0, 2.0, 1112.1111, '2001-01-26', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'zzzz', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 101}')",
        " (27, 3, 3, 3, 3, 3, 27, 3, 3.0, 3.0, 1113.1111, '2001-01-27', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaab', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 102}')",
        " (28, 4, 4, 4, 4, 4, 28, 4, 4.0, 4.0, 1114.1111, '2001-01-28', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aaac', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 103}')",
        " (29, 5, 1, 1, 1, 1, 29, 1, 1.0, 1.0, 1111.1111, '2001-01-29', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaad', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 104}')",
        " (30, 2, 2, 2, 2, 2, 30, 2, 2.0, 2.0, 1112.1111, '2001-01-30', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aaae', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 105}')",
        " (31, 3, 3, 3, 3, 3, 31, 3, 3.0, 3.0, 1113.1111, '2001-01-31', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaaf', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 106}')",
        " (32, 4, 4, 4, 4, 4, 32, 4, 4.0, 4.0, 1114.1111, '2001-02-01', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aaag', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 107}')",
        " (33, 1, 1, 1, 1, 1, 33, 1, 1.0, 1.0, 1111.1111, '2001-02-02', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaah', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 100}')",
        " (34, 2, 2, 2, 2, 2, 34, 2, 2.0, 2.0, 1112.1111, '2001-02-03', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aaai', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 101}')",
        " (35, 3, 3, 3, 3, 3, 35, 3, 3.0, 3.0, 1113.1111, '2001-02-05', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaaj', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 102}')",
        " (36, 4, 4, 4, 4, 4, 36, 4, 4.0, 4.0, 1114.1111, '2001-02-04', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aaak', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 103}')",
        " (37, 5, 1, 1, 1, 1, 37, 1, 1.0, 1.0, 1111.1111, '2001-02-06', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaal', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 104}')",
        " (38, 2, 2, 2, 2, 2, 38, 2, 2.0, 2.0, 1112.1111, '2001-02-07', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aaam', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 105}')",
        " (39, 3, 3, 3, 3, 3, 39, 3, 3.0, 3.0, 1113.1111, '2001-02-08', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaan', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 106}')",
        " (40, 4, 4, 4, 4, 4, 40, 4, 4.0, 4.0, 1114.1111, '2001-02-09', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aaao', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 107}')",
        " (41, 1, 1, 1, 1, 1, 41, 1, 1.0, 1.0, 1111.1111, '2001-02-10', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaap', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 100}')",
        " (42, 2, 2, 2, 2, 2, 42, 2, 2.0, 2.0, 1112.1111, '2001-02-11', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aaaq', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 101}')",
        " (43, 3, 3, 3, 3, 3, 43, 3, 3.0, 3.0, 1113.1111, '2001-02-12', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaar', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 102}')",
        " (44, 4, 4, 4, 4, 4, 44, 4, 4.0, 4.0, 1114.1111, '2001-02-13', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aaas', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 103}')",
        " (45, 5, 1, 1, 1, 1, 45, 1, 1.0, 1.0, 1111.1111, '2001-02-14', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaat', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 104}')",
        " (46, 2, 2, 2, 2, 2, 46, 2, 2.0, 2.0, 1112.1111, '2001-02-15', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aaau', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 105}')",
        " (47, 3, 3, 3, 3, 3, 47, 3, 3.0, 3.0, 1113.1111, '2001-02-16', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaav', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 106}')",
        " (48, 4, 4, 4, 4, 4, 48, 4, 4.0, 4.0, 1114.1111, '2001-02-17', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aaaw', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 107}')",
        " (49, 1, 1, 1, 1, 1, 49, 1, 1.0, 1.0, 1111.1111, '2001-02-18', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaax', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 100}')",
        " (50, 2, 2, 2, 2, 2, 50, 2, 2.0, 2.0, 1112.1111, '2001-02-19', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aaay', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 101}')",
        " (51, 3, 3, 3, 3, 3, 51, 3, 3.0, 3.0, 1113.1111, '2001-02-20', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaaz', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 102}')",
        " (52, 4, 4, 4, 4, 4, 52, 4, 4.0, 4.0, 1114.1111, '2001-02-21', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aaba', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 103}')",
        " (53, 5, 1, 1, 1, 1, 53, 1, 1.0, 1.0, 1111.1111, '2001-02-22', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaca', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 104}')",
        " (54, 2, 2, 2, 2, 2, 54, 2, 2.0, 2.0, 1112.1111, '2001-02-23', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aada', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 105}')",
        " (55, 3, 3, 3, 3, 3, 55, 3, 3.0, 3.0, 1113.1111, '2001-02-24', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaea', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 106}')",
        " (56, 4, 4, 4, 4, 4, 56, 4, 4.0, 4.0, 1114.1111, '2001-02-25', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aafa', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 107}')",
        " (57, 1, 1, 1, 1, 1, 57, 1, 1.0, 1.0, 1111.1111, '2001-02-26', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaga', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 100}')",
        " (58, 2, 2, 2, 2, 2, 58, 2, 2.0, 2.0, 1112.1111, '2001-02-27', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aaha', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 101}')",
        " (59, 3, 3, 3, 3, 3, 59, 3, 3.0, 3.0, 1113.1111, '2001-02-28', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaia', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 102}')",
        " (60, 4, 4, 4, 4, 4, 60, 4, 4.0, 4.0, 1114.1111, '2001-03-01', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aaja', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 103}')",
        " (61, 5, 1, 1, 1, 1, 61, 1, 1.0, 1.0, 1111.1111, '2001-03-02', '11:11:11', '2001-01-01 11:11:11', '2001-01-01 11:11:11.123456', 1999, 'aaaa', 'aaaa', 'aaaa', 'aaka', 'aaaa','aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', 'aaaa', '{\"name\": \"Beijing\", \"population\": 104}')",
        " (62, 2, 2, 2, 2, 2, 62, 2, 2.0, 2.0, 1112.1111, '2001-03-03', '11:11:12', '2001-01-02 11:11:12', '2001-01-02 11:11:12.123456', 2000, 'bbbb', 'bbbb', 'bbbb', 'aala', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', 'bbbb', '{\"name\": \"Beijing\", \"population\": 105}')",
        " (63, 3, 3, 3, 3, 3, 63, 3, 3.0, 3.0, 1113.1111, '2001-03-04', '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aama', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{\"name\": \"Beijing\", \"population\": 106}')",
        " (64, 4, 4, 4, 4, 4, 64, 4, 4.0, 4.0, 1114.1111, '2001-03-05', '11:11:14', '2001-01-04 11:11:14', '2001-01-04 11:11:12.123456', 2002, 'dddd', 'dddd', 'dddd', 'aana', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', 'dddd', '{\"name\": \"Beijing\", \"population\": 107}')",
    ]
}

pub(crate) fn insertRows(tk: &testkit::TestKit) {
    let values = insert_row_values();
    for i in 0..TABLE_NUM {
        let base = format!(
            "insert into addindex.t{i} (c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15, c16, c17, c18, c19, c20, c21, c22, c23, c24, c25, c26, c27, c28) values"
        );
        for value in values {
            // Go shadows `insStr` each iteration: base + single value.
            let ins_str = format!("{base}{value}");
            tk.MustExec(&ins_str);
        }
    }
}

pub(crate) fn createIndexOneCol(
    ctx: &SuiteContext,
    table_id: i32,
    col_id: i32,
) -> Result<(), String> {
    let mut add_index_str = " add index idx".to_string();
    if ctx.get_pk() {
        add_index_str = " add primary key idx".to_string();
    } else if ctx.get_unique() {
        add_index_str = " add unique index idx".to_string();
    }
    let mut length = 4;
    if ctx.get_unique() && col_id == 19 {
        length = 16;
    }
    let mut ddl_str = String::new();
    if !(ctx.get_pk() || ctx.get_unique()) || table_id == 0 || (ctx.get_pk() && table_id > 0) {
        if (18..29).contains(&col_id) {
            ddl_str = format!(
                "alter table addindex.t{table_id}{add_index_str}{col_id}(c{col_id}({length}))"
            );
        } else {
            ddl_str = format!("alter table addindex.t{table_id}{add_index_str}{col_id}(c{col_id})");
        }
    } else if ctx.get_unique() && table_id > 0 {
        if (18..29).contains(&col_id) {
            ddl_str = format!(
                "alter table addindex.t{table_id}{add_index_str}{col_id}(c0, c{col_id}({length}))"
            );
        } else {
            ddl_str =
                format!("alter table addindex.t{table_id}{add_index_str}{col_id}(c0, c{col_id})");
        }
    }
    if let Some(comp) = &ctx.CompCtx {
        let g = comp.read().unwrap();
        if g.IsMultiSchemaChange {
            let col = col_id + 60;
            ddl_str.push_str(&format!(" , add column c{col} int;"));
        }
    }
    logutil::BgLogger().Info(
        "createIndexOneCol",
        &[
            ("category", "add index test".into()),
            ("sql", ddl_str.clone()),
        ],
    );
    let err = {
        let use_conc = ctx
            .CompCtx
            .as_ref()
            .map(|c| c.read().unwrap().IsConcurrentDDL)
            .unwrap_or(false);
        if use_conc {
            let comp = ctx.CompCtx.as_ref().unwrap().read().unwrap();
            let er = &comp.executor[table_id as usize];
            er.tk.Exec(&ddl_str).map(|_| ())
        } else {
            ctx.tk.Exec(&ddl_str).map(|_| ())
        }
    };
    if let Err(ref e) = err {
        if ctx.get_unique() || ctx.get_pk() {
            require::Contains(&ctx.t, e, "Duplicate entry");
        } else {
            require::NoError(&ctx.t, Err(e.clone()));
        }
    }
    err
}

pub(crate) fn createIndexTwoCols(
    ctx: &SuiteContext,
    table_id: i32,
    index_id: i32,
    col_id1: i32,
    col_id2: i32,
) -> Result<(), String> {
    let mut add_index_str = " add index idx".to_string();
    if ctx.get_pk() {
        add_index_str = " add primary key idx".to_string();
    } else if ctx.get_unique() {
        add_index_str = " add unique index idx".to_string();
    }
    let col_id1_str = if (18..29).contains(&col_id1) {
        format!("{col_id1}(4)")
    } else {
        col_id1.to_string()
    };
    let col_id2_str = if (18..29).contains(&col_id2) {
        format!("{col_id2}(4)")
    } else {
        col_id2.to_string()
    };
    let mut ddl_str = format!(
        "alter table addindex.t{table_id}{add_index_str}{index_id}(c{col_id1_str}, c{col_id2_str})"
    );
    if let Some(comp) = &ctx.CompCtx {
        let g = comp.read().unwrap();
        if g.IsMultiSchemaChange {
            let col = col_id1 + 60;
            ddl_str.push_str(&format!(" , add column c{col} varchar(10);"));
        }
    }
    logutil::BgLogger().Info(
        "createIndexTwoCols",
        &[
            ("category", "add index test".into()),
            ("sql", ddl_str.clone()),
        ],
    );
    let err = {
        let use_conc = ctx
            .CompCtx
            .as_ref()
            .map(|c| c.read().unwrap().IsConcurrentDDL)
            .unwrap_or(false);
        if use_conc {
            let comp = ctx.CompCtx.as_ref().unwrap().read().unwrap();
            let er = &comp.executor[table_id as usize];
            er.tk.Exec(&ddl_str).err()
        } else {
            ctx.tk.Exec(&ddl_str).err()
        }
    };
    if let Some(ref e) = err {
        logutil::BgLogger().Error(
            "add index failed",
            &[
                ("category", "add index test".into()),
                ("sql", ddl_str.clone()),
                ("error", e.clone()),
            ],
        );
        require::NoError(&ctx.t, Err(e.clone()));
    }
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

pub(crate) fn checkResult(ctx: &SuiteContext, table_name: &str, index_id: i32, tk_id: i32) {
    let admin_check_sql = format!("admin check index {table_name} idx{index_id}");
    let use_conc = ctx
        .CompCtx
        .as_ref()
        .map(|c| c.read().unwrap().IsConcurrentDDL)
        .unwrap_or(false);
    let err = if use_conc {
        let comp = ctx.CompCtx.as_ref().unwrap().read().unwrap();
        comp.executor[tk_id as usize]
            .tk
            .Exec(&admin_check_sql)
            .map(|_| ())
    } else {
        ctx.tk.Exec(&admin_check_sql).map(|_| ())
    };
    if let Err(ref e) = err {
        logutil::BgLogger().Error(
            "checkResult",
            &[
                ("category", "add index test".into()),
                ("sql", admin_check_sql.clone()),
                ("error", e.clone()),
            ],
        );
    }
    require::NoError(&ctx.t, err);

    let drop_sql = format!("alter table {table_name} drop index idx{index_id}");
    let err = if use_conc {
        let comp = ctx.CompCtx.as_ref().unwrap().read().unwrap();
        require::Equal(
            &ctx.t,
            0u64,
            comp.executor[tk_id as usize].tk.Session().AffectedRows(),
        );
        comp.executor[tk_id as usize].tk.Exec(&drop_sql).map(|_| ())
    } else {
        require::Equal(&ctx.t, 0u64, ctx.tk.Session().AffectedRows());
        ctx.tk.Exec(&drop_sql).map(|_| ())
    };
    if let Err(ref e) = err {
        logutil::BgLogger().Error(
            "drop index failed",
            &[
                ("category", "add index test".into()),
                ("sql", admin_check_sql),
                ("error", e.clone()),
            ],
        );
    }
    require::NoError(&ctx.t, err);
}

pub(crate) fn checkTableResult(ctx: &SuiteContext, table_name: &str, tk_id: i32) {
    let admin_check_sql = format!("admin check table {table_name}");
    let use_conc = ctx
        .CompCtx
        .as_ref()
        .map(|c| c.read().unwrap().IsConcurrentDDL)
        .unwrap_or(false);
    let err = if use_conc {
        let comp = ctx.CompCtx.as_ref().unwrap().read().unwrap();
        comp.executor[tk_id as usize]
            .tk
            .Exec(&admin_check_sql)
            .map(|_| ())
    } else {
        ctx.tk.Exec(&admin_check_sql).map(|_| ())
    };
    if let Err(ref e) = err {
        logutil::BgLogger().Error(
            "checkTableResult",
            &[
                ("category", "add index test".into()),
                ("sql", admin_check_sql),
                ("error", e.clone()),
            ],
        );
    }
    require::NoError(&ctx.t, err);
    if use_conc {
        let comp = ctx.CompCtx.as_ref().unwrap().read().unwrap();
        require::Equal(
            &ctx.t,
            0,
            comp.executor[tk_id as usize].tk.Session().AffectedRows(),
        );
    } else {
        require::Equal(&ctx.t, 0u64, ctx.tk.Session().AffectedRows());
    }
}

/// TestOneColFrame test 1 col frame.
pub fn TestOneColFrame(
    ctx: &SuiteContext,
    col_ids: &[Vec<i32>],
    f: impl Fn(&SuiteContext, i32, &str, i32) -> Result<(), String>,
) {
    for table_id in 0..ctx.tableNum {
        let table_name = format!("addindex.t{table_id}");
        for &i in &col_ids[table_id as usize] {
            if let Some(wl) = &ctx.workload {
                wl.lock().unwrap().start(ctx, &[table_id, i]);
            }
            let mut fp_handle = None;
            if ctx.isFailpointsTest {
                ctx.failSync.lock().unwrap().add(1);
                let ctx_cancelled = ctx.cancelled.clone();
                let t = ctx.t.clone();
                let fail_sync = ctx.failSync.clone();
                fp_handle = Some(thread::spawn(move || {
                    useFailpoints_with(&t, &fail_sync, i);
                    let _ = ctx_cancelled;
                }));
            }
            let err = f(ctx, table_id, &table_name, i);
            if let Err(ref e) = err {
                if ctx.get_unique() || ctx.get_pk() {
                    require::Contains(&ctx.t, e, "Duplicate entry");
                } else {
                    logutil::BgLogger().Error(
                        "add index failed",
                        &[("category", "add index test".into()), ("error", e.clone())],
                    );
                    require::NoError(&ctx.t, Err(e.clone()));
                }
            }
            if let Some(wl) = &ctx.workload {
                let _ = wl.lock().unwrap().stop(ctx, -1);
            }
            if let Some(h) = fp_handle {
                let _ = h.join();
            }
            if err.is_ok() {
                checkResult(ctx, &table_name, i, table_id);
            }
        }
    }
}

/// TestTwoColsFrame test 2 columns frame.
pub fn TestTwoColsFrame(
    ctx: &SuiteContext,
    i_ids: &[Vec<i32>],
    j_ids: &[Vec<i32>],
    f: impl Fn(&SuiteContext, i32, &str, i32, i32, i32) -> Result<(), String>,
) {
    for table_id in 0..ctx.tableNum {
        let table_name = format!("addindex.t{table_id}");
        let mut index_id = 0;
        for &i in &i_ids[table_id as usize] {
            for &j in &j_ids[table_id as usize] {
                if let Some(wl) = &ctx.workload {
                    wl.lock().unwrap().start(ctx, &[table_id, i, j]);
                }
                let mut fp_handle = None;
                if ctx.isFailpointsTest {
                    ctx.failSync.lock().unwrap().add(1);
                    let t = ctx.t.clone();
                    let fail_sync = ctx.failSync.clone();
                    fp_handle = Some(thread::spawn(move || {
                        useFailpoints_with(&t, &fail_sync, i);
                    }));
                }
                let err = f(ctx, table_id, &table_name, index_id, i, j);
                if let Err(ref e) = err {
                    logutil::BgLogger().Error(
                        "add index failed",
                        &[("category", "add index test".into()), ("error", e.clone())],
                    );
                }
                require::NoError(&ctx.t, err.as_ref().map(|_| ()).map_err(|e| e.clone()));
                if let Some(wl) = &ctx.workload {
                    let _ = wl.lock().unwrap().stop(ctx, -1);
                }
                if let Some(h) = fp_handle {
                    let _ = h.join();
                }
                if err.is_ok() && i != j {
                    checkResult(ctx, &table_name, index_id, table_id);
                }
                index_id += 1;
            }
        }
    }
}

/// TestOneIndexFrame test 1 index frame.
pub fn TestOneIndexFrame(
    ctx: &SuiteContext,
    col_id: i32,
    f: impl Fn(&SuiteContext, i32, &str, i32) -> Result<(), String>,
) {
    for table_id in 0..ctx.tableNum {
        let table_name = format!("addindex.t{table_id}");
        if let Some(wl) = &ctx.workload {
            wl.lock().unwrap().start(ctx, &[table_id, col_id]);
        }
        let mut fp_handle = None;
        if ctx.isFailpointsTest {
            ctx.failSync.lock().unwrap().add(1);
            let t = ctx.t.clone();
            let fail_sync = ctx.failSync.clone();
            fp_handle = Some(thread::spawn(move || {
                useFailpoints_with(&t, &fail_sync, table_id);
            }));
        }
        let err = f(ctx, table_id, &table_name, col_id);
        if let Err(ref e) = err {
            logutil::BgLogger().Error(
                "add index failed",
                &[("category", "add index test".into()), ("error", e.clone())],
            );
        }
        require::NoError(&ctx.t, err.as_ref().map(|_| ()).map_err(|e| e.clone()));
        if let Some(wl) = &ctx.workload {
            let _ = wl.lock().unwrap().stop(ctx, -1);
        }
        if let Some(h) = fp_handle {
            let _ = h.join();
        }
        if err.is_ok() {
            if ctx.get_pk() {
                checkTableResult(ctx, &table_name, table_id);
            } else {
                checkResult(ctx, &table_name, col_id, table_id);
            }
        }
    }
}

/// AddIndexNonUnique test add index with non-unique key.
pub fn AddIndexNonUnique(
    ctx: &SuiteContext,
    table_id: i32,
    _table_name: &str,
    index_id: i32,
) -> Result<(), String> {
    ctx.set_pk(false);
    ctx.set_unique(false);
    createIndexOneCol(ctx, table_id, index_id)
}

/// AddIndexUnique test add index with unique key.
pub fn AddIndexUnique(
    ctx: &SuiteContext,
    table_id: i32,
    table_name: &str,
    index_id: i32,
) -> Result<(), String> {
    ctx.set_pk(false);
    ctx.set_unique(true);
    if index_id == 0 || index_id == 6 || index_id == 11 || index_id == 19 || table_id > 0 {
        let err = createIndexOneCol(ctx, table_id, index_id);
        if let Err(ref e) = err {
            logutil::BgLogger().Error(
                "add index failed",
                &[("category", "add index test".into()), ("error", e.clone())],
            );
        } else {
            logutil::BgLogger().Info(
                "add index success",
                &[
                    ("category", "add index test".into()),
                    ("table name", table_name.into()),
                    ("index ID", index_id.to_string()),
                ],
            );
        }
        require::NoError(&ctx.t, err.as_ref().map(|_| ()).map_err(|e| e.clone()));
        err
    } else {
        let err = createIndexOneCol(ctx, table_id, index_id);
        if let Err(ref e) = err {
            require::Contains(&ctx.t, e, "1062");
            logutil::BgLogger().Error(
                "add index failed",
                &[
                    ("category", "add index test".into()),
                    ("error", e.clone()),
                    ("table name", table_name.into()),
                    ("index ID", index_id.to_string()),
                ],
            );
        }
        err
    }
}

/// AddIndexPK test add index with pk.
pub fn AddIndexPK(
    ctx: &SuiteContext,
    table_id: i32,
    _table_name: &str,
    _col_id: i32,
) -> Result<(), String> {
    ctx.set_pk(true);
    ctx.set_unique(false);
    createIndexOneCol(ctx, table_id, 0)
}

/// AddIndexGenCol test add index with gen col.
pub fn AddIndexGenCol(
    ctx: &SuiteContext,
    table_id: i32,
    _table_name: &str,
    _col_id: i32,
) -> Result<(), String> {
    ctx.set_pk(false);
    ctx.set_unique(false);
    createIndexOneCol(ctx, table_id, 29)
}

/// AddIndexMultiCols test add index with 2 columns.
pub fn AddIndexMultiCols(
    ctx: &SuiteContext,
    table_id: i32,
    _table_name: &str,
    index_id: i32,
    col_id1: i32,
    col_id2: i32,
) -> Result<(), String> {
    ctx.set_pk(false);
    ctx.set_unique(false);
    if col_id1 != col_id2 {
        let err = createIndexTwoCols(ctx, table_id, index_id, col_id1, col_id2);
        if let Err(ref e) = err {
            logutil::BgLogger().Error(
                "add index failed",
                &[("category", "add index test".into()), ("error", e.clone())],
            );
        }
        require::NoError(&ctx.t, err.as_ref().map(|_| ()).map_err(|e| e.clone()));
        return err;
    }
    Ok(())
}

struct FailpointsPath {
    failpath: &'static str,
    in_term: &'static str,
}

static FAILPOINTS: &[FailpointsPath] = &[
    FailpointsPath {
        failpath: "github.com/pingcap/tidb/pkg/ddl/mockHighLoadForAddIndex",
        in_term: "return",
    },
    FailpointsPath {
        failpath: "github.com/pingcap/tidb/pkg/ddl/mockBackfillRunErr",
        in_term: "1*return",
    },
    FailpointsPath {
        failpath: "github.com/pingcap/tidb/pkg/ddl/mockBackfillSlow",
        in_term: "return",
    },
    FailpointsPath {
        failpath: "github.com/pingcap/tidb/pkg/ddl/MockCaseWhenParseFailure",
        in_term: "return(true)",
    },
    FailpointsPath {
        failpath: "github.com/pingcap/tidb/pkg/ddl/mockHighLoadForMergeIndex",
        in_term: "return",
    },
    FailpointsPath {
        failpath: "github.com/pingcap/tidb/pkg/ddl/mockMergeRunErr",
        in_term: "1*return",
    },
    FailpointsPath {
        failpath: "github.com/pingcap/tidb/pkg/ddl/mockMergeSlow",
        in_term: "return",
    },
];

fn useFailpoints_with(t: &TestCtx, fail_sync: &Arc<Mutex<FailSync>>, failpos: i32) {
    let _guard = FailSyncDone(fail_sync);
    let mut failpos = failpos % 7;
    if failpos < 0 {
        failpos = -failpos;
    }
    let fp = &FAILPOINTS[failpos as usize];
    require::NoError(t, failpoint::Enable(fp.failpath, fp.in_term));
    thread::sleep(failpoint_hold_duration());
    require::NoError(t, failpoint::Disable(fp.failpath));
}

struct FailSyncDone<'a>(&'a Arc<Mutex<FailSync>>);

impl Drop for FailSyncDone<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().done();
    }
}

/// InitTestFailpoint inits SuiteContext for failpoint tests.
pub fn InitTestFailpoint(t: &TestCtx) -> SuiteContext {
    let mut ctx = InitTest(t);
    ctx.isFailpointsTest = true;
    ctx
}

/// AssertExternalField checks fields tagged `external:"true"` are nil/empty.
pub fn AssertExternalField<T: ExternalTagged + ?Sized>(t: &TestCtx, subtask_meta: &T) {
    for field in subtask_meta.external_fields() {
        match field {
            ExternalTaggedField::Ptr { name, is_nil } => {
                require::Nil(t, is_nil, &name);
            }
            ExternalTaggedField::Struct { name, is_zero } => {
                require::True(t, is_zero, &format!("Field {name} should be empty"));
            }
            ExternalTaggedField::Slice { name, len } => {
                require::True(t, len == 0, &format!("Field {name} should be empty"));
            }
            ExternalTaggedField::Map { name, len } => {
                require::True(t, len == 0, &format!("Field {name} should be empty"));
            }
        }
    }
}
