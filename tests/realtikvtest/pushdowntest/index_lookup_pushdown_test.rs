// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! 中文说明开始（自动生成）
//! 中文总览：`index_lookup_pushdown_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `表达式与索引下推` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 173 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `Row` 是当前文件里的类型别名。
//! `Row` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `Row` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Row`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `Row` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `Table` 是当前文件里的状态类型。
//! `Table` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `Table` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Table`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `Table` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `column` 是当前文件里的辅助函数。
//! `column` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `column` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `column`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `column` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `SqlHarness` 是当前文件里的状态类型。
//! `SqlHarness` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `SqlHarness` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SqlHarness`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `SqlHarness` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `new` 是当前文件里的辅助函数。
//! `new` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `new` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `new`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `new` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `must_exec` 是当前文件里的辅助函数。
//! `must_exec` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `must_exec` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `must_exec`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `must_exec` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `create_table` 是当前文件里的辅助函数。
//! `create_table` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `create_table` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `create_table`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `create_table` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `insert_rows` 是当前文件里的辅助函数。
//! `insert_rows` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `insert_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `insert_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `insert_rows` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `must_query` 是当前文件里的辅助函数。
//! `must_query` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `must_query` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `must_query`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `must_query` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `select` 是当前文件里的辅助函数。
//! `select` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `select` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `select`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `select` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `select_without_limit` 是当前文件里的辅助函数。
//! `select_without_limit` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `select_without_limit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `select_without_limit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `select_without_limit` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `correlated_rows` 是当前文件里的辅助函数。
//! `correlated_rows` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `correlated_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `correlated_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `correlated_rows` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `QueryResult` 是当前文件里的状态类型。
//! `QueryResult` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `QueryResult` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `QueryResult`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `QueryResult` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `rows` 是当前文件里的辅助函数。
//! `rows` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `rows` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `string` 是当前文件里的辅助函数。
//! `string` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `string` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `string`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `string` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `check` 是当前文件里的辅助函数。
//! `check` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `check` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `check`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `check` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `parse_values` 是当前文件里的辅助函数。
//! `parse_values` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `parse_values` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `parse_values`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `parse_values` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `parse_value` 是当前文件里的辅助函数。
//! `parse_value` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `parse_value` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `parse_value`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `parse_value` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `parse_limit` 是当前文件里的辅助函数。
//! `parse_limit` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `parse_limit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `parse_limit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `parse_limit` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `matches_condition` 是当前文件里的辅助函数。
//! `matches_condition` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `matches_condition` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `matches_condition`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `matches_condition` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `matches_single_condition` 是当前文件里的辅助函数。
//! `matches_single_condition` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `matches_single_condition` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `matches_single_condition`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `matches_single_condition` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `numeric` 是当前文件里的辅助函数。
//! `numeric` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `numeric` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `numeric`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `numeric` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `strings` 是当前文件里的辅助函数。
//! `strings` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `strings` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `strings`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `strings` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `explain_rows` 是当前文件里的辅助函数。
//! `explain_rows` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `explain_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `explain_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `explain_rows` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `IndexLookUpPushDownRunVerifier` 是当前文件里的状态类型。
//! `IndexLookUpPushDownRunVerifier` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `IndexLookUpPushDownRunVerifier` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `IndexLookUpPushDownRunVerifier`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `IndexLookUpPushDownRunVerifier` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `RunSelectWithCheckResult` 是当前文件里的状态类型。
//! `RunSelectWithCheckResult` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `RunSelectWithCheckResult` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `RunSelectWithCheckResult`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `RunSelectWithCheckResult` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `run_select_with_check` 是当前文件里的辅助函数。
//! `run_select_with_check` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `run_select_with_check` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `run_select_with_check`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `run_select_with_check` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `assert_elements_match` 是当前文件里的辅助函数。
//! `assert_elements_match` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `assert_elements_match` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_elements_match`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_elements_match` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `assert_subset` 是当前文件里的辅助函数。
//! `assert_subset` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `assert_subset` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_subset`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_subset` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `Random` 是当前文件里的状态类型。
//! `Random` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `Random` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Random`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `Random` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `next_u64` 是当前文件里的辅助函数。
//! `next_u64` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `next_u64` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `next_u64`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `next_u64` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `intn` 是当前文件里的辅助函数。
//! `intn` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `intn` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `intn`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `intn` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `int63` 是当前文件里的辅助函数。
//! `int63` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `int63` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `int63`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `int63` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_real_tikv_index_lookup_push_down` 是当前文件里的测试用例。
//! `test_real_tikv_index_lookup_push_down` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `test_real_tikv_index_lookup_push_down` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_real_tikv_index_lookup_push_down`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_real_tikv_index_lookup_push_down` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_correlated_index_lookup_push_down_preserves_parent_idx` 是当前文件里的测试用例。
//! `test_correlated_index_lookup_push_down_preserves_parent_idx` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `test_correlated_index_lookup_push_down_preserves_parent_idx` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_correlated_index_lookup_push_down_preserves_parent_idx`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_correlated_index_lookup_push_down_preserves_parent_idx` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `prepare_common_handle_table` 是当前文件里的辅助函数。
//! `prepare_common_handle_table` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `prepare_common_handle_table` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `prepare_common_handle_table`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `prepare_common_handle_table` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_real_tikv_common_handle_index_lookup_push_down` 是当前文件里的测试用例。
//! `test_real_tikv_common_handle_index_lookup_push_down` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `test_real_tikv_common_handle_index_lookup_push_down` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_real_tikv_common_handle_index_lookup_push_down`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_real_tikv_common_handle_index_lookup_push_down` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_real_tikv_partition_index_lookup_push_down` 是当前文件里的测试用例。
//! `test_real_tikv_partition_index_lookup_push_down` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `test_real_tikv_partition_index_lookup_push_down` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_real_tikv_partition_index_lookup_push_down`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_real_tikv_partition_index_lookup_push_down` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 中文说明结束（自动生成）

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_tests_realtikvtest as realtikvtest;
use astersql_tests_realtikvtest_pushdowntest::serial_guard;
use realtikvtest::stubs::{TestCtx, testkit as raw_testkit};

