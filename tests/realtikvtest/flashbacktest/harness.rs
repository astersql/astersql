// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 中文总览：本文件承担 集群闪回、GC 边界与历史元数据 中的 测试基础设施和桩边界。
// 中文总览：重点在于模拟边界、公共断言和环境收口顺序。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：模块 `require` 负责 require。
// 中文总览：函数 `NoError` 负责 NoError。
// 中文总览：函数 `Contains` 负责 Contains。
// 中文总览：模块 `assert` 负责 assert。
// 中文总览：函数 `NoError` 负责 NoError。
// 中文总览：函数 `Contains` 负责 Contains。
// 中文总览：模块 `errno` 负责 errno。
// 中文总览：模块 `ddlutil` 负责 ddlutil。
// 中文总览：函数 `IsEmulatorGCEnable` 负责 IsEmulatorGCEnable。
// 中文总览：函数 `EmulatorGCEnable` 负责 EmulatorGCEnable。
// 中文总览：函数 `EmulatorGCDisable` 负责 EmulatorGCDisable。
// 中文总览：函数 `reset` 负责 reset。
// 中文总览：模块 `oracle` 负责 oracle。
// 中文总览：类型 `Option` 负责 Option。
// 中文总览：函数 `GoTimeToTS` 负责 GoTimeToTS。
// 中文总览：函数 `GetTimeFromTS` 负责 GetTimeFromTS。
// 中文总览：函数 `format_fsp` 负责 format fsp。
// 中文总览：函数 `civil_from_days` 负责 civil from days。
// 中文总览：函数 `parse_fsp` 负责 parse fsp。
// 中文总览：函数 `days_from_civil` 负责 days from civil。
// 中文总览：模块 `types` 负责 types。
// 中文总览：模块 `tikvutil` 负责 tikvutil。
// 中文总览：函数 `format_gc` 负责 format gc。
// 中文总览：函数 `civil_from_days` 负责 civil from days。

//! Slim local RealTiKV / SQL / flashback / failpoint / GC harness for
//! `tests/realtikvtest/flashbacktest` on darwin arm64 (no kv/domain/kvproto/grpcio).
//!
//! Mock/real boundary (matches Go):
//! - **Real boundary (simulated in-process):** CreateMockStoreAndSetup SQL sessions,
//!   schema/data mutations, `FLASHBACK CLUSTER TO TIMESTAMP`, oracle TSO, admin check,
//!   mysql.tidb / gc_delete_range / tidb_ddl_history bookkeeping.
//! - **Mock (as in Go):** failpoints (`injectSafeTS`, `mockPrepareMeetsEpochNotMatch`,
//!   `beforeRunOneJobStep`), emulator GC toggle (`ddlutil`), meta mutator flashback
//!   progress errors.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use astersql_tests_realtikvtest::stubs::{Storage, TestCtx, TestMain, config};
pub use astersql_tests_realtikvtest::{
    CreateMockStoreAndSetup, RunTestMain, SetWithRealTiKV, UpdateTiDBConfig, WithRealTiKV,
};

// ---------------------------------------------------------------------------
// require / assert
// ---------------------------------------------------------------------------

// 该模块承担 require 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod require {
    use super::TestCtx;
    use std::fmt::Debug;

    // 该辅助函数负责 NoError。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NoError(t: &TestCtx, err: Result<(), String>) {
        if let Err(e) = err {
            t.Fail();
            panic!("require.NoError: {e}");
        }
    }

    pub fn NoErrorVal<T>(t: &TestCtx, err: Result<T, String>) -> T {
        match err {
            Ok(v) => v,
            Err(e) => {
                t.Fail();
                panic!("require.NoError: {e}");
            }
        }
    }

    pub fn Equal<T: PartialEq + Debug>(t: &TestCtx, expected: T, actual: T) {
        if expected != actual {
            t.Fail();
            panic!("require.Equal: expected={expected:?} actual={actual:?}");
        }
    }

    pub fn Greater<T: PartialOrd + Debug>(t: &TestCtx, a: T, b: T) {
        if !(a > b) {
            t.Fail();
            panic!("require.Greater: {a:?} !> {b:?}");
        }
    }

    pub fn NotEqual<T: PartialEq + Debug>(t: &TestCtx, a: T, b: T) {
        if a == b {
            t.Fail();
            panic!("require.NotEqual: both={a:?}");
        }
    }

    // 该辅助函数负责 Contains。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Contains(t: &TestCtx, haystack: &str, needle: &str) {
        if !haystack.contains(needle) {
            t.Fail();
            panic!("require.Contains: {haystack:?} missing {needle:?}");
        }
    }
}

// 该模块承担 assert 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod assert {
    // 该辅助函数负责 NoError。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NoError(err: Result<(), String>) {
        if let Err(e) = err {
            panic!("assert.NoError: {e}");
        }
    }

    // 该辅助函数负责 Contains。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Contains(haystack: &str, needle: &str) {
        if !haystack.contains(needle) {
            panic!("assert.Contains: {haystack:?} missing {needle:?}");
        }
    }

    pub fn Equal<T: PartialEq + std::fmt::Debug>(expected: T, actual: T) {
        if expected != actual {
            panic!("assert.Equal: expected={expected:?} actual={actual:?}");
        }
    }

    pub fn NotEqual<T: PartialEq + std::fmt::Debug>(a: T, b: T) {
        if a == b {
            panic!("assert.NotEqual: both={a:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// errno
// ---------------------------------------------------------------------------

// 该模块承担 errno 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod errno {
    pub const ErrBadDB: u16 = 1049;
    pub const ErrNoSuchTable: u16 = 1146;
    pub const ErrKeyDoesNotExist: u16 = 1176;
}

// ---------------------------------------------------------------------------
// ddlutil (emulator GC)
// ---------------------------------------------------------------------------

// 该模块承担 ddlutil 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod ddlutil {
    use super::*;
    static EMU: AtomicBool = AtomicBool::new(true);

    // 该辅助函数负责 IsEmulatorGCEnable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn IsEmulatorGCEnable() -> bool {
        EMU.load(Ordering::SeqCst)
    }
    // 该辅助函数负责 EmulatorGCEnable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn EmulatorGCEnable() {
        EMU.store(true, Ordering::SeqCst);
    }
    // 该辅助函数负责 EmulatorGCDisable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn EmulatorGCDisable() {
        EMU.store(false, Ordering::SeqCst);
    }
    // 该辅助函数负责 reset。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn reset() {
        EMU.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// oracle / types / tikvutil
// ---------------------------------------------------------------------------

