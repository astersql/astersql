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

//! Datum state machine matching `cmd/importer/data.go`.
//!
//! 这个模块保存 importer 生成单列样本值时需要的最小状态机。
//! 它与 Go 版 `cmd/importer/data.go` 维持相同的职责边界：
//! 负责记录当前整数/时间值、重复次数和随机衰减进度，
//! 由上层生成器在不同类型列之间复用同一套“取值并递减 remains”的规则。
//! 这里不实现 SQL 编码或列定义解析，只提供可复用的基础取值原语。

use std::sync::Mutex;

use crate::stubs::{self, CivilTime};

/// `nextString`/`randString` 共享的字符表。
/// 顺序保持与 Go 常量一致，避免同一随机索引落到不同字符。
pub const alphabet: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Used by randString (also referenced from rand.rs / stats.rs).
/// 下面三个常量直接复刻 Go 的位缓存算法，用一次 `Int63` 生成多个字符索引。
pub const letterIdxBits: i64 = 6;
pub const letterIdxMask: i64 = (1 << letterIdxBits) - 1;
pub const letterIdxMax: i64 = 63 / letterIdxBits;

/// `datum` 对应 Go 的同名结构，封装单个值生成器的可变状态。
/// Rust 用内部 `Mutex` 模拟 Go 内嵌 `sync.Mutex` 的串行访问语义。
pub struct datum {
    inner: Mutex<DatumInner>,
}

/// 实际状态载体。
/// 字段顺序基本跟随 Go，方便后续逐项对照迁移。
struct DatumInner {
    // 当前整数值；由调用方在别处推进，这里只负责读取与边界钳制。
    intValue: i64,
    // 开启范围模式后用于夹紧当前值的下界。
    minIntValue: i64,
    // 开启范围模式后用于夹紧当前值的上界。
    maxIntValue: i64,
    // 时间类列共享一个 CivilTime，确保同一 datum 的多个派生格式一致。
    timeValue: CivilTime,
    // 当前这轮还允许重复返回同一值的次数。
    remains: u64,
    // 外部配置的目标重复次数；重置时会同步写回 remains。
    repeats: u64,
    // 记录步长配置；本文件只读取负号来决定初始落点。
    step: i64,
    // 控制 remains 递减路径的概率，语义与 Go 完全一致。
    probability: u32,
    // 只允许初始化一次，避免范围配置被后续调用覆盖。
    init: bool,
    // 标记是否需要在 `nextInt64` 中执行上下界夹紧。
    useRange: bool,
}

/// 构造与 Go 默认值一致的 datum。
/// `step=1`、`repeats=1`、`remains=1` 表示默认每次只消费一次当前值。
pub fn newDatum() -> datum {
    datum {
        inner: Mutex::new(DatumInner {
            intValue: 0,
            minIntValue: 0,
            maxIntValue: 0,
            timeValue: CivilTime::default(),
            remains: 1,
            repeats: 1,
            step: 1,
            probability: 100,
            init: false,
            useRange: false,
        }),
    }
}

impl datum {
    /// 返回当前步长，供上层决定如何推进数值序列。
    pub fn step(&self) -> i64 {
        self.inner.lock().unwrap().step
    }

    /// 更新步长，但不直接推进当前值。
    pub fn set_step(&self, step: i64) {
        self.inner.lock().unwrap().step = step;
    }

    /// 查询当前还可重复使用多少次。
    pub fn remains(&self) -> u64 {
        self.inner.lock().unwrap().remains
    }

    /// 直接覆写 remains，通常用于外部状态恢复或测试场景。
    pub fn set_remains(&self, v: u64) {
        self.inner.lock().unwrap().remains = v;
    }

    /// 查询配置上的重复次数目标值。
    pub fn repeats(&self) -> u64 {
        self.inner.lock().unwrap().repeats
    }

    /// 同时更新 repeats 和 remains，保持与 Go setter 的联动语义一致。
    pub fn set_repeats(&self, v: u64) {
        let mut g = self.inner.lock().unwrap();
        g.repeats = v;
        g.remains = v;
    }

    /// 返回 remains 随机衰减时使用的概率参数。
    pub fn probability(&self) -> u32 {
        self.inner.lock().unwrap().probability
    }

    /// 更新概率，不额外做范围修正，保持与 Go 的“原样接受配置”一致。
    pub fn set_probability(&self, v: u32) {
        self.inner.lock().unwrap().probability = v;
    }

    /// 手动消耗一次 remains。
    /// 这里显式保护 0，避免普通减法在 Rust 中触发下溢 panic。
    pub fn dec_remains(&self) {
        let mut g = self.inner.lock().unwrap();
        if g.remains > 0 {
            g.remains -= 1;
        }
    }

    /// 初始化整数范围。
    /// 与 Go 一样只在首次调用时生效，后续调用会被忽略。
    /// 当步长为负时，把当前值放到区间中点，便于反向遍历前先落在合法范围内。
    pub fn setInitInt64Value(&self, minv: i64, maxv: i64) {
        let mut d = self.inner.lock().unwrap();
        if d.init {
            return;
        }
        d.minIntValue = minv;
        d.maxIntValue = maxv;
        d.useRange = true;
        if d.step < 0 {
            d.intValue = minv.wrapping_add(maxv) / 2;
        }
        d.init = true;
    }

