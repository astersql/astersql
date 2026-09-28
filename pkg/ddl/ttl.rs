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

// 表级 TTL（Time To Live，存活时间）配置的校验与变更辅助。
//
// TTL 用于按时间列自动清理过期行。本模块提供：
// - 移除/变更表上的 TTL 元信息；
// - 校验临时表、被引用外键、时间列类型、清理间隔与作业周期；
// - 从建表选项聚合 `TTL` / `TTL_ENABLE` / `TTL_JOB_INTERVAL`。
//
// 文件前半保留对应 Go 逻辑的迁移上下文；后半提供可编译的 Rust 实现。

/*
// DDL 在移除、变更和校验 TTL 配置时的控制流。

// onTTLInfoRemove 对应 Go 的同名 DDL job 回调：清空 TableInfo.TTLInfo 后提交新版本。
pub fn onTTLInfoRemove(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(&mut jobCtx.metaMut, job, job.SchemaID)
        .map_err(errors::Trace)?;

    // Go 这里把 TTLInfo 置 nil，表示从表元信息中删除 TTL 配置。
    tblInfo.TTLInfo = None;
    ver = updateVersionAndTableInfo(jobCtx, job, &tblInfo, true).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, &tblInfo);
    Ok(ver)
}

// onTTLInfoChange 对应 Go 的 TTL ALTER 回调：合并 TTL、TTL_ENABLE、TTL_JOB_INTERVAL 三类选项。
pub fn onTTLInfoChange(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    // Go 注释说明三者至少有一个非 nil；解析失败会取消 job。
    let args = match model::GetAlterTTLInfoArgs(job) {
        Ok(args) => args,
        Err(err) => {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };
    let mut ttlInfo = args.TTLInfo;
    let ttlInfoEnable = args.TTLEnable;
    let ttlInfoJobInterval = args.TTLCronJobSchedule;

    let mut tblInfo = GetTableInfoAndCancelFaultJob(&mut jobCtx.metaMut, job, job.SchemaID)
        .map_err(errors::Trace)?;

    if let Some(ref mut info) = ttlInfo {
        // TTL_ENABLE 未显式设置时沿用原表配置，保持 Go 的兼容行为。
        if ttlInfoEnable.is_none() {
            if let Some(old) = tblInfo.TTLInfo.as_ref() {
                info.Enable = old.Enable;
            }
        }
        // TTL_JOB_INTERVAL 未显式设置时同样继承原值。
        if ttlInfoJobInterval.is_none() {
            if let Some(old) = tblInfo.TTLInfo.as_ref() {
                info.JobInterval = old.JobInterval.clone();
            }
        }
        tblInfo.TTLInfo = ttlInfo;
    }
    if let Some(enable) = ttlInfoEnable {
        if tblInfo.TTLInfo.is_none() {
            return Err(errors::Trace(
                dbterror::ErrSetTTLOptionForNonTTLTable.FastGenByArgs("TTL_ENABLE"),
            ));
        }
        tblInfo.TTLInfo.as_mut().unwrap().Enable = enable;
    }
    if let Some(jobInterval) = ttlInfoJobInterval {
        if tblInfo.TTLInfo.is_none() {
            return Err(errors::Trace(
                dbterror::ErrSetTTLOptionForNonTTLTable.FastGenByArgs("TTL_JOB_INTERVAL"),
            ));
        }
        tblInfo.TTLInfo.as_mut().unwrap().JobInterval = jobInterval;
    }

    ver = updateVersionAndTableInfo(jobCtx, job, &tblInfo, true).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, &tblInfo);
    Ok(ver)
}

// checkTTLInfoValid checks the TTL settings for a table.
// The argument `isForForeignKeyCheck` is used to check the table should not be referenced by foreign key.
// If `isForForeignKeyCheck` is `nil`, it will skip the foreign key check.
// checkTTLInfoValid 对应 Go 的 TTL 配置总校验：临时表、表达式、主键、外键引用和列类型依次检查。
pub fn checkTTLInfoValid(
    schema: ast::CIStr,
    tblInfo: &model::TableInfo,
    foreignKeyCheckIs: Option<infoschemactx::MetaOnlyInfoSchema>,
) -> Result<(), errors::Error> {
    if tblInfo.TempTableType != model::TempTableNone {
        return Err(dbterror::ErrTempTableNotAllowedWithTTL);
    }

    checkTTLIntervalExpr(tblInfo.TTLInfo.as_ref())?;
    checkPrimaryKeyForTTLTable(tblInfo)?;

    if let Some(is) = foreignKeyCheckIs {
        // 即使外键检查开关未启用，Go 仍做引用检查以避免 TTL 表被外键引用。
        let referredFK = checkTableHasForeignKeyReferred(&is, &schema.L, &tblInfo.Name.L, None, true);
        if referredFK.is_some() {
            return Err(dbterror::ErrUnsupportedTTLReferencedByFK);
        }
    }

    checkTTLInfoColumnType(tblInfo)
}

pub fn checkTTLIntervalExpr(ttlInfo: Option<&model::TTLInfo>) -> Result<(), errors::Error> {
    let ttlInfo = ttlInfo.unwrap();
    cache::EvalExpireTime(
        time::Now(),
        &ttlInfo.IntervalExprStr,
        ast::TimeUnitType(ttlInfo.IntervalTimeUnit),
    )
    .map(|_| ())
    .map_err(errors::Trace)
}

// checkTTLInfoColumnType 对应 Go 的 TTL 列存在性和时间类型校验。
pub fn checkTTLInfoColumnType(tblInfo: &model::TableInfo) -> Result<(), errors::Error> {
    let ttlInfo = tblInfo.TTLInfo.as_ref().unwrap();
    let colInfo = findColumnByName(&ttlInfo.ColumnName.L, tblInfo);
    if colInfo.is_none() {
        return Err(dbterror::ErrBadField.GenWithStackByArgs(&ttlInfo.ColumnName.O, "TTL config"));
    }
    if !types::IsTypeTime(colInfo.unwrap().FieldType.GetType()) {
        return Err(dbterror::ErrUnsupportedColumnInTTLConfig.GenWithStackByArgs(&ttlInfo.ColumnName.O));
    }

    Ok(())
}

// checkDropColumnWithTTLConfig 对应 Go 的 drop column 防护：TTL 使用的列不能被删除。
pub fn checkDropColumnWithTTLConfig(tblInfo: &model::TableInfo, colName: &str) -> Result<(), errors::Error> {
    if let Some(ttlInfo) = tblInfo.TTLInfo.as_ref() {
        if ttlInfo.ColumnName.L == colName {
            return Err(dbterror::ErrTTLColumnCannotDrop.GenWithStackByArgs(colName));
        }
    }

    Ok(())
}

// We should forbid creating a TTL table with clustered primary key that contains a column with type float/double.
// This is because currently we are using SQL to delete expired rows and when the primary key contains float/double column,
// it is hard to use condition `WHERE PK in (...)` to delete specified rows because some precision will be lost when comparing.
// checkPrimaryKeyForTTLTable 保留 Go 对 common handle 主键中 float/double 的禁用逻辑。
pub fn checkPrimaryKeyForTTLTable(tblInfo: &model::TableInfo) -> Result<(), errors::Error> {
    if !tblInfo.IsCommonHandle {
        // Go 只检查 clustered primary key；非 common handle 直接跳过。
        return Ok(());
    }

    let pk = tblInfo.GetPrimaryKey();
    if pk.is_none() {
        return Ok(());
    }

    for colDef in &pk.unwrap().Columns {
        let col = &tblInfo.Columns[colDef.Offset as usize];
        match col.GetType() {
            mysql::TypeFloat | mysql::TypeDouble => {
                return Err(dbterror::ErrUnsupportedPrimaryKeyTypeWithTTL);
            }
            _ => {}
        }
    }

    Ok(())
}

// getTTLInfoInOptions returns the aggregated ttlInfo, the ttlEnable, or an error.
// if TTL, TTL_ENABLE or TTL_JOB_INTERVAL is not set in the config, the corresponding return value will be nil.
// if both of TTL and TTL_ENABLE are set, the `ttlInfo.Enable` will be equal with `ttlEnable`.
// if both of TTL and TTL_JOB_INTERVAL are set, the `ttlInfo.JobInterval` will be equal with `ttlCronJobSchedule`.
// getTTLInfoInOptions 对应 Go 的建表选项聚合逻辑，返回 TTLInfo、TTL_ENABLE、TTL_JOB_INTERVAL 三个可选值。
pub fn getTTLInfoInOptions(
    options: Vec<ast::TableOption>,
) -> Result<(Option<model::TTLInfo>, Option<bool>, Option<String>), errors::Error> {
    let mut ttlInfo: Option<model::TTLInfo> = None;
    let mut ttlEnable: Option<bool> = None;
    let mut ttlCronJobSchedule: Option<String> = None;

    for op in options {
        match op.Tp {
            ast::TableOptionTTL => {
                let mut sb = strings::Builder::new();
                let restoreFlags = format::RestoreStringSingleQuotes | format::RestoreNameBackQuotes;
                let mut restoreCtx = format::NewRestoreCtx(restoreFlags, &mut sb);
                op.Value.Restore(&mut restoreCtx)?;

                let intervalExpr = sb.String();
                ttlInfo = Some(model::TTLInfo {
                    ColumnName: op.ColumnName.Name,
                    IntervalExprStr: intervalExpr,
                    IntervalTimeUnit: op.TimeUnitValue.Unit as i32,
                    Enable: true,
                    JobInterval: model::DefaultTTLJobInterval.to_string(),
                });
            }
            ast::TableOptionTTLEnable => {
                ttlEnable = Some(op.BoolValue);
            }
            ast::TableOptionTTLJobInterval => {
                ttlCronJobSchedule = Some(op.StrValue);
            }
            _ => {}
        }
    }

    if let Some(info) = ttlInfo.as_mut() {
        // Go 允许后出现的 TTL_ENABLE / TTL_JOB_INTERVAL 覆盖 TTL 子句内默认值。
        if let Some(enable) = ttlEnable {
            info.Enable = enable;
        }
        if let Some(schedule) = ttlCronJobSchedule.as_ref() {
            info.JobInterval = schedule.clone();
        }
    }
    Ok((ttlInfo, ttlEnable, ttlCronJobSchedule))
}
*/

