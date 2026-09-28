// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

//! 本模块是 importer 使用统计信息的最小接口层。
//! 它负责读取统计文件、包装直方图并在桶边界之间做随机抽样。
//! 与完整统计子系统相比，这里只保留 importer 真正会消费的能力。
//! 整数列、字符串列和时间列各自有不同的取样策略。
//! 字符串列的核心技巧是先找可共享前缀，再补齐剩余长度。
//! 时间列则复用时间差与格式化逻辑，保持 Go 的取样顺序。
//! 平均长度采用惰性初始化，避免未使用字符串列时白白遍历边界。
//! 桶索引的偶数与奇数含义直接对应 Go 里的区间值与重复值分支。
//! 空统计或不支持的复杂统计 JSON 会退回空表，而不是强行报错。
//! 这样主流程仍可运行，只是失去按统计分布造数的能力。
//! 这里的注释重点解释取样边界和分布假设。
//! 本次改动不改变任何桶选择、前缀计算或时间格式化行为。
//! 后续若扩展统计格式，也应先保证当前简化契约不被破坏。
//! 因为 importer 的价值在于稳定生成可插入数据，而不是完整还原统计系统。
//! 理解这些约束有助于后续调试“为什么样本分布和 Go 不同”的问题。

use std::sync::Mutex;

use crate::data::{randInt, randInt64, randString};
use crate::stubs::{self, HistogramCore, IndexInfo, Result, StatsTable, TableInfo, timestamp_diff};

/// `loadStats` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn loadStats(tblInfo: &TableInfo, path: &str) -> Result<StatsTable> {
    stubs::load_stats_file(tblInfo, path)
}