    // #nosec G404
    fn updateRemains(d: &mut DatumInner) {
        // 这里保留 Go 的“概率决定衰减幅度”模型：
        // 一条分支随机跳过多个重复次数，另一条分支只减 1。
        // Go uint64 arithmetic wraps on underflow (debug Rust would panic).
        if (stubs::rand_int31n(100) as u32) + 1 <= 100 - d.probability {
            // When remains==0, Go rand.Int63n(0) panics; avoid by matching only when >0,
            // else wrap like a completed underflow path.
            if d.remains == 0 {
                d.remains = d.remains.wrapping_sub(1);
            } else {
                d.remains = d
                    .remains
                    .wrapping_sub(stubs::rand_int63n(d.remains as i64) as u64 + 1);
            }
        } else {
            d.remains = d.remains.wrapping_sub(1);
        }
    }

    /// 返回当前整数值，并在需要时先做区间夹紧。
    /// 该方法不负责推进 `intValue` 本身；推进逻辑由其他生成路径决定。
    /// 它只消费 remains，确保调用方看到的重复行为和 Go 一致。
    pub fn nextInt64(&self) -> i64 {
        let mut d = self.inner.lock().unwrap();
        if d.useRange {
            d.intValue = d.intValue.min(d.maxIntValue);
            d.intValue = d.intValue.max(d.minIntValue);
        }
        Self::updateRemains(&mut d);
        d.intValue
    }

    /// 把当前整数值按 `alphabet` 视为进制表转换成短字符串。
    /// 生成顺序与 Go 完全一致：先取低位、最后反转，得到自然阅读顺序。
    /// `n` 只是上限，若整数很快除到 0，会提前结束。
    pub fn nextString(&self, n: i32) -> String {
        let mut data = self.nextInt64();
        let mut n = n;
        let mut value: Vec<u8> = Vec::new();
        loop {
            if n == 0 {
                break;
            }
            n -= 1;
            let idx = (data % alphabet.len() as i64) as usize;
            data /= alphabet.len() as i64;
            value.push(alphabet.as_bytes()[idx]);
            if data == 0 {
                break;
            }
        }
        value.reverse();
        String::from_utf8_lossy(&value).into_owned()
    }

    /// 以 `HH:MM:SS` 形式返回时间。
    /// 首次访问时才捕获当前时间，保证同一个 datum 的日期/时间/年份派生一致。
    pub fn nextTime(&self) -> String {
        let mut d = self.inner.lock().unwrap();
        if d.timeValue.is_zero() {
            d.timeValue = CivilTime::now();
        }
        Self::updateRemains(&mut d);
        d.timeValue.format_time()
    }

    /// 以 `YYYY-MM-DD` 形式返回日期，并复用同一份 timeValue。
    pub fn nextDate(&self) -> String {
        let mut d = self.inner.lock().unwrap();
        if d.timeValue.is_zero() {
            d.timeValue = CivilTime::now();
        }
        Self::updateRemains(&mut d);
        d.timeValue.format_date()
    }

    /// 以 `YYYY-MM-DD HH:MM:SS` 形式返回完整时间戳。
    pub fn nextTimestamp(&self) -> String {
        let mut d = self.inner.lock().unwrap();
        if d.timeValue.is_zero() {
            d.timeValue = CivilTime::now();
        }
        Self::updateRemains(&mut d);
        d.timeValue.format_datetime()
    }

    /// 只提取四位年份，保持与 Go `fmt.Sprintf("%04d")` 相同的输出契约。
    pub fn nextYear(&self) -> String {
        let mut d = self.inner.lock().unwrap();
        if d.timeValue.is_zero() {
            d.timeValue = CivilTime::now();
        }
        Self::updateRemains(&mut d);
        d.timeValue.format_year()
    }
}

/// 闭区间随机整数。
/// `+1` 保留 Go `rand.Intn(max-min+1)` 的上界可达语义。
pub fn randInt(minv: i32, maxv: i32) -> i32 {
    minv + stubs::rand_intn(maxv - minv + 1)
}

/// 闭区间随机 `i64`，与 Go `rand.Int63n` 包装方式一致。
pub fn randInt64(minv: i64, maxv: i64) -> i64 {
    minv + stubs::rand_int63n(maxv - minv + 1)
}

/// 生成固定长度的随机字符串。
/// 这里沿用 Go 社区常见的位缓存技巧，尽量减少随机数调用次数。
/// 只有索引落在字符表范围内时才接受该字符，因此能避开取模偏差。
pub fn randString(n: i32) -> String {
    let n = n.max(0) as usize;
    let mut b = vec![0u8; n];
    let mut i = n as i32 - 1;
    let mut cache = stubs::rand_int63();
    let mut remain = letterIdxMax;
    while i >= 0 {
        if remain == 0 {
            cache = stubs::rand_int63();
            remain = letterIdxMax;
        }
        let idx = (cache & letterIdxMask) as usize;
        if idx < alphabet.len() {
            b[i as usize] = alphabet.as_bytes()[idx];
            i -= 1;
        }
        cache >>= letterIdxBits;
        remain -= 1;
    }
    String::from_utf8_lossy(&b).into_owned()
}
