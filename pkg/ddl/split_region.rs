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

/*
// 建表/分区/索引相关 region 预切分、scatter 等待和 split policy 归一化流程。

// GlobalScatterGroupID is used to indicate the global scatter group ID.
pub const GlobalScatterGroupID: i64 = -1;

// splitPartitionTableRegion 对应 Go 的分区表 region 预切分入口。
// 它先创建带 DDL 内部来源的 timeout context，再按 split policy、shard row id、普通表前缀三种路径收集 regionIDs。
pub fn splitPartitionTableRegion(
    ctx: sessionctx::Context,
    store: kv::SplittableStore,
    tbInfo: model::TableInfo,
    parts: Vec<model::PartitionDefinition>,
    scatterScope: &str,
) {
    // Max partition count is 8192, should we sample and just choose some partitions to split?
    let (mut ctxWithTimeout, cancel) =
        context::WithTimeout(context::Background(), ctx.GetSessionVars().GetSplitRegionTimeout());
    // Go 使用 defer cancel；用 guard 保留 context 资源收尾。
    let _cancel = scopeguard::guard((), |_| cancel());
    ctxWithTimeout = kv::WithInternalSourceType(ctxWithTimeout, kv::InternalTxnDDL);

    let mut regionIDs: Vec<u64> = Vec::new();
    if hasSplitPolicies(&tbInfo) {
        regionIDs.extend(applySplitPoliciesForTable(
            ctxWithTimeout.clone(),
            ctx.clone(),
            store.clone(),
            tbInfo.clone(),
            tbInfo.ID,
        ));
        for def in &parts {
            regionIDs.extend(applySplitPoliciesForTable(
                ctxWithTimeout.clone(),
                ctx.clone(),
                store.clone(),
                tbInfo.clone(),
                def.ID,
            ));
        }
    } else if shardingBits(&tbInfo) > 0 && tbInfo.PreSplitRegions > 0 {
        regionIDs = Vec::with_capacity(parts.len() * (tbInfo.Indices.len() + 1));
        let (scatter, tableID) = getScatterConfig(scatterScope, tbInfo.ID);
        // Try to split global index region here.
        // 分区表带 shard row id 时，先处理全局索引，再逐个 physical partition 预切表 region。
        regionIDs.extend(splitIndexRegion(store.clone(), tbInfo.clone(), scatter, tableID));
        for def in &parts {
            regionIDs.extend(preSplitPhysicalTableByShardRowID(
                ctxWithTimeout.clone(),
                store.clone(),
                tbInfo.clone(),
                def.ID,
                scatterScope,
            ));
        }
    } else {
        regionIDs = Vec::with_capacity(parts.len());
        for def in &parts {
            regionIDs.push(SplitRecordRegion(
                ctxWithTimeout.clone(),
                store.clone(),
                def.ID,
                tbInfo.ID,
                scatterScope,
            ));
        }
    }
    if scatterScope != vardef::ScatterOff {
        WaitScatterRegionFinish(ctxWithTimeout, store, regionIDs);
    }
}

// splitTableRegion 对应 Go 的非分区表 region 预切分入口。
// policy 优先级最高，其次是 shard row id 预切，最后退化为表前缀 split。
pub fn splitTableRegion(
    ctx: sessionctx::Context,
    store: kv::SplittableStore,
    tbInfo: model::TableInfo,
    scatterScope: &str,
) {
    let (mut ctxWithTimeout, cancel) =
        context::WithTimeout(context::Background(), ctx.GetSessionVars().GetSplitRegionTimeout());
    let _cancel = scopeguard::guard((), |_| cancel());
    ctxWithTimeout = kv::WithInternalSourceType(ctxWithTimeout, kv::InternalTxnDDL);

    let mut regionIDs: Vec<u64> = if hasSplitPolicies(&tbInfo) {
        applySplitPoliciesForTable(
            ctxWithTimeout.clone(),
            ctx.clone(),
            store.clone(),
            tbInfo.clone(),
            tbInfo.ID,
        )
    } else if shardingBits(&tbInfo) > 0 && tbInfo.PreSplitRegions > 0 {
        preSplitPhysicalTableByShardRowID(
            ctxWithTimeout.clone(),
            store.clone(),
            tbInfo.clone(),
            tbInfo.ID,
            scatterScope,
        )
    } else {
        vec![SplitRecordRegion(
            ctxWithTimeout.clone(),
            store.clone(),
            tbInfo.ID,
            tbInfo.ID,
            scatterScope,
        )]
    };
    if scatterScope != vardef::ScatterOff {
        WaitScatterRegionFinish(ctxWithTimeout, store, regionIDs.drain(..).collect());
    }
}

// `tID` is used to control the scope of scatter. If it is `ScatterTable`, the corresponding tableID is used.
// If it is `ScatterGlobal`, the scatter configured at global level uniformly use -1 as `tID`.
// getScatterConfig 把系统变量 scatter scope 翻译成是否 scatter 以及 PD scatter group id。
pub fn getScatterConfig(scope: &str, tableID: i64) -> (bool, i64) {
    match scope {
        vardef::ScatterTable => (true, tableID),
        vardef::ScatterGlobal => (true, GlobalScatterGroupID),
        _ => (false, tableID),
    }
}

// preSplitPhysicalTableByShardRowID 对应 Go 的 shard row id 预切分。
// 它按 shard bits 和 PreSplitRegions 计算 split key，然后追加索引 region split 结果。
pub fn preSplitPhysicalTableByShardRowID(
    ctx: context::Context,
    store: kv::SplittableStore,
    tbInfo: model::TableInfo,
    physicalID: i64,
    scatterScope: &str,
) -> Vec<u64> {
    // Example:
    // sharding_bits = 4
    // PreSplitRegions = 2
    // then will pre-split 2^2 = 4 regions.
    // in this code:
    // max = 1 << sharding_bits = 16
    // step := int64(1 << (sharding_bits - tblInfo.PreSplitRegions)) = 1 << (4-2) = 4;
    // then split regionID is below:
    // 4 << 59 = 2305843009213693952
    // 8 << 59 = 4611686018427387904
    // 12 << 59 = 6917529027641081856
    // The 4 pre-split regions range is below:
    // 0 ~ 2305843009213693952
    // 2305843009213693952 ~ 4611686018427387904
    // 4611686018427387904 ~ 6917529027641081856
    // 6917529027641081856 ~ 9223372036854775807 ( (1 << 63) - 1 )
    // And the max _tidb_rowid is 9223372036854775807, it won't be negative number.

    // Split table region.
    let ft = if let Some(pkCol) = tbInfo.GetPkColInfo() {
        pkCol.FieldType
    } else {
        types::NewFieldType(mysql::TypeLonglong)
    };
    let shardFmt = autoid::NewShardIDFormat(ft, shardingBits(&tbInfo), tbInfo.AutoRandomRangeBits);
    let step: i64 = 1 << (shardFmt.ShardBits - tbInfo.PreSplitRegions);
    let maxv: i64 = 1 << shardFmt.ShardBits;
    let mut splitTableKeys: Vec<Vec<u8>> = Vec::with_capacity(1 << tbInfo.PreSplitRegions);
    splitTableKeys.push(tablecodec::GenTablePrefix(physicalID));
    let mut p = step;
    while p < maxv {
        let recordID = p << shardFmt.IncrementalBits;
        let recordPrefix = tablecodec::GenTableRecordPrefix(physicalID);
        let key = tablecodec::EncodeRecordKey(recordPrefix, kv::IntHandle(recordID));
        splitTableKeys.push(key);
        p += step;
    }
    let (scatter, tableID) = getScatterConfig(scatterScope, tbInfo.ID);
    let mut regionIDs = match store.SplitRegions(ctx, splitTableKeys, scatter, &tableID) {
        Ok(regionIDs) => regionIDs,
        Err(err) => {
            logutil::DDLLogger().Warn(
                "pre split some table regions failed",
                zap::Stringer("table", tbInfo.Name.clone()),
                zap::Int("successful region count", 0),
                zap::Error(&err),
            );
            Vec::new()
        }
    };
    regionIDs.extend(splitIndexRegion(store, tbInfo, scatter, physicalID));
    regionIDs
}

// SplitRecordRegion is to split region in store by table prefix.
// SplitRecordRegion 用 physicalTableID 生成表前缀，只期望返回一个 region id；异常时交给 TiKV 后续自动切分。
pub fn SplitRecordRegion(
    ctx: context::Context,
    store: kv::SplittableStore,
    physicalTableID: i64,
    tableID: i64,
    scatterScope: &str,
) -> u64 {
    let tableStartKey = tablecodec::GenTablePrefix(physicalTableID);
    let (scatter, tID) = getScatterConfig(scatterScope, tableID);
    let regionIDs = match store.SplitRegions(ctx, vec![tableStartKey], scatter, &tID) {
        Ok(regionIDs) => regionIDs,
        Err(err) => {
            // It will be automatically split by TiKV later.
            logutil::DDLLogger().Warn("split table region failed", zap::Error(&err));
            Vec::new()
        }
    };
    if regionIDs.len() == 1 {
        return regionIDs[0];
    }
    0
}

// splitIndexRegion 对应 Go 的索引 region 预切分。
// 对分区表，它跳过不属于当前 physical table 的 global/local index；普通 index 使用 id+1 作为边界。
pub fn splitIndexRegion(
    store: kv::SplittableStore,
    tblInfo: model::TableInfo,
    scatter: bool,
    physicalTableID: i64,
) -> Vec<u64> {
    let mut splitKeys: Vec<Vec<u8>> = Vec::with_capacity(tblInfo.Indices.len());
    for idx in &tblInfo.Indices {
        if tblInfo.GetPartitionInfo().is_some()
            && ((idx.Global && tblInfo.ID != physicalTableID)
                || (!idx.Global && tblInfo.ID == physicalTableID))
        {
            continue;
        }
        let mut id = idx.ID;
        // For normal index, split regions like
        // [t_tid_, t_tid_i_idx1ID+1),
        // [t_tid_i_idx1ID+1,	t_tid_i_idx2ID+1),
        // ...
        // [t_tid_i_idxMaxID+1, t_tid_r_xxxx)
        // For global index, split regions like
        // [t_tid_i_idx1ID, t_tid_i_idx2ID),
        // [t_tid_i_idx2ID, t_tid_i_idx3ID),
        // ...
        // [t_tid_i_idxMaxID, t_pid1_)
        if !idx.Global {
            id += 1;
        }
        let indexPrefix = tablecodec::EncodeTableIndexPrefix(physicalTableID, id);
        splitKeys.push(indexPrefix);
    }
    match store.SplitRegions(context::Background(), splitKeys, scatter, &physicalTableID) {
        Ok(regionIDs) => regionIDs,
        Err(err) => {
            logutil::DDLLogger().Warn(
                "pre split some table index regions failed",
                zap::Stringer("table", tblInfo.Name),
                zap::Int("successful region count", 0),
                zap::Error(&err),
            );
            Vec::new()
        }
    }
}

// WaitScatterRegionFinish will block until all regions are scattered.
// WaitScatterRegionFinish 逐个等待 scatter 完成；非 PD error 时中断，PD error 继续处理后续 region。
pub fn WaitScatterRegionFinish(
    ctx: context::Context,
    store: kv::SplittableStore,
    regionIDs: Vec<u64>,
) {
    for regionID in regionIDs {
        let err = store.WaitScatterRegionFinish(ctx.clone(), regionID, 0);
        if let Err(err) = err {
            logutil::DDLLogger().Warn(
                "wait scatter region failed",
                zap::Uint64("regionID", regionID),
                zap::Error(&err),
            );
            // We don't break for PDError because it may caused by ScatterRegion request failed.
            if !errors::Cause(&err).is::<tikverr::PDError>() {
                break;
            }
        }
    }
}

// hasSplitPolicies 检查表级或索引级 region split policy 是否存在。
pub fn hasSplitPolicies(tbInfo: &model::TableInfo) -> bool {
    if tbInfo.TableSplitPolicy.is_some() {
        return true;
    }
    for idx in &tbInfo.Indices {
        if idx.RegionSplitPolicy.is_some() {
            return true;
        }
    }
    false
}

// applySplitPoliciesForTable 按 Go 的表级 policy 先行、索引级 policy 后行的顺序生成并提交 split keys。
// Go 使用 goto index 跳过失败的表级 policy；这里用布尔标记保留同样的控制流。
pub fn applySplitPoliciesForTable(
    ctx: context::Context,
    sctx: sessionctx::Context,
    store: kv::SplittableStore,
    tbInfo: model::TableInfo,
    physicalTableID: i64,
) -> Vec<u64> {
    let mut regionIDs: Vec<u64> = Vec::new();

    let sc = sctx.GetSessionVars().StmtCtx;
    let svars = sctx.GetSessionVars();
    let (scatter, tableID) = getScatterConfig(svars.ScatterRegion, tbInfo.ID);

    // apply table policy
    let mut skip_to_index = false;
    if let Some(policy) = tbInfo.TableSplitPolicy.clone() {
        let lower = match parseValuesToDatums(sctx.GetExprCtx(), policy.Lower.clone()) {
            Ok(lower) => lower,
            Err(err) => {
                logutil::DDLLogger().Warn(
                    "failed to parse lower bound for table policy",
                    zap::String("table", tbInfo.Name.O.clone()),
                    zap::Error(&err),
                );
                skip_to_index = true;
                Vec::new()
            }
        };
        let upper = if skip_to_index {
            Vec::new()
        } else {
            match parseValuesToDatums(sctx.GetExprCtx(), policy.Upper.clone()) {
                Ok(upper) => upper,
                Err(err) => {
                    logutil::DDLLogger().Warn(
                        "failed to parse upper bound for table policy",
                        zap::String("table", tbInfo.Name.O.clone()),
                        zap::Error(&err),
                    );
                    skip_to_index = true;
                    Vec::new()
                }
            }
        };

        if !skip_to_index {
            let handleCols = regionsplit::BuildHandleColsForSplit(&tbInfo);
            let keys = match regionsplit::GetSplitTableKeys(
                sc.clone(),
                &tbInfo,
                handleCols,
                physicalTableID,
                lower,
                upper,
                policy.Regions as i32,
                None,
                dbterror::ErrInvalidSplitRegionRanges,
            ) {
                Ok(keys) => keys,
                Err(err) => {
                    logutil::DDLLogger().Warn(
                        "failed to generate split keys for table policy",
                        zap::String("table", tbInfo.Name.O.clone()),
                        zap::Error(&err),
                    );
                    skip_to_index = true;
                    Vec::new()
                }
            };

            if !skip_to_index {
                match store.SplitRegions(ctx.clone(), keys, scatter, &tableID) {
                    Ok(ids) => regionIDs = ids,
                    Err(err) => {
                        logutil::DDLLogger().Warn("split regions failed", zap::Error(&err));
                        skip_to_index = true;
                    }
                }
            }
        }
    }

    // 2. Apply index policies (including PRIMARY)
    // 无论表级 policy 是否失败，Go 的 goto index 都会继续执行这里的索引 policy。
    for idx in &tbInfo.Indices {
        if tbInfo.GetPartitionInfo().is_some()
            && ((idx.Global && tbInfo.ID != physicalTableID)
                || (!idx.Global && tbInfo.ID == physicalTableID))
        {
            continue;
        }

        if idx.RegionSplitPolicy.is_none() {
            continue;
        }

        // skip clustered primary
        if tbInfo.HasClusteredIndex() && idx.Primary {
            continue;
        }

        let policy = idx.RegionSplitPolicy.clone().unwrap();
        let lower = match parseValuesToDatums(sctx.GetExprCtx(), policy.Lower.clone()) {
            Ok(lower) => lower,
            Err(err) => {
                logutil::DDLLogger().Warn(
                    "failed to parse lower bound for index policy",
                    zap::String("table", tbInfo.Name.O.clone()),
                    zap::String("index", idx.Name.O.clone()),
                    zap::Error(&err),
                );
                continue;
            }
        };
        let upper = match parseValuesToDatums(sctx.GetExprCtx(), policy.Upper.clone()) {
            Ok(upper) => upper,
            Err(err) => {
                logutil::DDLLogger().Warn(
                    "failed to parse upper bound for index policy",
                    zap::String("table", tbInfo.Name.O.clone()),
                    zap::String("index", idx.Name.O.clone()),
                    zap::Error(&err),
                );
                continue;
            }
        };

        let keys = match regionsplit::GetSplitIndexKeys(
            sc.clone(),
            &tbInfo,
            idx,
            physicalTableID,
            lower,
            upper,
            policy.Regions as i32,
            None,
            dbterror::ErrInvalidSplitRegionRanges,
        ) {
            Ok(keys) => keys,
            Err(err) => {
                logutil::DDLLogger().Warn(
                    "failed to generate split keys for index policy",
                    zap::String("table", tbInfo.Name.O.clone()),
                    zap::String("index", idx.Name.O.clone()),
                    zap::Error(&err),
                );
                continue;
            }
        };

        match store.SplitRegions(ctx.clone(), keys, scatter, &tableID) {
            Ok(ids) => regionIDs.extend(ids),
            Err(err) => {
                logutil::DDLLogger().Warn("split regions failed", zap::Error(&err));
                continue;
            }
        }
    }

    regionIDs
}

// parseValuesToDatums 把 split policy 中保存的字符串表达式重新解析并求值成 Datum。
pub fn parseValuesToDatums(
    exprCtx: exprctx::ExprContext,
    values: Vec<String>,
) -> Result<Vec<types::Datum>, errors::Error> {
    let mut datums: Vec<types::Datum> = Vec::with_capacity(values.len());
    for val in values {
        let d = expression::ParseSimpleExpr(exprCtx.clone(), &val)?;
        let datum = d.Eval(exprCtx.GetEvalCtx(), chunk::Row::default())?;
        datums.push(datum);
    }
    Ok(datums)
}

// normalizeSplitPolicy 对应 Go 的 SPLIT REGION 选项归一化。
// 它补全 PRIMARY 名称、校验 clustered primary 限制、验证上下界列数，并把 AST 表达式 restore 成字符串。
pub fn normalizeSplitPolicy(
    ctx: expression::BuildContext,
    splitOpt: ast::SplitIndexOption,
    tbInfo: model::TableInfo,
) -> Result<(model::RegionSplitPolicy, String), errors::Error> {
    let mut splitOpt = splitOpt;
    let mut indexName = String::new();
    if !splitOpt.TableLevel {
        let pkName = strings::ToLower(mysql::PrimaryKeyName);
        indexName = splitOpt.IndexName.L.clone();
        // fill primary key name if empty
        if splitOpt.PrimaryKey && indexName.is_empty() {
            indexName = pkName.clone();
        }
        // set PrimaryKey if SPLIT INDEX `PRIMARY`
        if indexName == pkName {
            splitOpt.PrimaryKey = true;
        }
    }

    if tbInfo.HasClusteredIndex() && splitOpt.PrimaryKey {
        // cannot specify both SPLIT PRIMARY for CLUSTERED table
        // it is for unclustered primary
        return Err(dbterror::ErrForbiddenDDL.FastGenByArgs(
            "SPLIT PRIMARY is only for non-clustered table",
        ));
    }

    if splitOpt.SplitOpt.Num < 1 {
        // must larger than 1
        return Err(dbterror::ErrForbiddenDDL.FastGenByArgs(
            "SPLIT REGION number must not be zero or negative",
        ));
    }

    // default int, it is 1
    let mut colen = 1;
    if tbInfo.IsCommonHandle && splitOpt.PrimaryKey {
        let pk = tables::FindPrimaryIndex(&tbInfo);
        colen = pk.Columns.len();
    } else if !indexName.is_empty() {
        let idx = tbInfo.FindIndexByName(&indexName);
        let Some(idx) = idx else {
            return Err(dbterror::ErrWrongNameForIndex.GenWithStackByArgs(indexName));
        };
        colen = idx.Columns.len();
    }
    if colen != splitOpt.SplitOpt.Upper.len() || colen != splitOpt.SplitOpt.Lower.len() {
        return Err(dbterror::ErrInvalidSplitRegionRanges
            .GenWithStackByArgs("length of index columns and split values differ"));
    }

    let mut buf = strings::Builder::new();
    let restoreCtx = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut buf);

    let mut policy = model::RegionSplitPolicy {
        Regions: splitOpt.SplitOpt.Num,
        Lower: Vec::new(),
        Upper: Vec::new(),
    };

    policy.Lower = vec![String::new(); splitOpt.SplitOpt.Lower.len()];
    for (i, expr) in splitOpt.SplitOpt.Lower.iter().enumerate() {
        buf.Reset();
        // validate expr
        // Go 先 BuildSimpleExpr，再 Eval 验证表达式可求值，最后 Restore 为 policy 字符串。
        let d = expression::BuildSimpleExpr(ctx.clone(), expr).map_err(errors::Trace)?;
        d.Eval(ctx.GetEvalCtx(), chunk::Row::default())
            .map_err(errors::Trace)?;
        expr.Restore(&restoreCtx).map_err(errors::Trace)?;
        policy.Lower[i] = buf.String();
    }

    policy.Upper = vec![String::new(); splitOpt.SplitOpt.Upper.len()];
    for (i, expr) in splitOpt.SplitOpt.Upper.iter().enumerate() {
        buf.Reset();
        // validate expr
        let d = expression::BuildSimpleExpr(ctx.clone(), expr).map_err(errors::Trace)?;
        d.Eval(ctx.GetEvalCtx(), chunk::Row::default())
            .map_err(errors::Trace)?;
        expr.Restore(&restoreCtx).map_err(errors::Trace)?;
        policy.Upper[i] = buf.String();
    }

    Ok((policy, indexName))
}
*/

