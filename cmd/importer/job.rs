// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

//! `cmd/importer/job.go` 的 Rust 对照实现，负责把待插入任务分发给多个 worker。
//!
//! 模块本身不生成业务数据，而是围绕 `genRowDatas`、事务提交和线程间协作组织执行流。
//! 整体流程保持与 Go 版本一致：
//! 生产者先投递固定数量的空任务，再由 worker 按批次聚合后统一写入数据库，
//! 最后由等待阶段汇总吞吐并打印导入摘要。
//!
//! Rust 标准库的 `mpsc::Receiver` 不能像 Go channel 那样被多个消费者直接共享，
//! 因此这里额外引入一个分发线程，把总任务流轮询转发到每个 worker 私有通道上。
//! 这个适配只改变并发拼装方式，不改变“所有任务最终被某个 worker 消费”的语义。

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::genRowDatas;
use crate::parser::table;
use crate::stubs::{self, DB};

/// 向任务通道写入固定数量的占位任务。
///
/// 每个 `()` 代表一条待处理插入作业，具体内容由 worker 在消费时再生成。
/// 发送端在函数结束后被 drop，等价于 Go 版本里的 `close(jobChan)`，
/// 用来通知下游不会再有新的任务进入。
pub fn addJobs(jobCount: isize, jobChan: mpsc::SyncSender<()>) {
    for _ in 0..jobCount {
        let _ = jobChan.send(());
    }
    // Sender drop closes channel (Go close(jobChan)).
}

/// 为当前批次生成 SQL，并放到同一个事务中提交。
///
/// 这里保持 Go 版本的失败策略：任一阶段出错都直接终止流程，
/// 避免出现部分语句已提交、部分语句仍在重试的模糊状态。
/// `count` 表示本次批量插入的行数，而不是任务总量。
pub fn doInsert(table: &table, db: &DB, count: isize) {
    let sqls = match genRowDatas(table, count) {
        Ok(sqls) => sqls,
        Err(err) => stubs::fatal(format!("generate data failed: {err}")),
    };

    let txn = match db.Begin() {
        Ok(txn) => txn,
        Err(err) => stubs::fatal(format!("begin failed: {err}")),
    };

    for sql in &sqls {
        if let Err(err) = txn.Exec(sql) {
            stubs::fatal(format!("exec failed: {err}"));
        }
    }

    if let Err(err) = txn.Commit() {
        stubs::fatal(format!("commit failed: {err}"));
    }
}

/// 持续消费任务，并在累计到批大小时触发一次数据库写入。
///
/// worker 结束条件依赖输入通道关闭，这与 Go 中 `for range jobChan` 的生命周期一致。
/// 当剩余任务不足一个完整批次时，会在退出前补做一次尾批提交，
/// 保证所有已领取任务都被落库而不会因为凑不满 `batch` 被丢弃。
/// 最后的完成信号只表示该 worker 已停止接单并清空本地缓冲。
pub fn doJob(
    table: Arc<table>,
    db: DB,
    batch: isize,
    jobChan: mpsc::Receiver<()>,
    doneChan: mpsc::SyncSender<()>,
) {
    let mut count = 0;
    while jobChan.recv().is_ok() {
        count += 1;
        if count == batch {
            doInsert(&table, &db, count);
            count = 0;
        }
    }
    if count > 0 {
        doInsert(&table, &db, count);
    }
    let _ = doneChan.send(());
}

/// 等待全部 worker 结束后，输出整个导入过程的耗时与粗粒度吞吐。
///
/// 吞吐按 `jobCount / seconds` 计算，故当总耗时不足 1 秒时会保留 `-1`，
/// 直接复用 Go 版本的约定，避免制造带小数的另一套统计口径。
/// 这里消费完 `doneChan` 即可，不需要显式关闭接收端。
pub fn doWait(doneChan: mpsc::Receiver<()>, start_unix: i64, jobCount: isize, workerCount: isize) {
    for _ in 0..workerCount {
        let _ = doneChan.recv();
    }
    // Receiver drop closes; Go closes doneChan after draining.

    let now_unix = unix_now();
    let seconds = now_unix - start_unix;
    let mut tps: i64 = -1;
    if seconds > 0 {
        tps = jobCount as i64 / seconds;
    }
    println!(
        "[importer]total {jobCount} cases, cost {seconds} seconds, tps {tps}, start {start_unix}, now {now_unix}"
    );
}

/// 组织导入主流程：建通道、启动生产者、按约束启动 worker，并等待收尾。
///
/// 与 Go 实现一样，任务通道容量按 `16 * workerCount` 估算，
/// 让生产者在短时间内可以领先 worker 一段距离，减少频繁阻塞。
/// 如果表中存在 `incremental` 列，则强制退化为单 worker，
/// 以维持这类列对生成顺序和唯一性的隐含要求，避免并发生成破坏数据单调性。
/// 由于 Rust 无法把同一个 `Receiver` 直接克隆给多个线程，
/// 这里先由分发线程从总通道取任务，再按轮询方式分给各 worker，
/// 从而近似 Go 版本“多个 goroutine 竞争同一 channel”的负载分担效果。
pub fn doProcess(
    table: Arc<table>,
    dbs: &[DB],
    jobCount: isize,
    mut workerCount: isize,
    batch: isize,
) {
    let (job_tx, job_rx) = mpsc::sync_channel::<()>(16 * workerCount.max(1) as usize);
    let (done_tx, done_rx) = mpsc::sync_channel::<()>(workerCount.max(1) as usize);

    let start = unix_now();
    let job_count = jobCount;
    thread::spawn(move || {
        addJobs(job_count, job_tx);
    });

    for col in &table.columns {
        if col.incremental {
            workerCount = 1;
            break;
        }
    }

    // Fan-out job channel to workers (Go shares one channel).
    // std::mpsc::Receiver is not Clone — use a dispatcher thread.
    let (fan_txs, fan_rxs): (Vec<_>, Vec<_>) = (0..workerCount)
        .map(|_| mpsc::sync_channel::<()>(16))
        .unzip();
    let wc = workerCount;
    thread::spawn(move || {
        let mut i = 0usize;
        while job_rx.recv().is_ok() {
            if wc == 0 {
                break;
            }
            let _ = fan_txs[i % wc as usize].send(());
            i += 1;
        }
        // drop fan_txs to close worker channels
    });

    let mut handles = Vec::new();
    for (i, fan_rx) in fan_rxs.into_iter().enumerate() {
        let table = Arc::clone(&table);
        let db = dbs[i as usize].clone();
        let done_tx = done_tx.clone();
        handles.push(thread::spawn(move || {
            doJob(table, db, batch, fan_rx, done_tx);
        }));
    }
    drop(done_tx);

    doWait(done_rx, start, jobCount, workerCount);
    let mut worker_panic = None;
    for h in handles {
        if let Err(payload) = h.join()
            && worker_panic.is_none()
        {
            worker_panic = Some(payload);
        }
    }
    if let Some(payload) = worker_panic {
        std::panic::resume_unwind(payload);
    }
}

/// 返回当前 Unix 秒级时间戳。
///
/// 该辅助函数只服务于吞吐统计，保持与 Go 版本使用秒级 `Unix()` 的精度一致，
/// 不把格式化时间对象泄漏到并发逻辑里，避免线程间再做额外时间转换。
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
