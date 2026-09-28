// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// SEQUENCE（序列）DDL 相关逻辑。
//
// SEQUENCE 是一种按步长生成单调数值的数据库对象，常用于分配
// 自增主键或业务流水号。本文件包含：
// - 上方大段块注释中保留的 Go→Rust 机械翻译草稿（创建/ALTER/校验等）；
// - 下方可编译的序列元信息 [`SequenceInfo`]、选项枚举、校验与
//   内存目录 [`SequenceCatalog`]。
//
// 术语：`cache` 表示客户端一次预取的数值个数；`cycle` 表示到达
// 上/下限后是否回绕；`restart` 将当前值重置为指定起点。

/*
// sequence DDL job 的创建、参数默认值/校验、ALTER 与 restart 逻辑。

// onCreateSequence 对应创建 sequence 的 DDL job 入口。
pub fn onCreateSequence(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let schemaID = job.SchemaID;
    let args = match model::GetCreateTableArgs(job) {
        Ok(v) => v,
        Err(err) => {
            // Go 在参数非法时取消 job；保留状态变更和错误返回。
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };

    let tbInfo = args.TableInfo;
    tbInfo.State = model::StateNone;
    if let Err(err) = checkTableNotExists(jobCtx.infoCache, schemaID, tbInfo.Name.L.clone()) {
        if infoschema::ErrDatabaseNotExists.Equal(&err) || infoschema::ErrTableExists.Equal(&err) {
            job.State = model::JobStateCancelled;
        }
        return Err(errors::Trace(err));
    }

    createSequenceWithCheck(jobCtx.metaMut, job, tbInfo).map_err(errors::Trace)?;
    ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, tbInfo);
    Ok(ver)
}

// createSequenceWithCheck 校验并写入 sequence 表元信息。
pub fn createSequenceWithCheck(
    t: &mut meta::Mutator,
    job: &mut model::Job,
    tbInfo: &mut model::TableInfo,
) -> Result<(), errors::Error> {
    match tbInfo.State {
        model::StateNone | model::StatePublic => {
            // Go 状态流转 none -> public，并把 UpdateTS 设置为 mutator 的 StartTS。
            tbInfo.State = model::StatePublic;
            tbInfo.UpdateTS = t.StartTS;
            if let Err(err) = checkTableInfoValid(tbInfo) {
                job.State = model::JobStateCancelled;
                return Err(errors::Trace(err));
            }
            let sequenceBase = if tbInfo.Sequence.Increment >= 0 {
                tbInfo.Sequence.Start - 1
            } else {
                tbInfo.Sequence.Start + 1
            };
            t.CreateSequenceAndSetSeqValue(job.SchemaID, tbInfo, sequenceBase)
        }
        _ => Err(dbterror::ErrInvalidDDLState.GenWithStackByArgs("sequence", tbInfo.State)),
    }
}

// handleSequenceOptions 将 AST 中的 sequence option 覆盖到 SequenceInfo，并补默认 min/max/start。
pub fn handleSequenceOptions(
    seqOptions: Vec<Box<ast::SequenceOption>>,
    sequenceInfo: &mut model::SequenceInfo,
) {
    let mut minSetFlag = false;
    let mut maxSetFlag = false;
    let mut startSetFlag = false;
    for op in seqOptions {
        match op.Tp {
            ast::SequenceOptionIncrementBy => sequenceInfo.Increment = op.IntValue,
            ast::SequenceStartWith => {
                sequenceInfo.Start = op.IntValue;
                startSetFlag = true;
            }
            ast::SequenceMinValue => {
                sequenceInfo.MinValue = op.IntValue;
                minSetFlag = true;
            }
            ast::SequenceMaxValue => {
                sequenceInfo.MaxValue = op.IntValue;
                maxSetFlag = true;
            }
            ast::SequenceCache => {
                sequenceInfo.CacheValue = op.IntValue;
                sequenceInfo.Cache = true;
            }
            ast::SequenceNoCache => {
                sequenceInfo.CacheValue = 0;
                sequenceInfo.Cache = false;
            }
            ast::SequenceCycle => sequenceInfo.Cycle = true,
            ast::SequenceNoCycle => sequenceInfo.Cycle = false,
            _ => {}
        }
    }

    // min/max/start 的默认值依赖 increment 符号；这里保持 Go 的正向/反向两套默认值。
    if !(minSetFlag && maxSetFlag && startSetFlag) {
        if sequenceInfo.Increment >= 0 {
            if !minSetFlag {
                sequenceInfo.MinValue = model::DefaultPositiveSequenceMinValue;
            }
            if !startSetFlag {
                sequenceInfo.Start = std::cmp::max(
                    sequenceInfo.MinValue,
                    model::DefaultPositiveSequenceStartValue,
                );
            }
            if !maxSetFlag {
                sequenceInfo.MaxValue = model::DefaultPositiveSequenceMaxValue;
            }
        } else {
            if !maxSetFlag {
                sequenceInfo.MaxValue = model::DefaultNegativeSequenceMaxValue;
            }
            if !startSetFlag {
                sequenceInfo.Start = std::cmp::min(
                    sequenceInfo.MaxValue,
                    model::DefaultNegativeSequenceStartValue,
                );
            }
            if !minSetFlag {
                sequenceInfo.MinValue = model::DefaultNegativeSequenceMinValue;
            }
        }
    }
}

// validateSequenceOptions 对应 Go 的 sequence 参数合法性检查。
pub fn validateSequenceOptions(seqInfo: &model::SequenceInfo) -> bool {
    if seqInfo.Increment == 0 {
        // Increment 不能为 0。
        return false;
    }
    if seqInfo.Cache && seqInfo.CacheValue <= 0 {
        // Cache 开启时 cache value 必须大于 0。
        return false;
    }
    let maxIncrement = mathutil::Abs(seqInfo.Increment);
    seqInfo.MaxValue >= seqInfo.Start
        && seqInfo.MaxValue > seqInfo.MinValue
        && seqInfo.Start >= seqInfo.MinValue
        && seqInfo.MaxValue != i64::MAX
        && seqInfo.MinValue != i64::MIN
        && seqInfo.CacheValue < (i64::MAX - maxIncrement) / maxIncrement
}

// buildSequenceInfo 从 CREATE SEQUENCE 语句构造 SequenceInfo。
pub fn buildSequenceInfo(
    stmt: &ast::CreateSequenceStmt,
    ident: ast::Ident,
) -> Result<Box<model::SequenceInfo>, errors::Error> {
    let mut sequenceInfo = Box::new(model::SequenceInfo {
        Cache: model::DefaultSequenceCacheBool,
        Cycle: model::DefaultSequenceCycleBool,
        CacheValue: model::DefaultSequenceCacheValue,
        Increment: model::DefaultSequenceIncrementValue,
        ..Default::default()
    });

    // 处理表级 comment/engine 选项；Go 中 engine 固定为 InnoDB，因此跳过。
    for op in &stmt.TblOptions {
        match op.Tp {
            ast::TableOptionComment => sequenceInfo.Comment = op.StrValue.clone(),
            ast::TableOptionEngine => {}
            _ => return Err(dbterror::ErrSequenceUnsupportedTableOption.GenWithStackByArgs(op.StrValue.clone())),
        }
    }
    handleSequenceOptions(stmt.SeqOptions.clone(), &mut sequenceInfo);
    if !validateSequenceOptions(&sequenceInfo) {
        return Err(dbterror::ErrSequenceInvalidData.GenWithStackByArgs(ident.Schema.L, ident.Name.L));
    }
    Ok(sequenceInfo)
}

// alterSequenceOptions 将 ALTER SEQUENCE option 覆盖到旧 SequenceInfo，并返回是否需要 restart 及 restart 值。
pub fn alterSequenceOptions(
    sequenceOptions: Vec<Box<ast::SequenceOption>>,
    ident: ast::Ident,
    oldSequence: &mut model::SequenceInfo,
) -> Result<(bool, i64), errors::Error> {
    let mut restartFlag = false;
    let mut restartWithFlag = false;
    let mut restartValue = 0;

    for op in sequenceOptions {
        match op.Tp {
            ast::SequenceOptionIncrementBy => oldSequence.Increment = op.IntValue,
            ast::SequenceStartWith => oldSequence.Start = op.IntValue,
            ast::SequenceMinValue => oldSequence.MinValue = op.IntValue,
            ast::SequenceMaxValue => oldSequence.MaxValue = op.IntValue,
            ast::SequenceCache => {
                oldSequence.CacheValue = op.IntValue;
                oldSequence.Cache = true;
            }
            ast::SequenceNoCache => {
                oldSequence.CacheValue = 0;
                oldSequence.Cache = false;
            }
            ast::SequenceCycle => oldSequence.Cycle = true,
            ast::SequenceNoCycle => oldSequence.Cycle = false,
            ast::SequenceRestart => restartFlag = true,
            ast::SequenceRestartWith => {
                restartWithFlag = true;
                restartValue = op.IntValue;
            }
            _ => {}
        }
    }
    if !validateSequenceOptions(oldSequence) {
        return Err(dbterror::ErrSequenceInvalidData.GenWithStackByArgs(ident.Schema.L, ident.Name.L));
    }
    if restartWithFlag {
        return Ok((true, restartValue));
    }
    if restartFlag {
        return Ok((true, oldSequence.Start));
    }
    Ok((false, 0))
}

// onAlterSequence 对应 ALTER SEQUENCE job 入口：读取旧 TableInfo、替换 SequenceInfo、必要时重启序列值。
pub fn onAlterSequence(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let schemaID = job.SchemaID;
    let args = match model::GetAlterSequenceArgs(job) {
        Ok(v) => v,
        Err(err) => {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };
    let ident = args.Ident;
    let sequenceOpts = args.SeqOptions;

    let tblInfo = checkTableExistAndCancelNonExistJob(jobCtx.metaMut, job, schemaID).map_err(errors::Trace)?;
    let mut copySequenceInfo = (*tblInfo.Sequence).clone();
    let (restart, restartValue) = match alterSequenceOptions(sequenceOpts, ident, &mut copySequenceInfo) {
        Ok(v) => v,
        Err(err) => {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };

    // Go 使用 reflect.DeepEqual 判断 ALTER 是否实际改变配置；这里保留比较语义。
    let same = reflect::DeepEqual(&*tblInfo.Sequence, &copySequenceInfo);
    if same && !restart {
        job.State = model::JobStateDone;
        return Ok(ver);
    }
    tblInfo.Sequence = Box::new(copySequenceInfo);

    if restart {
        restartSequenceValue(jobCtx.metaMut, schemaID, tblInfo, restartValue).map_err(errors::Trace)?;
    }

    ver = updateVersionAndTableInfo(jobCtx, job, tblInfo, true).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, tblInfo);
    Ok(ver)
}

// restartSequenceValue 与 setval 类似：写入 restart 值前一个可用 base，而不是直接返回 restart 值。
pub fn restartSequenceValue(
    t: &mut meta::Mutator,
    dbID: i64,
    tblInfo: &mut model::TableInfo,
    seqValue: i64,
) -> Result<(), errors::Error> {
    let sequenceBase = if tblInfo.Sequence.Increment >= 0 {
        seqValue - 1
    } else {
        seqValue + 1
    };
    t.RestartSequenceValue(dbID, tblInfo, sequenceBase)
}
*/