// Region（分布式 KV 存储中的数据分片单位）预切分与 scatter 辅助。
//
// 建表/加索引时可按 shard row id、显式 split policy 或表前缀生成切分键，
// 调用存储层 SplitRegions，并可按 Table/Global 作用域等待 scatter
//（将新建 Region 打散到不同 store，避免热点集中）完成。
// 文件前半的大块注释保留了 Go 侧完整流程的迁移草稿。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Scatter 作用域：全局统一组，或按表 ID 分组。
pub enum ScatterScope {
    Global,
    Table,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 显式 Region 切分策略：上下界、目标 Region 数，或直接给出 value lists。
pub struct RegionSplitPolicy {
    pub lower: Vec<String>,
    pub upper: Vec<String>,
    pub num: u64,
    pub value_lists: Vec<Vec<String>>,
    pub index_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 参与预切分的表信息：物理 ID、分片位数、预切 Region 数与索引 ID。
pub struct SplitTableInfo {
    pub table_id: i64,
    pub partition_ids: Vec<i64>,
    pub shard_row_id_bits: u8,
    pub pre_split_regions: u8,
    pub index_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 切分或 scatter 过程中的错误。
pub enum SplitError {
    InvalidBounds,
    InvalidRegionCount,
    TooManyPreSplitRegions,
    InvalidExpression,
    ScatterFailed(u64),
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SplitError {}

/// 校验并归一化 SPLIT REGION 选项，生成 `RegionSplitPolicy`。
///
/// `value_lists` 为空时要求 `num >= 1` 且上下界等长；
/// 否则禁止同时指定 num/上下界，且每组 value 非空。
pub fn normalize_split_policy(
    lower: Vec<String>,
    upper: Vec<String>,
    num: u64,
    value_lists: Vec<Vec<String>>,
    index_name: Option<String>,
) -> Result<RegionSplitPolicy, SplitError> {
    if value_lists.is_empty() {
        if num < 1 {
            return Err(SplitError::InvalidRegionCount);
        }
        if lower.is_empty() || lower.len() != upper.len() {
            return Err(SplitError::InvalidBounds);
        }
    } else if num != 0 || !lower.is_empty() || !upper.is_empty() {
        return Err(SplitError::InvalidBounds);
    }
    if value_lists
        .iter()
        .any(|values| values.is_empty() || values.iter().any(|value| value.trim().is_empty()))
    {
        return Err(SplitError::InvalidExpression);
    }
    Ok(RegionSplitPolicy {
        lower,
        upper,
        num,
        value_lists,
        index_name: index_name.map(|name| name.to_ascii_lowercase()),
    })
}

/// 编码表记录键：`t{physical_id}_r{memcomparable(handle)}`。
pub fn encode_record_key(physical_id: i64, handle: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(18);
    key.extend_from_slice(b"t");
    key.extend_from_slice(&physical_id.to_be_bytes());
    key.extend_from_slice(b"_r");
    // 将有符号 handle 转为 memcomparable 无序编码（翻转符号位）。
    key.extend_from_slice(&(handle as u64 ^ (1_u64 << 63)).to_be_bytes());
    key
}

/// 编码索引键：`t{physical_id}_i{index_id}` 后跟各列值与 0 分隔。
pub fn encode_index_key(physical_id: i64, index_id: i64, values: &[String]) -> Vec<u8> {
    let mut key = Vec::new();
    key.extend_from_slice(b"t");
    key.extend_from_slice(&physical_id.to_be_bytes());
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&index_id.to_be_bytes());
    for value in values {
        key.extend_from_slice(value.as_bytes());
        key.push(0);
    }
    key
}

/// 按 shard_row_id_bits / pre_split_regions 为各物理表生成记录区预切分键。
pub fn pre_split_record_keys(info: &SplitTableInfo) -> Result<Vec<Vec<u8>>, SplitError> {
    if info.pre_split_regions > info.shard_row_id_bits
        || info.pre_split_regions > 15
        || info.shard_row_id_bits > 63
    {
        return Err(SplitError::TooManyPreSplitRegions);
    }
    // 无分区时用 table_id；有分区则对每个 partition_id 分别切分。
    let physical_ids: Vec<i64> = if info.partition_ids.is_empty() {
        vec![info.table_id]
    } else {
        info.partition_ids.clone()
    };
    let shard_count = 1_u64 << info.shard_row_id_bits;
    let shard_step = 1_u64 << (info.shard_row_id_bits - info.pre_split_regions);
    let incremental_bits = 63 - info.shard_row_id_bits;
    let mut keys = Vec::new();
    for physical_id in physical_ids {
        let mut table_prefix = b"t".to_vec();
        table_prefix.extend_from_slice(&physical_id.to_be_bytes());
        keys.push(table_prefix);
        let mut shard = shard_step;
        while shard < shard_count {
            let handle = shard << incremental_bits;
            keys.push(encode_record_key(physical_id, handle as i64));
            shard += shard_step;
        }
    }
    Ok(keys)
}

/// 按显式 policy 生成切分键：索引 value lists、记录 value lists，或上下界均匀插值。
pub fn policy_split_keys(
    info: &SplitTableInfo,
    policy: &RegionSplitPolicy,
) -> Result<Vec<Vec<u8>>, SplitError> {
    let physical_ids: Vec<i64> = if info.partition_ids.is_empty() {
        vec![info.table_id]
    } else {
        info.partition_ids.clone()
    };
    let mut keys = Vec::new();
    // 约定索引名形如 `idx{N}` 时按索引键切分。
    let index_id = policy.index_name.as_ref().and_then(|name| {
        name.strip_prefix("idx")
            .and_then(|id| id.parse::<i64>().ok())
    });
    for physical_id in physical_ids {
        if let Some(index_id) = index_id {
            for values in &policy.value_lists {
                keys.push(encode_index_key(physical_id, index_id, values));
            }
        } else if !policy.value_lists.is_empty() {
            for values in &policy.value_lists {
                let handle = values
                    .first()
                    .and_then(|value| value.parse().ok())
                    .ok_or(SplitError::InvalidExpression)?;
                keys.push(encode_record_key(physical_id, handle));
            }
        } else {
            // 在 [lower, upper) 上按 num 等分插入切分点。
            let low = policy
                .lower
                .first()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or(SplitError::InvalidExpression)?;
            let high = policy
                .upper
                .first()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or(SplitError::InvalidExpression)?;
            let span = high as i128 - low as i128;
            for offset in 1..policy.num {
                let handle = low as i128 + span * offset as i128 / policy.num as i128;
                keys.push(encode_record_key(physical_id, handle as i64));
            }
        }
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// 将布尔开关映射为 Scatter 作用域。
pub fn scatter_scope(global_scatter: bool) -> ScatterScope {
    if global_scatter {
        ScatterScope::Global
    } else {
        ScatterScope::Table
    }
}

/// 等待 scatter 结果：全部成功返回完成数，遇到非 PD 错误立即停止。
///
/// 此简化接口无法表达 Go 中可继续等待的 `PDError`，因此其 `Err(())`
/// 代表 Go 分支中的非 PD 错误。
pub fn wait_scatter_finished(
    results: impl IntoIterator<Item = Result<(), ()>>,
) -> Result<u64, SplitError> {
    let mut finished = 0;
    for result in results {
        match result {
            Ok(()) => finished += 1,
            Err(()) => return Err(SplitError::ScatterFailed(1)),
        }
    }
    Ok(finished)
}
