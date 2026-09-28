// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Lightning 有界 worker 池实现。
//
// 对应 Go `pkg/lightning/worker`：用有界 channel 存放固定数量的 `Worker` 令牌，
// `Apply` 借出、`Recycle` 归还，从而限制导入任务的并发 worker 数；可选挂接
// Prometheus 指标（空闲数、申请等待秒数）。

#![allow(non_snake_case)]

use crate::metric;
use std::sync::Arc;
use std::time::Instant;

/// Pool 对应 Go 的 worker 池：有界通道中的每个 Worker 是一枚并发许可。
pub struct Pool {
    /// 池容量上限，等于构造时预置的 worker 个数。
    limit: usize,
    /// 接收端：Apply 从此阻塞取出空闲 worker。
    workers: crossbeam_channel::Receiver<Arc<Worker>>,
    /// 发送端：Recycle 将 worker 写回通道。
    worker_sender: crossbeam_channel::Sender<Arc<Worker>>,
    /// 指标标签名，区分不同用途的池。
    name: String,
    /// 可选指标句柄；无 MetricContext 时为 None。
    metrics: Option<Arc<metric::Metrics>>,
}

/// Worker 是可复用的 worker 令牌，ID 从 1 开始并在池生命周期内保持稳定。
pub struct Worker {
    /// Worker 编号，构造时按 1..=limit 分配，不随借还变化。
    pub ID: i64,
}

/// NewPool 创建指定容量的有界池，并预先放入 limit 个 worker。
pub fn NewPool(ctx: &metric::MetricContext, limit: usize, name: String) -> Pool {
    let (worker_sender, workers) = crossbeam_channel::bounded(limit);
    for i in 0..limit {
        // Go 在构造阶段同步写满 channel；容量等于 limit，因此这里不会等待消费者。
        worker_sender
            .send(Arc::new(Worker { ID: (i + 1) as i64 }))
            .expect("worker pool receiver must exist during construction");
    }

    // 从上下文取出 Metrics；存在则把初始空闲数设为 limit。
    let metrics = metric::from_context(ctx).cloned();
    if let Some(metrics) = &metrics {
        metrics
            .idle_workers_gauge
            .with_label_values(&[&name])
            .set(limit as f64);
    }

    Pool {
        limit,
        workers,
        worker_sender,
        name,
        metrics,
    }
}

impl Pool {
    /// Apply 从池中阻塞取得一个 worker，并记录等待耗时和取得后的空闲数量。
    pub fn Apply(&self) -> Arc<Worker> {
        let start = Instant::now();
        // recv 对应 Go 的 `<-pool.workers`；池为空时等待 Recycle 归还令牌。
        let worker = self
            .workers
            .recv()
            .expect("worker pool channel unexpectedly closed");
        if let Some(metrics) = &self.metrics {
            metrics
                .idle_workers_gauge
                .with_label_values(&[&self.name])
                .set(self.workers.len() as f64);
            metrics
                .apply_worker_seconds_histogram
                .with_label_values(&[&self.name])
                .observe(start.elapsed().as_secs_f64());
        }
        worker
    }

    /// Recycle 把 worker 归还池中；None 对应 Go 的 nil，并保留原 panic 保护。
    pub fn Recycle(&self, worker: Option<Arc<Worker>>) {
        let worker = worker.expect("invalid restore worker");
        // send 对应 Go 的 channel 写入；重复归还导致池满时会阻塞，沿用原调用契约。
        self.worker_sender
            .send(worker)
            .expect("worker pool receiver unexpectedly closed");
        if let Some(metrics) = &self.metrics {
            metrics
                .idle_workers_gauge
                .with_label_values(&[&self.name])
                .set(self.workers.len() as f64);
        }
    }

    /// HasWorker 只观察当前通道长度，不预留 worker；结果可能立刻被其他并发申请者改变。
    pub fn HasWorker(&self) -> bool {
        !self.workers.is_empty()
    }

    /// limit 保留 Go 结构中的容量字段，供后续同包代码接线使用。
    pub fn limit(&self) -> usize {
        self.limit
    }
}
