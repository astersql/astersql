// Copyright 2026 AsterSQL.

use crate::{ChatCompletionMessageParamUnion, PromptGenerator, new};
use astersql_tests_llmtest_testcase::{Case, Manager};
use std::sync::Arc;

struct PanickingPromptGenerator;

impl PromptGenerator for PanickingPromptGenerator {
    fn name(&self) -> &'static str {
        "panicking"
    }

    fn groups(&self) -> Vec<&'static str> {
        vec!["panic"]
    }

    fn generate_prompt(
        &self,
        _group: &str,
        _count: i32,
        _exist_cases: &[Case],
    ) -> Option<Vec<ChatCompletionMessageParamUnion>> {
        panic!("worker prompt panic")
    }

    fn unmarshal(&self, _response: &str) -> Vec<Case> {
        unreachable!("generate_prompt panics first")
    }
}

#[test]
#[should_panic(expected = "worker prompt panic")]
fn wait_propagates_worker_panic_like_go() {
    let manager = Arc::new(Manager::new_empty("unused.json"));
    let mut generator = new(
        manager,
        1,
        String::new(),
        String::new(),
        String::new(),
        Arc::new(PanickingPromptGenerator),
        1,
    );

    generator.run();
    generator.wait();
}
