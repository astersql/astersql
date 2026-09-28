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

// `intest` 断言 API 的单元测试：消息格式、nil/函数/错误断言与开关行为。
//
// 通过 `catch_unwind` 捕获 panic 文本，对齐 Go 侧断言失败消息；
// `TEST_LOCK` 串行化对全局开关的并发修改。

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use super::{
    Assert, AssertArg, AssertFunc, AssertNoError, AssertNotNil, EnableAssert, EnableInternalCheck,
    InTest,
};

/// 串行化修改全局断言开关的测试，避免互相干扰。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 占位类型，用于非空断言覆盖。
struct Foo;

/// 从 panic payload 提取字符串消息。
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    String::from("non-string panic")
}

/// 执行 `check` 并断言其 panic 消息等于 `expected`。
fn assert_panics(expected: &str, check: impl FnOnce()) {
    let panic = panic::catch_unwind(AssertUnwindSafe(check)).expect_err("assertion must panic");
    assert_eq!(panic_message(panic), expected);
}

/// 覆盖 `Assert`：真值通过、假值与格式化消息 panic。
#[test]
fn test_assert() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
    assert!(InTest.load(std::sync::atomic::Ordering::SeqCst));
    EnableAssert.store(true, Ordering::Relaxed);
    EnableInternalCheck.store(true, Ordering::Relaxed);

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
}

/// 覆盖 `AssertNotNil`：各类 `Some` 通过，`None` 带消息 panic。
#[test]
fn test_assert_not_nil() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
    AssertNotNil(Some(""), &[]);
    AssertNotNil(Some("abc"), &[]);
    AssertNotNil(Some(0), &[]);
    AssertNotNil(Some(123), &[]);
    AssertNotNil(Some(true), &[]);
    AssertNotNil(Some(false), &[]);
    AssertNotNil(Some(Foo), &[]);
    AssertNotNil(Some(Box::new(Foo)), &[]);
    AssertNotNil(Some(true_fn as fn() -> bool), &[]);
    AssertNotNil(Some(false_fn as fn() -> bool), &[]);
    AssertNotNil(Some(string_fn as fn(&str) -> bool), &[]);

    assert_panics("assert failed", || AssertNotNil::<i32>(None, &[]));
    assert_panics("assert failed", || AssertNotNil::<Box<Foo>>(None, &[]));
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
}

fn true_fn() -> bool {
    true
}

fn false_fn() -> bool {
    false
}

fn string_fn(_: &str) -> bool {
    true
}

fn panic_fn_1() -> bool {
    panic!("inner panic1")
}

fn panic_fn_2() -> bool {
    panic!("inner panic2")
}

/// 覆盖 `AssertFunc`：真函数通过，假/空函数与内部 panic 透传。
#[test]
fn test_assert_func() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
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
    assert_panics("assert failed", || AssertFunc(None, &[]));
    assert_panics("inner panic1", || AssertFunc(Some(panic_fn_1), &[]));
    assert_panics("inner panic2", || AssertFunc(Some(panic_fn_2), &[]));
}

/// 覆盖 `AssertNoError`：无错通过，有错时消息含错误文本。
#[test]
fn test_assert_no_error() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
    AssertNoError(None, &[]);
    let error = "mock err1";
    assert_panics("assert failed, error is not nil: mock err1", || {
        AssertNoError(Some(&error), &[])
    });
}

/// 验证双开关均关时不检查；仅开 `EnableInternalCheck` 时仍会断言。
#[test]
fn test_assert_switches() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
    EnableAssert.store(false, Ordering::Relaxed);
    EnableInternalCheck.store(false, Ordering::Relaxed);
    Assert(false, &[]);

    EnableInternalCheck.store(true, Ordering::Relaxed);
    assert_panics("assert failed", || Assert(false, &[]));

    EnableAssert.store(true, Ordering::Relaxed);
    EnableInternalCheck.store(true, Ordering::Relaxed);
}
