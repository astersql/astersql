// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.
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

//! 组合器行为测试，对齐 Go `combinator_test.go` 场景与断言。
//! 覆盖并发 Transform、Filter/Enumerate、失败传播、Collect/Tap 与超时取消。
//! 并发用例依赖 sleep+高并发，在慢机器上可能因超时变脆，需保持与 Go 相同参数。
//! result_eq 用于精确三态比较；集合比较则用排序后的多重集相等。

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::{
    CollectAll, CollectMany, ConcatAll, Context, Done, Emit, Enumerate, Fail, FilterOut, FlatMap,
    Indexed, IterResult, Map, OfRange, Tap, Transform, WithBufferSize, WithConcurrency,
};

/// Go `context.WithTimeout` — cancel after `dur` via `WithCancel`.
/// 用后台 sleep + Cancel 模拟超时 context。
fn with_timeout(dur: Duration) -> (Context, crate::CancelFunc) {
    let (cx, cancel) = Context::WithCancel(&Context::background());
    let cancel_timer = cancel.clone();
    thread::spawn(move || {
        thread::sleep(dur);
        cancel_timer.Cancel();
    });
    (cx, cancel)
}

/// 排序后比较，忽略并发 Transform 的产出顺序。
fn assert_elements_match<T: Ord + Clone + std::fmt::Debug>(mut a: Vec<T>, mut b: Vec<T>) {
    a.sort();
    b.sort();
    assert_eq!(a, b);
}

/// 逐字段比较 IterResult，失败时打印 got/want。
fn result_eq<T: PartialEq + std::fmt::Debug>(got: IterResult<T>, want: IterResult<T>) {
    assert_eq!(
        got.Finished, want.Finished,
        "Finished mismatch: {got} vs {want}"
    );
    assert_eq!(got.Err, want.Err, "Err mismatch: {got} vs {want}");
    assert_eq!(got.Item, want.Item, "Item mismatch: {got} vs {want}");
}

// test_par_trans 对应 TestParTrans。
// context timeout、buffer=128、concurrency=64 下并发映射 200 个元素。
// 每项 sleep 100ms，依赖高并发在约 1s 超时内完成。
#[test]
fn test_par_trans() {
    let items = OfRange(0i32, 200);
    let mut mapped = Transform(
        items,
        |ctx: &Context, i: i32| -> Result<i32, String> {
            if ctx.Done() {
                return Err(ctx.Err());
            }
            thread::sleep(Duration::from_millis(100));
            Ok(i + 100)
        },
        vec![WithBufferSize(128), WithConcurrency(64)],
    );
    let (cx, cancel) = with_timeout(Duration::from_secs(1));
    let r = CollectAll(&cx, &mut *mapped);
    assert!(r.Err.is_none(), "{r}");
    let got = r.Item.expect("items");
    assert_eq!(got.len(), 200);
    let mut expect = OfRange(100i32, 300);
    let expect_items = CollectAll(&cx, &mut *expect).Item.expect("expect");
    // 并发产出无序，用多重集相等比较。
    assert_elements_match(expect_items, got);
    cancel.Cancel();
}

// test_filter 对应 TestFilter。
// FlatMap 为每个 n 生成 n..10 的乘积，再过滤掉 0 和非 13 倍数减一的值。
#[test]
fn test_filter() {
    let items = OfRange(0i32, 10);
    let items = FlatMap(items, |n| Map(OfRange(n, 10), move |i| n * i));
    let mut items = FilterOut(items, |n| *n == 0 || (*n + 1) % 13 != 0);
    let coll = CollectAll(&Context::background(), &mut *items);
    assert!(coll.Err.is_none(), "{coll}");
    // 期望集合与 Go 测试硬编码一致。
    assert_eq!(coll.Item.as_ref().expect("items"), &vec![12, 12, 25, 64]);
}

// test_enumerate 对应 TestEnumerate。
// Enumerate 生成 (Index, Item)，随后过滤偶数 Item，断言 Index 与原 Item 一致。
#[test]
fn test_enumerate() {
    let items = OfRange(0i32, 10);
    let enums = Enumerate(items);
    let mut enums = FilterOut(enums, |ni: &Indexed<i32>| ni.Item % 2 == 0);
    let coll = CollectAll(&Context::background(), &mut *enums);
    let expects = [1, 3, 5, 7, 9];
    let items = coll.Item.expect("items");
    for (i, col) in items.iter().enumerate() {
        // 对 OfRange(0..) 过滤偶数后，Index 仍等于原值。
        assert_eq!(col.Item, col.Index);
        assert_eq!(expects[i], col.Item);
    }
}