/// `histogram` 是 importer 对统计直方图的轻量包装。
/// 它只保留随机取样所需的桶边界、索引元信息与字符串平均长度缓存。
/// 这里不试图复刻完整统计对象，避免把 importer 拉回重型依赖链。
pub struct histogram {
    pub core: HistogramCore,
    pub index: Option<IndexInfo>,
    pub avgLen: Mutex<i32>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl histogram {
    pub fn from_core(core: HistogramCore, index: Option<IndexInfo>) -> Self {
        Self {
            core,
            index,
            avgLen: Mutex::new(0),
        }
    }

    /// 按桶累计计数随机落点，再映射成 `Bounds` 里的边界索引。
    /// 偶数索引表示命中桶内部区间，要在上下界之间继续随机。
    /// 奇数索引表示命中 `Repeat` 区域，直接返回上界值以对齐 Go 分布。
    pub fn getRandomBoundIdx(&self) -> i32 {
        let cnt = self.core.Buckets[self.core.Buckets.len() - 1].Count;
        let randCnt = randInt64(0, cnt);
        for (i, bkt) in self.core.Buckets.iter().enumerate() {
            if bkt.Count >= randCnt {
                if bkt.Count - bkt.Repeat > randCnt {
                    return 2 * i as i32;
                }
                return 2 * i as i32 + 1;
            }
        }
        0
    }

    /// 整数列复用边界索引语义。
    /// 命中区间时在上下界之间随机，命中重复值时直接取桶边界。
    /// 这样生成出的热点值比例才能与 Go 的桶采样保持一致。
    pub fn randInt(&self) -> i64 {
        let idx = self.getRandomBoundIdx();
        if idx % 2 == 0 {
            let lower = self.core.Bounds.GetRow(idx).GetInt64(0);
            let upper = self.core.Bounds.GetRow(idx + 1).GetInt64(0);
            return randInt64(lower, upper);
        }
        self.core.Bounds.GetRow(idx).GetInt64(0)
    }

    /// 字符串样本长度不从概率模型推导，而是直接用边界串平均长度近似。
    /// 结果还会被 `maxLen` 截断，避免在宽字符串统计上生成过长样本。
    pub fn getAvgLen(&self, maxLen: i32) -> i32 {
        let l = self.core.Bounds.NumRows();
        let mut totalLen = 0;
        for i in 0..l {
            totalLen += self.core.Bounds.GetRow(i).GetString(0).len() as i32;
        }
        let mut avg = (totalLen / l).min(maxLen);
        if avg == 0 {
            avg = 1;
        }
        avg
    }

    /// 字符串列先抽桶，再按上下界求一个仍落在区间内的公共前缀。
    /// 剩余长度用随机字符补齐，因此生成串更像“位于边界之间”而不是复制边界。
    /// `avgLen` 需要由调用方提前惰性初始化，否则这里只会追加零长度后缀。
    pub fn randString(&self) -> String {
        let idx = self.getRandomBoundIdx();
        if idx % 2 == 0 {
            let lower = self.core.Bounds.GetRow(idx).GetString(0);
            let upper = self.core.Bounds.GetRow(idx + 1).GetString(0);
            let mut prefix = getValidPrefix(&lower, &upper);
            let avg = *self.avgLen.lock().unwrap();
            let restLen = avg - prefix.len() as i32;
            if restLen > 0 {
                prefix.push_str(&randString(restLen));
            }
            return prefix;
        }
        self.core.Bounds.GetRow(idx).GetString(0)
    }

    /// 时间列沿用 Go 版本的“先算差值、再按天偏移”策略。
    /// 即使 `unit` 参与 `timestamp_diff`，真正落点仍通过 `add_days` 生成。
    /// 这看起来不够通用，但保持现状才能避免与既有 importer 样本分布脱节。
    pub fn randDate(&self, unit: &str, mysqlFmt: &str, dateFmt: &str) -> String {
        let idx = self.getRandomBoundIdx();
        if idx % 2 == 0 {
            let lower = self.core.Bounds.GetRow(idx).GetTime(0);
            let upper = self.core.Bounds.GetRow(idx + 1).GetTime(0);
            let diff = timestamp_diff(unit, &lower, &upper);
            if diff == 0 {
                return lower.DateFormat(mysqlFmt).unwrap_or_else(|err| {
                    stubs::fatal(err.Error());
                });
            }
            let delta = randInt(0, (diff as i32) - 1);
            let l = lower.GoTime().unwrap_or_else(|err| {
                stubs::fatal(err.Error());
            });
            let l = l.add_days(delta);
            return l.Format(dateFmt);
        }
        self.core
            .Bounds
            .GetRow(idx)
            .GetTime(0)
            .DateFormat(mysqlFmt)
            .unwrap_or_else(|err| {
                stubs::fatal(err.Error());
            })
    }

    /// 平均长度只在第一次真正需要字符串采样时计算一次。
    /// 这里用互斥锁缓存结果，是为了保持共享直方图在并发调用下也只做一次遍历。
    /// 零值既代表“尚未初始化”，也与 `getAvgLen` 的最小返回值 1 不冲突。
    pub fn ensure_avg_len(&self, n: i32) {
        let mut g = self.avgLen.lock().unwrap();
        if *g == 0 {
            *g = self.getAvgLen(n);
        }
    }
}

// #nosec G404
/// 寻找一个严格位于 `lower`/`upper` 字节序之间的最短前缀。
/// Go 的 `range string` 只产生 UTF-8 字符的起始字节偏移；这里保持同样的偏移集合，
/// 但仍按这些偏移处的原始字节比较和构造结果。
/// 随机字节使用与 Go 相同的包裹加法风格，避免生成不同的边界字符分布。
pub fn getValidPrefix(lower: &str, upper: &str) -> String {
    let lb = lower.as_bytes();
    let ub = upper.as_bytes();
    for (i, _) in lower.char_indices() {
        if i >= ub.len() {
            stubs::fatal(format!(
                "lower is larger than upper lower={lower} upper={upper}"
            ));
        }
        if lb[i] != ub[i] {
            // Go: uint8(rand.Intn(int(upper[i]-lower[i]))) + lower[i] (byte wrap).
            let span = ub[i].wrapping_sub(lb[i]) as i32;
            let randCh = (stubs::rand_intn(span) as u8).wrapping_add(lb[i]);
            let mut newBytes = Vec::with_capacity(i + 1);
            newBytes.extend_from_slice(&lb[..i]);
            newBytes.push(randCh);
            return String::from_utf8_lossy(&newBytes).into_owned();
        }
    }
    lower.to_string()
}