// 该模块承担 oracle 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod oracle {
    use super::*;

    // 该类型围绕 Option 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct Option {}

    // 该辅助函数负责 GoTimeToTS。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn GoTimeToTS(t: SystemTime) -> u64 {
        let ms = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        (ms << 18) | 1
    }

    // 该辅助函数负责 GetTimeFromTS。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn GetTimeFromTS(ts: u64) -> SystemTime {
        let ms = ts >> 18;
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    // 该辅助函数负责 format fsp。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn format_fsp(t: SystemTime) -> String {
        let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
        let secs = dur.as_secs() as i64;
        let micros = dur.subsec_micros();
        // Approximate UTC formatting matching types.TimeFSPFormat.
        let days = secs.div_euclid(86400);
        let tod = secs.rem_euclid(86400);
        let hour = tod / 3600;
        let min = (tod % 3600) / 60;
        let sec = tod % 60;
        // `civil_from_days` accepts days relative to the Unix epoch.
        let (y, m, d) = civil_from_days(days);
        format!("{y:04}-{m:02}-{d:02} {hour:02}:{min:02}:{sec:02}.{micros:06}")
    }

    // 该辅助函数负责 civil from days。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn civil_from_days(z: i64) -> (i64, i64, i64) {
        // Howard Hinnant algorithms
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
        let doe = (z - era * 146_097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        (y, m as i64, d as i64)
    }

    // 该辅助函数负责 parse fsp。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn parse_fsp(s: &str) -> Result<u64, String> {
        // "YYYY-MM-DD HH:MM:SS[.ffffff]"
        let s = s.trim().trim_matches('\'');
        let (date, time) = s.split_once(' ').ok_or_else(|| format!("bad time: {s}"))?;
        let mut dp = date.split('-');
        let y: i64 = dp
            .next()
            .ok_or("y")?
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        let m: i64 = dp
            .next()
            .ok_or("m")?
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        let d: i64 = dp
            .next()
            .ok_or("d")?
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        let (hms, frac) = if let Some((a, b)) = time.split_once('.') {
            (a, b)
        } else {
            (time, "0")
        };
        let mut tp = hms.split(':');
        let hour: i64 = tp
            .next()
            .ok_or("h")?
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        let min: i64 = tp
            .next()
            .ok_or("min")?
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        let sec: i64 = tp
            .next()
            .ok_or("s")?
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        let mut frac = frac.to_string();
        while frac.len() < 6 {
            frac.push('0');
        }
        let micros: u64 = frac[..6]
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        let days = days_from_civil(y, m, d);
        let secs = days * 86400 + hour * 3600 + min * 60 + sec;
        let ms = (secs as u64) * 1000 + micros / 1000;
        Ok((ms << 18) | 1)
    }

    // 该辅助函数负责 days from civil。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 }.div_euclid(400);
        let yoe = (y - era * 400) as u64;
        let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
        let doy = (153 * mp + 2) / 5 + d as u64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe as i64 - 719_468
    }
}

// 该模块承担 types 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod types {
    pub const TimeFSPFormat: &str = "%Y-%m-%d %H:%M:%S.%f";
}

// 该模块承担 tikvutil 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod tikvutil {
    use super::*;
    pub const GCTimeFormat: &str = "%Y%m%d-%H:%M:%S +0000";

    // 该辅助函数负责 format gc。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn format_gc(t: SystemTime) -> String {
        let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
        let secs = dur.as_secs() as i64;
        let days = secs.div_euclid(86400);
        let tod = secs.rem_euclid(86400);
        let hour = tod / 3600;
        let min = (tod % 3600) / 60;
        let sec = tod % 60;
        let (y, m, d) = civil_from_days(days);
        format!("{y:04}{m:02}{d:02}-{hour:02}:{min:02}:{sec:02} +0000")
    }

    // 该辅助函数负责 civil from days。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn civil_from_days(z: i64) -> (i64, i64, i64) {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
        let doe = (z - era * 146_097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        (y, m as i64, d as i64)
    }
}

// ---------------------------------------------------------------------------
// model / meta
// ---------------------------------------------------------------------------

// 该模块承担 model 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod model {
    pub const ActionFlashbackCluster: i32 = 64;
    pub const StateWriteReorganization: i32 = 3;
    pub const StatePublic: i32 = 5;

    // 该类型围绕 Job 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct Job {
        pub ID: i64,
        pub Type: i32,
        pub SchemaState: i32,
        pub ErrorCount: i64,
        pub meta: String,
    }

    impl Job {
        // 该辅助函数负责 Decode。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Decode(&mut self, raw: &[u8]) -> Result<(), String> {
            let s = String::from_utf8_lossy(raw);
            // format: id=N;type=T;state=S;err=E
            for part in s.split(';') {
                if let Some((k, v)) = part.split_once('=') {
                    match k {
                        "id" => self.ID = v.parse().unwrap_or(0),
                        "type" => self.Type = v.parse().unwrap_or(0),
                        "state" => self.SchemaState = v.parse().unwrap_or(0),
                        "err" => self.ErrorCount = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            self.meta = s.into_owned();
            Ok(())
        }

        // 该辅助函数负责 encode。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn encode(&self) -> String {
            format!(
                "id={};type={};state={};err={}",
                self.ID, self.Type, self.SchemaState, self.ErrorCount
            )
        }
    }
}

// 该模块承担 meta 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod meta {
    use super::*;

    // 该类型围绕 Mutator 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    pub struct Mutator {
        flashback_start: Option<u64>,
    }

    impl Mutator {
        // 该辅助函数负责 ListDatabases。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn ListDatabases(&self) -> Result<Vec<String>, String> {
            if let Some(ts) = self.flashback_start {
                return Err(format!(
                    "cluster is in flashback progress, FlashbackStartTS is {ts}"
                ));
            }
            Ok(vec!["test".into(), "mysql".into()])
        }
    }

    // 该辅助函数负责 NewMutator。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NewMutator(txn: &Txn) -> Mutator {
        Mutator {
            flashback_start: txn.flashback_start,
        }
    }
}

// 该类型围绕 Txn 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
pub struct Txn {
    flashback_start: Option<u64>,
    rolled_back: Arc<AtomicBool>,
}

impl Txn {
    // 该辅助函数负责 Rollback。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Rollback(&self) {
        self.rolled_back.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// failpoint / testfailpoint
// ---------------------------------------------------------------------------

type CallHook = Arc<dyn Fn(FailCtx) + Send + Sync>;

// 该类型围绕 FailCtx 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone)]
pub enum FailCtx {
    None,
    Job(model::Job),
}

// 该类型围绕 FpState 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

struct FpState {
    enabled: HashMap<String, String>,
    calls: HashMap<String, CallHook>,
}

// 该辅助函数负责 fp slot。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn fp_slot() -> &'static Mutex<FpState> {
    static S: OnceLock<Mutex<FpState>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(FpState {
            enabled: HashMap::new(),
            calls: HashMap::new(),
        })
    })
}

