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

//! Misc prompt generator (Go `tests/llmtest/generator/misc.go`).

// 本文件对应 `tests/llmtest/generator/misc.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use crate::prompt::{PromptGenerator, SimplePromptResponse, register_prompt_generator};
use crate::stubs::openai::{self, ChatCompletionMessageParamUnion};
use astersql_tests_llmtest_logger::{Global, zap};
use astersql_tests_llmtest_testcase::Case;
use std::sync::Arc;

/// Corresponds to Go `miscPromptGenerator`.
// `MiscPromptGenerator` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
pub struct MiscPromptGenerator;

// 这里实现 `PromptGenerator` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl PromptGenerator for MiscPromptGenerator {
    // `name` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn name(&self) -> &'static str {
        "misc"
    }

    // `groups` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn groups(&self) -> Vec<&'static str> {
        vec!["cte"]
    }

    // `generate_prompt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn generate_prompt(
        &self,
        group: &str,
        count: i32,
        exist_cases: &[Case],
    ) -> Option<Vec<ChatCompletionMessageParamUnion>> {
        let mut messages = Vec::with_capacity(2);

        let system_prompt = r#"You are a professional QA engineer testing a new SQL database compatible with MySQL. You are tasked with testing the compatibility of the database with MySQL for a specific feature. You should write the queries to cover the corner cases of the operation. The common cases are not needed. You should try to use this operation with different valid argument types to test the implicit type conversion. You should try to use this operation with NULL to test the behavior of NULL. Please return a valid JSON object with the key "queries" and an array of strings as the value. Be careful with the escape characters. You should avoid using NOW(), RAND() or any other functions that return different results on each call. You should pack the related DDL in the same query. You should CREATE and DROP the table before and after using it. The SELECT statement should have stable order.

    IMPORTANT: Don't put anything else in the response.

    EXAMPLE INPUT:
    Return 3 random SQL queries using this operation: CTE.

    EXAMPLE JSON OUTPUT:
    {"queries": ["CREATE TABLE t1 (id int, name varchar(255));with cte1 as (select * from t1) select * from cte1 order by id;DROP TABLE t1;", "with recursive qn as (select 1 from dual union all select 1 from dual) select * from qn;", "with recursive cte2 as (select 1 as col_1, 2 as col_2) select c1.col_1, c2.col_2 from cte2 as c1, cte2 as c2 where c2.col_2 = 1;"]}"#
            .replace("\n\n    EXAMPLE JSON OUTPUT:", "\n    \n    EXAMPLE JSON OUTPUT:");
        messages.push(openai::SystemMessage(system_prompt));

        if !exist_cases.is_empty() {
            messages.push(openai::UserMessage(format!(
                "Return {} random SQL queries using this operation: {}.",
                exist_cases.len(),
                group
            )));

            let exist_response: Vec<String> = exist_cases.iter().map(|c| c.sql.clone()).collect();
            let assistant_message = match (SimplePromptResponse {
                queries: exist_response,
            })
            .marshal()
            {
                Ok(message) => message,
                Err(err) => {
                    // should never happen
                    Global.Info("failed to marshal exist response", &[zap::Error(err)]);
                    return None;
                }
            };
            messages.push(openai::AssistantMessage(assistant_message));
        }
        messages.push(openai::UserMessage(format!(
            "Return {} random SQL queries using this operation: {}.",
            count, group
        )));

        Some(messages)
    }

    // `unmarshal` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn unmarshal(&self, response: &str) -> Vec<Case> {
        let resp = match SimplePromptResponse::unmarshal_json(response) {
            Ok(resp) => resp,
            Err(err) => {
                Global.Error(
                    "failed to unmarshal misc prompt response",
                    &[zap::Error(&err), zap::String("response", response)],
                );
                return Vec::new();
            }
        };

        let mut cases = Vec::with_capacity(resp.queries.len());
        for q in resp.queries {
            cases.push(Case {
                sql: q,
                ..Default::default()
            });
        }
        cases
    }
}

/// Go `init` registration.
// `register` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
pub fn register() {
    register_prompt_generator(Arc::new(MiscPromptGenerator));
}
