// Copyright 2026 AsterSQL.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::stubs::{OptionFuncAlias, RestrictedSQLExecutor, Row};

struct BlockingInitialHeartbeatSession {
    calls: Sender<Instant>,
    release_initial: Receiver<()>,
    call_count: usize,
}

impl RestrictedSQLExecutor for BlockingInitialHeartbeatSession {
    fn ExecRestrictedSQL(
        &mut self,
        _ctx: &Context,
        _opts: &[OptionFuncAlias],
        _sql: &str,
        _args: &[SqlValue],
    ) -> Result<Vec<Row>> {
        Ok(Vec::new())
    }
}

impl Session for BlockingInitialHeartbeatSession {
    fn ExecuteInternal(&mut self, _ctx: &Context, _sql: &str, _args: &[SqlValue]) -> Result<()> {
        self.calls.send(Instant::now()).unwrap();
        self.call_count += 1;
        if self.call_count == 1 {
            self.release_initial.recv().unwrap();
        }
        Ok(())
    }

    fn Close(&mut self) {}
}

#[test]
fn ticker_starts_before_the_initial_heartbeat_like_go() {
    let interval = Duration::from_secs(1);
    let (calls_tx, calls_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let session: Arc<Mutex<Box<dyn Session>>> =
        Arc::new(Mutex::new(Box::new(BlockingInitialHeartbeatSession {
            calls: calls_tx,
            release_initial: release_rx,
            call_count: 0,
        })));
    let mut manager = HeartbeatManager {
        session: Some(session),
        ctx: Some(Context::Background()),
        restore_id: 7,
        interval,
        stop_tx: None,
        join: None,
    };

    manager.Start();
    calls_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("initial heartbeat should start");
    thread::sleep(interval + Duration::from_millis(50));
    release_tx.send(()).unwrap();

    calls_rx
        .recv_timeout(Duration::from_millis(400))
        .expect("an elapsed Go ticker tick should run immediately after the initial heartbeat");
    manager.Stop();
}