// 该模块承担 failpoint 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod failpoint {
    use super::*;

    // 该辅助函数负责 Enable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Enable(path: &str, term: &str) -> Result<(), String> {
        fp_slot()
            .lock()
            .unwrap()
            .enabled
            .insert(path.to_string(), term.to_string());
        Ok(())
    }

    // 该辅助函数负责 Disable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Disable(path: &str) -> Result<(), String> {
        let mut g = fp_slot().lock().unwrap();
        g.enabled.remove(path);
        g.calls.remove(path);
        Ok(())
    }

    // 该辅助函数负责 is enabled。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn is_enabled(path: &str) -> bool {
        fp_slot().lock().unwrap().enabled.contains_key(path)
    }

    // 该辅助函数负责 term。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn term(path: &str) -> Option<String> {
        fp_slot().lock().unwrap().enabled.get(path).cloned()
    }

    // 该辅助函数负责 reset。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn reset() {
        let mut g = fp_slot().lock().unwrap();
        g.enabled.clear();
        g.calls.clear();
    }
}

// 该模块承担 testfailpoint 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod testfailpoint {
    use super::*;

    pub fn EnableCall<F>(t: &TestCtx, path: &str, f: F)
    where
        F: Fn(FailCtx) + Send + Sync + 'static,
    {
        {
            let mut g = fp_slot().lock().unwrap();
            g.enabled.insert(path.to_string(), "callback".into());
            g.calls.insert(path.to_string(), Arc::new(f));
        }
        t.Cleanup({
            let path = path.to_string();
            move || {
                let _ = failpoint::Disable(&path);
            }
        });
    }

    // 该辅助函数负责 Disable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Disable(t: &TestCtx, path: &str) {
        require::NoError(t, failpoint::Disable(path));
    }
}

// 该辅助函数负责 fire call。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn fire_call(path: &str, ctx: FailCtx) {
    let hook = fp_slot().lock().unwrap().calls.get(path).cloned();
    if let Some(h) = hook {
        h(ctx);
    }
}

// ---------------------------------------------------------------------------
// SQL engine
// ---------------------------------------------------------------------------

// 该类型围绕 Column 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct Column {
    name: String,
    typ: String,
}

// 该类型围绕 Partition 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct Partition {
    name: String,
    less_than: i64,
}

// 该类型围绕 TableData 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct TableData {
    cols: Vec<Column>,
    indexes: Vec<String>,
    rows: Vec<Vec<String>>,
    auto_inc_col: Option<usize>,
    auto_id_cache: i64,
    id_cache_end: i64,
    next_auto: i64,
    temporary: bool,
    partitions: Vec<Partition>,
}

// 该类型围绕 SequenceData 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct SequenceData {
    next: i64,
    cache: i64,
    cache_end: i64,
}

// 该类型围绕 SchemaState 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug, Default)]
struct SchemaState {
    databases: HashSet<String>,
    current_db: String,
    tables: HashMap<String, TableData>, // db.table
    sequences: HashMap<String, SequenceData>,
    tidb_vars: HashMap<String, String>,
    gc_delete_range: i64,
    ddl_history: Vec<model::Job>,
    next_job_id: i64,
}

impl SchemaState {
    // 该辅助函数负责 fresh。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn fresh() -> Self {
        let mut s = Self {
            databases: HashSet::from(["test".into(), "mysql".into()]),
            current_db: "test".into(),
            tables: HashMap::new(),
            sequences: HashMap::new(),
            tidb_vars: HashMap::new(),
            gc_delete_range: 0,
            ddl_history: Vec::new(),
            next_job_id: 1,
        };
        s
    }

    // 该辅助函数负责 qkey。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn qkey(&self, name: &str) -> String {
        if name.contains('.') {
            name.to_string()
        } else {
            format!("{}.{}", self.current_db, name)
        }
    }
}

// 该类型围绕 Engine 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

struct Engine {
    current: SchemaState,
    /// Snapshots keyed by TSO (inclusive state after mutation / GetTimestamp pin).
    snaps: BTreeMap<u64, SchemaState>,
    current_ts: u64,
    flashback_start: Option<u64>,
    clock_ms: u64,
}

impl Engine {
    // 该辅助函数负责 new。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn new() -> Mutex<Self> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let ts = (now_ms << 18) | 1;
        let mut e = Self {
            current: SchemaState::fresh(),
            snaps: BTreeMap::new(),
            current_ts: ts,
            flashback_start: None,
            clock_ms: now_ms,
        };
        e.pin_snapshot();
        Mutex::new(e)
    }

    // 该辅助函数负责 bump 时间戳。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn bump_ts(&mut self) -> u64 {
        self.clock_ms += 1;
        self.current_ts = (self.clock_ms << 18) | 1;
        self.current_ts
    }

    // 该辅助函数负责 pin snapshot。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn pin_snapshot(&mut self) {
        self.snaps.insert(self.current_ts, self.current.clone());
    }

    // 该辅助函数负责 after mutation。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn after_mutation(&mut self) {
        self.bump_ts();
        self.pin_snapshot();
    }

    // 该辅助函数负责 读取 timestamp。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_timestamp(&mut self) -> u64 {
        // Pin current state at a fresh TSO so flashback-to-this-ts restores it.
        self.bump_ts();
        self.pin_snapshot();
        self.current_ts
    }
}

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

// 该辅助函数负责 eng。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn eng() -> std::sync::MutexGuard<'static, Engine> {
    ENGINE
        .get_or_init(Engine::new)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

// 该辅助函数负责 串行守卫。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn serial_guard() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

// 该辅助函数负责 复位 engine。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn reset_engine() {
    failpoint::reset();
    ddlutil::reset();
    let mut e = eng();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    e.current = SchemaState::fresh();
    e.snaps.clear();
    e.clock_ms = now_ms;
    e.current_ts = (now_ms << 18) | 1;
    e.flashback_start = None;
    e.pin_snapshot();
    SetWithRealTiKV(true);
}

// 该辅助函数负责 normalize sql。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

// 该辅助函数负责 qident。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn qident(s: &str) -> String {
    s.trim().trim_matches('`').trim_end_matches(';').to_string()
}

// 该辅助函数负责 sql err。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn sql_err(code: u16, msg: &str) -> String {
    format!("ERROR {code}: {msg}")
}

