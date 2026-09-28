// Copyright 2026 AsterSQL.

//! Rust/Go 公开契约对等测试：覆盖 source、组合器与 Transform 错误路径。
//! 断言值与 Finished/Err 语义需与 Go `iter` 包行为一致，防止移植漂移。
//! 本文件聚合烟雾级契约，细节场景见 combinator_test。
//! Transform 错误观测用有限次 TryNext，避免 CollectAll 在并发错误下阻塞。

use crate::{
    CollectAll, CollectMany, ConcatAll, Context, Done, Emit, Enumerate, Fail, FilterOut, FlatMap,
    FromSlice, Map, MapFilter, OfRange, TakeFirst, Tap, Throw, Transform, TryMap, WithBufferSize,
    WithConcurrency,
};

#[test]
fn go_rust_public_contract_matches() {
    let ctx = Context::background();

    // sources: slice / range / fail / func
    // 切片源按序产出全部元素。
    // FromSlice 耗尽后稳定 Done。
    let mut s = FromSlice(vec![1, 2, 3]);
    assert_eq!(CollectAll(&ctx, &mut *s).Item.unwrap(), vec![1, 2, 3]);

    // 半开区间 [1,4)。
    // 与 Go OfRange 上界不含一致。
    let mut r = OfRange(1i32, 4);
    assert_eq!(CollectAll(&ctx, &mut *r).Item.unwrap(), vec![1, 2, 3]);

    // Fail：立即 Throw，Finished=false。
    // 不把错误伪装成 Done。
    let mut f = Fail::<i32>("boom");
    let fr = f.TryNext(&ctx);
    assert_eq!(fr.Err.as_deref(), Some("boom"));
    assert!(!fr.Finished);

    let mut g = crate::Func(|_c| Emit(7i32));
    assert_eq!(g.TryNext(&ctx).Item, Some(7));
    // Func always calls generator; next call emits again unless generator tracks state
    // 无状态 Func 会反复 Emit；有状态闭包才能 Done。
    let mut once = {
        let mut done = false;
        crate::Func(move |_c| {
            if done {
                return Done();
            }
            done = true;
            Emit(9i32)
        })
    };
    assert_eq!(CollectAll(&ctx, &mut *once).Item.unwrap(), vec![9]);

    // Done/Emit/Throw display + FinishedOrError
    // Display 格式与 Go String() 约定对齐，便于日志对拍。
    // FinishedOrError 对 Done/Throw 为真，对 Emit 为假。
    assert_eq!(format!("{}", Done::<i32>()), "IterResult.Done()");
    assert_eq!(format!("{}", Emit(3)), "IterResult.Emit(3)");
    assert_eq!(
        format!("{}", Throw::<i32>("e".into())),
        "IterResult.Throw(e)"
    );
    assert!(Done::<i32>().FinishedOrError());
    assert!(Throw::<i32>("x".into()).FinishedOrError());
    assert!(!Emit(1).FinishedOrError());

    // combinators
    // Map 一对一变换。
    // 顺序保持，无并发重排。
    let mut mapped = Map(FromSlice(vec![1, 2, 3]), |x| x * 10);
    assert_eq!(
        CollectAll(&ctx, &mut *mapped).Item.unwrap(),
        vec![10, 20, 30]
    );

    // FilterOut：谓词为真则丢弃（此处丢掉偶数）。
    // 与标准库 filter 保留语义相反，需特别注意。
    let mut filtered = FilterOut(FromSlice(vec![1, 2, 3, 4]), |x| *x % 2 == 0);
    assert_eq!(CollectAll(&ctx, &mut *filtered).Item.unwrap(), vec![1, 3]);

    let mut taken = TakeFirst(FromSlice(vec![1, 2, 3, 4]), 2);
    assert_eq!(CollectAll(&ctx, &mut *taken).Item.unwrap(), vec![1, 2]);

    // CollectMany 截断收集，成功时 Finished=false。
    // 未消费完的上游剩余元素被丢弃（TakeFirst 截断）。
    let many = CollectMany(&ctx, FromSlice(vec![1, 2, 3, 4]), 2);
    assert_eq!(many.Item.unwrap(), vec![1, 2]);
    assert!(!many.Finished);

    let mut flat = FlatMap(FromSlice(vec![1, 2]), |x| FromSlice(vec![x, x + 10]));
    assert_eq!(
        CollectAll(&ctx, &mut *flat).Item.unwrap(),
        vec![1, 11, 2, 12]
    );

    // MapFilter：偶数映射为 *2 并保留，奇数 skip。
    let mut mf = MapFilter(FromSlice(vec![1, 2, 3, 4]), |x| {
        if x % 2 == 0 {
            (x * 2, false)
        } else {
            (0, true)
        }
    });
    assert_eq!(CollectAll(&ctx, &mut *mf).Item.unwrap(), vec![4, 8]);

    // TryMap：中途 Err 变为 Throw，已产出项不影响后续调用语义。
    let mut tm = TryMap(FromSlice(vec![1, 2, 3]), |x| {
        if x == 2 { Err("bad".into()) } else { Ok(x + 1) }
    });
    assert_eq!(tm.TryNext(&ctx).Item, Some(2));
    assert_eq!(tm.TryNext(&ctx).Err.as_deref(), Some("bad"));

    let mut cat = ConcatAll(vec![FromSlice(vec![1, 2]), FromSlice(vec![3])]);
    assert_eq!(CollectAll(&ctx, &mut *cat).Item.unwrap(), vec![1, 2, 3]);

    let mut en = Enumerate(FromSlice(vec!["a", "b"]));
    let e0 = en.TryNext(&ctx).Item.unwrap();
    assert_eq!(e0.Index, 0);
    assert_eq!(e0.Item, "a");
    let e1 = en.TryNext(&ctx).Item.unwrap();
    assert_eq!(e1.Index, 1);
    assert!(en.TryNext(&ctx).Finished);

    let mut tapped = 0;
    let mut tap = Tap(FromSlice(vec![1, 2]), move |_| tapped += 1);
    let _ = CollectAll(&ctx, &mut *tap);
    // tapped moved into closure; check via side channel
    // 用原子计数验证 Tap 副作用次数等于元素数。
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    let c2 = counter.clone();
    let mut tap2 = Tap(FromSlice(vec![1, 2, 3]), move |_| {
        c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let _ = CollectAll(&ctx, &mut *tap2);
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);

    // Transform (concurrent impure map)
    // 并发映射结果无序，排序后再比。
    let mut tr = Transform(
        FromSlice(vec![1, 2, 3, 4]),
        |_ctx, x| Ok(x * 2),
        vec![WithConcurrency(2), WithBufferSize(2)],
    );
    let mut got = CollectAll(&ctx, &mut *tr).Item.unwrap();
    got.sort();
    assert_eq!(got, vec![2, 4, 6, 8]);

    // Transform 内 mapper 报错应最终被观察到。
    let mut tr_err = Transform(
        FromSlice(vec![1, 2, 3]),
        |_ctx, x| {
            if x == 2 { Err("tf".into()) } else { Ok(x) }
        },
        vec![WithBufferSize(1)],
    );
    let mut saw_err = false;
    for _ in 0..8 {
        let r = tr_err.TryNext(&ctx);
        if r.Err.is_some() {
            saw_err = true;
            break;
        }
        if r.Finished {
            break;
        }
    }
    assert!(saw_err);

    // empty collect
    // 空源 CollectAll 得到空 Vec，且 Finished=false。
    let mut empty = FromSlice::<i32>(vec![]);
    let all = CollectAll(&ctx, &mut *empty);
    assert_eq!(all.Item.unwrap(), Vec::<i32>::new());
    assert!(!all.Finished);
}
