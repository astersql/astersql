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

//! INSERT 语句生成与数据库连接辅助。
//!
//! 该模块对应 Go 版 `cmd/importer/db.go`，职责是把解析后的表定义转成
//! 可直接执行的 `insert into ... values ...` SQL 文本，并负责按列类型生成
//! 随机值或递增值。这里不做批量执行调度，只提供单行/多行 SQL 拼装和连接
//! 生命周期辅助，便于 `main.rs` 按 worker 数量复用同一套生成逻辑。
//!
//! 注释重点保留几类约束：
//! - 列上声明的 `min`/`max`/`set`/`hist` 会覆盖默认范围。
//! - 唯一索引列必须走递增路径，避免随机生成时打破唯一性假设。
//! - decimal/floating/time 类别保持与 Go 相同的字符串化策略，保证导入器
//!   观察到的 SQL 字面量形状一致。
//! - 本文件只生成值与打开/关闭连接，不负责 schema 创建或事务控制。
//!
//! INSERT generation and DB helpers matching `cmd/importer/db.go`.

use crate::config::DBConfig;
use crate::data::{randInt, randInt64, randString};
use crate::parser::{column, table};
use crate::rand::{randDate, randTime, randTimestamp, randYear};
use crate::stubs::{
    self, DB, Error, Result, TypeBlob, TypeDate, TypeDatetime, TypeDouble, TypeDuration, TypeFloat,
    TypeLong, TypeLongBlob, TypeLonglong, TypeMediumBlob, TypeNewDecimal, TypeShort, TypeString,
    TypeTimestamp, TypeTiny, TypeTinyBlob, TypeVarchar, TypeYear,
};

/// 解析列级整数上下界。
///
/// Go 版本允许列定义用字符串覆盖调用方传入的默认范围；这里只在 `min`
/// 非空时才触发解析，与原始控制流保持一致，避免把“未配置范围”误判为
/// `0..0`。解析失败仍走 fatal，说明这是配置错误而非运行期随机波动。
pub fn intRangeValue(column: &column, mut minv: i64, mut maxv: i64) -> (i64, i64) {
    if !column.min.is_empty() {
        minv = match column.min.parse::<i64>() {
            Ok(v) => v,
            Err(err) => stubs::fatal(err.to_string()),
        };
        if !column.max.is_empty() {
            maxv = match column.max.parse::<i64>() {
                Ok(v) => v,
                Err(err) => stubs::fatal(err.to_string()),
            };
        }
    }
    (minv, maxv)
}

/// 生成字符串列的一个值。
///
/// 优先级与 Go 侧一致：
/// 1. 直方图存在时，优先复用分布并确保平均长度已初始化。
/// 2. `set` 非空时，从离散候选集中随机挑选。
/// 3. 两者都没有时，再退化到通用随机字符串。
///
/// 这样可以让导入数据在“拟合已有分布”和“随机填充”之间保持稳定顺序。
pub fn randStringValue(column: &column, n: i32) -> String {
    if let Some(hist) = column.hist.as_ref() {
        hist.ensure_avg_len(n);
        return hist.randString();
    }
    if !column.set.is_empty() {
        let idx = randInt(0, column.set.len() as i32 - 1) as usize;
        return column.set[idx].clone();
    }
    randString(randInt(1, n))
}

/// 生成一个整数值，优先尊重直方图和离散集合。
///
/// `set` 分支中的解析失败不会中止流程，而是记录告警并返回 `0`；
/// 这保持了 Go 版“尽量继续造数”的行为，避免单个坏样本让整个导入任务退出。
pub fn randInt64Value(column: &column, minv: i64, maxv: i64) -> i64 {
    if let Some(hist) = column.hist.as_ref() {
        return hist.randInt();
    }
    if !column.set.is_empty() {
        let idx = randInt(0, column.set.len() as i32 - 1) as usize;
        return match column.set[idx].parse::<i64>() {
            Ok(data) => data,
            Err(err) => {
                stubs::log_warn(format!("rand int64 failed: {err}"));
                0
            }
        };
    }
    let (minv, maxv) = intRangeValue(column, minv, maxv);
    randInt64(minv, maxv)
}

