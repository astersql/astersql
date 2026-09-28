// Copyright 2026 AsterSQL.

//! `bufferedMapping` 背压契约：结果被消费后才释放 outstanding 配额。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::{CollectAll, Context, Done, Emit, Func, Transform, WithBufferSize, WithConcurrency};

#[test]
fn transform_does_not_pull_past_unconsumed_buffer() {
    let pulls = Arc::new(AtomicUsize::new(0));
    let observed = pulls.clone();
    let source = Func(move |_ctx| {
        let n = observed.fetch_add(1, Ordering::SeqCst);
        if n == 100 { Done() } else { Emit(n) }
    });
    let mut transformed = Transform(
        source,
        |_ctx, item| Ok(item),
        vec![WithBufferSize(2), WithConcurrency(2)],
    );

    assert!(transformed.TryNext(&Context::background()).Item.is_some());
    thread::sleep(Duration::from_millis(50));

    // Go outstanding capacity is two. Consuming one result permits exactly one
    // additional upstream pull, so at most three pulls may have occurred.
    assert!(pulls.load(Ordering::SeqCst) <= 3);

    // Drain the iterator so the producer has no background work when the test exits.
    let _ = CollectAll(&Context::background(), &mut *transformed);
}
