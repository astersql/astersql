// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 并发导入调度：生产者投递 job token，多个 worker 批量事务插入。
//
// 每个 job 代表一行待插入数据；worker 攒满 `batch` 后在同一事务中提交，
// 通道关闭后刷掉剩余未满批的行。返回耗时与粗略 TPS。

use crate::config::ImporterError;
use crate::db::{Database, generate_row_data_batch};
use crate::parser::Table;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// 导入结果报告：完成 job 数、耗时、每秒事务数（秒数为 0 时为 -1）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessReport {
    pub jobs: usize,
    pub elapsed: Duration,
    pub transactions_per_second: i64,
}

/// 在一个事务中生成并执行 `count` 条 INSERT。
fn do_insert(table: &Table, database: &dyn Database, count: usize) -> Result<(), ImporterError> {
    let statements = generate_row_data_batch(table, count)?;
    let mut transaction = database.begin()?;
    for statement in statements {
        transaction.execute(&statement)?;
    }
    transaction.commit()
}

/// 单个 worker：从共享接收端取 job，满批插入；通道关闭后刷剩余。
fn do_job(
    table: Arc<Table>,
    database: Arc<dyn Database>,
    batch: usize,
    jobs: Arc<Mutex<mpsc::Receiver<()>>>,
) -> Result<(), ImporterError> {
    let mut count = 0;
    loop {
        // Receiver 被 Mutex 保护：多 worker 竞争同一队列。
        let received = jobs.lock().unwrap().recv();
        match received {
            Ok(()) => {
                count += 1;
                if count == batch {
                    do_insert(&table, database.as_ref(), count)?;
                    count = 0;
                }
            }
            Err(_) => break,
        }
    }
    if count > 0 {
        do_insert(&table, database.as_ref(), count)?;
    }
    Ok(())
}

/// 启动生产者与 `worker_count` 个 worker，等待全部完成后汇总报告。
pub fn process_jobs(
    table: Arc<Table>,
    databases: &[Arc<dyn Database>],
    job_count: usize,
    worker_count: usize,
    batch: usize,
) -> Result<ProcessReport, ImporterError> {
    if worker_count == 0 || databases.len() < worker_count || batch == 0 {
        return Err(ImporterError::InvalidConfig(
            "worker_count and batch must be positive and each worker needs a database".to_owned(),
        ));
    }
    let start = Instant::now();
    let (sender, receiver) = mpsc::sync_channel(16 * worker_count);
    let receiver = Arc::new(Mutex::new(receiver));
    // 生产者仅发送空 token；实际行内容由 worker 侧按表结构生成。
    let producer = thread::spawn(move || {
        for _ in 0..job_count {
            if sender.send(()).is_err() {
                break;
            }
        }
    });
    let mut workers = Vec::with_capacity(worker_count);
    for database in databases.iter().take(worker_count) {
        let table = Arc::clone(&table);
        let database = Arc::clone(database);
        let receiver = Arc::clone(&receiver);
        workers.push(thread::spawn(move || {
            do_job(table, database, batch, receiver)
        }));
    }
    // The coordinator must not keep the receiver alive after all workers exit.
    // Otherwise an early worker error can leave the bounded-channel producer
    // blocked forever while this thread waits to join it.
    drop(receiver);
    let mut worker_result = Ok(());
    for worker in workers {
        let result = worker.join().map_err(|_| ImporterError::WorkerPanic);
        if worker_result.is_ok() {
            worker_result = result.and_then(|result| result);
        }
    }
    producer.join().map_err(|_| ImporterError::WorkerPanic)?;
    worker_result?;
    let elapsed = start.elapsed();
    let seconds = elapsed.as_secs();
    Ok(ProcessReport {
        jobs: job_count,
        elapsed,
        // 不足 1 秒时 Go 侧同样返回 -1，避免除零与虚高 TPS。
        transactions_per_second: if seconds == 0 {
            -1
        } else {
            job_count as i64 / seconds as i64
        },
    })
}