/// 为递增列取下一个整数值。
///
/// 调用前先把列级 `min/max` 覆盖到默认边界，再把范围写回 `column.data`
/// 的内部状态。这样不同整数类型虽然共用同一个递增器，但每列首次进入时
/// 仍会按自己的合法区间初始化。
pub fn nextInt64Value(column: &column, minv: i64, maxv: i64) -> i64 {
    let (minv, maxv) = intRangeValue(column, minv, maxv);
    column.data.setInitInt64Value(minv, maxv);
    column.data.nextInt64()
}

/// 把整数按 decimal 精度拆成十进制字符串。
///
/// 这里延续 Go 的文本拼装方式，而不是依赖数据库端再做格式化：
/// 先补足前导零，再按 `decimal` 从末尾切出小数部分，可保证 `0012` 在
/// `decimal=3` 时输出 `0.012`，与原工具生成的 SQL 字面量完全同形。
pub fn intToDecimalString(intValue: i64, decimal: i32) -> String {
    let mut data = intValue.to_string();
    if data.len() < decimal as usize {
        data = format!("{}{}", "0".repeat(decimal as usize - data.len()), data);
    }
    let split_at = data.len().saturating_sub(decimal as usize);
    let dec = data[split_at..].to_string();
    data = data[..split_at].to_string();
    if data.is_empty() {
        data = "0".to_string();
    }
    if !dec.is_empty() {
        data = format!("{data}.{dec}");
    }
    data
}

/// 批量生成多行 INSERT 语句。
///
/// 该函数只是重复调用 `genRowData` 并保留首个错误，方便上层按批次发给
/// worker；它不做去重或批量拼接，因而可以最大程度复用单行生成逻辑。
pub fn genRowDatas(table: &table, count: isize) -> Result<Vec<String>> {
    let mut datas = Vec::with_capacity(count as usize);
    for _ in 0..count {
        datas.push(genRowData(table)?);
    }
    Ok(datas)
}

/// 生成单条完整的 INSERT 语句。
///
/// 拼装顺序严格跟随 `table.columns`，因为列值和 `table.columnList` 必须一一
/// 对应。循环里始终追加逗号，最后统一 `pop()`，这是 Go 版先追加后裁尾的
/// 直接映射，可避免在首列/末列上引入额外分支。
pub fn genRowData(table: &table) -> Result<String> {
    let mut values = String::new();
    for i in 0..table.columns.len() {
        let data = genColumnData(table, i)?;
        values.push_str(&data);
        values.push(',');
    }
    values.pop();
    Ok(format!(
        "insert into {} ({}) values ({});",
        table.name, table.columnList, values
    ))
}

