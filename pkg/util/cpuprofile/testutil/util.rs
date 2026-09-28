// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// CPU 剖析测试用负载模拟工具。
//
// 提供可取消的忙循环 worker，按标签（或 `sql_global_uid`）分组制造 CPU 采样负载，
// 便于验证带标签的 pprof 采集路径。

#![allow(non_snake_case)]

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// 一组剖析标签：`(key, value)` 列表。
pub type LabelSet = Vec<(String, String)>;

/// A cheap, clonable cancellation signal shared by all CPU-load workers.
/// 廉价可克隆的取消信号，供所有 CPU 负载 worker 共享。
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// 构造未取消的令牌。
    pub fn new() -> Self {
        Self::default()
    }

    /// 标记为已取消，通知 worker 退出忙循环。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 是否已收到取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Running CPU-load workers together with the labels associated with each one.
/// 正在运行的 CPU 负载 worker 及其各自标签集合。
pub struct CpuLoad {
    label_sets: Vec<LabelSet>,
    workers: Vec<(JoinHandle<()>, mpsc::Receiver<()>)>,
}

impl CpuLoad {
    /// 返回各 worker 对应的标签集合。
    pub fn label_sets(&self) -> &[LabelSet] {
        &self.label_sets
    }

    /// Waits for every worker after its cancellation token has been cancelled.
    /// 在取消令牌已触发后，于超时内等待全部 worker 结束。
    pub fn join_timeout(mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        for (_, done) in &self.workers {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if done.recv_timeout(remaining).is_err() {
                return false;
            }
        }
        for (worker, _) in self.workers.drain(..) {
            if worker.join().is_err() {
                return false;
            }
        }
        true
    }
}

/// Starts one worker for each label and one worker containing all labels.
/// 为每个标签启动一个 worker，并再启动一个包含全部标签的合并 worker。
pub fn mock_cpu_load<I, S>(cancel: &CancellationToken, labels: I) -> CpuLoad
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut all = LabelSet::new();
    let mut groups = Vec::new();
    for label in labels {
        let label = label.as_ref();
        // 值取 `"{label} value"` 的十六进制，对齐 Go MockCPULoad。
        let pair = (
            label.to_owned(),
            encode_hex(format!("{label} value").as_bytes()),
        );
        all.push(pair.clone());
        groups.push(vec![pair]);
    }
    groups.push(all);
    spawn_workers(cancel, groups)
}

/// Starts workers labelled with the Go `sql_global_uid` profile key.
/// 使用 Go 侧 `sql_global_uid` 剖析键启动各值及合并 worker。
pub fn mock_cpu_load_v2<I, S>(cancel: &CancellationToken, label_values: I) -> CpuLoad
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut all = LabelSet::new();
    let mut groups = Vec::new();
    for value in label_values {
        let pair = ("sql_global_uid".to_owned(), value.as_ref().to_owned());
        all.push(pair.clone());
        groups.push(vec![pair]);
    }
    groups.push(all);
    spawn_workers(cancel, groups)
}

// Compatibility names retained for legacy callers.
/// Go 风格别名：`MockCPULoad`。
pub fn MockCPULoad(cancel: &CancellationToken, labels: Vec<String>) -> CpuLoad {
    mock_cpu_load(cancel, labels)
}

/// Go 风格别名：`MockCPULoadV2`。
pub fn MockCPULoadV2(cancel: &CancellationToken, label_values: Vec<String>) -> CpuLoad {
    mock_cpu_load_v2(cancel, label_values)
}

/// 为每组标签启动命名线程，并在退出时通过 channel 发完成信号。
fn spawn_workers(cancel: &CancellationToken, label_sets: Vec<LabelSet>) -> CpuLoad {
    let workers = label_sets
        .iter()
        .cloned()
        .map(|labels| {
            let cancel = cancel.clone();
            let (done_tx, done_rx) = mpsc::channel();
            let worker = thread::Builder::new()
                .name(thread_name(&labels))
                .spawn(move || {
                    mock_cpu_load_by_thread_with_labels(&cancel, &labels);
                    let _ = done_tx.send(());
                })
                .expect("failed to start CPU-load worker");
            (worker, done_rx)
        })
        .collect();
    CpuLoad {
        label_sets,
        workers,
    }
}

/// 忙循环消耗 CPU，直到取消；`black_box` 防止优化掉标签与累加结果。
fn mock_cpu_load_by_thread_with_labels(cancel: &CancellationToken, labels: &LabelSet) {
    black_box(labels);
    while !cancel.is_cancelled() {
        let mut sum = 0_u64;
        for i in 0_u64..1_000_000 {
            sum = sum.wrapping_add(i * 2);
        }
        black_box(sum);
    }
}

/// 将字节编码为小写十六进制字符串。
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

/// 根据标签集合生成线程名，便于在剖析结果中识别。
fn thread_name(labels: &LabelSet) -> String {
    if labels.is_empty() {
        return "cpuprofile-load".to_owned();
    }
    let labels = labels
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("cpuprofile-load:{labels}")
}
