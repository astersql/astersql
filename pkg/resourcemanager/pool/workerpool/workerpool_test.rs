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

use crate::workerpool::{Channel, Context, Error, TaskMayPanic, Worker, WorkerPool};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct BufferedTask;

impl TaskMayPanic for BufferedTask {
    fn RecoverArgs(&self) -> (String, String, Option<Error>) {
        (String::new(), String::new(), None)
    }
}

struct CountingWorker(Arc<AtomicUsize>);

impl Worker<BufferedTask, ()> for CountingWorker {
    fn HandleTask(&mut self, _task: BufferedTask, _send: &mut dyn FnMut(())) -> Result<(), Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

#[test]
fn closed_task_channel_is_drained_before_worker_exits() {
    const TASKS: usize = 256;
    let tasks = Channel::bounded(TASKS);
    for _ in 0..TASKS {
        assert!(tasks.send(BufferedTask));
    }
    tasks.close();

    let handled = Arc::new(AtomicUsize::new(0));
    let worker_handled = Arc::clone(&handled);
    let mut pool = WorkerPool::<BufferedTask, ()>::NewWorkerPool("drain", (), 1, move || {
        CountingWorker(Arc::clone(&worker_handled))
    });
    pool.SetTaskReceiver(tasks);
    pool.Start(Context::background());
    pool.Release();

    assert_eq!(handled.load(Ordering::SeqCst), TASKS);
}

#[test]
fn receive_timeout_preserves_buffered_close_and_open_wait() {
    use crate::workerpool::RecvTimeoutError;
    use std::time::Duration;
    let buffered = Channel::bounded(1);
    assert!(buffered.send(7));
    buffered.close();
    assert_eq!(buffered.recv_timeout(Duration::from_millis(1)), Ok(Some(7)));
    assert_eq!(buffered.recv_timeout(Duration::from_millis(1)), Ok(None));
    let open = Channel::<i32>::bounded(0);
    assert_eq!(
        open.recv_timeout(Duration::from_millis(1)),
        Err(RecvTimeoutError::Timeout)
    );
    assert!(!open.is_closed());
    open.close();
    assert_eq!(open.recv_timeout(Duration::from_millis(1)), Ok(None));
}

#[test]
fn tune_without_close_wait_still_waits_for_worker_to_accept_exit_request() {
    struct BlockingWorker {
        entered: std::sync::mpsc::Sender<()>,
        release: Channel<()>,
    }
    impl Worker<BufferedTask, ()> for BlockingWorker {
        fn HandleTask(&mut self, _: BufferedTask, _: &mut dyn FnMut(())) -> Result<(), Error> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
        fn Close(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let releases = Channel::bounded(2);
    let tasks = Channel::bounded(2);
    let mut pool = WorkerPool::NewWorkerPool("exit-handshake", (), 2, {
        let releases = releases.clone();
        move || BlockingWorker {
            entered: entered_tx.clone(),
            release: releases.clone(),
        }
    });
    pool.SetTaskReceiver(tasks.clone());
    pool.Start(Context::background());
    assert!(tasks.send(BufferedTask));
    assert!(tasks.send(BufferedTask));
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let (tuned_tx, tuned_rx) = std::sync::mpsc::channel();
    let tuning = std::thread::spawn(move || {
        pool.Tune(1, false);
        tuned_tx.send(()).unwrap();
        pool.CloseAndWait();
    });
    let early = tuned_rx.recv_timeout(std::time::Duration::from_millis(50));
    releases.send(());
    releases.send(());
    tuning.join().unwrap();
    assert!(
        matches!(early, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
        "Go's unbuffered exit request must be accepted by a worker even when wait=false"
    );
}
