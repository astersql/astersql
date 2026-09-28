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

// 任务管理器迭代选择逻辑：在已注册任务中挑选可加速或可暂停的候选。
//
// 由 `lib.rs` 通过 `include!` 并入；`iter` 遍历各分片，用谓词（canBoost/canPause）
// 比较创建时间与运行并发，选出 Overclock/Downclock 的目标任务。

impl TaskManager {
    /// 查找适合提升并发（boost）的任务：优先未达初始并发，其次较新任务。
    fn getBoostTask(&self) -> (u64, Option<Meta>) {
        self.iter(canBoost)
    }

    /// 查找适合暂停/降并发的任务，并向其 exitCh 发送信号。
    fn pauseTask(&self) {
        let (_, result) = self.iter(canPause);
        if let Some(result) = result {
            // 通道未关闭时非阻塞发送退出信号，避免阻塞调度循环。
            if !result.exitCh.is_closed() {
                let _ = result.exitCh.try_send();
            }
        }
    }

    /// 通用迭代：对每个 Meta 调用谓词 `f`，返回选中的 (taskID, Meta)。
    ///
    /// 谓词返回 `(is_find, pause_find)`：`is_find` 表示更新当前候选，
    /// `pause_find` 为 true 时提前结束全部分片扫描。
    fn iter(&self, f: fn(&Meta, Instant) -> (bool, bool)) -> (u64, Option<Meta>) {
        let mut compareTS = Instant::now();
        let mut tid = 0;
        let mut result = None;

        for container in &self.task {
            let stats = container.stats.read().unwrap();
            let mut break_find = false;
            for (id, meta) in stats.iter() {
                // 尚未选出候选时：优先取 running!=0 的任务作为初始比较基准。
                if result.is_none() {
                    if meta.running.load(Ordering::SeqCst) != 0 {
                        result = Some(meta.clone());
                    }
                    tid = *id;
                    compareTS = meta.createTS;
                    continue;
                }

                let (is_find, pause_find) = f(meta, compareTS);
                if is_find {
                    tid = *id;
                    result = Some(meta.clone());
                    compareTS = meta.createTS;
                }
                if pause_find {
                    break_find = true;
                    break;
                }
            }
            if break_find {
                break;
            }
        }
        (tid, result)
    }
}

/// 判断任务是否适合降并发：运行数超过初始并发则立即命中并停止扫描；
/// 否则取创建时间更早且仍在运行的任务作为候选。
fn canPause(m: &Meta, minv: Instant) -> (bool, bool) {
    let running = m.running.load(Ordering::SeqCst);
    if m.initialConcurrency < running && running != 0 {
        return (true, true);
    }
    if m.createTS < minv && running != 0 {
        return (true, false);
    }
    (false, false)
}

/// 判断任务是否适合升并发：运行数低于初始并发则立即命中；
/// 否则取创建时间更晚的任务作为候选。
fn canBoost(m: &Meta, maxv: Instant) -> (bool, bool) {
    if m.running.load(Ordering::SeqCst) < m.initialConcurrency {
        return (true, true);
    }
    if m.createTS > maxv {
        return (true, false);
    }
    (false, false)
}
