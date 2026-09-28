// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 索引顾问运行时选项：默认值、校验与持久化读写形状。
//
// 选项包括单表最大推荐索引数、单索引最大列数、参与分析的最大查询数以及超时。
// 数值通过 `OptionStore` 抽象读写（对应 Go 中 `mysql.tidb_kernel_options`）。

// 索引顾问选项的默认值、校验和 SQL 调用形状。
// sessionctx、AST 值、executor 等均是后续 Rust 模块接线的外部依赖。
// #![allow(dead_code, non_snake_case, non_upper_case_globals)]
//
// pub const OptModule: &str = "index_advisor";
// pub const OptMaxNumIndex: &str = "max_num_index";
// pub const OptMaxIndexColumns: &str = "max_index_columns";
// pub const OptMaxNumQuery: &str = "max_num_query";
// pub const OptTimeout: &str = "timeout";
//
// pub static AllOptions: [&str; 4] = [OptMaxNumIndex, OptMaxIndexColumns, OptMaxNumQuery, OptTimeout];
//
// fillOption 对应 Go 的 fillOption：先读取持久化值，再让用户选项覆盖，最后填充缺省字段。
// pub fn fillOption(sctx: &sessionctx::Context, opt: &mut Option, userOptions: &[ast::RecommendIndexOption]) -> Result<(), errors::Error> {
// Go 先读取持久化配置，再按用户选项覆盖；不能把默认值提前写回存储。
//     let (mut vals, _desc) = GetOptions(sctx, &AllOptions)?;
//     for userOpt in userOptions {
//         vals.insert(userOpt.Option.clone(), optionVal(&userOpt.Option, &userOpt.Value)?);
//     }
// 0 表示调用方没有显式设置，解析失败时保留 Go 忽略 ParseInt 错误的形状。
//     if opt.MaxNumIndexes == 0 { opt.MaxNumIndexes = vals[OptMaxNumIndex].parse::<i32>().unwrap_or_default(); }
//     if opt.MaxIndexWidth == 0 { opt.MaxIndexWidth = vals[OptMaxIndexColumns].parse::<i32>().unwrap_or_default(); }
//     if opt.MaxNumQuery == 0 { opt.MaxNumQuery = vals[OptMaxNumQuery].parse::<i32>().unwrap_or_default(); }
// timeout 使用 duration 文本，解析错误与 Go 一样向调用方返回。
//     if opt.Timeout == 0 { opt.Timeout = parseDuration(&vals[OptTimeout])?; }
//     Ok(())
// }
//
// SetOptions 保留 Go 中逐个调用 SetOption 的顺序；任一选项失败都会立即返回。
// pub fn SetOptions(sctx: &sessionctx::Context, options: &[ast::RecommendIndexOption]) -> Result<(), errors::Error> {
// 保留 Go 的逐项顺序；中途失败时不会继续写后续选项。
//     for opt in options { SetOption(sctx, &opt.Option, &opt.Value)?; }
//     Ok(())
// }
//
// optionVal 对应 Go 的类型分支：整数选项要求正数，timeout 要求 duration 字符串。
// fn optionVal(opt: &str, val: &ast::ValueExpr) -> Result<String, errors::Error> {
//     match opt {
//         OptMaxNumIndex | OptMaxIndexColumns | OptMaxNumQuery => {
//             let x = intVal(val)?;
// 三个数量类选项必须为正数，负数和零都在这里拒绝。
//             if x <= 0 { return Err(errors::Errorf(format!("invalid value {} for {}", x, opt))); }
//             Ok(x.to_string())
//         }
//         OptTimeout => {
//             let v = val.GetValue().as_string().ok_or_else(|| errors::Errorf(format!("invalid value type for {}, expected a duration string", opt)))?;
// timeout 只接受字符串，不把数字或其它 AST 值隐式转换成 duration。
//             let d = parseDuration(&v)?;
//             if d < 0 { return Err(errors::Errorf(format!("invalid value {} for {}", d, opt))); }
//             Ok(v)
//         }
//         _ => Err(errors::Errorf(format!("unknown option {}", opt))),
//     }
// }
//
// SetOption 保留原 Go 的 UPSERT 模板，但这里只构造并转交抽象 executor，不执行数据库动作。
// pub fn SetOption(sctx: &sessionctx::Context, opt: &str, val: &ast::ValueExpr) -> Result<(), errors::Error> {
//     let v = optionVal(opt, val)?;
//     let template = "INSERT INTO mysql.tidb_kernel_options VALUES (%?, %?, %?, now(), 'valid', %?)\n        ON DUPLICATE KEY UPDATE value = %?, updated_at=now(), description = %?";
//     exec(sctx, template, &[OptModule, opt, &v, &description(opt), &v, &description(opt)])
// }
//
// GetOptions 对应 Go 的查询、行遍历、缺省值补齐和描述生成。
// pub fn GetOptions(sctx: &sessionctx::Context, opts: &[&str]) -> Result<(std::collections::HashMap<String, String>, std::collections::HashMap<String, String>), errors::Error> {
// 保留 Go 直接拼接 IN 列表的形状；真正的参数化由后续 executor 接线处理。
//     let names = opts.iter().map(|opt| format!("'{}'", opt)).collect::<Vec<_>>().join(",");
//     let sql = format!("SELECT name, value FROM mysql.tidb_kernel_options WHERE module = '{}' AND name in ({})", OptModule, names);
//     let rows = exec(sctx, &sql, &[])?;
//     let mut vals = std::collections::HashMap::new();
// 结果集按 name/value 两列映射；缺失项随后用 defaultVal 补齐。
//     for row in rows { vals.insert(row.GetString(0), row.GetString(1)); }
//     for opt in opts { vals.entry((*opt).to_string()).or_insert_with(|| defaultVal(opt).to_string()); }
//     let mut desc = std::collections::HashMap::new();
//     for opt in opts { desc.insert((*opt).to_string(), description(opt).to_string()); }
//     Ok((vals, desc))
// }
//
// fn description(opt: &str) -> &str {
//     match opt { OptMaxNumIndex => "The maximum number of indexes to recommend for a table.", OptMaxIndexColumns => "The maximum number of columns in an index.", OptMaxNumQuery => "The maximum number of queries to recommend indexes.", OptTimeout => "The timeout of index advisor.", _ => "" }
// }
// fn defaultVal(opt: &str) -> &str {
//     match opt { OptMaxNumIndex => "5", OptMaxIndexColumns => "3", OptMaxNumQuery => "1000", OptTimeout => "30s", _ => "" }
// }
// fn intVal(val: &ast::ValueExpr) -> Result<i32, errors::Error> {
//     match val.GetValue() { ast::Value::Int(v) => Ok(v as i32), ast::Value::Uint(v) => Ok(v as i32), ast::Value::Int64(v) => Ok(v as i32), other => Err(errors::Errorf(format!("invalid value type {:?}", other))) }
// }
//
// 以下是外部依赖边界：不伪造数据库执行，只表达 Go 中 duration、结果集和选项结构。
// fn parseDuration(_value: &str) -> Result<i64, errors::Error> { Ok(0) }
// fn exec(_sctx: &sessionctx::Context, _sql: &str, _args: &[&dyn std::fmt::Debug]) -> Result<Vec<chunk::Row>, errors::Error> { Ok(Vec::new()) }
// pub struct Option { pub MaxNumIndexes: i32, pub MaxIndexWidth: i32, pub MaxNumQuery: i32, pub Timeout: i64 }
// */
use std::collections::BTreeMap;
use std::time::Duration;