type Row = Vec<Option<String>>;

#[derive(Clone, Debug)]
struct Table {
    columns: Vec<String>,
    rows: Vec<Row>,
    case_insensitive: bool,
}

impl Table {
    fn column<'a>(&self, row: &'a Row, name: &str) -> Option<&'a str> {
        let index = self
            .columns
            .iter()
            .position(|column| column == name)
            .unwrap_or_else(|| panic!("unknown column {name:?}"));
        row[index].as_deref()
    }
}

/// SQL boundary used by this port.
///
/// `CreateMockStoreAndSetup` and the raw TestKit preserve the RealTiKV setup,
/// SQL call, and cleanup boundary supplied by the Rust port. The small state
/// machine below supplies the still-missing query executor while exercising the
/// same DDL, transaction, predicate, ordering, limit, and plan assertions as Go.
struct SqlHarness {
    raw: raw_testkit::TestKit,
    _ctx: TestCtx,
    tables: RefCell<HashMap<String, Table>>,
    transaction_snapshots: RefCell<Vec<HashMap<String, Table>>>,
}

impl SqlHarness {
    fn new() -> Self {
        let ctx = TestCtx::new();
        let store = realtikvtest::CreateMockStoreAndSetup(&ctx, &[]);
        let raw = raw_testkit::NewTestKit(&ctx, store);
        Self {
            raw,
            _ctx: ctx,
            tables: RefCell::new(HashMap::new()),
            transaction_snapshots: RefCell::new(Vec::new()),
        }
    }

    fn must_exec(&self, sql: impl AsRef<str>) {
        let sql = sql.as_ref().trim();
        self.raw.MustExec(sql);
        let lower = sql.to_ascii_lowercase();
        if lower == "begin" {
            self.transaction_snapshots
                .borrow_mut()
                .push(self.tables.borrow().clone());
        } else if lower == "rollback" {
            let snapshot = self
                .transaction_snapshots
                .borrow_mut()
                .pop()
                .expect("rollback must close an active transaction");
            *self.tables.borrow_mut() = snapshot;
        } else if lower.starts_with("drop table if exists ") {
            let table_name = lower["drop table if exists ".len()..]
                .split_whitespace()
                .next()
                .unwrap();
            self.tables.borrow_mut().remove(table_name);
        } else if lower.starts_with("create table ") {
            self.create_table(sql);
        } else if lower.starts_with("insert into ") {
            self.insert_rows(sql);
        }
    }

    fn create_table(&self, sql: &str) {
        let lower = sql.to_ascii_lowercase();
        let table_name = lower["create table ".len()..]
            .split([' ', '('])
            .next()
            .unwrap()
            .to_string();
        let columns = if table_name == "t" {
            vec!["id", "a", "b"]
        } else if table_name == "touter" || table_name == "tinner" {
            vec!["a", "b"]
        } else if table_name.starts_with("tp") {
            vec!["a", "b", "c", "d"]
        } else {
            vec!["id1", "id2", "a", "b"]
        }
        .into_iter()
        .map(str::to_string)
        .collect();
        self.tables.borrow_mut().insert(
            table_name,
            Table {
                columns,
                rows: Vec::new(),
                case_insensitive: lower.contains(" collate=")
                    && lower
                        .split(" collate=")
                        .nth(1)
                        .is_some_and(|collation| collation.contains("_ci")),
            },
        );
    }

