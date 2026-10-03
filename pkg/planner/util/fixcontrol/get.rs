// Copyright 2023 PingCAP, Inc.
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

// fix-control 编号常量与按类型读取接口。
//
// fix-control 以 issue 编号为键、字符串为值，控制优化器局部行为
//（如 plan cache、分区访问、index merge 等）；本文件提供编号常量
// 以及字符串 / 布尔 / 整数 / 浮点的查找与默认值回退。

#![allow(non_snake_case, non_upper_case_globals)]

use std::{collections::HashMap, num::IntErrorKind};

// 以下常量与 Go 的 fix-control 编号一一对应；用途注释保留，便于人工追踪对应 issue。
/// 禁用不支持列投影的 Get/BatchGet fast path。
// Fix52592：禁用不支持列投影的 Get/BatchGet fast path。
pub const Fix52592: u64 = 52592;
/// 控制分区表是否允许 plan cache（计划缓存）。
// Fix33031：控制分区表是否允许 plan cache。
pub const Fix33031: u64 = 33031;
/// 控制优化阶段是否求值非相关子查询，主要供 Index Advisor 使用。
// Fix43817：控制优化阶段是否求值非相关子查询，主要供 Index Advisor 使用。
pub const Fix43817: u64 = 43817;
/// 控制无 global stats 时是否使用 dynamic-mode 访问分区表。
// Fix44262：控制无 global stats 时是否使用 dynamic-mode 访问分区表。
pub const Fix44262: u64 = 44262;
/// 控制构造 range 时是否考虑 CNF 条目的非 point range。
// Fix44389：控制构造 range 时是否考虑 CNF 条目的非 point range。
pub const Fix44389: u64 = 44389;
/// 控制复杂场景下是否缓存 Batch/PointGet。
// Fix44830：控制复杂场景下是否缓存 Batch/PointGet。
pub const Fix44830: u64 = 44830;
/// 控制 Plan Cache 可缓存查询的最大参数数量。
// Fix44823：控制 Plan Cache 可缓存查询的最大参数数量。
pub const Fix44823: u64 = 44823;
/// 控制 IndexJoin 内侧行数估算：NDV 上界默认 OFF；只使用部分连接键的
/// EQ-prefix 扫描行数下界默认 ON。显式 OFF 同时禁用二者（#69974）。
pub const Fix44855: u64 = 44855;
/// 控制 Skyline pruning 是否使用 access range 行数选择 access path。
// Fix45132：控制 Skyline pruning 是否使用 access range 行数选择 access path。
pub const Fix45132: u64 = 45132;
/// 控制是否消除 apply operator。
// Fix45822：控制是否消除 apply operator。
pub const Fix45822: u64 = 45822;
/// 控制是否缓存访问 generated column 的计划。
// Fix45798：控制是否缓存访问 generated column 的计划。
pub const Fix45798: u64 = 45798;
/// 控制 DataSource 已找到 unenforced plan 后是否继续探索 enforced plan。
// Fix46177：控制 DataSource 已找到 unenforced plan 后是否继续探索 enforced plan。
pub const Fix46177: u64 = 46177;
/// 控制是否允许 rowEst 小于 1；该 fix-control 已废弃。
// Fix47400：控制是否允许 rowEst 小于 1；该 fix-control 已废弃。
pub const Fix47400: u64 = 47400;
/// 测试专用，控制存在风险优化时是否强制使用 plan cache。
// Fix49736：测试专用，控制存在风险优化时是否强制使用 plan cache。
pub const Fix49736: u64 = 49736;
/// 控制存在其它单索引 range path 时是否自动生成 index merge path。
// Fix52869：控制存在其它单索引 range path 时是否自动生成 index merge path。
pub const Fix52869: u64 = 52869;
/// 控制 index access 是否应用 range intersection。
// Fix54337：控制 index access 是否应用 range intersection。
pub const Fix54337: u64 = 54337;
/// 控制是否执行 HeavyFunctionOptimize，移除 TopN 中函数的使用。
// Fix56318：控制是否执行 HeavyFunctionOptimize，移除 TopN 中函数的使用。
pub const Fix56318: u64 = 56318;

/// 按键查找字符串值，返回 (值, 是否存在)；map 为 None 时视为不存在。
// GetStr 对应 Go 的 map 查找，返回值和 exists 标志保持原顺序。
pub fn GetStr<'a>(
    fixControlMap: impl Into<Option<&'a HashMap<u64, String>>>,
    key: u64,
) -> (String, bool) {
    let Some(map) = fixControlMap.into() else {
        return (String::new(), false);
    };
    match map.get(&key) {
        Some(value) => (value.clone(), true),
        None => (String::new(), false),
    }
}