use std::collections::BTreeSet;

/// 默认 TTL 清理作业调度间隔（1 小时）。
pub const DEFAULT_TTL_JOB_INTERVAL: &str = "1h";

/// TTL 可用的列类型分类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    /// 日期类型。
    Date,
    /// 日期时间类型。
    DateTime,
    /// 时间戳类型。
    Timestamp,
    /// 单精度浮点类型。
    Float,
    /// 双精度浮点类型。
    Double,
    /// 整数类型（不可作为 TTL 时间列）。
    Integer,
    /// 字符串类型（不可作为 TTL 时间列）。
    String,
    /// 其他未支持类型。
    Other,
}

/// 列定义：名称与类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnDefinition {
    /// 列名。
    pub name: String,
    /// 列类型。
    pub column_type: ColumnType,
}

/// 表上的 TTL 配置。
///
/// `interval_expression` 与 `interval_unit` 描述过期判定间隔；
/// `job_interval` 描述后台清理作业的调度周期。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TtlInfo {
    /// 用作过期判断的时间列名。
    pub column_name: String,
    /// 过期间隔数值（须为正）。
    pub interval_expression: i64,
    /// 过期间隔时间单位。
    pub interval_unit: String,
    /// 是否启用 TTL 清理。
    pub enable: bool,
    /// 清理作业调度间隔，如 `1h`、`25h`。
    pub job_interval: String,
}

