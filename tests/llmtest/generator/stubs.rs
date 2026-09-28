// Copyright 2026 AsterSQL.
//! Local stand-ins for `github.com/openai/openai-go` (+ option)
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Production algorithms live in the generator modules and call these
//! boundaries the same way Go calls the OpenAI SDK.

// 本文件对应 `tests/llmtest/generator/stubs.rs`，本次任务只补中文解释，不改行为。
// 本文件提供轻量测试桩，而不是完整生产实现。
// 桩只覆盖当前测试真正触达的接口形状。
// 关键阅读点是全局开关、记录点和资源回收。
// 未覆盖的真实能力不会被假装支持。
// 中文注释会帮助区分桩职责与真实边界。
// 这类文件最怕隐式状态污染，因此会强调 reset 和 cleanup。
use astersql_tests_llmtest_testcase::AnyValue;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// When true, [`simple_prompt_response_marshal`] fails (parity for rare marshal errors).
// `FORCE_MARSHAL_ERR` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static FORCE_MARSHAL_ERR: AtomicBool = AtomicBool::new(false);

/// Test hook: force JSON marshal of `simplePromptResponse` to fail.
// `set_force_marshal_error` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
pub fn set_force_marshal_error(force: bool) {
    FORCE_MARSHAL_ERR.store(force, Ordering::SeqCst);
}

// `marshal_should_fail` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
pub(crate) fn marshal_should_fail() -> bool {
    FORCE_MARSHAL_ERR.load(Ordering::SeqCst)
}

// `CompletionsFn` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
type CompletionsFn = Arc<
    dyn Fn(&ChatCompletionNewParams, &[RequestOption]) -> Result<ChatCompletion, String>
        + Send
        + Sync,
>;

// `COMPLETIONS_HANDLER` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static COMPLETIONS_HANDLER: Mutex<Option<CompletionsFn>> = Mutex::new(None);
// `LAST_COMPLETION_PARAMS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static LAST_COMPLETION_PARAMS: Mutex<Option<ChatCompletionNewParams>> = Mutex::new(None);
// `LAST_COMPLETION_OPTS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static LAST_COMPLETION_OPTS: Mutex<Vec<RequestOption>> = Mutex::new(Vec::new());
// `LAST_CLIENT_OPTS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static LAST_CLIENT_OPTS: Mutex<Vec<RequestOption>> = Mutex::new(Vec::new());

/// Install the scripted Chat Completions handler used by [`openai::Client`].
// `set_completions_handler` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
pub fn set_completions_handler<F>(handler: F)
where
    F: Fn(&ChatCompletionNewParams, &[RequestOption]) -> Result<ChatCompletion, String>
        + Send
        + Sync
        + 'static,
{
    let mut g = COMPLETIONS_HANDLER
        .lock()
        .expect("completions handler mutex");
    *g = Some(Arc::new(handler) as CompletionsFn);
}

/// Clear the scripted Chat Completions handler.
// `clear_completions_handler` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
pub fn clear_completions_handler() {
    let mut g = COMPLETIONS_HANDLER
        .lock()
        .expect("completions handler mutex");
    *g = None;
}

/// Last `ChatCompletionNewParams` observed by the stub client (tests).
// `last_completion_params` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
pub fn last_completion_params() -> Option<ChatCompletionNewParams> {
    LAST_COMPLETION_PARAMS
        .lock()
        .expect("last params mutex")
        .clone()
}

/// Extra request options passed to Completions.New (tests).
// `last_completion_options` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
pub fn last_completion_options() -> Vec<RequestOption> {
    LAST_COMPLETION_OPTS
        .lock()
        .expect("last opts mutex")
        .clone()
}

/// Options passed to [`openai::NewClient`] (tests).
// `last_client_options` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
pub fn last_client_options() -> Vec<RequestOption> {
    LAST_CLIENT_OPTS
        .lock()
        .expect("last client opts mutex")
        .clone()
}

/// Clear captured stub request state.
// `clear_captured_requests` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
pub fn clear_captured_requests() {
    *LAST_COMPLETION_PARAMS.lock().expect("last params") = None;
    LAST_COMPLETION_OPTS.lock().expect("last opts").clear();
    LAST_CLIENT_OPTS.lock().expect("last client opts").clear();
}

/// OpenAI request option stand-in (`option.RequestOption`).
#[derive(Clone, Debug, PartialEq)]
// `RequestOption` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub enum RequestOption {
    ApiKey(String),
    BaseUrl(String),
    JsonSet { key: String, value: AnyValue },
}

// 模块 `option` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod option {
    use super::{AnyValue, RequestOption};

    // `WithAPIKey` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn WithAPIKey(key: impl Into<String>) -> RequestOption {
        RequestOption::ApiKey(key.into())
    }

    // `WithBaseURL` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn WithBaseURL(url: impl Into<String>) -> RequestOption {
        RequestOption::BaseUrl(url.into())
    }

    // `WithJSONSet` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn WithJSONSet(key: impl Into<String>, value: AnyValue) -> RequestOption {
        RequestOption::JsonSet {
            key: key.into(),
            value,
        }
    }
}

