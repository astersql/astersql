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
