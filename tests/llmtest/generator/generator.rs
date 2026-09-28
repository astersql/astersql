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

//! Test case generator workers (Go `tests/llmtest/generator/generator.go`).

// 本文件对应 `tests/llmtest/generator/generator.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use crate::prompt::PromptGenerator;
use crate::stubs::RequestOption;
use crate::stubs::openai::{self, ChatCompletion, ChatCompletionNewParams, Client};
use crate::stubs::option;
use astersql_tests_llmtest_logger::{Global, zap};
use astersql_tests_llmtest_testcase::{AnyValue, Case, Manager};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// Buffered input channel matching Go `chan string` (capacity = len(Groups)).
// `InputCh` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
struct InputCh {
    inner: Mutex<InputChInner>,
    cv: Condvar,
}

// `InputChInner` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
struct InputChInner {
    queue: VecDeque<String>,
    closed: bool,
}

// 这里实现 `InputCh` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl InputCh {
    // `with_capacity` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    fn with_capacity(cap: usize) -> Self {
        Self {
            inner: Mutex::new(InputChInner {
                queue: VecDeque::with_capacity(cap),
                closed: false,
            }),
            cv: Condvar::new(),
        }
    }

    // `send` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn send(&self, item: String) {
        let mut g = self.inner.lock().expect("inputCh mutex");
        assert!(!g.closed, "send on closed inputCh");
        g.queue.push_back(item);
        self.cv.notify_one();
    }

    // `close` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn close(&self) {
        let mut g = self.inner.lock().expect("inputCh mutex");
        g.closed = true;
        self.cv.notify_all();
    }

    // `recv` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn recv(&self) -> Option<String> {
        let mut g = self.inner.lock().expect("inputCh mutex");
        loop {
            if let Some(item) = g.queue.pop_front() {
                return Some(item);
            }
            if g.closed {
                return None;
            }
            g = self.cv.wait(g).expect("inputCh condvar");
        }
    }
}

// `Shared` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
struct Shared {
    input_ch: InputCh,
    case_manager: Arc<Manager>,
    openai_token: String,
    openai_base_url: String,
    model_name: String,
    prompt_generator: Arc<dyn PromptGenerator>,
    test_case_count: i32,
}

/// TestCaseGenerator generates test cases and writes them to `caseManager`
/// to reach a specific count.
// `TestCaseGenerator` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
pub struct TestCaseGenerator {
    shared: Arc<Shared>,
    parallelism: usize,
    workers: Vec<JoinHandle<()>>,
}

/// New creates a new TestCaseGenerator.
// `new` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
pub fn new(
    case_manager: Arc<Manager>,
    parallelism: usize,
    openai_token: String,
    openai_base_url: String,
    model_name: String,
    prompt_generator: Arc<dyn PromptGenerator>,
    test_case_count: i32,
) -> TestCaseGenerator {
    let cap = prompt_generator.groups().len();
    TestCaseGenerator {
        shared: Arc::new(Shared {
            input_ch: InputCh::with_capacity(cap),
            case_manager,
            openai_token,
            openai_base_url,
            model_name,
            prompt_generator,
            test_case_count,
        }),
        parallelism,
        workers: Vec::new(),
    }
}

// 这里实现 `TestCaseGenerator` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl TestCaseGenerator {
    /// Run starts the generator.
    // `run` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn run(&mut self) {
        for _ in 0..self.parallelism {
            let shared = Arc::clone(&self.shared);
            self.workers
                .push(std::thread::spawn(move || run_worker(shared)));
        }

        for group in self.shared.prompt_generator.groups() {
            self.shared.input_ch.send(group.to_string());
        }
        self.shared.input_ch.close();
    }

    /// Wait waits for all workers to finish.
    // `wait` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn wait(&mut self) {
        while let Some(worker) = self.workers.pop() {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
    }
}

// `generate_test_sqls_for_function` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn generate_test_sqls_for_function(
    shared: &Shared,
    client: &Client,
    group: &str,
) -> Result<Vec<Case>, String> {
    let exist_cases = shared.case_manager.exist_cases(group);
    if shared.test_case_count <= 0 || exist_cases.len() >= shared.test_case_count as usize {
        return Ok(Vec::new());
    }

    Global.Info(
        "generating test SQLs for function",
        &[
            zap::String("group", group),
            zap::Int("existCases", exist_cases.len() as i32),
            zap::Int("generateCount", shared.test_case_count),
        ],
    );

    let prompt = shared
        .prompt_generator
        .generate_prompt(group, shared.test_case_count, &exist_cases)
        .ok_or_else(|| "failed to generate prompt".to_string())?;

    let mut options: Vec<RequestOption> = Vec::new();
    if shared.model_name.contains("deepseek") {
        // Together always returns the reasoning part in the response.
        options.push(option::WithJSONSet(
            "provider",
            AnyValue::Object(vec![(
                "ignore".to_string(),
                AnyValue::Array(vec![AnyValue::String("Together".to_string())]),
            )]),
        ));
    }

    let params = ChatCompletionNewParams {
        model: shared.model_name.clone(),
        messages: prompt,
        // `JSON_SCHEMA` is not implemented by many models, so use `JSON_OBJECT` instead.
        response_format_type: openai::RESPONSE_FORMAT_JSON_OBJECT.to_string(),
        // Usually the input uses less than 250 tokens, and the output uses less than 5000 tokens.
        max_tokens: 6000,
    };

    let completion: ChatCompletion = client.chat_completions_new(params, options)?;

    Global.Debug(
        "chat completions raw response",
        &[zap::String("raw completion", completion.raw_json())],
    );
    if completion.choices.is_empty() {
        return Err("no completion choices".to_string());
    }

    let cases = shared
        .prompt_generator
        .unmarshal(&completion.choices[0].message.content);

    Global.Info("generated cases", &[zap::Any("queries", &cases)]);
    Ok(cases)
}

// `run_worker` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
fn run_worker(shared: Arc<Shared>) {
    let client = openai::NewClient(vec![
        option::WithAPIKey(shared.openai_token.clone()),
        option::WithBaseURL(shared.openai_base_url.clone()),
        // For deepseek series model, enable reasoning will remove the reasoning part from
        // the content. Ref https://openrouter.ai/docs/use-cases/reasoning-tokens.
        option::WithJSONSet("include_reasoning", AnyValue::Bool(true)),
    ]);

    while let Some(input) = shared.input_ch.recv() {
        let cases = match generate_test_sqls_for_function(&shared, &client, &input) {
            Ok(cases) => cases,
            Err(err) => {
                Global.Error("failed to generate test SQLs", &[zap::Error(err)]);
                continue;
            }
        };

        for c in cases {
            shared.case_manager.append_case(&input, c);
        }
    }
}
