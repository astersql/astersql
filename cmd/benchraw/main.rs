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

//! benchraw 命令入口，对齐 `cmd/benchraw/main.go` 的 RawKV Put 压测流程。
//! 这里保留 Go 版本的启动顺序：解析参数、设置日志级别、后台启动 pprof、
//! 构造固定大小的 value、并发写入，再输出总耗时与数据量摘要。
//! 该文件只负责装配命令行行为，真正的 TiKV 访问与环境差异由 `stubs` 层屏蔽。

use std::io::{self, Write};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crate::stubs::{
    self, ClientFactory, Flags, LogLevel, Security, default_client_factory, split_pd_addrs,
};

/// 返回命令级默认参数。
/// 对齐 Go 版本的包级 `var` 默认值，便于二进制入口和测试共用同一套初始配置。
pub fn default_flags() -> Flags {
    Flags::default()
}

/// 二进制入口，对齐 Go 的 `main`。
/// 这里刻意把参数解析与执行拆开，方便在不依赖真实进程环境时做一致性校验。
pub fn main() {
    let args = stubs::args_from_env();
    let Some(flags) = stubs::parse_flags(&args) else {
        return;
    };
    run_with_flags(flags, default_client_factory(), true);
}

/// 在参数已经确定后执行压测主流程。
/// 该拆分点同时服务真实入口和对齐测试，避免两套启动路径逐渐漂移。
///
/// `start_pprof` 为真时，启动与 Go 后台 goroutine 等价的 pprof HTTP 服务。
pub fn run_with_flags(flags: Flags, factory: ClientFactory, start_pprof: bool) {
    stubs::set_log_level(LogLevel::Warn);

    if start_pprof {
        // 保持 Go 的后台监听语义：服务失败只记录，不阻塞压测主线程。
        // Go: go func() { terror.Log(errors.Trace(http.ListenAndServe(":9191", nil))) }()
        thread::spawn(|| {
            let err = stubs::listen_and_serve(":9191");
            stubs::terror_log(err.map(stubs::errors_trace));
        });
    }

    let value_size = usize::try_from(flags.value_size)
        .unwrap_or_else(|_| stubs::fatal("makeslice: len out of range"));
    let value = vec![0u8; value_size];
    let t = Instant::now();
    batch_raw_put(&flags, &value, factory);

    let elapsed = t.elapsed();
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "\nelapse:{elapsed:?}, total {}", flags.data_cnt);
}

/// 执行与 Go `batchRawPut` 一致的并发盲写压测。
/// 这里先创建一个共享客户端，再按 worker 数切出每个线程负责的连续 key 区间。
///
/// 每个 worker `i` 写入 `key_{base*i + j}`，其中 `j in 0..base`，
/// `base = dataCnt / workerCnt`。
/// 余数部分会被直接舍弃，这不是 Rust 特有简化，而是刻意保留 Go 原始实现行为。
pub fn batch_raw_put(flags: &Flags, value: &[u8], factory: ClientFactory) {
    let pd_addrs = split_pd_addrs(&flags.pd_addr);
    let security = Security {
        ClusterSSLCA: flags.ssl_ca.clone(),
        ClusterSSLCert: flags.ssl_cert.clone(),
        ClusterSSLKey: flags.ssl_key.clone(),
    };

    let cli = match factory(pd_addrs, security) {
        Ok(c) => c,
        Err(e) => stubs::fatal(e.Error()),
    };

    let worker_cnt = flags.worker_cnt.max(0) as usize;
    let data_cnt = flags.data_cnt.max(0) as usize;
    if worker_cnt == 0 {
        // Go 会在除零时直接失败；这里显式 fatal 以保留快速失败的约束。
        stubs::fatal("concurrent num is zero");
    }
    let base = data_cnt / worker_cnt;
    // 每个线程持有独立 value 副本，避免共享可变缓冲区带来额外同步语义。
    let value = value.to_vec();

    let mut handles = Vec::with_capacity(worker_cnt);
    for i in 0..worker_cnt {
        let cli = Arc::clone(&cli);
        let value = value.clone();
        handles.push(thread::spawn(move || {
            for j in 0..base {
                let k = base * i + j;
                let key = format!("key_{k}");
                if let Some(err) = cli.Put(key.into_bytes(), value.clone()) {
                    stubs::fatal_put_failed(&err);
                }
            }
        }));
    }
    for h in handles {
        // 将 worker 的 panic 继续向外传播，保持与 Go 中未恢复 goroutine 失败相近的可见性。
        h.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
}

/// 生成与 Go `fmt.Printf("\nelapse:%v, total %v\n", ...)` 对齐的摘要字符串。
pub fn format_elapse_line(elapsed: &str, total: i64) -> String {
    format!("\nelapse:{elapsed}, total {total}\n")
}