/// 序列对象的运行时元信息与当前进度。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceInfo {
    /// 起始值（START WITH）。
    pub start: i64,
    /// 允许的最小值。
    pub min_value: i64,
    /// 允许的最大值。
    pub max_value: i64,
    /// 每次递增/递减的步长（不能为 0）。
    pub increment: i64,
    /// 缓存（预取）大小；`NoCache` 时记为 1。
    pub cache: u64,
    /// 是否在越界后回绕到另一端。
    pub cycle: bool,
    /// 对象注释。
    pub comment: String,
    /// 当前内部基数（下一次 nextval 基于此计算）。
    pub current: i64,
}

/// CREATE / ALTER SEQUENCE 时可指定的选项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SequenceOption {
    /// 设置起始值。
    Start(i64),
    /// 设置最小值。
    MinValue(i64),
    /// 恢复为默认最小值（依赖 increment 符号）。
    NoMinValue,
    /// 设置最大值。
    MaxValue(i64),
    /// 恢复为默认最大值（依赖 increment 符号）。
    NoMaxValue,
    /// 设置步长。
    Increment(i64),
    /// 开启缓存并设置缓存大小。
    Cache(u64),
    /// 关闭缓存。
    NoCache,
    /// 设置是否 cycle。
    Cycle(bool),
    /// 设置注释。
    Comment(String),
    /// 重启序列；`Some(v)` 表示 RESTART WITH v，`None` 表示重启到 START。
    Restart(Option<i64>),
}