// #nosec G404
/// 为指定列生成 SQL 字面量。
///
/// 这是导入器的核心分发点：先决定该列本次是否走递增模式，再根据 MySQL
/// 类型选择相应的生成策略。返回值已经带好字符串型/时间型所需引号，调用方
/// 只负责把各列拼成一行 values 列表。
pub fn genColumnData(table: &table, col_idx: usize) -> Result<String> {
    let column = &table.columns[col_idx];
    let tp = &column.tp;
    let mut incremental = column.incremental;
    if incremental {
        // 递增列并不是“每次都递增”，而是按概率切换；这让样本既能保留
        // 序列特征，又能穿插随机值，行为与 Go 的 `rand.Int31n` 分支一致。
        incremental = (stubs::rand_int31n(100) as u32) + 1 <= column.data.probability();
        // If incremental, there is only one worker, so it is safe to directly access datum.
        // 当本轮没有命中递增路径时，需要扣减 remains，保证后续还能在总量上
        // 维持列配置的概率预算，而不是无限次重试递增。
        if !incremental && column.data.remains() > 0 {
            column.data.dec_remains();
        }
    }
    // 唯一索引列强制改走递增模式，优先满足唯一性约束，而不是尊重原始概率。
    if table.uniqIndices.contains_key(&column.name) {
        incremental = true;
    }
    let isUnsigned = stubs::HasUnsignedFlag(tp.GetFlag());

    match tp.GetType() {
        TypeTiny => {
            // tinyint 直接映射到 8 位上下界；无符号与有符号分开取值，避免
            // 在公共随机函数里再猜测列 flag。
            let data = if incremental {
                if isUnsigned {
                    nextInt64Value(column, 0, u8::MAX as i64)
                } else {
                    nextInt64Value(column, i8::MIN as i64, i8::MAX as i64)
                }
            } else if isUnsigned {
                randInt64Value(column, 0, u8::MAX as i64)
            } else {
                randInt64Value(column, i8::MIN as i64, i8::MAX as i64)
            };
            Ok(data.to_string())
        }
        TypeShort => {
            // smallint 路径与 Go 保持同样的上下界常量选择，仅复用公共生成器。
            let data = if incremental {
                if isUnsigned {
                    nextInt64Value(column, 0, u16::MAX as i64)
                } else {
                    nextInt64Value(column, i16::MIN as i64, i16::MAX as i64)
                }
            } else if isUnsigned {
                randInt64Value(column, 0, u16::MAX as i64)
            } else {
                randInt64Value(column, i16::MIN as i64, i16::MAX as i64)
            };
            Ok(data.to_string())
        }
        TypeLong => {
            // int 使用 32 位边界，即便 Rust 内部统一走 i64，也不扩大 SQL 类型语义。
            let data = if incremental {
                if isUnsigned {
                    nextInt64Value(column, 0, u32::MAX as i64)
                } else {
                    nextInt64Value(column, i32::MIN as i64, i32::MAX as i64)
                }
            } else if isUnsigned {
                randInt64Value(column, 0, u32::MAX as i64)
            } else {
                randInt64Value(column, i32::MIN as i64, i32::MAX as i64)
            };
            Ok(data.to_string())
        }
        TypeLonglong => {
            // bigint unsigned 仍限制在 `i64::MAX - 1`，这是对 Go 行为的照搬：
            // 原实现用有符号容器承载随机值，避免溢出到不可表示区间。
            let data = if incremental {
                if isUnsigned {
                    nextInt64Value(column, 0, i64::MAX - 1)
                } else {
                    nextInt64Value(column, i32::MIN as i64, i32::MAX as i64)
                }
            } else if isUnsigned {
                randInt64Value(column, 0, i64::MAX - 1)
            } else {
                randInt64Value(column, i32::MIN as i64, i32::MAX as i64)
            };
            Ok(data.to_string())
        }
        TypeVarchar | TypeString | TypeTinyBlob | TypeBlob | TypeMediumBlob | TypeLongBlob => {
            // 字符串/Blob 类统一按带单引号的文本字面量输出；这里不转义内容，
            // 因为生成器假设 `randString` / hist / set 已提供可直接写入 SQL 的值。
            let data = if incremental {
                column.data.nextString(tp.GetFlen())
            } else {
                randStringValue(column, tp.GetFlen())
            };
            Ok(format!("'{data}'"))
        }
        TypeFloat | TypeDouble => {
            // 浮点列底层仍从整数域取样，再转成 f64；这样既保留 Go 的范围选取，
            // 也避免引入额外的小数分布规则。
            let data = if incremental {
                if isUnsigned {
                    nextInt64Value(column, 0, i64::MAX - 1) as f64
                } else {
                    nextInt64Value(column, i32::MIN as i64, i32::MAX as i64) as f64
                }
            } else if isUnsigned {
                randInt64Value(column, 0, i64::MAX - 1) as f64
            } else {
                randInt64Value(column, i32::MIN as i64, i32::MAX as i64) as f64
            };
            // Go strconv.FormatFloat(data, 'f', -1, 64)
            Ok(format!("{data}"))
        }
        TypeDate => {
            // 日期/时间族都在这里补引号，让调用方不必区分哪些类型需要 SQL 文本包装。
            let data = if incremental {
                column.data.nextDate()
            } else {
                randDate(column)
            };
            Ok(format!("'{data}'"))
        }
        TypeDatetime | TypeTimestamp => {
            // datetime 与 timestamp 共用时间戳生成策略，和 Go 一样不在此处分叉时区语义。
            let data = if incremental {
                column.data.nextTimestamp()
            } else {
                randTimestamp(column)
            };
            Ok(format!("'{data}'"))
        }
        TypeDuration => {
            // duration 输出的是 `'HH:MM:SS'` 形态字符串，保持插入语句可直接执行。
            let data = if incremental {
                column.data.nextTime()
            } else {
                randTime(column)
            };
            Ok(format!("'{data}'"))
        }
        TypeYear => {
            // year 也按字符串字面量输出，避免不同数据库驱动对裸数字年份有差异解析。
            let data = if incremental {
                column.data.nextYear()
            } else {
                randYear(column)
            };
            Ok(format!("'{data}'"))
        }
        TypeNewDecimal => {
            // flen 决定总位数上界，decimal 决定小数位；生成时先在整数域挑值，
            // 再统一交给 `intToDecimalString` 排版，避免不同分支重复处理补零逻辑。
            let mut limit = pow10_i64(tp.GetFlen());
            if limit < 0 {
                limit = i64::MAX;
            }
            // Go math.Pow10 can overflow to Inf -> cast negative; guard preserved.
            // 有符号 decimal 取对称区间，无符号则从 0 开始，这与 Go 的样本范围一致。
            let intVal = if incremental {
                if isUnsigned {
                    nextInt64Value(column, 0, limit - 1)
                } else {
                    nextInt64Value(column, (-limit + 1) / 2, (limit - 1) / 2)
                }
            } else if isUnsigned {
                randInt64Value(column, 0, limit - 1)
            } else {
                randInt64Value(column, (-limit + 1) / 2, (limit - 1) / 2)
            };
            Ok(intToDecimalString(intVal, tp.GetDecimal()))
        }
        _ => Err(Error::new(format!(
            // 未支持类型直接返回错误，由上层决定是否终止当前批次。
            "unsupported column type - {}",
            column.String()
        ))),
    }
}

