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

//! benchkv 命令的 Rust 入口，负责驱动一次无冲突写入压测并导出 Prometheus 指标。
//! 整体流程与 `cmd/benchkv/main.go` 保持一致：解析参数、初始化存储和指标、并发写入，再读取 `/metrics` 输出。
//! 生产入口使用 canonical TiKV driver、Prometheus 和真实 HTTP；同一适配层也为 parity test 提供显式测试后端。
//! 该文件本身只编排时序，不承载复杂业务状态；真正的可观测性和存储行为都委托给注入依赖完成。

use std::thread;
use std::time::Instant;

use crate::stubs::{
    self, Flags, HttpServer, Metrics, RuntimeDeps, Storage, terror_call, terror_log, trace,
};

/// Package-level flag defaults (Go `var` block).
pub fn default_flags() -> Flags {
    Flags::default()
}

/// 入口函数对应 Go `main`，只负责把进程环境转换为显式参数并交给可测试的执行函数。
/// 这样二进制入口和 parity test 可以共享同一条主流程，而不是在测试里直接操纵全局状态。
pub fn main() {
    let args = stubs::args_from_env();
    let flags = stubs::parse_flags_or_exit(&args);
    stubs::set_log_level(stubs::LogLevel::Error);
    run_with_flags(flags, RuntimeDeps::default());
}

/// 在参数已经确定后执行完整压测流程，供真实入口和对齐测试复用。
/// 这里把依赖显式传入，保持与 Go 主流程同样的调用顺序，同时避免隐藏的全局可变状态。
pub fn run_with_flags(flags: Flags, deps: RuntimeDeps) {
    let (store, metrics, http) = init(&flags, &deps);

    // Go 里 `make([]byte, *valueSize)` 会在长度为负时 panic；转换失败同样作为致命配置错误传播。
    let value_size = usize::try_from(flags.value_size).expect("negative value size");
    let value = vec![0u8; value_size];
    let t = Instant::now();
    batch_rw(&flags, &store, &metrics, &value);

    // Go 主流程会回环请求本地 `/metrics`；这里沿用同样顺序，确保最终输出包含刚刚压测产生的计数。
    let (resp, err) = stubs::http_get("http://localhost:9191/metrics", &metrics, &http);
    stubs::must_nil(err);

    let (text, err1) = stubs::read_all(&resp.body);
    terror_log(trace(err1));

    // 输出顺序与 Go 保持一致：先打印指标正文，再打印本次运行耗时和目标写入总数。
    println!("{}", String::from_utf8_lossy(&text));
    println!("\nelapse:{:?}, total {}", t.elapsed(), flags.data_cnt);

    // Go 用 defer 在全部输出完成、函数返回前关闭响应体；关闭失败只记录日志。
    if let Some(err) = resp.Close() {
        stubs::log_function_call_errored(&err);
    }
}

/// 初始化存储、指标注册和 HTTP 服务，语义对应 Go `Init`。
/// 返回值显式带出三类依赖，避免像 Go 一样依赖包级变量，便于后续测试按需替换。
pub fn init(flags: &Flags, deps: &RuntimeDeps) -> (Storage, Metrics, HttpServer) {
    let metrics = deps.metrics.clone();
    let http = deps.http.clone();

    // 优先使用外部注入的 store，测试可以绕开真实 Open；未注入时再按 Go 路径拼接 PD 地址并打开 TiKV 存储。
    let store = if let Some(s) = deps.store_override.clone() {
        s
    } else {
        let path = format!("tikv://{}?cluster=1", flags.pd_addr);
        let (opened, err) = deps.driver.Open(&path);
        stubs::must_nil(err);
        opened
    };

    metrics.must_register_all();
    // 注册动态 handler；每次抓取都读取当前指标，而不是缓存初始化时的零值正文。
    http.HandleMetrics(&metrics);

    let http_bg = http.clone();
    // Go 这里是后台 goroutine；Rust 用线程模拟，保持监听与压测主路径并行而不阻塞初始化返回。
    thread::spawn(move || {
        let err1 = http_bg.ListenAndServe(":9191");
        terror_log(trace(err1));
    });

    (store, metrics, http)
}

/// 并发执行一组互不冲突的写事务，职责对应 Go `batchRW`。
/// key 通过 worker 编号和局部序号分片生成，避免不同线程写到同一条记录，从而把压测重点放在事务吞吐而非冲突处理上。
pub fn batch_rw(flags: &Flags, store: &Storage, metrics: &Metrics, value: &[u8]) {
    // Go 在启动 worker 前直接做整数除法，因此 worker 为 0 时会 panic。
    let base = flags.data_cnt / flags.worker_cnt;
    // Go `WaitGroup.Add` 拒绝负计数；Rust 在创建线程前以同样的致命方式拒绝它。
    let worker_cnt = usize::try_from(flags.worker_cnt).expect("negative worker count");
    // 与 Go 一样使用整除平均分片；余数被自然舍弃，因此总实际写入量可能小于 `data_cnt`。
    let mut handles = Vec::with_capacity(worker_cnt);

    for i in 0..worker_cnt {
        let store = store.clone();
        let metrics = metrics.clone();
        let value = value.to_vec();
        let i = i as i64;
        handles.push(thread::spawn(move || {
            // 每个线程只处理自己的连续 key 区间，保证 `base * i + j` 在不同 worker 之间不重叠。
            for j in 0..base {
                metrics.txn_counter.WithLabelValues(&["txn"]).Inc();
                let start = Instant::now();
                let k = base * i + j;
                let (mut txn, err) = store.Begin();
                if let Some(e) = err {
                    if store.is_production() {
                        stubs::fatal_process(e.Error());
                    } else {
                        stubs::fatal(e.Error());
                    }
                }
                let key = format!("key_{k}");
                let err = txn.Set(key.as_bytes(), &value);
                terror_log(trace(err));
                let err = txn.Commit();
                if err.is_some() {
                    metrics
                        .txn_rolledback_counter
                        .WithLabelValues(&["txn"])
                        .Inc();
                    // 提交失败时沿用 Go 逻辑：先记失败指标，再尝试回滚，并把回滚错误交给 terror 统一记录。
                    terror_call(|| txn.Rollback());
                }
                // 时延统计覆盖从 Begin 之后到提交/回滚分支结束，保持与 Go `time.Since(start)` 一致的观测范围。
                metrics
                    .txn_durations
                    .WithLabelValues(&["txn"])
                    .Observe(stubs::duration_seconds(start.elapsed()));
            }
        }));
    }
    // 对齐 Go `WaitGroup.Wait` 的“主流程等待全部 worker 完成”语义；若线程 panic，则继续向上传播为致命失败。
    for h in handles {
        if let Err(payload) = h.join() {
            std::panic::resume_unwind(payload);
        }
    }
}