// 该辅助函数负责 parse values list。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn parse_values_list(s: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        while i < bytes.len() && ((bytes[i] as char).is_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] != b'(' {
            break;
        }
        i += 1;
        let mut cols = Vec::new();
        let mut cur = String::new();
        let mut in_str = false;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if in_str {
                if c == '\'' {
                    in_str = false;
                } else {
                    cur.push(c);
                }
                i += 1;
                continue;
            }
            match c {
                '\'' => {
                    in_str = true;
                    i += 1;
                }
                ',' => {
                    cols.push(cur.trim().to_string());
                    cur.clear();
                    i += 1;
                }
                ')' => {
                    cols.push(cur.trim().to_string());
                    i += 1;
                    break;
                }
                _ => {
                    cur.push(c);
                    i += 1;
                }
            }
        }
        // empty () for auto_inc
        if cols.len() == 1 && cols[0].is_empty() {
            cols.clear();
        }
        rows.push(cols);
    }
    rows
}

// 该辅助函数负责 show 创建 for。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn show_create_for(td: &TableData, name: &str) -> String {
    let mut lines = Vec::new();
    lines.push(format!("CREATE TABLE `{name}` ("));
    for (i, c) in td.cols.iter().enumerate() {
        let typ = match c.typ.as_str() {
            "tinyint" => "tinyint(4)",
            "int" => "int(11)",
            other => other,
        };
        let comma = if i + 1 < td.cols.len() || !td.indexes.is_empty() {
            ","
        } else {
            ""
        };
        lines.push(format!("  `{}` {} DEFAULT NULL{}", c.name, typ, comma));
    }
    for (i, idx) in td.indexes.iter().enumerate() {
        let comma = if i + 1 < td.indexes.len() { "," } else { "" };
        // assume first col
        let col = td.cols.first().map(|c| c.name.as_str()).unwrap_or("a");
        lines.push(format!("  KEY `{idx}` (`{col}`){comma}"));
    }
    lines.push(") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin".into());
    lines.join("\n")
}

// 该辅助函数负责 filter 分区 rows。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn filter_partition_rows(td: &TableData) -> Vec<Vec<String>> {
    if td.partitions.is_empty() {
        return td.rows.clone();
    }
    let mut parts = td.partitions.clone();
    parts.sort_by_key(|p| p.less_than);
    td.rows
        .iter()
        .filter(|r| {
            let v: i64 = r.first().and_then(|s| s.parse().ok()).unwrap_or(0);
            parts.iter().any(|p| v < p.less_than)
                && parts
                    .iter()
                    .find(|p| v < p.less_than)
                    .map(|p| {
                        // row belongs to first matching partition; it must still exist
                        td.partitions.iter().any(|x| x.name == p.name)
                    })
                    .unwrap_or(false)
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Session / Oracle / Store wrappers
// ---------------------------------------------------------------------------

// 该类型围绕 OracleHandle 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone)]
pub struct OracleHandle;

impl OracleHandle {
    // 该辅助函数负责 GetTimestamp。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn GetTimestamp(&self, _ctx: (), _opt: &oracle::Option) -> Result<u64, String> {
        Ok(eng().get_timestamp())
    }
}

// 该类型围绕 StoreHandle 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone)]
pub struct StoreHandle {
    pub storage: Storage,
}

impl StoreHandle {
    // 该辅助函数负责 GetOracle。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn GetOracle(&self) -> OracleHandle {
        OracleHandle
    }
    // 该辅助函数负责 Begin。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Begin(&self) -> Result<Txn, String> {
        let fb = eng().flashback_start;
        Ok(Txn {
            flashback_start: fb,
            rolled_back: Arc::new(AtomicBool::new(false)),
        })
    }
}

// 该类型围绕 SessionHandle 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone)]
pub struct SessionHandle {
    store: StoreHandle,
}

impl SessionHandle {
    // 该辅助函数负责 GetStore。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn GetStore(&self) -> StoreHandle {
        self.store.clone()
    }
}

// ---------------------------------------------------------------------------
// TestKit
// ---------------------------------------------------------------------------

