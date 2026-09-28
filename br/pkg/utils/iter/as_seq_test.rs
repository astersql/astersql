// Copyright 2026 AsterSQL.

//! Go `AsSeq` 契约测试：顺序产出、错误传播与消费者提前停止。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{AsSeq, Context, Done, Emit, Func, Throw};

#[test]
fn as_seq_yields_items_in_order() {
    let mut next = 0;
    let source = Func(move |_ctx| {
        if next == 3 {
            return Done();
        }
        let item = next;
        next += 1;
        Emit(item)
    });

    let got: Result<Vec<_>, _> = AsSeq(&Context::background(), source).collect();
    assert_eq!(got.expect("sequence succeeds"), vec![0, 1, 2]);
}

#[test]
fn as_seq_yields_error_and_continues_when_consumer_continues() {
    let mut calls = 0;
    let source = Func(move |_ctx| {
        calls += 1;
        match calls {
            1 => Emit(7),
            2 => Throw("boom".into()),
            _ => Emit(9),
        }
    });

    let mut seq = AsSeq(&Context::background(), source);
    assert_eq!(seq.next(), Some(Ok(7)));
    assert_eq!(seq.next(), Some(Err("boom".into())));
    assert_eq!(seq.next(), Some(Ok(9)));
}

#[test]
fn as_seq_does_not_pull_after_consumer_stops() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let source = Func(move |_ctx| {
        let n = observed.fetch_add(1, Ordering::SeqCst);
        Emit(n)
    });

    let got: Vec<_> = AsSeq(&Context::background(), source).take(2).collect();
    assert_eq!(got, vec![Ok(0), Ok(1)]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
