// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 对象存储访问量统计：请求次数与读写字节数。
//
// 对应 Go `objstore/recording`：在并发读写对象时用原子计数累加 GET/PUT 类请求
// 与流量，供备份等路径做观测；`Option` 接收者模拟 Go 侧可为 nil 的指针。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// GET/PUT 类请求计数器，对应 Go 的 `Requests`。
// Requests 对应 Go 的 Requests，分别累计读取类请求与写入类请求。
// AtomicU64 保留 Go atomic.Uint64 可被多线程并发更新的语义；Relaxed 足以表达纯计数器。
#[derive(Default)]
pub struct Requests {
    /// 读取类（GET/HEAD）请求次数。
    pub get: AtomicU64,
    /// 写入类（PUT/POST）请求次数。
    pub put: AtomicU64,
}

impl Requests {
    /// 读取当前 GET/PUT 计数快照（两计数器各自原子，无跨字段事务一致性）。
    // snapshot returns the current GET/PUT counters as one observable snapshot.
    // The two counters remain independently atomic, matching the Go implementation.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.get.load(Ordering::Relaxed),
            self.put.load(Ordering::Relaxed),
        )
    }

    /// 按 HTTP 方法把一次请求记入 GET 或 PUT；空请求不计数。
    // rec 对应 Go 的私有方法：空请求不计数，再按 HTTP 方法归入 GET 或 PUT。
    fn rec<T>(&self, http_req: Option<&http::Request<T>>) {
        let Some(http_req) = http_req else {
            return;
        };

        // HEAD 只读取元数据，因此与 GET 一起计数；POST 通常写入数据，因此与 PUT 一起计数。
        match http_req.method() {
            &http::Method::GET | &http::Method::HEAD => {
                self.get.fetch_add(1, Ordering::Relaxed);
            }
            &http::Method::PUT | &http::Method::POST => {
                self.put.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    /// 把另一计数器快照累加到自身，对应 Go `Merge`。
    // merge 对应 Go 的 Merge，把另一计数器的当前快照原子累加到自身。
    pub fn merge(&self, other: &Requests) {
        self.get
            .fetch_add(other.get.load(Ordering::Relaxed), Ordering::Relaxed);
        self.put
            .fetch_add(other.put.load(Ordering::Relaxed), Ordering::Relaxed);
    }
}

// Display 对应 Go Requests.String，读取两个原子值并保持原有输出格式。
impl fmt::Display for Requests {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{get: {}, put: {}}}",
            self.get.load(Ordering::Relaxed),
            self.put.load(Ordering::Relaxed)
        )
    }
}

/// 读写字节流量计数，对应 Go 的 `Traffic`。
// Traffic 对应 Go 的 Traffic，记录从对象存储读取和写入对象存储的字节数。
#[derive(Default)]
pub struct Traffic {
    /// 从对象存储读出的字节数。
    pub read: AtomicU64,
    /// 写入对象存储的字节数。
    pub write: AtomicU64,
}

// Display 对应 Go Traffic.String，并保持 `{r: ..., w: ...}` 格式。
impl fmt::Display for Traffic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{r: {}, w: {}}}",
            self.read.load(Ordering::Relaxed),
            self.write.load(Ordering::Relaxed)
        )
    }
}

/// 组合请求次数与流量的访问统计，对应 Go 的 `AccessStats`。
// AccessStats 对应 Go 的 AccessStats，组合请求次数和传输字节数两组统计。
#[derive(Default)]
pub struct AccessStats {
    /// 请求次数统计。
    pub requests: Requests,
    /// 传输字节统计。
    pub traffic: Traffic,
}

impl AccessStats {
    /// 合并另一份统计；各字段独立累加，无跨字段事务一致性。
    // merge 对应 Go 的 Merge；每个字段独立读取快照并累加，不提供跨字段事务一致性。
    pub fn merge(&self, other: &AccessStats) {
        self.requests.merge(&other.requests);
        self.traffic.read.fetch_add(
            other.traffic.read.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.traffic.write.fetch_add(
            other.traffic.write.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    /// 记录一次 HTTP 请求；`stats`/`http_req` 为 `None` 时为 no-op。
    // rec_request 对应 Go RecRequest；Option 模拟可为 nil 的接收者和 HTTP 请求。
    pub fn rec_request<T>(stats: Option<&AccessStats>, http_req: Option<&http::Request<T>>) {
        let Some(stats) = stats else {
            return;
        };
        stats.requests.rec(http_req);
    }

    /// 累加成功读取的字节数。
    // rec_read 对应 Go RecRead，把本次成功读取的字节数累加到流量统计。
    pub fn rec_read(stats: Option<&AccessStats>, n: usize) {
        let Some(stats) = stats else {
            return;
        };
        stats.traffic.read.fetch_add(n as u64, Ordering::Relaxed);
    }

    /// 累加成功写入的字节数。
    // rec_write 对应 Go RecWrite，把本次成功写入的字节数累加到流量统计。
    pub fn rec_write(stats: Option<&AccessStats>, n: usize) {
        let Some(stats) = stats else {
            return;
        };
        stats.traffic.write.fetch_add(n as u64, Ordering::Relaxed);
    }
}

// Display 对应 Go AccessStats.String，委托两个子统计量生成原有嵌套格式。
impl fmt::Display for AccessStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{requests: {}, traffic: {}}}",
            self.requests, self.traffic
        )
    }
}
