// Copyright 2026 AsterSQL.

//! Crate entry for `tests/llmtest/generator`
//! (Go package `github.com/pingcap/tidb/tests/llmtest/generator`).

// 本文件对应 `tests/llmtest/generator/lib.rs`，本次任务只补中文解释，不改行为。
// 本文件主要负责模块接线和导出关系说明。
// 阅读时关注哪些模块只在测试条件下启用。
// 中文注释只帮助快速判断依赖方向。
#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

#[path = "stubs.rs"]
// 模块 `stubs` 在这里被显式接线，方便按既定边界编译。
pub mod stubs;

#[path = "prompt.rs"]
// 模块 `prompt` 在这里被显式接线，方便按既定边界编译。
mod prompt;

#[path = "expression.rs"]
// 模块 `expression` 在这里被显式接线，方便按既定边界编译。
mod expression;

#[path = "dml.rs"]
// 模块 `dml` 在这里被显式接线，方便按既定边界编译。
mod dml;

#[path = "misc.rs"]
// 模块 `misc` 在这里被显式接线，方便按既定边界编译。
mod misc;

#[path = "generator.rs"]
// 模块 `generator` 在这里被显式接线，方便按既定边界编译。
mod generator;

pub use generator::{TestCaseGenerator, new};
pub use prompt::{
    PromptGenerator, SimplePromptResponse, all_prompt_generators, ensure_init,
    get_prompt_generator, register_prompt_generator,
};
pub use stubs::openai::ChatCompletionMessageParamUnion;
pub use stubs::{RequestOption, option};

#[cfg(test)]
#[path = "parity_test.rs"]
// 模块 `parity_test` 在这里被显式接线，方便按既定边界编译。
mod parity_test;

#[cfg(test)]
mod dml_test;

#[cfg(test)]
mod generator_test;