/// 查找字符串；键不存在时返回 defaultVal。
// GetStrWithDefault 在键不存在时返回调用方提供的默认字符串。
pub fn GetStrWithDefault<'a>(
    fixControlMap: impl Into<Option<&'a HashMap<u64, String>>>,
    key: u64,
    defaultVal: impl Into<String>,
) -> String {
    let (value, exists) = GetStr(fixControlMap, key);
    if !exists { defaultVal.into() } else { value }
}

/// 按 TiDBOptOn 语义解析布尔：不区分大小写的 ON 或精确值 `1` 为 true。
// GetBool 对应 Go 的 TiDBOptOn 判断：不区分大小写的 ON 或精确值 1 表示 true。
pub fn GetBool<'a>(
    fixControlMap: impl Into<Option<&'a HashMap<u64, String>>>,
    key: u64,
) -> (bool, bool) {
    let Some(map) = fixControlMap.into() else {
        return (false, false);
    };
    let Some(rawValue) = map.get(&key) else {
        return (false, false);
    };
    (rawValue.eq_ignore_ascii_case("ON") || rawValue == "1", true)
}

/// 解析布尔；键不存在时返回 defaultVal。
// GetBoolWithDefault 在键不存在时使用默认布尔值。
pub fn GetBoolWithDefault<'a>(
    fixControlMap: impl Into<Option<&'a HashMap<u64, String>>>,
    key: u64,
    defaultVal: bool,
) -> bool {
    let (value, exists) = GetBool(fixControlMap, key);
    if !exists { defaultVal } else { value }
}

/// 按十进制解析 i64，返回 (值, 是否存在, 解析结果)；仅键存在时可能产生 Err。
// GetInt 对应 strconv.ParseInt(rawValue, 10, 64)，保留存在标志和解析错误三个返回值。
pub fn GetInt<'a>(
    fixControlMap: impl Into<Option<&'a HashMap<u64, String>>>,
    key: u64,
) -> (i64, bool, Result<(), String>) {
    let Some(map) = fixControlMap.into() else {
        return (0, false, Ok(()));
    };
    let Some(rawValue) = map.get(&key) else {
        return (0, false, Ok(()));
    };
    match rawValue.parse::<i64>() {
        Ok(value) => (value, true, Ok(())),
        Err(err) => {
            // strconv.ParseInt returns the nearest signed limit together with
            // ErrRange. Preserve that value because callers of GetInt can
            // inspect all three results independently.
            let value = match err.kind() {
                IntErrorKind::PosOverflow => i64::MAX,
                IntErrorKind::NegOverflow => i64::MIN,
                _ => 0,
            };
            (value, true, Err(err.to_string()))
        }
    }
}

/// 解析整数；键不存在或解析失败时回退到 defaultVal。
// GetIntWithDefault 在键不存在或十进制解析失败时回退到 defaultVal。
pub fn GetIntWithDefault<'a>(
    fixControlMap: impl Into<Option<&'a HashMap<u64, String>>>,
    key: u64,
    defaultVal: i64,
) -> i64 {
    let (value, exists, err) = GetInt(fixControlMap, key);
    if !exists || err.is_err() {
        defaultVal
    } else {
        value
    }
}

/// 按 f64 解析浮点，返回 (值, 是否存在, 解析结果)；仅键存在时可能产生 Err。
// GetFloat 对应 strconv.ParseFloat(rawValue, 64)，错误只在键存在时产生。
pub fn GetFloat<'a>(
    fixControlMap: impl Into<Option<&'a HashMap<u64, String>>>,
    key: u64,
) -> (f64, bool, Result<(), String>) {
    let Some(map) = fixControlMap.into() else {
        return (0.0, false, Ok(()));
    };
    let Some(rawValue) = map.get(&key) else {
        return (0.0, false, Ok(()));
    };
    match rawValue.parse::<f64>() {
        Ok(value) if value.is_infinite() && !is_go_infinity(rawValue) => {
            (value, true, Err("value out of range for f64".to_owned()))
        }
        Ok(value) => (value, true, Ok(())),
        Err(err) => (0.0, true, Err(err.to_string())),
    }
}

fn is_go_infinity(value: &str) -> bool {
    let unsigned = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    unsigned.eq_ignore_ascii_case("inf") || unsigned.eq_ignore_ascii_case("infinity")
}

/// 解析浮点；键不存在或解析失败时返回 defaultVal。
// GetFloatWithDefault 在键不存在或浮点解析失败时返回默认值。
pub fn GetFloatWithDefault<'a>(
    fixControlMap: impl Into<Option<&'a HashMap<u64, String>>>,
    key: u64,
    defaultVal: f64,
) -> f64 {
    let (value, exists, err) = GetFloat(fixControlMap, key);
    if !exists || err.is_err() {
        defaultVal
    } else {
        value
    }
}
