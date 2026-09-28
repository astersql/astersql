// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::time::Duration;

use astersql_br_pkg_utiltest_fakecluster::{Context as FcContext, SubscribeFlushEventRequest};

use crate::{Context, NewPDSimWithTestContext, NewTestContextWithSeed};

#[test]
fn flush_store_observes_cancellation_after_the_call_starts() {
    let tc = NewTestContextWithSeed(7);
    let pd = NewPDSimWithTestContext(Vec::new(), String::new(), &tc).unwrap();
    let store = pd.cluster.EnsureStore(1, 1);
    let _stream = store
        .SubscribeFlushEvent(
            FcContext::background(),
            &SubscribeFlushEventRequest::default(),
        )
        .unwrap();

    for checkpoint in 1..=1024 {
        pd.flushStore(&Context::background(), 1, pd.task_start() + checkpoint)
            .unwrap();
    }

    let (ctx, cancel) = Context::with_cancel();
    let pd_for_flush = Arc::clone(&pd);
    let flush = std::thread::spawn(move || {
        pd_for_flush.flushStore(&ctx, 1, pd_for_flush.task_start() + 1025)
    });
    std::thread::sleep(Duration::from_millis(20));
    cancel.cancel();

    let err = flush.join().unwrap().unwrap_err();
    assert!(err.to_string().contains("context canceled"), "{err}");
}