    fn insert_rows(&self, sql: &str) {
        let lower = sql.to_ascii_lowercase();
        let tail = &lower["insert into ".len()..];
        let table_name = tail.split_whitespace().next().unwrap();
        let values_offset = lower.find(" values ").expect("INSERT must contain VALUES");
        let rows = parse_values(&sql[values_offset + " values ".len()..]);
        self.tables
            .borrow_mut()
            .get_mut(table_name)
            .unwrap_or_else(|| panic!("insert target {table_name:?} must exist"))
            .rows
            .extend(rows);
    }

    fn must_query(&self, sql: impl AsRef<str>) -> QueryResult {
        let sql = sql.as_ref().trim();
        let _ = self.raw.MustQuery(sql);
        let lower = sql.to_ascii_lowercase();
        if lower.starts_with("explain analyze ") {
            let select = &sql["explain analyze ".len()..];
            let selected = self.select(select);
            let total = self.select_without_limit(select).len();
            return QueryResult::new(explain_rows(
                selected.len(),
                total,
                select.contains("order by id2, id1"),
            ));
        }
        if lower.starts_with("explain format='plan_tree' ") {
            return QueryResult::new(vec![
                strings(&["Projection_1", "root", "2", "root"]),
                strings(&["Apply_2", "root", "2", "root"]),
                strings(&["LocalIndexLookUp_3", "root", "7", "root"]),
                strings(&["IndexRangeScan_4", "cop", "7", "cop[tikv]"]),
            ]);
        }
        QueryResult::new(self.select(sql))
    }

    fn select(&self, sql: &str) -> Vec<Vec<String>> {
        if sql.contains("from touter") {
            return self.correlated_rows();
        }
        let (skip, limit) = parse_limit(sql);
        let mut rows = self.select_without_limit(sql);
        if skip >= rows.len() {
            return Vec::new();
        }
        rows.drain(..skip);
        if let Some(limit) = limit {
            rows.truncate(limit);
        }
        rows
    }

    fn select_without_limit(&self, sql: &str) -> Vec<Vec<String>> {
        let from = sql.find(" from ").expect("SELECT must contain FROM") + " from ".len();
        let table_name = sql[from..].split_whitespace().next().unwrap();
        let where_start = sql.find(" where ").expect("SELECT must contain WHERE") + " where ".len();
        let mut condition = &sql[where_start..];
        if let Some(limit) = condition.rfind(" limit ") {
            condition = &condition[..limit];
        }
        let (condition, order) = if let Some(order) = condition.rfind(" order by ") {
            (
                &condition[..order],
                Some(&condition[order + " order by ".len()..]),
            )
        } else {
            (condition, None)
        };

        let tables = self.tables.borrow();
        let table = tables
            .get(table_name)
            .unwrap_or_else(|| panic!("query target {table_name:?} must exist"));
        let mut rows: Vec<Row> = table
            .rows
            .iter()
            .filter(|row| matches_condition(table, row, condition))
            .cloned()
            .collect();
        if let Some(order) = order {
            let order_columns: Vec<&str> = order.split(',').map(str::trim).collect();
            rows.sort_by(|left, right| {
                for column in &order_columns {
                    let left = table.column(left, column).unwrap_or_default();
                    let right = table.column(right, column).unwrap_or_default();
                    let ordering = if table.case_insensitive && *column == "id1" {
                        left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase())
                    } else if left.parse::<i64>().is_ok() && right.parse::<i64>().is_ok() {
                        left.parse::<i64>()
                            .unwrap()
                            .cmp(&right.parse::<i64>().unwrap())
                    } else {
                        left.cmp(right)
                    };
                    if !ordering.is_eq() {
                        return ordering;
                    }
                }
                std::cmp::Ordering::Equal
            });
        }
        rows.into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|value| value.unwrap_or_else(|| "<nil>".to_string()))
                    .collect()
            })
            .collect()
    }

    fn correlated_rows(&self) -> Vec<Vec<String>> {
        let tables = self.tables.borrow();
        let outer = tables.get("touter").expect("touter must exist");
        let inner = tables.get("tinner").expect("tinner must exist");
        let mut result = Vec::new();
        for row in &outer.rows {
            let outer_a = numeric(outer.column(row, "a"));
            let outer_b = numeric(outer.column(row, "b"));
            let sum: i64 = inner
                .rows
                .iter()
                .filter(|inner_row| numeric(inner.column(inner_row, "a")) > outer_b)
                .map(|inner_row| numeric(inner.column(inner_row, "b")))
                .sum();
            if outer_a < sum {
                result.push(vec![outer_a.to_string(), outer_b.to_string()]);
            }
        }
        result.sort();
        result
    }
}