/// 执行一条 SQL；空字符串被视为 no-op。
///
/// 这样上层在可选 DDL / stats 语句为空时不必额外包分支，延续 Go 版入口的
/// 宽松调用约定。
pub fn execSQL(db: &DB, sql: &str) -> Result<()> {
    if sql.is_empty() {
        return Ok(());
    }
    db.Exec(sql).map_err(stubs::trace)?;
    Ok(())
}

/// 根据配置打开一个数据库连接。
///
/// Rust stub 隐藏了具体驱动细节，但公开接口仍保持与 Go 的 `createDB` 等价：
/// 只消费用户、密码、地址、端口和库名这五类配置。
pub fn createDB(cfg: &DBConfig) -> Result<DB> {
    stubs::open_db(&cfg.User, &cfg.Password, &cfg.Host, cfg.Port, &cfg.Name)
}

/// 关闭单个数据库连接，并把底层错误按统一 trace 包装返回。
pub fn closeDB(db: &DB) -> Result<()> {
    db.Close().map_err(stubs::trace)
}

/// 批量创建连接句柄。
///
/// 返回值长度应与 worker 数一致；一旦中途某个连接创建失败，就立即返回错误，
/// 不在这里做回滚，保持与 Go 版相同的“由调用方负责后续清理”边界。
pub fn createDBs(cfg: &DBConfig, count: isize) -> Result<Vec<DB>> {
    let mut dbs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        dbs.push(createDB(cfg).map_err(stubs::trace)?);
    }
    Ok(dbs)
}

/// 尽力关闭全部连接。
///
/// 这里刻意吞掉单个关闭错误并继续处理剩余连接，因为资源回收阶段的目标是
/// “尽可能多地释放句柄”，而不是为了第一个错误中断整个收尾流程。
pub fn closeDBs(dbs: &[DB]) {
    for db in dbs {
        if let Err(err) = closeDB(db) {
            stubs::log_error(format!("close DB failed: {err}"));
        }
    }
}

/// 计算 decimal `flen` 对应的 10 的幂。
///
/// Go 的 `math.Pow10` 在超大指数下会先溢出到 `Inf`，再在整数转换时表现出
/// 特殊值；Rust 这里直接把 `n >= 19` 钳到 `i64::MAX`，以稳定复现“上界足够大，
/// 后续再由 decimal 分支保护”的语义，而不依赖平台浮点到整数的边界行为。
///
/// Safe Pow10 for decimal flen; matches Go overflow-to-negative guard.
pub fn pow10_i64(n: i32) -> i64 {
    if n < 0 {
        return 0;
    }
    if n >= 19 {
        return i64::MAX; // will be treated via limit < 0 branch if we used Go cast; keep positive MAX
    }
    10i64.pow(n as u32)
}
