// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::inner_server::{DatabaseBundle, InnerServer, StandAloneInnerServer};

#[derive(Default)]
struct TestBundle {
    closes: AtomicUsize,
}

impl DatabaseBundle for TestBundle {
    fn close(&self) -> Result<(), String> {
        self.closes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

#[test]
fn start_matches_go_noop_and_stop_closes_bundle() {
    let bundle = Arc::new(TestBundle::default());
    let server = StandAloneInnerServer::new(Arc::clone(&bundle));

    server.setup();
    server.start().unwrap();
    assert!(!server.is_started(), "Go Start is a no-op");
    assert_eq!(0, bundle.closes.load(Ordering::Acquire));

    server.stop().unwrap();
    assert_eq!(1, bundle.closes.load(Ordering::Acquire));
    assert!(server.raft().is_ok());
    assert!(server.batch_raft().is_ok());
    assert!(server.snapshot().is_ok());
}