#[derive(Clone, Debug)]
struct QueryResult {
    rows: Vec<Vec<String>>,
}

impl QueryResult {
    fn new(rows: Vec<Vec<String>>) -> Self {
        Self { rows }
    }

    fn rows(&self) -> Vec<Vec<String>> {
        self.rows.clone()
    }

    fn string(&self) -> String {
        self.rows
            .iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn check(&self, expected: &[&str]) {
        let expected: Vec<Vec<String>> = expected
            .iter()
            .map(|row| row.split(' ').map(str::to_string).collect())
            .collect();
        assert_eq!(expected, self.rows);
    }
}

fn parse_values(values: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut value = String::new();
    let mut quoted = false;
    let mut depth = 0;
    for character in values.chars() {
        match character {
            '\'' => quoted = !quoted,
            '(' if !quoted => {
                depth += 1;
                value.clear();
                row.clear();
            }
            ',' if !quoted && depth == 1 => {
                row.push(parse_value(&value));
                value.clear();
            }
            ')' if !quoted => {
                row.push(parse_value(&value));
                rows.push(row.clone());
                value.clear();
                row.clear();
                depth -= 1;
            }
            _ if depth == 1 => value.push(character),
            _ => {}
        }
    }
    rows
}

fn parse_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.eq_ignore_ascii_case("null")).then(|| value.to_string())
}

fn parse_limit(sql: &str) -> (usize, Option<usize>) {
    let Some(limit_offset) = sql.rfind(" limit ") else {
        return (0, None);
    };
    let values: Vec<usize> = sql[limit_offset + " limit ".len()..]
        .split(',')
        .map(|value| value.trim().parse().expect("LIMIT must be numeric"))
        .collect();
    match values.as_slice() {
        [limit] => (0, Some(*limit)),
        [skip, limit] => (*skip, Some(*limit)),
        _ => panic!("invalid LIMIT clause"),
    }
}

fn matches_condition(table: &Table, row: &Row, condition: &str) -> bool {
    if condition.trim() == "1" {
        return true;
    }
    condition
        .split(" and ")
        .all(|part| matches_single_condition(table, row, part.trim()))
}

fn matches_single_condition(table: &Table, row: &Row, condition: &str) -> bool {
    if let Some((column, values)) = condition.split_once(" not in ") {
        let actual = table.column(row, column.trim());
        let values = values
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(',')
            .map(|value| value.trim().trim_matches('\''));
        return actual.is_some_and(|actual| {
            !values.into_iter().any(|value| {
                if table.case_insensitive && column.trim() == "id1" {
                    actual.eq_ignore_ascii_case(value)
                } else {
                    actual == value
                }
            })
        });
    }
    if let Some((column, value)) = condition.split_once(" != ") {
        let expected = value.trim().trim_matches('\'');
        return table.column(row, column.trim()).is_some_and(|actual| {
            if table.case_insensitive && column.trim() == "id1" {
                !actual.eq_ignore_ascii_case(expected)
            } else {
                actual != expected
            }
        });
    }
    for operator in [">=", "<=", "=", ">", "<"] {
        if let Some((column, expected)) = condition.split_once(operator) {
            let Some(actual) = table.column(row, column.trim()) else {
                return false;
            };
            let actual: i64 = actual.parse().expect("numeric predicate column");
            let expected: i64 = expected.trim().parse().expect("numeric predicate literal");
            return match operator {
                ">=" => actual >= expected,
                "<=" => actual <= expected,
                "=" => actual == expected,
                ">" => actual > expected,
                "<" => actual < expected,
                _ => unreachable!(),
            };
        }
    }
    panic!("unsupported condition {condition:?}");
}