/// 内核选项模块名，对应持久化表中的 module 字段。
pub const OPT_MODULE: &str = "index_advisor";
/// 单表最多推荐索引个数。
pub const OPT_MAX_NUM_INDEX: &str = "max_num_index";
/// 单个索引最多包含的列数（索引宽度）。
pub const OPT_MAX_INDEX_COLUMNS: &str = "max_index_columns";
/// workload 中最多分析的查询条数。
pub const OPT_MAX_NUM_QUERY: &str = "max_num_query";
/// 顾问整体超时时长。
pub const OPT_TIMEOUT: &str = "timeout";
/// 全部可配置选项名列表。
pub const ALL_OPTIONS: [&str; 4] = [
    OPT_MAX_NUM_INDEX,
    OPT_MAX_INDEX_COLUMNS,
    OPT_MAX_NUM_QUERY,
    OPT_TIMEOUT,
];

#[derive(Clone, Debug, PartialEq, Eq)]
/// 一次顾问运行的参数快照；字段为 0/ZERO 时由 `fill_options` 从存储补齐。
pub struct AdvisorOptions {
    pub max_num_indexes: usize,
    pub max_index_width: usize,
    pub max_num_query: usize,
    pub timeout: Duration,
}
impl Default for AdvisorOptions {
    fn default() -> Self {
        Self {
            max_num_indexes: 5,
            max_index_width: 3,
            max_num_query: 1000,
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 用户传入的选项值：整数、无符号整数或 duration 文本。
pub enum OptionValue {
    Integer(i64),
    Unsigned(u64),
    Text(String),
}

/// 选项持久化抽象：对应 Go 侧通过 SQL 读写 `tidb_kernel_options`。
pub trait OptionStore {
    /// 批量读取 module 下若干选项名的当前值。
    fn get(&self, module: &str, names: &[&str]) -> Result<BTreeMap<String, String>, String>;
    /// 写入（或 UPSERT）单个选项及其描述。
    fn set(&self, module: &str, name: &str, value: &str, description: &str) -> Result<(), String>;
}

/// 先读持久化值，再用用户覆盖，最后填充当前结构中仍为“未设置”的字段。
pub fn fill_options(
    store: &dyn OptionStore,
    current: &mut AdvisorOptions,
    overrides: &[(String, OptionValue)],
) -> Result<(), String> {
    let mut values = get_options(store, &ALL_OPTIONS)?;
    for (name, value) in overrides {
        values.insert(name.clone(), option_value(name, value.clone())?);
    }
    // 0 表示调用方未显式设置，从存储/默认值补齐。
    if current.max_num_indexes == 0 {
        current.max_num_indexes = parse_positive(&values, OPT_MAX_NUM_INDEX)?;
    }
    if current.max_index_width == 0 {
        current.max_index_width = parse_positive(&values, OPT_MAX_INDEX_COLUMNS)?;
    }
    if current.max_num_query == 0 {
        current.max_num_query = parse_positive(&values, OPT_MAX_NUM_QUERY)?;
    }
    if current.timeout.is_zero() {
        current.timeout = parse_duration(
            values
                .get(OPT_TIMEOUT)
                .ok_or_else(|| "missing timeout".to_string())?,
        )?;
    }
    Ok(())
}

/// 校验并持久化单个选项。
pub fn set_option(store: &dyn OptionStore, name: &str, value: OptionValue) -> Result<(), String> {
    let value = option_value(name, value)?;
    store.set(OPT_MODULE, name, &value, description(name))
}

/// 按调用方给定顺序逐项校验并持久化；遇到首个错误立即返回。
pub fn set_options(
    store: &dyn OptionStore,
    options: &[(String, OptionValue)],
) -> Result<(), String> {
    for (name, value) in options {
        set_option(store, name, value.clone())?;
    }
    Ok(())
}

/// 读取选项；缺失项用 `default_value` 补齐。
pub fn get_options(
    store: &dyn OptionStore,
    names: &[&str],
) -> Result<BTreeMap<String, String>, String> {
    let mut values = store.get(OPT_MODULE, names)?;
    for name in names {
        values
            .entry((*name).to_string())
            .or_insert_with(|| default_value(name).to_string());
    }
    Ok(values)
}

/// 按选项名校验用户值并转为持久化字符串。
fn option_value(name: &str, value: OptionValue) -> Result<String, String> {
    match name {
        OPT_MAX_NUM_INDEX | OPT_MAX_INDEX_COLUMNS | OPT_MAX_NUM_QUERY => {
            // 数量类选项必须为正数。
            let value = match value {
                OptionValue::Integer(value) if value > 0 => value as u64,
                OptionValue::Unsigned(value) if value > 0 && value <= i64::MAX as u64 => value,
                _ => return Err(format!("invalid value for {name}")),
            };
            Ok(value.to_string())
        }
        OPT_TIMEOUT => match value {
            OptionValue::Text(value) => {
                // timeout 只接受 duration 字符串，解析失败则拒绝。
                parse_duration(&value)?;
                Ok(value)
            }
            _ => Err(format!(
                "invalid value type for {name}, expected a duration string"
            )),
        },
        _ => Err(format!("unknown option {name}")),
    }
}

/// 从映射中解析正整数选项。
fn parse_positive(values: &BTreeMap<String, String>, name: &str) -> Result<usize, String> {
    // Go deliberately ignores strconv.ParseInt errors while filling the runtime
    // option snapshot, leaving the corresponding field at zero.
    Ok(values
        .get(name)
        .ok_or_else(|| format!("missing option {name}"))?
        .parse::<usize>()
        .unwrap_or_default())
}

/// 解析 Go 风格 duration 文本（支持小数和连续单位，如 `1h30m`）。
pub fn parse_duration(value: &str) -> Result<Duration, String> {
    if value.is_empty() {
        return Err(format!("invalid duration {value}"));
    }
    let (negative, value) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    if value == "0" {
        return Ok(Duration::ZERO);
    }
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut nanos = 0_u128;
    while cursor < bytes.len() {
        let number_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        let integer_end = cursor;
        let mut fraction_start = cursor;
        let mut has_fraction = false;
        if cursor < bytes.len() && bytes[cursor] == b'.' {
            has_fraction = true;
            cursor += 1;
            fraction_start = cursor;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
        }
        if (integer_end == number_start && fraction_start == cursor)
            || (has_fraction && fraction_start == cursor)
        {
            return Err(format!("invalid duration {value}"));
        }
        let integer = if integer_end == number_start {
            0
        } else {
            value[number_start..integer_end]
                .parse::<u128>()
                .map_err(|_| format!("invalid duration {value}"))?
        };
        let fraction = if has_fraction {
            value[fraction_start..cursor]
                .parse::<u128>()
                .map_err(|_| format!("invalid duration {value}"))?
        } else {
            0
        };
        let scale = 10_u128
            .checked_pow((cursor - fraction_start) as u32)
            .ok_or_else(|| format!("invalid duration {value}"))?;
        let remaining = &value[cursor..];
        let (unit_len, multiplier) = if remaining.starts_with("ns") {
            (2, 1_u128)
        } else if remaining.starts_with("us") {
            (2, 1_000)
        } else if remaining.starts_with("µs") || remaining.starts_with("μs") {
            (2, 1_000)
        } else if remaining.starts_with("ms") {
            (2, 1_000_000)
        } else if remaining.starts_with('s') {
            (1, 1_000_000_000)
        } else if remaining.starts_with('m') {
            (1, 60 * 1_000_000_000)
        } else if remaining.starts_with('h') {
            (1, 3_600 * 1_000_000_000)
        } else {
            return Err(format!("invalid duration {value}"));
        };
        cursor += remaining
            .chars()
            .take(unit_len)
            .map(char::len_utf8)
            .sum::<usize>();
        let whole = integer
            .checked_mul(multiplier)
            .ok_or_else(|| format!("invalid duration {value}"))?;
        let partial = fraction
            .checked_mul(multiplier)
            .ok_or_else(|| format!("invalid duration {value}"))?
            / scale;
        nanos = nanos
            .checked_add(whole)
            .and_then(|sum| sum.checked_add(partial))
            .ok_or_else(|| format!("invalid duration {value}"))?;
        if nanos > i64::MAX as u128 {
            return Err(format!("invalid duration {value}"));
        }
    }
    if negative && nanos != 0 {
        return Err(format!("invalid duration -{value}"));
    }
    Ok(Duration::from_nanos(nanos as u64))
}

/// 返回选项的英文描述文本（写入持久化 description 字段）。
pub fn description(name: &str) -> &'static str {
    match name {
        OPT_MAX_NUM_INDEX => "The maximum number of indexes to recommend for a table.",
        OPT_MAX_INDEX_COLUMNS => "The maximum number of columns in an index.",
        OPT_MAX_NUM_QUERY => "The maximum number of queries to recommend indexes.",
        OPT_TIMEOUT => "The timeout of index advisor.",
        _ => "",
    }
}
/// 返回选项的默认值字符串。
pub fn default_value(name: &str) -> &'static str {
    match name {
        OPT_MAX_NUM_INDEX => "5",
        OPT_MAX_INDEX_COLUMNS => "3",
        OPT_MAX_NUM_QUERY => "1000",
        OPT_TIMEOUT => "30s",
        _ => "",
    }
}
