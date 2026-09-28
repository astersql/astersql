// Copyright 2026 AsterSQL.
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

// skip 迁移回归：校验 short/long 条件下的 helper、skip 文案与命令行解析。
//
// 用可记录调用的假测试上下文对照 Go 行为，不依赖真实 testing.T。

use std::any::Any;

use crate::{TestContext, not_under_long_with, short_from_args, under_short_with};

/// 记录 helper/skip 调用的假 `TestContext`，用于断言跳过逻辑。
#[derive(Default)]
struct RecordingTest {
    helper_calls: usize,
    skip_calls: Vec<Vec<String>>,
}

impl TestContext for RecordingTest {
    fn helper(&mut self) {
        self.helper_calls += 1;
    }

    fn skip(&mut self, args: &[&dyn Any]) -> ! {
        // 将 Any 参数渲染为字符串，便于与 Go 跳过文案逐项比对。
        self.skip_calls
            .push(args.iter().map(render_argument).collect());
        std::panic::panic_any("test skipped")
    }
}

/// 将测试 skip 参数按常见类型渲染为字符串；未知类型 panic。
fn render_argument(argument: &&dyn Any) -> String {
    if let Some(value) = argument.downcast_ref::<&str>() {
        (*value).to_owned()
    } else if let Some(value) = argument.downcast_ref::<String>() {
        value.clone()
    } else if let Some(value) = argument.downcast_ref::<i32>() {
        value.to_string()
    } else {
        panic!("unexpected test argument type")
    }
}

/// 非 short 不跳过；short 时调用 skip，前缀为 Go 文案 `disabled under -short`。
#[test]
fn under_short_marks_helper_and_only_skips_in_short_mode() {
    let mut regular = RecordingTest::default();
    under_short_with(&mut regular, false, &[&"details", &7_i32]);
    assert_eq!(regular.helper_calls, 1);
    assert!(regular.skip_calls.is_empty());

    let mut short = RecordingTest::default();
    let skipped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        under_short_with(&mut short, true, &[&"details", &7_i32]);
    }));
    assert!(skipped.is_err());
    assert_eq!(short.helper_calls, 1);
    assert_eq!(
        short.skip_calls,
        vec![vec![
            "disabled under -short".to_owned(),
            "details".to_owned(),
            "7".to_owned(),
        ]]
    );
}

/// long=true 不跳过；否则跳过，前缀保留 Go 文案 `disabled not under -short`。
#[test]
fn not_under_long_marks_helper_and_preserves_go_skip_text() {
    let mut long = RecordingTest::default();
    not_under_long_with(&mut long, true, &[&"details"]);
    assert_eq!(long.helper_calls, 1);
    assert!(long.skip_calls.is_empty());

    let mut regular = RecordingTest::default();
    let skipped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        not_under_long_with(&mut regular, false, &[&"details"]);
    }));
    assert!(skipped.is_err());
    assert_eq!(regular.helper_calls, 1);
    assert_eq!(
        regular.skip_calls,
        vec![vec![
            "disabled not under -short".to_owned(),
            "details".to_owned(),
        ]]
    );
}

/// short 标志解析兼容 Go 布尔写法，且以后出现的同名标志为准。
#[test]
fn short_flag_matches_go_boolean_flag_forms_and_last_value() {
    assert!(!short_from_args(["test-binary"]));
    assert!(short_from_args(["test-binary", "-test.short"]));
    assert!(short_from_args(["test-binary", "-test.short=true"]));
    assert!(short_from_args(["test-binary", "--short=1"]));
    assert!(!short_from_args([
        "test-binary",
        "-test.short",
        "-test.short=false",
    ]));
}

/// Go `testing.T.Skip` terminates the current test goroutine instead of returning.
#[test]
fn skip_stops_test_execution_like_go() {
    let mut test = RecordingTest::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        under_short_with(&mut test, true, &[]);
    }));
    assert!(
        result.is_err(),
        "execution continued after TestContext::skip"
    );
}