/// Chat completion request / response types mirroring the openai-go surface used here.
#[derive(Clone, Debug, PartialEq)]
// `ChatCompletionNewParams` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct ChatCompletionNewParams {
    pub model: String,
    pub messages: Vec<ChatCompletionMessageParamUnion>,
    pub response_format_type: String,
    pub max_tokens: i64,
}

#[derive(Clone, Debug, PartialEq)]
// `ChatCompletion` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct ChatCompletion {
    pub choices: Vec<ChatCompletionChoice>,
    pub raw_json: String,
}

#[derive(Clone, Debug, PartialEq)]
// `ChatCompletionChoice` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct ChatCompletionChoice {
    pub message: ChatCompletionMessage,
}

#[derive(Clone, Debug, PartialEq)]
// `ChatCompletionMessage` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct ChatCompletionMessage {
    pub content: String,
}

// 这里实现 `ChatCompletion` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl ChatCompletion {
    /// Go `completion.JSON.RawJSON()`.
    // `raw_json` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn raw_json(&self) -> &str {
        &self.raw_json
    }
}

/// Message union used in chat prompts (`openai.ChatCompletionMessageParamUnion`).
#[derive(Clone, Debug, PartialEq, Eq)]
// `ChatCompletionMessageParamUnion` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub enum ChatCompletionMessageParamUnion {
    System { content: String },
    User { content: String },
    Assistant { content: String },
}

// 这里实现 `ChatCompletionMessageParamUnion` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl ChatCompletionMessageParamUnion {
    // `role` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn role(&self) -> &'static str {
        match self {
            Self::System { .. } => "system",
            Self::User { .. } => "user",
            Self::Assistant { .. } => "assistant",
        }
    }

    // `content` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn content(&self) -> &str {
        match self {
            Self::System { content } | Self::User { content } | Self::Assistant { content } => {
                content
            }
        }
    }
}

// 模块 `openai` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod openai {
    use super::*;

    // `ChatCompletionMessageParamUnion` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub type ChatCompletionMessageParamUnion = super::ChatCompletionMessageParamUnion;
    // `ChatCompletionNewParams` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub type ChatCompletionNewParams = super::ChatCompletionNewParams;
    // `ChatCompletion` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub type ChatCompletion = super::ChatCompletion;

    // `SystemMessage` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn SystemMessage(content: impl Into<String>) -> ChatCompletionMessageParamUnion {
        ChatCompletionMessageParamUnion::System {
            content: content.into(),
        }
    }

    // `UserMessage` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn UserMessage(content: impl Into<String>) -> ChatCompletionMessageParamUnion {
        ChatCompletionMessageParamUnion::User {
            content: content.into(),
        }
    }

    // `AssistantMessage` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn AssistantMessage(content: impl Into<String>) -> ChatCompletionMessageParamUnion {
        ChatCompletionMessageParamUnion::Assistant {
            content: content.into(),
        }
    }

    /// Identity wrapper matching Go `openai.F(...)` generics helper.
    // `F` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn F<T>(v: T) -> T {
        v
    }

    // `RESPONSE_FORMAT_JSON_OBJECT` 记录跨函数共享的固定约束、错误文本或全局状态。
    // 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
    // 中文注释强调它为什么需要稳定。
    pub const RESPONSE_FORMAT_JSON_OBJECT: &str = "json_object";

    /// Go `openai.NewClient(options...)`.
    // `NewClient` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn NewClient(options: Vec<RequestOption>) -> Client {
        if let Ok(mut g) = LAST_CLIENT_OPTS.lock() {
            *g = options.clone();
        }
        let mut api_key = String::new();
        let mut base_url = String::new();
        let mut extra = Vec::new();
        for opt in options {
            match opt {
                RequestOption::ApiKey(k) => api_key = k,
                RequestOption::BaseUrl(u) => base_url = u,
                other => extra.push(other),
            }
        }
        Client {
            api_key,
            base_url,
            client_extra: extra,
        }
    }

    #[derive(Clone, Debug)]
    // `Client` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct Client {
        pub api_key: String,
        pub base_url: String,
        pub client_extra: Vec<RequestOption>,
    }

    // 这里实现 `Client` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl Client {
        /// Go `client.Chat.Completions.New(ctx, params, opts...)`.
        // `chat_completions_new` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn chat_completions_new(
            &self,
            params: ChatCompletionNewParams,
            options: Vec<RequestOption>,
        ) -> Result<ChatCompletion, String> {
            if let Ok(mut g) = LAST_COMPLETION_PARAMS.lock() {
                *g = Some(params.clone());
            }
            if let Ok(mut g) = LAST_COMPLETION_OPTS.lock() {
                *g = options.clone();
            }
            let handler = COMPLETIONS_HANDLER
                .lock()
                .expect("completions handler")
                .clone();
            match handler {
                Some(h) => h(&params, &options),
                None => Err("openai stub: no completions handler configured".to_string()),
            }
        }
    }
}

// 这里实现 `fmt::Display` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl fmt::Display for RequestOption {
    // `fmt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey(_) => write!(f, "ApiKey(***)"),
            Self::BaseUrl(u) => write!(f, "BaseUrl({u})"),
            Self::JsonSet { key, value } => write!(f, "JsonSet({key}={value})"),
        }
    }
}