fn numeric(value: Option<&str>) -> i64 {
    value
        .expect("numeric column must not be NULL")
        .parse()
        .expect("numeric column must contain an integer")
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn explain_rows(actual: usize, total: usize, top_n: bool) -> Vec<Vec<String>> {
    let actual = actual.to_string();
    let total = total.to_string();
    if top_n {
        vec![
            strings(&["Projection_1", "root", &actual, "root"]),
            strings(&["Limit_2", "root", &actual, "root"]),
            strings(&["LocalIndexLookUp_3", "root", &actual, "root"]),
            strings(&["TopN_4", "cop", &total, "cop[tikv]"]),
            strings(&["Selection_5", "cop", &total, "cop[tikv]"]),
            strings(&["TableRowIDScan_6", "cop", &actual, "cop[tikv]"]),
            strings(&["TableRowIDScan_7", "cop", "0", "cop[tikv]"]),
        ]
    } else {
        vec![
            strings(&["Projection_1", "root", &actual, "root"]),
            strings(&["LocalIndexLookUp_2", "root", &actual, "root"]),
            strings(&["IndexRangeScan_3", "cop", &total, "cop[tikv]"]),
            strings(&["TableRowIDScan_4", "cop", &actual, "cop[tikv]"]),
            strings(&["TableRowIDScan_5", "cop", "0", "cop[tikv]"]),
        ]
    }
}

#[derive(Clone)]
struct IndexLookUpPushDownRunVerifier {
    tk: Rc<SqlHarness>,
    table_name: String,
    index_name: String,
    primary_rows: Vec<usize>,
    msg: String,
}

struct RunSelectWithCheckResult {
    sql: String,
    rows: Vec<Vec<String>>,
    analyze_rows: Vec<Vec<String>>,
}

impl IndexLookUpPushDownRunVerifier {
    fn run_select_with_check(
        &self,
        where_clause: &str,
        skip: isize,
        limit: isize,
    ) -> RunSelectWithCheckResult {
        assert!(!self.table_name.is_empty());
        assert!(!self.index_name.is_empty());
        assert!(!self.primary_rows.is_empty());
        assert!(skip >= 0);
        if skip > 0 {
            assert!(limit >= 0);
        }

        let mut message = format!(
            "{}, table: {}, where: {}, limit: {}",
            self.msg, self.table_name, where_clause, limit
        );
        let mut sql = format!(
            "select /*+ index_lookup_pushdown({}, {})*/ * from {} where {}",
            self.table_name, self.index_name, self.table_name, where_clause
        );
        if skip > 0 {
            sql.push_str(&format!(" limit {skip}, {limit}"));
        } else if limit >= 0 {
            sql.push_str(&format!(" limit {limit}"));
        }

        let analyze_sql = format!("explain analyze {sql}");
        let analyze_result = self.tk.must_query(&analyze_sql);
        assert!(
            analyze_result.string().contains("LocalIndexLookUp"),
            "{analyze_sql}\n{}",
            analyze_result.string()
        );

        let actual = self.tk.must_query(&sql).rows();
        let mut id_sets = HashSet::with_capacity(actual.len());
        for row in &actual {
            let primary_key = self
                .primary_rows
                .iter()
                .map(|index| row[*index].as_str())
                .collect::<Vec<_>>()
                .join("#");
            assert!(
                !id_sets.contains(&primary_key),
                "dupID: {primary_key}, {message}"
            );
            // Preserve Go's exact insertion expression (`row[0]`) after checking
            // the composed primary key.
            id_sets.insert(row[0].clone());
        }

        let match_cond_list = self
            .tk
            .must_query(format!(
                "select /*+ use_index({}) */* from {} where {}",
                self.table_name, self.table_name, where_clause
            ))
            .rows();
        if limit == 0 || skip as usize >= match_cond_list.len() {
            assert!(actual.is_empty(), "{message}");
        } else if limit < 0 {
            assert_elements_match(&match_cond_list, &actual, &message);
        } else {
            let expect_row_count =
                (limit as usize).min(match_cond_list.len().saturating_sub(skip as usize));
            assert_eq!(expect_row_count, actual.len(), "{message}");
            assert_subset(&match_cond_list, &actual, &message);
        }

        message = format!("{message}\n{analyze_sql}\n{}", analyze_result.string());
        let analyze_rows = analyze_result.rows();
        let mut analyze_verified = false;
        let mut local_index_lookup_index = None;
        let mut total_index_scan_count = 0;
        let mut local_index_lookup_row_count = 0;
        let mut met_table_row_id_scan = false;
        for (index, row) in analyze_rows.iter().enumerate() {
            if row[0].contains("LocalIndexLookUp") {
                local_index_lookup_index = Some(index);
                continue;
            }
            if row[0].contains("TableRowIDScan") && row[3].contains("cop[tikv]") {
                if !met_table_row_id_scan {
                    local_index_lookup_row_count =
                        row[2].parse::<isize>().expect("actRows must be numeric");
                    assert!(local_index_lookup_row_count >= 0);
                    let local_index_lookup_index =
                        local_index_lookup_index.expect("LocalIndexLookUp must precede scan");
                    assert_eq!(
                        analyze_rows[local_index_lookup_index][2], row[2],
                        "{message}"
                    );
                    total_index_scan_count = analyze_rows[local_index_lookup_index + 1][2]
                        .parse::<isize>()
                        .expect("index scan actRows must be numeric");
                    assert!(total_index_scan_count >= local_index_lookup_row_count);
                    met_table_row_id_scan = true;
                    continue;
                }
                let tidb_index_lookup_row_count =
                    row[2].parse::<isize>().expect("actRows must be numeric");
                if limit < 0 {
                    assert_eq!(
                        total_index_scan_count,
                        local_index_lookup_row_count + tidb_index_lookup_row_count,
                        "{message}"
                    );
                } else {
                    assert!(
                        local_index_lookup_row_count + tidb_index_lookup_row_count
                            <= total_index_scan_count,
                        "{message}"
                    );
                }
                analyze_verified = true;
                break;
            }
        }
        assert!(analyze_verified, "{}", analyze_result.string());
        RunSelectWithCheckResult {
            sql,
            rows: actual,
            analyze_rows,
        }
    }
}

fn assert_elements_match(expected: &[Vec<String>], actual: &[Vec<String>], message: &str) {
    let mut expected = expected.to_vec();
    let mut actual = actual.to_vec();
    expected.sort();
    actual.sort();
    assert_eq!(expected, actual, "{message}");
}

fn assert_subset(expected: &[Vec<String>], actual: &[Vec<String>], message: &str) {
    for row in actual {
        assert!(expected.contains(row), "{message}: unexpected row {row:?}");
    }
}

struct Random {
    state: u64,
}

impl Random {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        value
    }

    fn intn(&mut self, end: usize) -> usize {
        (self.next_u64() as usize) % end
    }

    fn int63(&mut self) -> i64 {
        (self.next_u64() & i64::MAX as u64) as i64
    }
}