/// 序列参数校验或目录操作失败时的错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SequenceError {
    /// 步长非法（为 0 或超过值域跨度）。
    InvalidIncrement,
    /// 最小/最大值边界非法。
    InvalidBounds,
    /// 起始值越出 [min, max]。
    StartOutOfBounds,
    /// 缓存大小非法（为 0）。
    InvalidCache,
    /// 同名序列已存在。
    AlreadyExists,
    /// 目标序列不存在。
    NotFound,
}

impl std::fmt::Display for SequenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for SequenceError {}

/// 按步长符号返回正向/负向序列的默认起止与缓存配置。
pub fn sequence_defaults(increment: i64) -> SequenceInfo {
    if increment >= 0 {
        SequenceInfo {
            start: 1,
            min_value: 1,
            max_value: i64::MAX - 1,
            increment,
            cache: 1000,
            cycle: false,
            comment: String::new(),
            current: 0,
        }
    } else {
        SequenceInfo {
            start: -1,
            min_value: i64::MIN + 1,
            max_value: -1,
            increment,
            cache: 1000,
            cycle: false,
            comment: String::new(),
            current: 0,
        }
    }
}

/// 根据选项列表构建完整的 `SequenceInfo`（含默认值填充与校验）。
pub fn build_sequence_info(options: &[SequenceOption]) -> Result<SequenceInfo, SequenceError> {
    // 取最后一次出现的 Increment；未指定则默认为 1。
    let increment = options
        .iter()
        .rev()
        .find_map(|option| match option {
            SequenceOption::Increment(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(1);
    let mut info = sequence_defaults(increment);
    apply_sequence_options(&mut info, options, false)?;
    info.current = restart_sequence_base(info.start, info.increment);
    validate_sequence_options(&info)?;
    Ok(info)
}

/// 将选项应用到已有 `SequenceInfo`；`altering` 为真时才处理 Restart。
///
/// 成功时返回可选的重启目标值（仅 ALTER 且带 RESTART 时为 `Some`）。
pub fn apply_sequence_options(
    info: &mut SequenceInfo,
    options: &[SequenceOption],
    altering: bool,
) -> Result<Option<i64>, SequenceError> {
    let mut restart = None;
    for option in options {
        match option {
            SequenceOption::Start(value) => info.start = *value,
            SequenceOption::MinValue(value) => info.min_value = *value,
            SequenceOption::NoMinValue => {
                info.min_value = if info.increment >= 0 { 1 } else { i64::MIN + 1 }
            }
            SequenceOption::MaxValue(value) => info.max_value = *value,
            SequenceOption::NoMaxValue => {
                info.max_value = if info.increment >= 0 {
                    i64::MAX - 1
                } else {
                    -1
                }
            }
            SequenceOption::Increment(value) => info.increment = *value,
            SequenceOption::Cache(value) => info.cache = *value,
            SequenceOption::NoCache => info.cache = 1,
            SequenceOption::Cycle(value) => info.cycle = *value,
            SequenceOption::Comment(value) => info.comment = value.clone(),
            SequenceOption::Restart(value) if altering => {
                restart = Some(value.unwrap_or(info.start))
            }
            SequenceOption::Restart(_) => {}
        }
    }
    validate_sequence_options(info)?;
    // 重启时写入「目标值前一个可用基数」，与 setval/restart 语义对齐。
    if let Some(value) = restart {
        info.current = restart_sequence_base(value, info.increment);
    }
    Ok(restart)
}

/// 校验序列参数：步长、边界、起始值与缓存大小。
pub fn validate_sequence_options(info: &SequenceInfo) -> Result<(), SequenceError> {
    if info.increment == 0 {
        return Err(SequenceError::InvalidIncrement);
    }
    // Go rejects the signed integer endpoints themselves.  Reserving one
    // value on each side keeps the cache*increment overflow check valid and
    // matches model.validateSequenceOptions.
    if info.min_value == i64::MIN || info.max_value == i64::MAX || info.min_value >= info.max_value
    {
        return Err(SequenceError::InvalidBounds);
    }
    if info.start < info.min_value || info.start > info.max_value {
        return Err(SequenceError::StartOutOfBounds);
    }
    if info.cache == 0 {
        return Err(SequenceError::InvalidCache);
    }
    // Go also rejects cache sizes whose multiplication by the increment can
    // overflow an int64 while allocating the next cached range.
    let max_increment = (info.increment as i128).abs();
    let max_cache = (i64::MAX as i128 - max_increment) / max_increment;
    if (info.cache as i128) >= max_cache {
        return Err(SequenceError::InvalidCache);
    }
    // 步长绝对值不能超过整个值域跨度。
    let span = info.max_value as i128 - info.min_value as i128;
    if max_increment > span {
        return Err(SequenceError::InvalidIncrement);
    }
    Ok(())
}

/// 计算重启/创建时写入的内部基数：正向为 value-1，负向为 value+1。
pub fn restart_sequence_base(value: i64, increment: i64) -> i64 {
    if increment >= 0 {
        value.wrapping_sub(1)
    } else {
        value.wrapping_add(1)
    }
}

/// 内存中的序列目录：按 (schema_id, 小写名) 索引。
#[derive(Default)]
pub struct SequenceCatalog {
    sequences: std::collections::BTreeMap<(i64, String), SequenceInfo>,
}

impl SequenceCatalog {
    /// 创建序列；`if_not_exists` 为真时重复创建返回 Ok(false) 而非报错。
    pub fn create(
        &mut self,
        schema_id: i64,
        name: &str,
        info: SequenceInfo,
        if_not_exists: bool,
    ) -> Result<bool, SequenceError> {
        let key = (schema_id, name.to_ascii_lowercase());
        if self.sequences.contains_key(&key) {
            return if if_not_exists {
                Ok(false)
            } else {
                Err(SequenceError::AlreadyExists)
            };
        }
        self.sequences.insert(key, info);
        Ok(true)
    }

    /// 对已存在序列应用 ALTER 选项，返回可选的重启目标值。
    pub fn alter(
        &mut self,
        schema_id: i64,
        name: &str,
        options: &[SequenceOption],
    ) -> Result<Option<i64>, SequenceError> {
        let key = (schema_id, name.to_ascii_lowercase());
        let info = self
            .sequences
            .get_mut(&key)
            .ok_or(SequenceError::NotFound)?;
        // Go applies ALTER options to a copy and only replaces the stored
        // metadata after every option has passed validation.
        let mut updated = info.clone();
        let restart = apply_sequence_options(&mut updated, options, true)?;
        *info = updated;
        Ok(restart)
    }

    /// 删除序列；`if_exists` 为真时缺失返回 Ok(false)。
    pub fn drop_sequence(
        &mut self,
        schema_id: i64,
        name: &str,
        if_exists: bool,
    ) -> Result<bool, SequenceError> {
        let key = (schema_id, name.to_ascii_lowercase());
        match self.sequences.remove(&key) {
            Some(_) => Ok(true),
            None if if_exists => Ok(false),
            None => Err(SequenceError::NotFound),
        }
    }
}
