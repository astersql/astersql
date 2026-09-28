// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 迁移对照单测：校验与 Go `intest` 一致的断言消息与 nil/错误行为。
//
// 侧重格式串（含 `%+v`/`%v`）以及 `AssertNotNil`/`AssertFunc`/`AssertNoError`
// 的核心路径，作为 Aster 迁移后的行为锚定。

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

use super::{Assert, AssertArg, AssertFunc, AssertNoError, AssertNotNil, InTest};

/// 从 panic payload 取字符串；非字符串则失败。
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_owned())
        })
        .expect("assertion panic must contain a string")
}

/// 断言 `check` 会 panic 且消息等于 `expected`。
fn assert_panics(expected: &str, check: impl FnOnce()) {
    let payload = panic::catch_unwind(AssertUnwindSafe(check)).expect_err("assertion must panic");
    assert_eq!(panic_message(payload), expected);
}

fn true_fn() -> bool {
    true
}

fn false_fn() -> bool {
    false
}

fn panic_fn() -> bool {
    panic!("inner panic1")
}

/// 对照 Go：条件断言与 `%s`/`%d`/`%+v`/`%v` 消息格式化。
#[test]
fn go_assert_and_message_formatting_behavior() {
    assert!(InTest.load(std::sync::atomic::Ordering::SeqCst));
    Assert(true, &[]);
    assert_panics("assert failed", || Assert(false, &[]));
    assert_panics("assert failed, msg1", || {
        Assert(false, &[AssertArg::from("msg1")])
    });
    assert_panics("assert failed, msg2 a b 1", || {
        Assert(
            false,
            &[
                AssertArg::from("msg2 %s %s %d"),
                AssertArg::from("a"),
                AssertArg::from("b"),
                AssertArg::from(1),
            ],
        )
    });
    assert_panics("assert failed, 123", || {
        Assert(false, &[AssertArg::from(123)])
    });
    assert_panics("assert failed, value=7", || {
        Assert(false, &[AssertArg::from("value=%+v"), AssertArg::from(7)])
    });
    assert_panics("assert failed, ratio=1.5", || {
        Assert(
            false,
            &[AssertArg::from("ratio=%v"), AssertArg::from(1.5_f64)],
        )
    });
}

/// 对照 Go：nil、函数与错误断言的成功/失败消息。
#[test]
fn go_nil_function_and_error_behavior() {
    AssertNotNil(Some(""), &[]);
    AssertNotNil(Some("abc"), &[]);
    AssertNotNil(Some(0), &[]);
    AssertNotNil(Some(123), &[]);
    AssertNotNil(Some(false), &[]);
    assert_panics("assert failed", || AssertNotNil::<i32>(None, &[]));
    assert_panics("assert failed, msg1", || {
        AssertNotNil::<i32>(None, &[AssertArg::from("msg1")])
    });
    assert_panics("assert failed, msg2 a b 1", || {
        AssertNotNil::<i32>(
            None,
            &[
                AssertArg::from("msg2 %s %s %d"),
                AssertArg::from("a"),
                AssertArg::from("b"),
                AssertArg::from(1),
            ],
        )
    });
    AssertFunc(Some(true_fn), &[]);
    assert_panics("assert failed", || AssertFunc(Some(false_fn), &[]));
    assert_panics("assert failed, msg3", || {
        AssertFunc(Some(false_fn), &[AssertArg::from("msg3")])
    });
    assert_panics("assert failed, msg4 c d 2", || {
        AssertFunc(
            Some(false_fn),
            &[
                AssertArg::from("msg4 %s %s %d"),
                AssertArg::from("c"),
                AssertArg::from("d"),
                AssertArg::from(2),
            ],
        )
    });
    assert_panics("inner panic1", || AssertFunc(Some(panic_fn), &[]));
    assert_panics("assert failed", || AssertFunc(None, &[]));
    AssertNoError(None, &[]);
    let error = "mock err1";
    assert_panics("assert failed, error is not nil: mock err1", || {
        AssertNoError(Some(&error), &[])
    });
}