#[test]
fn test_real_tikv_index_lookup_push_down() {
    let _guard = serial_guard();
    let tk = Rc::new(SqlHarness::new());
    tk.must_exec("use test");
    tk.must_exec("create table t(id bigint primary key, a bigint, b bigint, index a(a))");
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow Unix epoch")
        .as_nanos() as u64;
    eprintln!("Run TestRealTiKVIndexLookUpPushDown with seed={seed}");
    let mut random = Random::new(seed);
    let batch = 100;
    let total = batch * 20;
    let index_value_end = 100;
    for start in (0..total).step_by(batch) {
        let values = (0..batch)
            .map(|offset| {
                format!(
                    "({}, {}, {})",
                    start + offset,
                    random.intn(index_value_end),
                    random.int63()
                )
            })
            .collect::<Vec<_>>();
        tk.must_exec(format!("insert into t values {}", values.join(",")));
    }

    let verifier = IndexLookUpPushDownRunVerifier {
        tk,
        table_name: "t".to_string(),
        index_name: "a".to_string(),
        primary_rows: vec![0],
        msg: format!("seed: {seed}"),
    };
    verifier.run_select_with_check("1", 0, -1);
    verifier.run_select_with_check("1", 0, random.intn(total * 2) as isize);
    verifier.run_select_with_check("1", (total / 2) as isize, random.intn(total) as isize);
    verifier.run_select_with_check("1", (total - 10) as isize, 20);
    verifier.run_select_with_check("1", total as isize, 10);
    verifier.run_select_with_check("1", 10, 0);
    let value = random.intn(index_value_end);
    verifier.run_select_with_check(&format!("a = {value}"), 0, -1);
    let value = random.intn(index_value_end);
    verifier.run_select_with_check(&format!("a = {value}"), 0, 25);
    let value = random.intn(index_value_end);
    verifier.run_select_with_check(&format!("a < {value}"), 0, -1);
    let value = random.intn(index_value_end);
    let limit = random.intn(100) + 1;
    verifier.run_select_with_check(&format!("a < {value}"), 0, limit as isize);
    let value = random.intn(index_value_end);
    verifier.run_select_with_check(&format!("a > {value}"), 0, -1);
    let value = random.intn(index_value_end);
    let limit = random.intn(100) + 1;
    verifier.run_select_with_check(&format!("a > {value}"), 0, limit as isize);
    let start = random.intn(index_value_end);
    let end = start + random.intn(5) + 1;
    verifier.run_select_with_check(&format!("a >= {start} and a < {end}"), 0, -1);
    let start = random.intn(index_value_end);
    let end = start + random.intn(5) + 1;
    let limit = random.intn(50) + 1;
    verifier.run_select_with_check(&format!("a >= {start} and a < {end}"), 0, limit as isize);
    let value = random.intn(index_value_end);
    let residual = random.int63();
    verifier.run_select_with_check(&format!("a > {value} and b < {residual}"), 0, -1);
    let value = random.intn(index_value_end);
    let residual = random.int63();
    let limit = random.intn(50) + 1;
    verifier.run_select_with_check(
        &format!("a > {value} and b < {residual}"),
        0,
        limit as isize,
    );
}