/// 参与 TTL 校验的精简表元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TtlTable {
    /// 表列集合。
    pub columns: Vec<ColumnDefinition>,
    /// 当前 TTL 配置；`None` 表示未配置。
    pub ttl: Option<TtlInfo>,
    /// 是否为临时表（不允许配置 TTL）。
    pub temporary: bool,
    /// 是否为缓存表；Go 的本层 TTL 校验不读取该标志。
    pub cached: bool,
    /// 是否使用 clustered 主键（common handle）。
    /// common handle 指非整数自增主键的聚簇索引主键。
    pub common_handle: bool,
    /// 主键列名集合。
    pub primary_key_columns: BTreeSet<String>,
    /// 表自身的外键列；Go 仅检查其它表是否引用当前表，本字段不参与该检查。
    pub foreign_key_columns: BTreeSet<String>,
}

/// 建表/改表选项中与 TTL 相关的单项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TtlOption {
    /// `TTL = (col, expr, unit)` 定义。
    Definition {
        column_name: String,
        interval_expression: i64,
        interval_unit: String,
    },
    /// `TTL_ENABLE` 开关。
    Enable(bool),
    /// `TTL_JOB_INTERVAL` 作业周期。
    JobInterval(String),
}

/// TTL 配置校验与变更错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TtlError {
    /// TTL 列不存在。
    ColumnNotFound(String),
    /// TTL 列类型不是时间类型。
    UnsupportedColumnType,
    /// 过期间隔非法。
    InvalidInterval,
    /// 作业调度间隔非法。
    InvalidJobInterval,
    /// 临时表不允许 TTL。
    TemporaryTable,
    /// 缓存表不允许 TTL。
    CachedTable,
    /// 存在外键约束时不允许 TTL。
    ForeignKey,
    /// clustered 主键场景下不支持的主键配置。
    UnsupportedPrimaryKey,
    /// 待删除列正被 TTL 使用。
    ColumnUsedByTtl,
}