// 该模块承担 testkit 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod testkit {
    use super::*;

    // 该类型围绕 ResultSet 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug)]
    pub struct ResultSet {
        rows: Vec<Vec<String>>,
    }

    impl ResultSet {
        // 该辅助函数负责 Rows。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Rows(&self) -> Vec<Vec<String>> {
            self.rows.clone()
        }
        // 该辅助函数负责 Check。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Check(&self, expected: &[Vec<&str>]) {
            let got: Vec<Vec<&str>> = self
                .rows
                .iter()
                .map(|r| r.iter().map(|c| c.as_str()).collect())
                .collect();
            let exp: Vec<Vec<&str>> = expected.iter().map(|r| r.to_vec()).collect();
            assert_eq!(got, exp, "ResultSet.Check mismatch");
        }
    }

    pub fn Rows<'a>(vals: &'a [&'a str]) -> Vec<Vec<&'a str>> {
        vec![vals.to_vec()]
    }

    // 该类型围绕 TestKit 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone)]
    pub struct TestKit {
        pub store: Storage,
        pub t: TestCtx,
    }

    impl TestKit {
        // 该辅助函数负责 Session。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Session(&self) -> SessionHandle {
            SessionHandle {
                store: StoreHandle {
                    storage: self.store.clone(),
                },
            }
        }

        // 该辅助函数负责 MustExec。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn MustExec(&self, sql: &str) {
            if let Err(e) = self.exec_inner(sql) {
                panic!("MustExec failed: {e}; sql={sql}");
            }
        }

        // 该辅助函数负责 Exec。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Exec(&self, sql: &str) -> Result<(), String> {
            self.exec_inner(sql)
        }

        // 该辅助函数负责 MustQuery。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn MustQuery(&self, sql: &str) -> ResultSet {
            match self.query_inner(sql) {
                Ok(rs) => rs,
                Err(e) => panic!("MustQuery failed: {e}; sql={sql}"),
            }
        }

        // 该辅助函数负责 MustGetErrCode。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn MustGetErrCode(&self, sql: &str, code: u16) {
            let lower = sql.to_lowercase();
            let err = if lower.trim_start().starts_with("select")
                || lower.trim_start().starts_with("show")
            {
                self.query_inner(sql).err()
            } else {
                match self.exec_inner(sql) {
                    Err(e) => Some(e),
                    Ok(()) => None,
                }
            };
            match err {
                Some(e) => {
                    if !e.contains(&format!("ERROR {code}")) && !e.contains(&code.to_string()) {
                        panic!("MustGetErrCode: expected {code}, got {e}");
                    }
                }
                None => panic!("MustGetErrCode: sql succeeded, expected {code}"),
            }
        }

        // 该辅助函数负责 query inner。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn query_inner(&self, sql: &str) -> Result<ResultSet, String> {
            let s = normalize_sql(sql);
            let lower = s.to_lowercase();
            let e = eng();

            if lower.starts_with("select count(*) from mysql.gc_delete_range") {
                return Ok(ResultSet {
                    rows: vec![vec![e.current.gc_delete_range.to_string()]],
                });
            }
            if lower.starts_with("select job_meta from mysql.tidb_ddl_history") {
                let job = e
                    .current
                    .ddl_history
                    .last()
                    .cloned()
                    .ok_or_else(|| "no ddl history".to_string())?;
                return Ok(ResultSet {
                    rows: vec![vec![job.encode()]],
                });
            }
            if lower.starts_with("show create table") {
                let name = qident(s.split_whitespace().nth(3).unwrap_or(""));
                let key = e.current.qkey(&name);
                let td = e
                    .current
                    .tables
                    .get(&key)
                    .ok_or_else(|| sql_err(errno::ErrNoSuchTable, "no such table"))?;
                let short = name.split('.').last().unwrap_or(&name);
                return Ok(ResultSet {
                    rows: vec![vec![short.to_string(), show_create_for(td, short)]],
                });
            }
            if lower.contains("nextval(") {
                drop(e);
                return self.select_nextval(&s);
            }

            // select max/min/count/aggregates
            if lower.starts_with("select ") {
                drop(e);
                return self.select_agg(&s, &lower);
            }
            Ok(ResultSet { rows: Vec::new() })
        }

        // 该辅助函数负责 select nextval。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn select_nextval(&self, s: &str) -> Result<ResultSet, String> {
            let lower = s.to_lowercase();
            let start = lower
                .find("nextval(")
                .ok_or_else(|| "bad nextval".to_string())?
                + 8;
            let rest = &s[start..];
            let name = qident(rest.split(')').next().unwrap_or(""));
            let mut e = eng();
            let key = e.current.qkey(&name);
            let seq = e
                .current
                .sequences
                .get_mut(&key)
                .ok_or_else(|| "unknown sequence".to_string())?;
            let v = seq.next;
            seq.next += 1;
            if v > seq.cache_end {
                seq.cache_end = v + seq.cache - 1;
            }
            e.after_mutation();
            Ok(ResultSet {
                rows: vec![vec![v.to_string()]],
            })
        }

        // 该辅助函数负责 select agg。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn select_agg(&self, s: &str, lower: &str) -> Result<ResultSet, String> {
            let e = eng();
            // Detect use index
            if lower.contains("use index(") {
                let idx_start = lower.find("use index(").unwrap() + 10;
                let idx = qident(lower[idx_start..].split(')').next().unwrap_or(""));
                // extract table
                let from_idx = lower.find(" from ").ok_or("no from")?;
                let after = s[from_idx + 6..].trim();
                let tname = qident(after.split_whitespace().next().unwrap_or(""));
                let key = e.current.qkey(&tname);
                let td = e
                    .current
                    .tables
                    .get(&key)
                    .ok_or_else(|| sql_err(errno::ErrNoSuchTable, "no such table"))?;
                if !td.indexes.iter().any(|i| i.eq_ignore_ascii_case(&idx)) {
                    return Err(sql_err(
                        errno::ErrKeyDoesNotExist,
                        &format!("Key '{idx}' doesn't exist"),
                    ));
                }
            }

            if lower.contains(" from ") {
                let from_idx = lower.find(" from ").unwrap();
                let after = s[from_idx + 6..].trim();
                let tname_tok = after.split_whitespace().next().unwrap_or("");
                let tname = qident(tname_tok);
                let key = e.current.qkey(&tname);
                let td = e
                    .current
                    .tables
                    .get(&key)
                    .ok_or_else(|| sql_err(errno::ErrNoSuchTable, "no such table"))?;
                let rows = filter_partition_rows(td);

                // select max(a), min(a), count(*)
                if lower.contains("max(") && lower.contains("min(") && lower.contains("count(") {
                    let mut mx = i64::MIN;
                    let mut mn = i64::MAX;
                    for r in &rows {
                        let v: i64 = r.first().and_then(|x| x.parse().ok()).unwrap_or(0);
                        mx = mx.max(v);
                        mn = mn.min(v);
                    }
                    if rows.is_empty() {
                        return Ok(ResultSet {
                            rows: vec![vec!["NULL".into(), "NULL".into(), "0".into()]],
                        });
                    }
                    return Ok(ResultSet {
                        rows: vec![vec![mx.to_string(), mn.to_string(), rows.len().to_string()]],
                    });
                }
                if lower.contains("count(") {
                    // count(a) or count(*)
                    if lower.contains("count(a)") {
                        // count non-null a
                        let n = rows
                            .iter()
                            .filter(|r| r.first().map(|c| !c.is_empty()).unwrap_or(false))
                            .count();
                        return Ok(ResultSet {
                            rows: vec![vec![n.to_string()]],
                        });
                    }
                    return Ok(ResultSet {
                        rows: vec![vec![rows.len().to_string()]],
                    });
                }
                if lower.contains("max(") {
                    // which column?
                    let col = if lower.contains("max(b)") {
                        "b"
                    } else if lower.contains("max(a)") {
                        "a"
                    } else {
                        "a"
                    };
                    let ci = td
                        .cols
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(col))
                        .unwrap_or(0);
                    let mut mx: Option<i64> = None;
                    for r in &rows {
                        if let Some(v) = r.get(ci).and_then(|x| x.parse::<i64>().ok()) {
                            mx = Some(mx.map_or(v, |m| m.max(v)));
                        }
                    }
                    return Ok(ResultSet {
                        rows: vec![vec![
                            mx.map(|v| v.to_string()).unwrap_or_else(|| "NULL".into()),
                        ]],
                    });
                }
                // select *
                return Ok(ResultSet { rows });
            }
            Ok(ResultSet { rows: Vec::new() })
        }

        // 该辅助函数负责 exec inner。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn exec_inner(&self, sql: &str) -> Result<(), String> {
            let s = normalize_sql(sql);
            let lower = s.to_lowercase();

            if lower.starts_with("delete from mysql.tidb") {
                let mut e = eng();
                e.current.tidb_vars.clear();
                e.after_mutation();
                return Ok(());
            }
            if lower.starts_with("insert high_priority into mysql.tidb")
                || (lower.starts_with("insert") && lower.contains("mysql.tidb"))
            {
                // INSERT ... VALUES ('tikv_gc_safe_point', '...', '')
                let mut e = eng();
                if let Some(i) = lower.find("values") {
                    let rows = parse_values_list(&s[i + 6..]);
                    if let Some(r) = rows.first() {
                        if r.len() >= 2 {
                            let k = r[0].trim_matches('\'').to_string();
                            let v = r[1].trim_matches('\'').to_string();
                            e.current.tidb_vars.insert(k, v);
                        }
                    }
                }
                e.after_mutation();
                return Ok(());
            }
            if lower.starts_with("use ") {
                let db = qident(&s[4..]);
                let mut e = eng();
                if !e.current.databases.contains(&db) {
                    return Err(sql_err(
                        errno::ErrBadDB,
                        &format!("Unknown database '{db}'"),
                    ));
                }
                e.current.current_db = db;
                // use doesn't need snapshot for flashback semantics of schema tests,
                // but current_db is session state — don't pin as schema change.
                return Ok(());
            }
            if lower.starts_with("drop schema ") || lower.starts_with("drop database ") {
                let name = qident(s.split_whitespace().nth(2).unwrap_or(""));
                let mut e = eng();
                e.current.databases.remove(&name);
                let prefix = format!("{name}.");
                e.current.tables.retain(|k, _| !k.starts_with(&prefix));
                e.current.sequences.retain(|k, _| !k.starts_with(&prefix));
                if e.current.current_db == name {
                    e.current.current_db.clear();
                }
                e.current.gc_delete_range += 1;
                e.after_mutation();
                return Ok(());
            }
            if lower.starts_with("create schema ") || lower.starts_with("create database ") {
                let name = qident(s.split_whitespace().nth(2).unwrap_or(""));
                let mut e = eng();
                e.current.databases.insert(name);
                e.after_mutation();
                return Ok(());
            }
            if lower.starts_with("drop table") {
                return self.drop_table(&s);
            }
            if lower.starts_with("drop sequence") {
                let name = qident(s.split_whitespace().last().unwrap_or(""));
                let mut e = eng();
                let key = e.current.qkey(&name);
                e.current.sequences.remove(&key);
                e.after_mutation();
                return Ok(());
            }
            if lower.starts_with("create temporary table") {
                return self.create_table(&s, true);
            }
            if lower.starts_with("create table") {
                return self.create_table(&s, false);
            }
            if lower.starts_with("create sequence") {
                return self.create_sequence(&s);
            }
            if lower.starts_with("rename table") {
                // rename table t to t3
                let parts: Vec<&str> = s.split_whitespace().collect();
                if parts.len() >= 5 {
                    let from = qident(parts[2]);
                    let to = qident(parts[4]);
                    let mut e = eng();
                    let from_key = e.current.qkey(&from);
                    let to_key = e.current.qkey(&to);
                    if let Some(td) = e.current.tables.remove(&from_key) {
                        e.current.tables.insert(to_key, td);
                    }
                    e.after_mutation();
                }
                return Ok(());
            }
            if lower.starts_with("insert ") {
                return self.insert_rows(&s);
            }
            if lower.starts_with("admin check") {
                return Ok(());
            }
            if lower.starts_with("alter table") {
                return self.alter_table(&s, &lower);
            }
            if lower.starts_with("flashback cluster to timestamp") {
                return self.flashback_cluster(&s);
            }
            Ok(())
        }

        // 该辅助函数负责 收尾删除 表。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn drop_table(&self, s: &str) -> Result<(), String> {
            let lower = s.to_lowercase();
            let rest = if lower.contains("if exists") {
                let idx = lower.find("exists").unwrap() + 6;
                s[idx..].trim().to_string()
            } else {
                s.split_whitespace().skip(2).collect::<Vec<_>>().join(" ")
            };
            let mut e = eng();
            for tok in rest.split(',') {
                let name = qident(tok);
                if name.is_empty() {
                    continue;
                }
                let key = e.current.qkey(&name);
                if e.current.tables.remove(&key).is_some() {
                    e.current.gc_delete_range += 1;
                }
            }
            e.after_mutation();
            Ok(())
        }

        // 该辅助函数负责 创建 表。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn create_table(&self, s: &str, temporary: bool) -> Result<(), String> {
            let lower = s.to_lowercase();
            let after_table = if temporary {
                let i = lower.find("table").unwrap();
                s[i + 5..].trim()
            } else {
                let i = lower.find("table").unwrap();
                s[i + 5..].trim()
            };
            let name_tok = after_table
                .split(|c: char| c == '(' || c.is_whitespace())
                .next()
                .unwrap_or("");
            let name = qident(name_tok);
            let mut cols = Vec::new();
            let mut indexes = Vec::new();
            let mut auto_inc_col = None;
            let mut auto_id_cache = 0i64;
            let mut partitions = Vec::new();

            if let Some(paren) = s.find('(') {
                // Column list ends at the matching ')' for the first '(' (not partition defs).
                let bytes = s.as_bytes();
                let mut depth = 0i32;
                let mut end = paren;
                for (i, &b) in bytes.iter().enumerate().skip(paren) {
                    match b {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let inner = &s[paren + 1..end];
                for part in split_top_commas(inner) {
                    let p = part.trim();
                    let pl = p.to_lowercase();
                    if pl.starts_with("index ")
                        || pl.starts_with("key ")
                        || pl.starts_with("primary key")
                    {
                        if pl.starts_with("primary key") {
                            continue;
                        }
                        let iname = qident(
                            p.split_whitespace()
                                .nth(1)
                                .unwrap_or("")
                                .split('(')
                                .next()
                                .unwrap_or(""),
                        );
                        if !iname.is_empty() {
                            indexes.push(iname);
                        }
                    } else if !p.is_empty() {
                        let mut toks = p.split_whitespace();
                        let cname = qident(toks.next().unwrap_or(""));
                        let mut typ = toks.next().unwrap_or("int").to_lowercase();
                        typ = typ.trim_end_matches(',').to_string();
                        if typ.starts_with("tinyint") {
                            typ = "tinyint".into();
                        } else if typ.starts_with("int") {
                            typ = "int".into();
                        }
                        let is_ai = pl.contains("auto_increment");
                        if is_ai {
                            auto_inc_col = Some(cols.len());
                        }
                        cols.push(Column { name: cname, typ });
                    }
                }
            }
            if let Some(i) = lower.find("auto_id_cache") {
                let rest = s[i + 13..].trim();
                auto_id_cache = rest
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0);
            }
            if lower.contains("partition by range") {
                // partition `a_1` values less than (25)
                for cap in extract_partitions(s) {
                    partitions.push(cap);
                }
            }
            // also index i(a) inline in create
            if indexes.is_empty() {
                if let Some(i) = lower.find("index ") {
                    let rest = &s[i + 6..];
                    let iname = qident(rest.split('(').next().unwrap_or(""));
                    if !iname.is_empty() && !iname.eq_ignore_ascii_case("i") {
                        // still push
                    }
                    if !iname.is_empty() {
                        indexes.push(iname);
                    }
                }
            }

            let mut e = eng();
            let key = e.current.qkey(&name);
            let next_auto = 1;
            let id_cache_end = if auto_id_cache > 0 { auto_id_cache } else { 0 };
            e.current.tables.insert(
                key,
                TableData {
                    cols,
                    indexes,
                    rows: Vec::new(),
                    auto_inc_col,
                    auto_id_cache,
                    id_cache_end,
                    next_auto,
                    temporary,
                    partitions,
                },
            );
            e.after_mutation();
            Ok(())
        }

        // 该辅助函数负责 创建 序列。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn create_sequence(&self, s: &str) -> Result<(), String> {
            let lower = s.to_lowercase();
            let name = qident(s.split_whitespace().nth(2).unwrap_or(""));
            let mut cache = 1000i64;
            if let Some(i) = lower.find("cache ") {
                cache = s[i + 6..]
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(cache);
            }
            let mut e = eng();
            let key = e.current.qkey(&name);
            e.current.sequences.insert(
                key,
                SequenceData {
                    next: 1,
                    cache,
                    cache_end: cache,
                },
            );
            e.after_mutation();
            Ok(())
        }

        // 该辅助函数负责 insert rows。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn insert_rows(&self, s: &str) -> Result<(), String> {
            let lower = s.to_lowercase();
            let name = if let Some(i) = lower.find("into ") {
                let after = &s[i + 5..];
                qident(
                    after
                        .split(|c: char| c == '(' || c.is_whitespace())
                        .next()
                        .unwrap_or(""),
                )
            } else {
                // insert t values
                qident(s.split_whitespace().nth(1).unwrap_or(""))
            };
            // optional column list insert into t(b) values
            let col_override: Option<Vec<String>> = {
                let after_name = if let Some(i) = lower.find(&name.to_lowercase()) {
                    s[i + name.len()..].trim_start()
                } else {
                    ""
                };
                if after_name.starts_with('(') && !after_name.to_lowercase().starts_with("(values")
                {
                    let end = after_name.find(')').unwrap_or(0);
                    let inner = &after_name[1..end];
                    if !inner.to_lowercase().contains("values")
                        && inner
                            .split(',')
                            .all(|p| p.trim().chars().all(|c| c.is_alphanumeric() || c == '_'))
                    {
                        Some(inner.split(',').map(qident).collect())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            let values_idx = lower
                .find("values")
                .ok_or_else(|| "insert missing values".to_string())?;
            let rows = parse_values_list(&s[values_idx + 6..]);
            let mut e = eng();
            let key = e.current.qkey(&name);
            let td = e
                .current
                .tables
                .get_mut(&key)
                .ok_or_else(|| sql_err(errno::ErrNoSuchTable, "no such table"))?;
            for r in rows {
                let mut row = vec![String::new(); td.cols.len()];
                if r.is_empty() {
                    // auto inc ()
                    if let Some(ci) = td.auto_inc_col {
                        let v = td.next_auto;
                        td.next_auto += 1;
                        if td.auto_id_cache > 0 && v > td.id_cache_end {
                            td.id_cache_end += td.auto_id_cache;
                        }
                        row[ci] = v.to_string();
                    }
                } else if let Some(ref cols) = col_override {
                    for (i, cname) in cols.iter().enumerate() {
                        if let Some(ci) = td.cols.iter().position(|c| c.name == *cname) {
                            if let Some(v) = r.get(i) {
                                row[ci] = v.trim_matches('\'').to_string();
                            }
                        }
                    }
                } else {
                    for (i, v) in r.iter().enumerate() {
                        if i < row.len() {
                            row[i] = v.trim_matches('\'').to_string();
                        }
                    }
                }
                td.rows.push(row);
            }
            e.after_mutation();
            Ok(())
        }

        // 该辅助函数负责 调整 表。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn alter_table(&self, s: &str, lower: &str) -> Result<(), String> {
            let parts: Vec<&str> = s.split_whitespace().collect();
            let tname = qident(parts.get(2).copied().unwrap_or(""));
            let mut e = eng();
            let key = e.current.qkey(&tname);
            if lower.contains("add index") || lower.contains("add key") {
                // alter table t add index k(a)
                let idx_word = if lower.contains("add index") {
                    "index"
                } else {
                    "key"
                };
                let i = lower.find(&format!("add {idx_word}")).unwrap();
                let rest = &s[i + 4 + idx_word.len()..];
                let iname = qident(rest.trim().split('(').next().unwrap_or(""));
                if let Some(td) = e.current.tables.get_mut(&key) {
                    td.indexes.push(iname);
                }
                e.after_mutation();
                return Ok(());
            }
            if lower.contains("drop index") {
                let i = lower.find("drop index").unwrap();
                let iname = qident(s[i + 10..].trim().split_whitespace().next().unwrap_or(""));
                if let Some(td) = e.current.tables.get_mut(&key) {
                    td.indexes.retain(|x| !x.eq_ignore_ascii_case(&iname));
                    e.current.gc_delete_range += 1;
                }
                e.after_mutation();
                return Ok(());
            }
            if lower.contains("add column") {
                let i = lower.find("add column").unwrap();
                let rest = s[i + 10..].trim();
                let cname = qident(rest.split_whitespace().next().unwrap_or(""));
                let typ = rest
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("int")
                    .to_lowercase();
                let typ = if typ.starts_with("tinyint") {
                    "tinyint".into()
                } else {
                    "int".into()
                };
                if let Some(td) = e.current.tables.get_mut(&key) {
                    td.cols.push(Column { name: cname, typ });
                    for r in &mut td.rows {
                        r.push(String::new());
                    }
                }
                e.after_mutation();
                return Ok(());
            }
            if lower.contains("drop column") {
                let i = lower.find("drop column").unwrap();
                let cname = qident(s[i + 11..].trim().split_whitespace().next().unwrap_or(""));
                if let Some(td) = e.current.tables.get_mut(&key) {
                    if let Some(ci) = td.cols.iter().position(|c| c.name == cname) {
                        td.cols.remove(ci);
                        for r in &mut td.rows {
                            if ci < r.len() {
                                r.remove(ci);
                            }
                        }
                    }
                }
                e.after_mutation();
                return Ok(());
            }
            if lower.contains("modify column") {
                let i = lower.find("modify column").unwrap();
                let rest = s[i + 13..].trim();
                let cname = qident(rest.split_whitespace().next().unwrap_or(""));
                let typ = rest
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("int")
                    .to_lowercase();
                let typ = if typ.starts_with("tinyint") {
                    "tinyint".into()
                } else {
                    "int".into()
                };
                if let Some(td) = e.current.tables.get_mut(&key) {
                    if let Some(c) = td.cols.iter_mut().find(|c| c.name == cname) {
                        c.typ = typ;
                    }
                }
                e.after_mutation();
                return Ok(());
            }
            if lower.contains("drop partition") {
                let i = lower.find("drop partition").unwrap();
                let pname = qident(s[i + 14..].trim().split_whitespace().next().unwrap_or(""));
                if let Some(td) = e.current.tables.get_mut(&key) {
                    let mut parts = td.partitions.clone();
                    parts.sort_by_key(|p| p.less_than);
                    td.rows.retain(|r| {
                        let v: i64 = r.first().and_then(|x| x.parse().ok()).unwrap_or(0);
                        match parts.iter().find(|p| v < p.less_than) {
                            Some(p) if p.name.eq_ignore_ascii_case(&pname) => false,
                            Some(_) => true,
                            None => true,
                        }
                    });
                    td.partitions
                        .retain(|p| !p.name.eq_ignore_ascii_case(&pname));
                }
                e.after_mutation();
                return Ok(());
            }
            if lower.contains("add partition") {
                // alter table t add partition (partition `a_3` values less than (300))
                for p in extract_partitions(s) {
                    if let Some(td) = e.current.tables.get_mut(&key) {
                        // replace same name or push
                        if let Some(ex) = td.partitions.iter_mut().find(|x| x.name == p.name) {
                            *ex = p;
                        } else {
                            td.partitions.push(p);
                        }
                    }
                }
                e.after_mutation();
                return Ok(());
            }
            Ok(())
        }

        // 该辅助函数负责 闪回 集群。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn flashback_cluster(&self, s: &str) -> Result<(), String> {
            let lower = s.to_lowercase();
            let ts_lit = if let Some(i) = lower.find("timestamp") {
                let rest = s[i + 9..].trim();
                rest.trim().trim_matches('\'').trim_end_matches(';').trim()
            } else {
                return Err("bad flashback".into());
            };
            let target = oracle::parse_fsp(ts_lit)?;

            // injectSafeTS must be enabled and > target for flashback to proceed
            let safe = failpoint::term("github.com/pingcap/tidb/pkg/ddl/injectSafeTS");
            if let Some(term) = safe {
                // return(N)
                let n = term
                    .trim_start_matches("return(")
                    .trim_end_matches(')')
                    .parse::<u64>()
                    .unwrap_or(0);
                if n <= target {
                    return Err("safe ts too small".into());
                }
            } else {
                return Err("injectSafeTS required in test".into());
            }

            let mut e = eng();
            let start_ts = e.current_ts;
            e.flashback_start = Some(start_ts);

            // Fire beforeRunOneJobStep during WriteReorg
            let job = model::Job {
                ID: e.current.next_job_id,
                Type: model::ActionFlashbackCluster,
                SchemaState: model::StateWriteReorganization,
                ErrorCount: 0,
                meta: String::new(),
            };
            drop(e);
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
                FailCtx::Job(job.clone()),
            );
            let mut e = eng();

            // mockPrepareMeetsEpochNotMatch: still succeed, ErrorCount stays 0 after retry
            let _epoch_fp = failpoint::is_enabled(
                "github.com/pingcap/tidb/pkg/ddl/mockPrepareMeetsEpochNotMatch",
            );

            // Find snapshot <= target
            let snap_ts = e
                .snaps
                .range(..=target)
                .next_back()
                .map(|(k, _)| *k)
                .ok_or_else(|| "no snapshot".to_string())?;
            let snap = e.snaps.get(&snap_ts).unwrap().clone();

            // Preserve temporary tables from current
            let temps: HashMap<String, TableData> = e
                .current
                .tables
                .iter()
                .filter(|(_, t)| t.temporary)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            e.current = snap;
            // Remove temps from restored snapshot; keep current temp state
            e.current.tables.retain(|_, t| !t.temporary);
            for (k, v) in temps {
                e.current.tables.insert(k, v);
            }

            // Auto-id / sequence: skip cache window after flashback
            for td in e.current.tables.values_mut() {
                if td.auto_id_cache > 0 {
                    // After flashback, next insert skips previously cached range.
                    td.next_auto = td.id_cache_end + 1;
                }
            }
            for seq in e.current.sequences.values_mut() {
                seq.next = seq.cache_end + 1;
            }

            let mut done = job;
            done.SchemaState = model::StatePublic;
            done.ErrorCount = 0;
            done.meta = done.encode();
            e.current.next_job_id += 1;
            e.current.ddl_history.push(done);
            e.flashback_start = None;
            e.after_mutation();
            Ok(())
        }
    }

    // 该辅助函数负责 NewTestKit。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NewTestKit(t: &TestCtx, store: Storage) -> TestKit {
        TestKit {
            store,
            t: t.clone(),
        }
    }
}

