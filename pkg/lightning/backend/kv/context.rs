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
// Copyright 2026 AsterSQL.

// Lightning 导入路径的轻量表达式/表变更上下文。
//
// 从系统变量（sysVars）解析编码与求值所需的会话参数，对应 Go 侧 litExprContext /
// litTableMutateContext：供 SQL→KV 编码时计算生成列、行格式与 mutation 校验开关等。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use encode::Datum;

const MODE_STRICT_TRANS_TABLES: u64 = 0x0020_0000;
const MODE_STRICT_ALL_TABLES: u64 = 0x0040_0000;
const MODE_NO_ZERO_IN_DATE: u64 = 0x0080_0000;
const MODE_NO_ZERO_DATE: u64 = 0x0100_0000;
const MODE_ERROR_FOR_DIVISION_BY_ZERO: u64 = 0x0400_0000;
const MODE_ALLOW_INVALID_DATES: u64 = 0x1_0000_0000;

static ENABLE_ROW_LEVEL_CHECKSUM: AtomicBool = AtomicBool::new(false);

/// Go `vardef.EnableRowLevelChecksum` 的进程级开关。
pub fn SetGlobalRowLevelChecksumEnabled(enabled: bool) {
    ENABLE_ROW_LEVEL_CHECKSUM.store(enabled, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorLevel {
    Error,
    Warn,
    Ignore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportTypeFlags {
    pub TruncateAsWarning: bool,
    pub IgnoreZeroInDateErr: bool,
    pub IgnoreInvalidDateErr: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatementErrorLevels {
    pub Truncate: ErrorLevel,
    pub BadNull: ErrorLevel,
    pub NoDefault: ErrorLevel,
    pub DividedByZero: ErrorLevel,
}

/// 表达式求值上下文：SQL Mode、当前时间戳、包大小、时区及用户变量等。
#[derive(Clone, Debug)]
pub struct litExprContext {
    pub SQLMode: u64,
    pub TypeFlags: ImportTypeFlags,
    pub ErrLevels: StatementErrorLevels,
    pub CurrentTimestamp: i64,
    /// 单包最大允许字节数（max_allowed_packet）。
    pub MaxAllowedPacket: u64,
    /// 除法结果精度增量（div_precision_increment）。
    pub DivPrecisionIncrement: i32,
    pub DefaultWeekFormat: String,
    pub BlockEncryptionMode: String,
    pub GroupConcatMaxLen: u64,
    pub TimeZone: String,
    /// 用户变量表；键统一为小写。
    pub UserVars: HashMap<String, Datum>,
}

fn normalizedSystemVars(sysVars: &HashMap<String, String>) -> HashMap<String, String> {
    sysVars
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

/// 根据 SQL Mode、系统变量与时间戳构造表达式上下文。
pub fn newLitExprContext(
    sqlMode: u64,
    sysVars: &HashMap<String, String>,
    timestamp: i64,
) -> Result<litExprContext, String> {
    let sysVars = normalizedSystemVars(sysVars);
    let strict = sqlMode & (MODE_STRICT_TRANS_TABLES | MODE_STRICT_ALL_TABLES) != 0;
    let type_flags = ImportTypeFlags {
        TruncateAsWarning: !strict,
        IgnoreZeroInDateErr: !strict
            || sqlMode & MODE_NO_ZERO_DATE == 0
            || sqlMode & MODE_NO_ZERO_IN_DATE == 0,
        IgnoreInvalidDateErr: sqlMode & MODE_ALLOW_INVALID_DATES != 0,
    };
    let err_levels = StatementErrorLevels {
        Truncate: if strict {
            ErrorLevel::Error
        } else {
            ErrorLevel::Warn
        },
        BadNull: if strict {
            ErrorLevel::Error
        } else {
            ErrorLevel::Warn
        },
        NoDefault: if strict {
            ErrorLevel::Error
        } else {
            ErrorLevel::Warn
        },
        DividedByZero: if sqlMode & MODE_ERROR_FOR_DIVISION_BY_ZERO == 0 {
            ErrorLevel::Ignore
        } else if strict {
            ErrorLevel::Error
        } else {
            ErrorLevel::Warn
        },
    };
    // 解析数值型系统变量，缺失时使用默认值。
    let parse = |name: &str, default: u64| -> Result<u64, String> {
        sysVars.get(name).map_or(Ok(default), |value| {
            value.parse().map_err(|_| format!("invalid {name}"))
        })
    };
    let time_zone = sysVars
        .get("time_zone")
        .cloned()
        .unwrap_or_else(|| "SYSTEM".into());
    if !time_zone.eq_ignore_ascii_case("SYSTEM") {
        time_zone
            .parse::<tablecodec::time::Location>()
            .map_err(|_| format!("invalid time_zone: {time_zone}"))?;
    }
    let max_allowed_packet = parse("max_allowed_packet", 64 * 1024 * 1024)?;
    if !(1024..=1_073_741_824).contains(&max_allowed_packet) {
        return Err("invalid max_allowed_packet".into());
    }
    let div_precision_increment = parse("div_precision_increment", 4)?;
    if div_precision_increment > 30 {
        return Err("invalid div_precision_increment".into());
    }
    let default_week_format = parse("default_week_format", 0)?;
    if default_week_format > 7 {
        return Err("invalid default_week_format".into());
    }
    let group_concat_max_len = parse("group_concat_max_len", 1024)?;
    if group_concat_max_len < 4 {
        return Err("invalid group_concat_max_len".into());
    }
    let block_encryption_mode = sysVars
        .get("block_encryption_mode")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "aes-128-ecb".into());
    const ENCRYPTION_MODES: &[&str] = &[
        "aes-128-ecb",
        "aes-192-ecb",
        "aes-256-ecb",
        "aes-128-cbc",
        "aes-192-cbc",
        "aes-256-cbc",
        "aes-128-ofb",
        "aes-192-ofb",
        "aes-256-ofb",
        "aes-128-cfb",
        "aes-192-cfb",
        "aes-256-cfb",
    ];
    if !ENCRYPTION_MODES
        .iter()
        .any(|mode| mode.eq_ignore_ascii_case(&block_encryption_mode))
    {
        return Err("invalid block_encryption_mode".into());
    }
    Ok(litExprContext {
        SQLMode: sqlMode,
        TypeFlags: type_flags,
        ErrLevels: err_levels,
        // timestamp<=0 时取当前 Unix 秒，与 Go 侧“未显式指定则用 now”一致。
        CurrentTimestamp: if timestamp > 0 {
            timestamp
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_secs() as i64
        },
        MaxAllowedPacket: max_allowed_packet,
        DivPrecisionIncrement: div_precision_increment as i32,
        DefaultWeekFormat: default_week_format.to_string(),
        BlockEncryptionMode: block_encryption_mode,
        GroupConcatMaxLen: group_concat_max_len,
        TimeZone: time_zone,
        UserVars: HashMap::new(),
    })
}

impl litExprContext {
    /// 设置用户变量（名称按小写规范化）。
    pub fn setUserVarVal(&mut self, name: &str, dt: Datum) {
        self.UserVars.insert(name.to_ascii_lowercase(), dt);
    }
    /// 删除用户变量（名称大小写不敏感）。
    pub fn unsetUserVar(&mut self, varName: &str) {
        self.UserVars.remove(&varName.to_ascii_lowercase());
    }
}

/// 表变更（mutate/DML）上下文：行编码版本、行级校验和、mutation checker 与断言级别。
///
/// 行级校验和（row-level checksum）在行值中嵌入校验，便于检测导入/复制损坏；
/// mutation checker 在写入前校验变更是否符合断言级别。
#[derive(Clone, Debug, Default)]
pub struct MutateBuffers {
    pub WriteStmtBuffer: Vec<u8>,
}

/// Lightning 行号分片生成器；保留独立状态与会话配置的 shard step。
#[derive(Clone, Debug)]
pub struct RowIDShardGenerator {
    state: Arc<AtomicU64>,
    shard_step: i64,
}

impl RowIDShardGenerator {
    pub fn GetShardStep(&self) -> i64 {
        self.shard_step
    }

    pub fn Next(&self) -> u64 {
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            let mut next = current;
            next ^= next << 13;
            next ^= next >> 7;
            next ^= next << 17;
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return next,
                Err(actual) => current = actual,
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct litTableMutateContext {
    pub exprCtx: litExprContext,
    pub RowEncodingEnabled: bool,
    pub RowLevelChecksumEnabled: bool,
    pub MutationChecker: bool,
    pub AssertionLevel: String,
    pub MutateBuffers: MutateBuffers,
    pub RowIDShardGenerator: RowIDShardGenerator,
}

impl litTableMutateContext {
    /// Lightning 不支持临时表，因此不提供替代分配器。
    pub fn AlternativeAllocators(&self) -> (Option<crate::Allocators>, bool) {
        (None, false)
    }
    pub fn GetExprCtx(&self) -> &litExprContext {
        &self.exprCtx
    }
    pub fn ConnectionID(&self) -> u64 {
        0
    }
    pub fn InRestrictedSQL(&self) -> bool {
        false
    }
    pub fn TxnAssertionLevel(&self) -> &str {
        &self.AssertionLevel
    }
    pub fn EnableMutationChecker(&self) -> bool {
        self.MutationChecker
    }
    /// 返回 (新行编码是否启用, 行级校验和是否启用)。
    pub fn GetRowEncodingConfig(&self) -> (bool, bool) {
        (self.RowEncodingEnabled, self.RowLevelChecksumEnabled)
    }
    pub fn GetMutateBuffers(&self) -> &MutateBuffers {
        &self.MutateBuffers
    }
    pub fn GetRowIDShardGenerator(&self) -> &RowIDShardGenerator {
        &self.RowIDShardGenerator
    }
    pub fn GetReservedRowIDAlloc(&self) -> (i64, bool) {
        (0, true)
    }
    pub fn GetStatisticsSupport(&self) -> bool {
        true
    }
    pub fn UpdatePhysicalTableDelta(&self, _: i64, _: i64, _: i64) {}
    pub fn GetCachedTableSupport(&self) -> bool {
        false
    }
    pub fn GetTemporaryTableSupport(&self) -> bool {
        false
    }
    pub fn GetExchangePartitionDMLSupport(&self) -> bool {
        false
    }
}

/// 基于表达式上下文与系统变量构造表变更上下文。
pub fn newLitTableMutateContext(
    exprCtx: &litExprContext,
    sysVars: &HashMap<String, String>,
) -> Result<litTableMutateContext, String> {
    let sysVars = normalizedSystemVars(sysVars);
    // 将 "1"/"ON" 与 "0"/"OFF" 解析为布尔；其它值报错。
    let parse_bool = |name: &str, default: bool| match sysVars.get(name) {
        None => Ok(default),
        Some(value) if value == "1" || value.eq_ignore_ascii_case("ON") => Ok(true),
        Some(value) if value == "0" || value.eq_ignore_ascii_case("OFF") => Ok(false),
        Some(_) => Err(format!("invalid {name}")),
    };
    let shard_step = sysVars
        .get("tidb_shard_allocate_step")
        .map_or(Ok(i64::MAX), |value| {
            value
                .parse::<i64>()
                .map_err(|_| "invalid tidb_shard_allocate_step".to_string())
        })?;
    if shard_step < 1 {
        return Err("invalid tidb_shard_allocate_step".into());
    }
    let row_encoding_enabled = match sysVars.get("tidb_row_format_version").map(String::as_str) {
        None | Some("1") => false,
        Some("2") => true,
        Some(_) => return Err("invalid tidb_row_format_version".into()),
    };
    let assertion_level = sysVars
        .get("tidb_txn_assertion_level")
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_else(|| "OFF".into());
    if !matches!(assertion_level.as_str(), "OFF" | "FAST" | "STRICT") {
        return Err("invalid tidb_txn_assertion_level".into());
    }
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos() as u64
        | 1;
    Ok(litTableMutateContext {
        exprCtx: exprCtx.clone(),
        // 与 TiDB SessionVars 默认值一致：仅显式指定版本 2 时启用新行编码。
        RowEncodingEnabled: row_encoding_enabled,
        RowLevelChecksumEnabled: row_encoding_enabled
            && ENABLE_ROW_LEVEL_CHECKSUM.load(Ordering::Relaxed),
        MutationChecker: parse_bool("tidb_enable_mutation_checker", false)?,
        AssertionLevel: assertion_level,
        MutateBuffers: MutateBuffers::default(),
        RowIDShardGenerator: RowIDShardGenerator {
            state: Arc::new(AtomicU64::new(seed)),
            shard_step,
        },
    })
}