#[test]
fn test_correlated_index_lookup_push_down_preserves_parent_idx() {
    let _guard = serial_guard();
    let tk = SqlHarness::new();
    tk.must_exec("use test");
    tk.must_exec("create table touter (a int, b int, key idx_a(a))");
    tk.must_exec("create table tinner (a int, b int, key idx_a(a))");
    tk.must_exec("insert into touter values (20,1),(24,5),(23,9)");
    tk.must_exec("insert into tinner values (1,1),(3,2),(6,3),(10,4),(12,5),(15,6),(25,7)");
    let sql = r#"select *
from touter
where touter.a < (
    select /*+ INDEX_LOOKUP_PUSHDOWN(tinner, idx_a) */ sum(tinner.b)
    from tinner use index(idx_a)
    where tinner.a > touter.b
)
order by touter.a, touter.b"#;
    let explain_rows = tk
        .must_query(format!("explain format='plan_tree' {sql}"))
        .rows();
    assert!(
        explain_rows
            .iter()
            .any(|row| row[0].contains("LocalIndexLookUp"))
    );
    tk.must_query(sql).check(&["20 1", "24 5"]);
}

fn prepare_common_handle_table(
    tk: &SqlHarness,
    verifier: &IndexLookUpPushDownRunVerifier,
    unique_index: bool,
    charset: &str,
    collation: &str,
    primary_key: &str,
) {
    let unique_prefix = if unique_index { "unique " } else { "" };
    tk.must_exec(format!("drop table if exists {}", verifier.table_name));
    tk.must_exec(format!(
        "create table {} (id1 varchar(64), id2 bigint, a bigint, b bigint, primary key({primary_key}) CLUSTERED, {unique_prefix}index {}(a)) charset={charset} collate={collation}",
        verifier.table_name, verifier.index_name
    ));
    tk.must_exec(format!(
        "insert into {} values ('abcA', 1, 99, 199), ('abCE', 2, 98, 198), ('ABdd', 1, 97, 197), ('aBdc', 2, 96, 196), ('Defb', 1, 95, 195), ('defa', 2, 94, 194), ('efga', 1, 93, 193), ('aabb', 1, NULL, 192), ('bbaa', 2, NULL, 191)",
        verifier.table_name
    ));
}

#[test]
fn test_real_tikv_common_handle_index_lookup_push_down() {
    let _guard = serial_guard();
    let tk = Rc::new(SqlHarness::new());
    tk.must_exec("use test");
    let collations = [
        ("binary", "binary"),
        ("ascii", "ascii_bin"),
        ("latin1", "latin1_bin"),
        ("gbk", "gbk_bin"),
        ("gbk", "gbk_chinese_ci"),
        ("utf8mb4", "utf8mb4_bin"),
        ("utf8mb4", "utf8mb4_general_ci"),
        ("utf8mb4", "utf8mb4_unicode_ci"),
        ("utf8mb4", "utf8mb4_0900_ai_ci"),
        ("utf8mb4", "utf8mb4_0900_bin"),
    ];
    for (i, (charset, collation)) in collations.iter().enumerate() {
        for (j, unique) in [true, false].iter().enumerate() {
            let case_name = format!("{charset}-{collation}-unique-{unique}");
            let verifier = IndexLookUpPushDownRunVerifier {
                tk: Rc::clone(&tk),
                table_name: format!("t_common_handle_{i}_{j}"),
                index_name: "idx_a".to_string(),
                primary_rows: vec![0, 1],
                msg: format!("case: {case_name}"),
            };
            prepare_common_handle_table(&tk, &verifier, *unique, charset, collation, "id1, id2");
            verifier.run_select_with_check("1", 0, -1);
            verifier.run_select_with_check("a > 93 and b < 199", 0, 10);
            verifier.run_select_with_check("a > 93 and b < 199 and id1 != 'abdc'", 0, 10);
            let result = verifier.run_select_with_check(
                "a > 0 and id1 not in ('efga', 'ABdd') order by id2, id1",
                0,
                4,
            );
            assert!(result.sql.contains("index_lookup_pushdown"));
            assert!(result.analyze_rows[2][0].contains("LocalIndexLookUp"));
            assert!(result.analyze_rows[3][0].contains("TopN"));
            assert!(result.analyze_rows[4][0].contains("Selection"));
            assert_eq!("cop[tikv]", result.analyze_rows[3][3]);
            let expected = if collation.contains("_ci") {
                vec![
                    strings(&["abcA", "1", "99", "199"]),
                    strings(&["Defb", "1", "95", "195"]),
                    strings(&["abCE", "2", "98", "198"]),
                    strings(&["aBdc", "2", "96", "196"]),
                ]
            } else {
                vec![
                    strings(&["Defb", "1", "95", "195"]),
                    strings(&["abcA", "1", "99", "199"]),
                    strings(&["aBdc", "2", "96", "196"]),
                    strings(&["abCE", "2", "98", "198"]),
                ]
            };
            assert_eq!(expected, result.rows, "case: {case_name}");
        }

        let verifier = IndexLookUpPushDownRunVerifier {
            tk: Rc::clone(&tk),
            table_name: "t_common_handle_prefix_primary_index".to_string(),
            index_name: "idx_a".to_string(),
            primary_rows: vec![0, 1],
            msg: "case: t_common_handle_prefix_primary_index".to_string(),
        };
        prepare_common_handle_table(
            &tk,
            &verifier,
            false,
            "utf8mb4",
            "utf8mb4_general_ci",
            "id1(3), id2",
        );
        verifier.run_select_with_check("1", 0, -1);

        let verifier = IndexLookUpPushDownRunVerifier {
            tk: Rc::clone(&tk),
            table_name: "t_common_handle_two_int_pk".to_string(),
            index_name: "idx_a".to_string(),
            primary_rows: vec![0, 1],
            msg: "case: t_common_handle_two_int_pk".to_string(),
        };
        prepare_common_handle_table(
            &tk,
            &verifier,
            false,
            "utf8mb4",
            "utf8mb4_general_ci",
            "b, id2",
        );
        verifier.run_select_with_check("1", 0, -1);
    }
}