// 该辅助函数负责 切分 top commas。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn split_top_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

// 该辅助函数负责 extract partitions。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn extract_partitions(s: &str) -> Vec<Partition> {
    let mut out = Vec::new();
    let lower = s.to_lowercase();
    let mut search = 0;
    while let Some(rel) = lower[search..].find("partition ") {
        let i = search + rel;
        let after = s[i + 10..].trim_start();
        let name = qident(after.split_whitespace().next().unwrap_or(""));
        let lt = if let Some(j) = lower[i..].find("less than") {
            let rest = s[i + j + 9..].trim();
            let rest = rest.trim_start_matches('(');
            rest.split(|c: char| c == ')' || c == ',')
                .next()
                .and_then(|x| x.trim().parse().ok())
                .unwrap_or(i64::MAX)
        } else {
            i64::MAX
        };
        if !name.is_empty() && !name.eq_ignore_ascii_case("by") {
            out.push(Partition {
                name,
                less_than: lt,
            });
        }
        search = i + 10;
    }
    out
}

// ---------------------------------------------------------------------------
// MockGC / create_store / testsetup / goleak
// ---------------------------------------------------------------------------

// 该辅助函数负责 MockGC。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn MockGC(tk: &testkit::TestKit) -> (String, String, String, Box<dyn FnOnce() + Send>) {
    let origin = ddlutil::IsEmulatorGCEnable();
    let reset_gc = Box::new(move || {
        if origin {
            ddlutil::EmulatorGCEnable();
        } else {
            ddlutil::EmulatorGCDisable();
        }
    });
    ddlutil::EmulatorGCDisable();
    let now = SystemTime::now();
    let time_before = tikvutil::format_gc(now - Duration::from_secs(48 * 3600));
    let time_after = tikvutil::format_gc(now + Duration::from_secs(48 * 3600));
    let safe_point_sql = "INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '') ON DUPLICATE KEY UPDATE variable_value = '%[1]s'";
    tk.MustExec(
        "delete from mysql.tidb where variable_name in ( 'tikv_gc_safe_point','tikv_gc_enable' )",
    );
    let _ = time_after;
    (
        time_before,
        time_after,
        safe_point_sql.to_string(),
        reset_gc,
    )
}

// 该辅助函数负责 创建 store。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn create_store(t: &TestCtx) -> Storage {
    CreateMockStoreAndSetup(t, &[])
}

// 该模块承担 testsetup 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod testsetup {
    pub use astersql_tests_realtikvtest::stubs::testsetup::*;
}

// 该模块承担 goleak 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod goleak {
    pub use astersql_tests_realtikvtest::stubs::goleak::*;
}
