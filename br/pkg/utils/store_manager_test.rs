// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use astersql_errors::SharedError;

use super::kvproto::metapb::Store;
use super::store_manager::{GrpcConn, GrpcConnFactory, KeepaliveParams, PdClient, StoreManager};
use super::stubs::context::Context;

#[derive(Default)]
struct TestConn {
    closes: AtomicUsize,
}

impl GrpcConn for TestConn {
    fn target(&self) -> String {
        "test-store".to_owned()
    }

    fn close(&self) -> Result<(), SharedError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct TestPdClient;

impl PdClient for TestPdClient {
    fn get_store(&self, _ctx: &Context, store_id: u64) -> Result<Store, SharedError> {
        let mut store = Store::new();
        store.set_id(store_id);
        store.set_address("127.0.0.1:20160".to_owned());
        Ok(store)
    }
}

struct TestFactory {
    conn: Arc<TestConn>,
    dials: AtomicUsize,
}

impl GrpcConnFactory for TestFactory {
    fn dial(&self, _ctx: &Context, _store: &Store) -> Result<Arc<dyn GrpcConn>, SharedError> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        Ok(self.conn.clone())
    }
}

fn manager() -> (Arc<StoreManager>, Arc<TestFactory>, Arc<TestConn>) {
    let conn = Arc::new(TestConn::default());
    let factory = Arc::new(TestFactory {
        conn: conn.clone(),
        dials: AtomicUsize::new(0),
    });
    let manager = Arc::new(StoreManager::NewStoreManager(
        Arc::new(TestPdClient),
        KeepaliveParams::default(),
        None,
        factory.clone(),
    ));
    (manager, factory, conn)
}

#[test]
fn try_with_conn_serializes_callbacks_like_go() {
    let (manager, _, _) = manager();
    let ctx = Context::new();
    let (first_entered_tx, first_entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let first_manager = manager.clone();
    let first_ctx = ctx.clone();
    let first = thread::spawn(move || {
        first_manager
            .TryWithConn(&first_ctx, 7, |_| {
                first_entered_tx.send(()).expect("announce first callback");
                release_rx.recv().expect("release first callback");
                Ok(())
            })
            .expect("first TryWithConn");
    });
    first_entered_rx.recv().expect("first callback entered");

    let (second_entered_tx, second_entered_rx) = mpsc::channel();
    let second_manager = manager.clone();
    let second = thread::spawn(move || {
        second_manager
            .TryWithConn(&ctx, 7, |_| {
                second_entered_tx
                    .send(())
                    .expect("announce second callback");
                Ok(())
            })
            .expect("second TryWithConn");
    });

    assert!(
        second_entered_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "Go holds grpcClis.mu while invoking the callback"
    );
    release_tx.send(()).expect("release first callback");
    first.join().expect("first thread");
    second.join().expect("second thread");
    second_entered_rx.recv().expect("second callback entered");
}

#[test]
fn close_retains_cached_connections_like_go() {
    let (manager, factory, conn) = manager();
    let ctx = Context::new();
    manager
        .WithConn(&ctx, 9, |_| {})
        .expect("initial connection");

    manager.Close();
    assert_eq!(conn.closes.load(Ordering::SeqCst), 1);
    manager
        .WithConn(&ctx, 9, |_| {})
        .expect("reuse closed cached connection");
    assert_eq!(
        factory.dials.load(Ordering::SeqCst),
        1,
        "Go Close does not delete entries from grpcClis.clis"
    );
}
