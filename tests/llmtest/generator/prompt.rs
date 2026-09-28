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

//! Prompt generator registry (Go `tests/llmtest/generator/prompt.go`).

// 本文件对应 `tests/llmtest/generator/prompt.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use crate::stubs::openai::ChatCompletionMessageParamUnion;
use crate::stubs::{marshal_should_fail, set_force_marshal_error};
use astersql_tests_llmtest_testcase::{AnyValue, Case, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// PromptGenerator is the interface for prompt generator.
// `PromptGenerator` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
pub trait PromptGenerator: Send + Sync {
    // `name` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn name(&self) -> &'static str;
    // `groups` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn groups(&self) -> Vec<&'static str>;

    // `generate_prompt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn generate_prompt(
        &self,
        group: &str,
        count: i32,
        exist_cases: &[Case],
    ) -> Option<Vec<ChatCompletionMessageParamUnion>>;

    // `unmarshal` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn unmarshal(&self, response: &str) -> Vec<Case>;
}

/// `simplePromptResponse` from Go `expression.go` (package-level, shared by generators).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// `SimplePromptResponse` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
pub struct SimplePromptResponse {
    pub queries: Vec<String>,
}

// 这里实现 `SimplePromptResponse` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl SimplePromptResponse {
    /// Go `json.Marshal(simplePromptResponse{Queries: ...})`.
    // `marshal` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn marshal(&self) -> Result<String, String> {
        if marshal_should_fail() {
            return Err("forced marshal error".to_string());
        }
        let arr = AnyValue::Array(
            self.queries
                .iter()
                .map(|q| AnyValue::String(q.clone()))
                .collect(),
        );
        let obj = AnyValue::Object(vec![("queries".to_string(), arr)]);
        Ok(json::marshal(&obj))
    }

    /// Go `json.Unmarshal([]byte(response), &resp)`.
    // `unmarshal_json` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn unmarshal_json(response: &str) -> Result<Self, String> {
        let v = json::unmarshal(response.as_bytes())?;
        if matches!(v, AnyValue::Null) {
            return Ok(Self::default());
        }
        let AnyValue::Object(fields) = v else {
            return Err("simplePromptResponse must be object".to_string());
        };
        // encoding/json processes duplicates in order and reuses slice storage.
        // A null string leaves its previous value untouched; null/[] slices reset it.
        let mut queries = Vec::new();
        let mut length = 0;
        for (key, value) in fields {
            // Go's Unicode simple fold also maps the long s to ASCII S.
            if !key.replace('ſ', "s").eq_ignore_ascii_case("queries") {
                continue;
            }
            let items = match value {
                AnyValue::Null => {
                    queries.clear();
                    length = 0;
                    continue;
                }
                AnyValue::Array(items) => items,
                _ => return Err("queries must be array".to_string()),
            };
            length = items.len();
            if length == 0 {
                queries.clear();
            }
            if queries.len() < length {
                queries.resize(length, String::new());
            }
            for (index, item) in items.into_iter().enumerate() {
                match item {
                    AnyValue::String(s) => queries[index] = s,
                    AnyValue::Null => {}
                    _ => return Err("queries element must be string".to_string()),
                }
            }
        }
        queries.truncate(length);
        Ok(Self { queries })
    }
}

// `generators` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn generators() -> &'static Mutex<HashMap<String, Arc<dyn PromptGenerator>>> {
    // `GENERATORS` 记录跨函数共享的固定约束、错误文本或全局状态。
    // 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
    static GENERATORS: OnceLock<Mutex<HashMap<String, Arc<dyn PromptGenerator>>>> = OnceLock::new();
    GENERATORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a prompt generator (Go `registerPromptGenerator`).
// `register_prompt_generator` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
pub fn register_prompt_generator(g: Arc<dyn PromptGenerator>) {
    let mut map = generators().lock().expect("generators mutex");
    map.insert(g.name().to_string(), g);
}

/// AllPromptGenerators returns all the registered generators.
///
/// Go map iteration order is undefined; Rust HashMap order is likewise unstable.
// `all_prompt_generators` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
pub fn all_prompt_generators() -> Vec<Arc<dyn PromptGenerator>> {
    ensure_init();
    let map = generators().lock().expect("generators mutex");
    map.values().cloned().collect()
}

/// GetPromptGenerator returns the generator by name.
// `get_prompt_generator` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
pub fn get_prompt_generator(name: &str) -> Option<Arc<dyn PromptGenerator>> {
    ensure_init();
    generators()
        .lock()
        .expect("generators mutex")
        .get(name)
        .cloned()
}

// `INIT` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
static INIT: OnceLock<()> = OnceLock::new();

/// Force package `init` registration of dml / expression / misc generators.
// `ensure_init` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
pub fn ensure_init() {
    INIT.get_or_init(|| {
        set_force_marshal_error(false);
        crate::dml::register();
        crate::expression::register();
        crate::misc::register();
    });
}
