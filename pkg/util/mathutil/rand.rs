// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// MySQL 兼容伪随机数生成器（MysqlRng）。
//
// TiDB/AsterSQL 用其实现 `RAND()` 等与 MySQL 一致的序列；双种子用互斥锁保护，
// 以便 Gen 与会话态（session state）读写在并发下安全。

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 随机数取模上限，与 MySQL/Go 侧 `maxRandValue` 一致。
const maxRandValue: u32 = 0x3FFF_FFFF;

// MysqlRng is the MySQL-compatible random number generator used by TiDB.
// The seeds share one mutex so Gen and the session-state accessors are thread-safe.
/// 与 MySQL 算法兼容的双种子 RNG；`seeds` 为 (seed1, seed2)。
pub struct MysqlRng {
    seeds: Mutex<(u32, u32)>,
}

// NewWithSeed creates an RNG from the supplied seed.
/// 由外部种子推导 seed1/seed2，返回堆上 MysqlRng。
pub fn NewWithSeed(seed: i64) -> Box<MysqlRng> {
    // 与 Go 相同的线性组合与取模，保证跨语言序列一致。
    let seed1 = (seed.wrapping_mul(0x1_0001).wrapping_add(55_555_555) as u32) % maxRandValue;
    let seed2 = (seed.wrapping_mul(0x1000_0001) as u32) % maxRandValue;
    Box::new(MysqlRng {
        seeds: Mutex::new((seed1, seed2)),
    })
}

// NewWithTime creates an RNG from the current Unix timestamp in nanoseconds.
/// 以当前 Unix 纳秒时间戳为种子构造 RNG（时钟回拨时用负偏移）。
pub fn NewWithTime() -> Box<MysqlRng> {
    let seed = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i64,
        Err(err) => -(err.duration().as_nanos() as i64),
    };
    NewWithSeed(seed)
}

impl MysqlRng {
    // Gen generates the next random number and advances both seeds atomically.
    /// 原子推进双种子并返回 `[0, 1)` 区间内的下一个随机数。
    pub fn Gen(&self) -> f64 {
        let mut seeds = self.seeds.lock().expect("MysqlRng mutex poisoned");
        seeds.0 = seeds.0.wrapping_mul(3).wrapping_add(seeds.1) % maxRandValue;
        // seed2 deliberately uses the newly updated seed1, matching Go/MySQL order.
        // seed2 有意使用刚更新的 seed1，顺序与 Go/MySQL 一致。
        seeds.1 = seeds.0.wrapping_add(seeds.1).wrapping_add(33) % maxRandValue;
        seeds.0 as f64 / maxRandValue as f64
    }

    // SetSeed1 updates seed1 for session-state restoration.
    /// 写入 seed1，用于会话态恢复。
    pub fn SetSeed1(&self, seed: u32) {
        self.seeds.lock().expect("MysqlRng mutex poisoned").0 = seed;
    }

    // SetSeed2 updates seed2 for session-state restoration.
    /// 写入 seed2，用于会话态恢复。
    pub fn SetSeed2(&self, seed: u32) {
        self.seeds.lock().expect("MysqlRng mutex poisoned").1 = seed;
    }

    // GetSeed1 returns seed1 for session-state serialization.
    /// 读取 seed1，用于会话态序列化。
    pub fn GetSeed1(&self) -> u32 {
        self.seeds.lock().expect("MysqlRng mutex poisoned").0
    }

    // GetSeed2 returns seed2 for session-state serialization.
    /// 读取 seed2，用于会话态序列化。
    pub fn GetSeed2(&self) -> u32 {
        self.seeds.lock().expect("MysqlRng mutex poisoned").1
    }
}