// test_failure 对应 TestFailure。
// ConcatAll 在两个 range 中间插入 Fail，验证后续组合器遇到上游错误时不返回部分 item。
#[test]
fn test_failure() {
    let items = ConcatAll(vec![
        OfRange(0i32, 5),
        Fail::<i32>("meow?"),
        OfRange(5i32, 10),
    ]);
    let items = FlatMap(items, |n| Map(OfRange(n, 10), move |i| n * i));
    let mut items = FilterOut(items, |n| *n == 0 || (*n + 1) % 13 != 0);
    let coll = CollectAll(&Context::background(), &mut *items);
    // 失败必须传播，且 Item 为空（无部分结果）。
    assert!(coll.Err.is_some(), "{coll}");
    assert!(coll.Item.is_none(), "{coll}");
}

// test_collect 对应 TestCollect。
// CollectMany 只收集前 10 个元素，并与 CollectAll(range 0..10) 的结果比较。
#[test]
fn test_collect() {
    let items = OfRange(0i32, 100);
    let ctx = Context::background();
    let coll = CollectMany(&ctx, items, 10);
    assert!(coll.Err.is_none(), "{coll}");
    let got = coll.Item.as_ref().expect("items");
    assert_eq!(got.len(), 10);
    let mut expect = OfRange(0i32, 10);
    assert_eq!(
        got,
        CollectAll(&ctx, &mut *expect)
            .Item
            .as_ref()
            .expect("expect")
    );
}

// test_tapping 对应 TestTapping。
// Tap 的闭包有副作用：累加已消费元素；CollectAll 驱动迭代器真正执行。
#[test]
fn test_tapping() {
    let items = OfRange(0i32, 101);
    let ctx = Context::background();
    let n = Arc::new(Mutex::new(0i32));
    let n2 = n.clone();
    let mut items = Tap(items, move |i| {
        *n2.lock().unwrap() += *i;
    });
    let _ = CollectAll(&ctx, &mut *items);
    // 0..=100 之和为 5050。
    assert_eq!(*n.lock().unwrap(), 5050);
}

// test_some 对应 TestSome。
// TryNext 在耗尽后应稳定返回 Done；Go 测试连续调用两次 Done。
#[test]
fn test_some() {
    let mut it = OfRange(0i32, 2);
    let c = Context::background();
    result_eq(it.TryNext(&c), Emit(0));
    result_eq(it.TryNext(&c), Emit(1));
    result_eq(it.TryNext(&c), Done::<i32>());
    // 二次 Done：耗尽后幂等。
    result_eq(it.TryNext(&c), Done::<i32>());
}

// test_error_during_transforming 对应 TestErrorDuringTransforming。
// Transform 闭包在 i == 10 时返回错误，CollectAll 应把该错误传播出来。
#[test]
fn test_error_during_transforming() {
    let items = OfRange(1i32, 20);
    let mut items = Transform(
        items,
        |_ctx: &Context, i: i32| -> Result<i32, String> {
            if i == 10 {
                return Err("meow".into());
            }
            Ok(i)
        },
        vec![WithBufferSize(16), WithConcurrency(8)],
    );

    let coll = CollectAll(&Context::background(), &mut *items);
    let err = coll.Err.expect("error");
    assert!(err.contains("meow"), "{err}");
}

// test_error_before_transforming 对应 TestErrorBeforeTransforming。
// 上游 Fail 在 Transform 启动前出现；用 channel 与超时防止错误传播时阻塞。
#[test]
fn test_error_before_transforming() {
    let mut items = Transform(
        Fail::<i32>("meow"),
        |_ctx: &Context, _i: i32| -> Result<i32, String> { Ok(0) },
        vec![WithBufferSize(1)],
    );

    // 另线程 Collect，主线程 recv_timeout，捕捉死锁回归。
    let (tx, rx) = mpsc::channel::<IterResult<Vec<i32>>>();
    thread::spawn(move || {
        let _ = tx.send(CollectAll(&Context::background(), &mut *items));
    });

    match rx.recv_timeout(Duration::from_secs(1)) {
        Ok(coll) => {
            let err = coll.Err.expect("error");
            assert!(err.contains("meow"), "{err}");
        }
        Err(_) => panic!("Transform blocked while propagating upstream error"),
    }
}