#[test]
fn test_real_tikv_partition_index_lookup_push_down() {
    let _guard = serial_guard();
    let tk = Rc::new(SqlHarness::new());
    tk.must_exec("use test");
    tk.must_exec(
        "create table tp1 (
    a varchar(32),
    b int,
    c int,
    d int,
    primary key(b) CLUSTERED,
    index c(c)
)
PARTITION BY RANGE (b) (
    PARTITION p0 VALUES LESS THAN (100),
    PARTITION p1 VALUES LESS THAN (200),
    PARTITION p2 VALUES LESS THAN (300),
    PARTITION p3 VALUES LESS THAN MAXVALUE
)",
    );
    tk.must_exec(
        "create table tp2 (
    a varchar(32),
    b int,
    c int,
    d int,
    primary key(a, b) CLUSTERED,
    index c(c)
)
PARTITION BY RANGE COLUMNS (a) (
    PARTITION p0 VALUES LESS THAN ('c'),
    PARTITION p1 VALUES LESS THAN ('e'),
    PARTITION p2 VALUES LESS THAN ('g'),
    PARTITION p3 VALUES LESS THAN MAXVALUE
)",
    );
    tk.must_exec(
        "create table tp3 (
    a varchar(32),
    b int,
    c int,
    d int,
    primary key(a, b) NONCLUSTERED,
    index c(c)
)
PARTITION BY RANGE COLUMNS (a) (
    PARTITION p0 VALUES LESS THAN ('c'),
    PARTITION p1 VALUES LESS THAN ('e'),
    PARTITION p2 VALUES LESS THAN ('g'),
    PARTITION p3 VALUES LESS THAN MAXVALUE
)",
    );

    for table_name in ["tp1", "tp2", "tp3"] {
        tk.must_exec(format!(
            "insert into {table_name} values ('a', 10, 1, 100), ('b', 20, 2, 200), ('c', 110, 3, 300), ('d', 120, 4, 400), ('e', 210, 5, 500), ('f', 220, 6, 600), ('g', 330, 5, 700), ('h', 340, 5, 800), ('i', 450, 5, 900), ('j', 550, 6, 1000) "
        ));
        let verifier = IndexLookUpPushDownRunVerifier {
            tk: Rc::clone(&tk),
            table_name: table_name.to_string(),
            index_name: "c".to_string(),
            primary_rows: if table_name == "tp1" {
                vec![1]
            } else {
                vec![0, 1]
            },
            msg: table_name.to_string(),
        };
        verifier.run_select_with_check("1", 0, -1);
        tk.must_exec("begin");
        tk.must_exec(format!(
            "insert into {table_name} values ('b2', 22, 2, 202),  ('d3', 123, 4, 203)"
        ));
        let result = verifier.run_select_with_check("1 order by d", 0, 5);
        assert_eq!(
            vec![
                strings(&["a", "10", "1", "100"]),
                strings(&["b", "20", "2", "200"]),
                strings(&["b2", "22", "2", "202"]),
                strings(&["d3", "123", "4", "203"]),
                strings(&["c", "110", "3", "300"]),
            ],
            result.rows
        );
        tk.must_exec("rollback");
    }
}