/// 以 Debug 形式格式化错误。
impl std::fmt::Display for TtlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
/// 标准错误 trait 实现。
impl std::error::Error for TtlError {}

/// 移除表上的 TTL 配置。
pub fn remove_ttl_info(table: &mut TtlTable) {
    table.ttl = None;
}

/// 设置或清空 TTL 配置；若新配置存在则先校验。
pub fn change_ttl_info(table: &mut TtlTable, ttl: Option<TtlInfo>) -> Result<(), TtlError> {
    if let Some(info) = ttl.as_ref() {
        validate_ttl_info(table, info)?;
    }
    table.ttl = ttl;
    Ok(())
}

/// 校验 TTL 配置相对表元信息是否合法。
pub fn validate_ttl_info(table: &TtlTable, ttl: &TtlInfo) -> Result<(), TtlError> {
    if table.temporary {
        return Err(TtlError::TemporaryTable);
    }
    // 按列名定位 TTL 时间列，并要求类型为 Date/DateTime/Timestamp。
    let normalized = ttl.column_name.to_ascii_lowercase();
    let column = table
        .columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(&normalized))
        .ok_or_else(|| TtlError::ColumnNotFound(ttl.column_name.clone()))?;
    if !matches!(
        column.column_type,
        ColumnType::Date | ColumnType::DateTime | ColumnType::Timestamp
    ) {
        return Err(TtlError::UnsupportedColumnType);
    }
    if ttl.interval_expression <= 0 || ttl.interval_unit.trim().is_empty() {
        return Err(TtlError::InvalidInterval);
    }
    validate_job_interval(&ttl.job_interval)?;
    // Go 仅禁止 common handle 中的 float/double 主键列；TTL 列是否属于主键并非条件。
    if table.common_handle
        && table.columns.iter().any(|column| {
            table
                .primary_key_columns
                .iter()
                .any(|name| column.name.eq_ignore_ascii_case(name))
                && matches!(column.column_type, ColumnType::Float | ColumnType::Double)
        })
    {
        return Err(TtlError::UnsupportedPrimaryKey);
    }
    Ok(())
}

/// 校验作业间隔字符串：正整数 + 单位 `s`/`m`/`h`/`d`。
pub fn validate_job_interval(value: &str) -> Result<(), TtlError> {
    let value = value.trim();
    // 拆出数字前缀与单位后缀；单位必须是秒/分/时/天之一。
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .ok_or(TtlError::InvalidJobInterval)?;
    let (number, unit) = value.split_at(split);
    let amount: u64 = number.parse().map_err(|_| TtlError::InvalidJobInterval)?;
    if amount == 0 || !matches!(unit, "s" | "m" | "h" | "d") {
        return Err(TtlError::InvalidJobInterval);
    }
    Ok(())
}

/// 删除列前检查：TTL 使用的列不可删除。
pub fn check_drop_column_with_ttl(table: &TtlTable, column_name: &str) -> Result<(), TtlError> {
    if table
        .ttl
        .as_ref()
        .is_some_and(|ttl| ttl.column_name.eq_ignore_ascii_case(column_name))
    {
        Err(TtlError::ColumnUsedByTtl)
    } else {
        Ok(())
    }
}

/// 从选项列表聚合 TTL 定义、`TTL_ENABLE` 与 `TTL_JOB_INTERVAL`。
///
/// 后出现的 Enable/JobInterval 会覆盖 TTL 子句内的默认值。
pub fn get_ttl_info_in_options(
    options: &[TtlOption],
) -> Result<(Option<TtlInfo>, Option<bool>, Option<String>), TtlError> {
    let mut info = None;
    let mut enable = None;
    let mut schedule = None;
    for option in options {
        match option {
            TtlOption::Definition {
                column_name,
                interval_expression,
                interval_unit,
            } => {
                info = Some(TtlInfo {
                    column_name: column_name.clone(),
                    interval_expression: *interval_expression,
                    interval_unit: interval_unit.to_ascii_uppercase(),
                    enable: true,
                    job_interval: DEFAULT_TTL_JOB_INTERVAL.to_string(),
                });
            }
            TtlOption::Enable(value) => enable = Some(*value),
            TtlOption::JobInterval(value) => {
                schedule = Some(value.clone());
            }
        }
    }
    // 若同时给出 TTL 定义与开关/周期选项，把后者写回 TTLInfo。
    if let Some(ttl) = info.as_mut() {
        if let Some(value) = enable {
            ttl.enable = value;
        }
        if let Some(value) = schedule.as_ref() {
            ttl.job_interval = value.clone();
        }
    }
    Ok((info, enable, schedule))
}
