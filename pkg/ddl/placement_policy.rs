// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Placement Policy（放置策略）目录与 DDL 辅助逻辑。
//
// Placement Policy 描述表/库/分区数据在集群中的副本分布策略（主区域、副本数、label 约束等）。
// 本文件前半为 Go DDL job 状态机草稿（块注释保留），后半为实现可用的策略目录
// `PlacementPolicyCatalog`：创建/替换、修改、分阶段删除，以及引用归一化与占用检查。

/*
// placement policy DDL job 的创建、删除、修改、引用检查与 session 层归一化流程。

#![allow(non_snake_case, non_camel_case_types, dead_code, unused_variables)]

// onCreatePlacementPolicy 对应 Go 的创建策略 DDL job 处理函数。
// 它先校验 placement settings，再根据同名策略是否存在选择 create 或 replace 路径。
fn onCreatePlacementPolicy(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let args = match model::GetPlacementPolicyArgs(job) {
        Ok(args) => args,
        Err(err) => {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };
    let mut policyInfo = args.Policy;
    let orReplace = args.ReplaceOnExist;
    policyInfo.State = model::StateNone;

    if let Err(err) = checkPolicyValidation(policyInfo.PlacementSettings) {
        job.State = model::JobStateCancelled;
        return Err(errors::Trace(err));
    }

    let metaMut = jobCtx.metaMut;
    let existPolicy = match getPlacementPolicyByName(jobCtx.infoCache, metaMut, policyInfo.Name) {
        Ok(policy) => policy,
        Err(err) => {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };

    if let Some(existPolicy) = existPolicy {
        if !orReplace {
            job.State = model::JobStateCancelled;
            return Err(infoschema::ErrPlacementPolicyExists.GenWithStackByArgs(existPolicy.Name));
        }

        // OR REPLACE 路径复用旧 ID，仅替换 PlacementSettings，并刷新依赖对象上的 bundle。
        let mut replacePolicy = existPolicy.Clone();
        replacePolicy.PlacementSettings = policyInfo.PlacementSettings;
        if let Err(err) = updateExistPlacementPolicy(metaMut, &mut replacePolicy) {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }

        job.SchemaID = replacePolicy.ID;
        ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
        job.FinishDBJob(model::JobStateDone, model::StatePublic, ver, None);
        return Ok(ver);
    }

    match policyInfo.State {
        model::StateNone => {
            // Go 状态机为 none -> public；创建 meta policy 后立即完成 job。
            policyInfo.State = model::StatePublic;
            metaMut.CreatePolicy(policyInfo).map_err(errors::Trace)?;
            job.SchemaID = policyInfo.ID;
            ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
            job.FinishDBJob(model::JobStateDone, model::StatePublic, ver, None);
            Ok(ver)
        }
        _ => {
            // Go 认为其他状态不可达；这里保留 invalid DDL state 错误。
            Err(dbterror::ErrInvalidDDLState.GenWithStackByArgs("policy", policyInfo.State))
        }
    }
}

// checkPolicyValidation 对应 Go 的选项校验，实际语义由 placement.NewBundleFromOptions 承担。
fn checkPolicyValidation(info: &model::PlacementSettings) -> Result<(), errors::Error> {
    placement::NewBundleFromOptions(info)?;
    Ok(())
}

// getPolicyInfo 对应 Go：按 ID 从 meta 取策略，并把 meta.ErrPolicyNotExists 转成 infoschema 错误。
fn getPolicyInfo(t: &mut meta::Mutator, policyID: i64) -> Result<Box<model::PolicyInfo>, errors::Error> {
    match t.GetPolicy(policyID) {
        Ok(policy) => Ok(policy),
        Err(err) => {
            if meta::ErrPolicyNotExists.Equal(err) {
                return Err(infoschema::ErrPlacementPolicyNotExists.GenWithStackByArgs(
                    format!("(Policy ID {})", policyID),
                ));
            }
            Err(err)
        }
    }
}

// getPlacementPolicyByName 对应 Go：优先使用当前版本的 infoschema cache，否则直接扫描 meta。
fn getPlacementPolicyByName(
    infoCache: &infoschema::InfoCache,
    t: &mut meta::Mutator,
    policyName: ast::CIStr,
) -> Result<Option<Box<model::PolicyInfo>>, errors::Error> {
    let currVer = t.GetSchemaVersion()?;

    let is = infoCache.GetLatest();
    if is.is_some() && is.SchemaMetaVersion() == currVer {
        // cache 与 meta 版本一致时，直接从 infoschema 查，避免额外 meta 扫描。
        if let Some(policy) = is.PolicyByName(policyName) {
            return Ok(Some(policy));
        }
        return Ok(None);
    }

    let policies = t.ListPolicies().map_err(errors::Trace)?;
    for policy in policies {
        if policy.Name.L == policyName.L {
            return Ok(Some(policy));
        }
    }
    Ok(None)
}

// checkPlacementPolicyExistAndCancelNonExistJob 对应 Go：策略不存在时取消当前 DDL job。
fn checkPlacementPolicyExistAndCancelNonExistJob(
    t: &mut meta::Mutator,
    job: &mut model::Job,
    policyID: i64,
) -> Result<Box<model::PolicyInfo>, errors::Error> {
    match getPolicyInfo(t, policyID) {
        Ok(policy) => Ok(policy),
        Err(err) => {
            if infoschema::ErrPlacementPolicyNotExists.Equal(err) {
                job.State = model::JobStateCancelled;
            }
            Err(err)
        }
    }
}

// checkPlacementPolicyRefValidAndCanNonValidJob 对应 Go：nil ref 直接通过，否则检查引用 ID。
fn checkPlacementPolicyRefValidAndCanNonValidJob(
    t: &mut meta::Mutator,
    job: &mut model::Job,
    ref_: Option<&model::PolicyRefInfo>,
) -> Result<Option<Box<model::PolicyInfo>>, errors::Error> {
    if ref_.is_none() {
        return Ok(None);
    }
    checkPlacementPolicyExistAndCancelNonExistJob(t, job, ref_.unwrap().ID).map(Some)
}

// checkAllTablePlacementPoliciesExistAndCancelNonExistJob 对应 Go：检查表和全部分区引用的 policy 是否存在。
fn checkAllTablePlacementPoliciesExistAndCancelNonExistJob(
    t: &mut meta::Mutator,
    job: &mut model::Job,
    tblInfo: &model::TableInfo,
) -> Result<(), errors::Error> {
    checkPlacementPolicyRefValidAndCanNonValidJob(t, job, tblInfo.PlacementPolicyRef.as_ref())
        .map_err(errors::Trace)?;

    if tblInfo.Partition.is_none() {
        return Ok(());
    }

    for def in tblInfo.Partition.as_ref().unwrap().Definitions.iter() {
        checkPlacementPolicyRefValidAndCanNonValidJob(t, job, def.PlacementPolicyRef.as_ref())
            .map_err(errors::Trace)?;
    }
    Ok(())
}

// onDropPlacementPolicy 对应 Go 的删除策略状态机：Public -> WriteOnly -> DeleteOnly -> None。
fn onDropPlacementPolicy(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let args = model::GetPlacementPolicyArgs(job).map_err(errors::Trace)?;
    let metaMut = jobCtx.metaMut;
    let mut policyInfo = checkPlacementPolicyExistAndCancelNonExistJob(metaMut, job, args.PolicyID)
        .map_err(errors::Trace)?;

    if let Err(err) = checkPlacementPolicyNotInUse(jobCtx.infoCache, metaMut, &policyInfo) {
        if dbterror::ErrPlacementPolicyInUse.Equal(err) {
            job.State = model::JobStateCancelled;
        }
        return Err(errors::Trace(err));
    }

    match policyInfo.State {
        model::StatePublic => {
            // public -> write only，先写 meta，再更新 schema version。
            policyInfo.State = model::StateWriteOnly;
            metaMut.UpdatePolicy(&policyInfo).map_err(errors::Trace)?;
            ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
            job.SchemaState = model::StateWriteOnly;
        }
        model::StateWriteOnly => {
            // write only -> delete only，继续推进删除状态机。
            policyInfo.State = model::StateDeleteOnly;
            metaMut.UpdatePolicy(&policyInfo).map_err(errors::Trace)?;
            ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
            job.SchemaState = model::StateDeleteOnly;
        }
        model::StateDeleteOnly => {
            // 最后一段删除 meta policy，并完成 DDL job；Go 注释说明这里暂不考虑 binlog sync。
            policyInfo.State = model::StateNone;
            metaMut.DropPolicy(policyInfo.ID).map_err(errors::Trace)?;
            ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
            job.FinishDBJob(model::JobStateDone, model::StateNone, ver, None);
        }
        _ => {
            return Err(errors::Trace(
                dbterror::ErrInvalidDDLState.GenWithStackByArgs("policy", policyInfo.State),
            ));
        }
    }
    Ok(ver)
}

// onAlterPlacementPolicy 对应 Go 的修改策略 DDL：读取旧策略、替换 settings、刷新依赖 bundle 后完成 job。
fn onAlterPlacementPolicy(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let args = match model::GetPlacementPolicyArgs(job) {
        Ok(args) => args,
        Err(err) => {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };

    let metaMut = jobCtx.metaMut;
    let oldPolicy = checkPlacementPolicyExistAndCancelNonExistJob(metaMut, job, args.PolicyID)?;
    let mut newPolicyInfo = (*oldPolicy).clone();
    newPolicyInfo.PlacementSettings = args.Policy.PlacementSettings;

    checkPolicyValidation(newPolicyInfo.PlacementSettings)?;

    if let Err(err) = updateExistPlacementPolicy(metaMut, &mut newPolicyInfo) {
        job.State = model::JobStateCancelled;
        return Err(errors::Trace(err));
    }

    ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
    job.FinishDBJob(model::JobStateDone, model::StatePublic, ver, None);
    Ok(ver)
}

// updateExistPlacementPolicy 对应 Go：更新 meta 后，找出依赖该 policy 的对象并重建对应 bundle。
fn updateExistPlacementPolicy(
    t: &mut meta::Mutator,
    policy: &mut model::PolicyInfo,
) -> Result<(), errors::Error> {
    t.UpdatePolicy(policy).map_err(errors::Trace)?;

    let (_dbIDs, partIDs, tblInfos) =
        getPlacementPolicyDependedObjectsIDs(t, policy).map_err(errors::Trace)?;

    let bundle = placement::NewBundleFromOptions(policy.PlacementSettings).map_err(errors::Trace)?;
    let mut bundles: Vec<Box<placement::Bundle>> =
        Vec::with_capacity(tblInfos.len() + partIDs.len() + 2);

    for tbl in tblInfos {
        let mut cp = bundle.Clone();
        let mut ids = vec![tbl.ID];
        if tbl.Partition.is_some() {
            for pDef in tbl.Partition.as_ref().unwrap().Definitions.iter() {
                ids.push(pDef.ID);
            }
        }
        bundles.push(cp.Reset(placement::RuleIndexTable, ids));
    }

    for id in partIDs {
        let mut cp = bundle.Clone();
        bundles.push(cp.Reset(placement::RuleIndexPartition, vec![id]));
    }

    // resetRangeFn 对应 Go 的闭包：只有 range 当前使用该 policy 时才重建 global/meta bundle。
    let mut resetRangeFn = |ctx: context::Context, rangeName: &str| -> Result<(), errors::Error> {
        let mut rangeBundleID = placement::TiDBBundleRangePrefixForGlobal;
        if rangeName == placement::KeyRangeMeta {
            rangeBundleID = placement::TiDBBundleRangePrefixForMeta;
        }
        let policyName = GetRangePlacementPolicyName(ctx, rangeBundleID)?;
        if policyName == policy.Name.L {
            let mut cp = bundle.Clone();
            bundles.push(cp.RebuildForRange(rangeName, policyName));
        }
        Ok(())
    };

    resetRangeFn(context::TODO(), placement::KeyRangeGlobal)?;
    resetRangeFn(context::TODO(), placement::KeyRangeMeta)?;

    if !bundles.is_empty() {
        // Go 这里会向 PD 发 HTTP 请求；只保留 infosync 调用形状，不实际执行外部 I/O。
        infosync::PutRuleBundlesWithDefaultRetry(context::TODO(), bundles)
            .map_err(|err| errors::Wrapf(err, "failed to notify PD the placement rules"))?;
    }
    Ok(())
}

// checkPlacementPolicyNotInUse 对应 Go：选择 infoschema cache 或 meta 扫描，再检查特殊 range。
fn checkPlacementPolicyNotInUse(
    infoCache: &infoschema::InfoCache,
    t: &mut meta::Mutator,
    policy: &model::PolicyInfo,
) -> Result<(), errors::Error> {
    let currVer = t.GetSchemaVersion()?;
    let is = infoCache.GetLatest();
    let err = if is.is_some() && is.SchemaMetaVersion() == currVer {
        CheckPlacementPolicyNotInUseFromInfoSchema(is, policy)
    } else {
        CheckPlacementPolicyNotInUseFromMeta(t, policy)
    };
    err?;
    checkPlacementPolicyNotInUseFromRange(policy)
}

// CheckPlacementPolicyNotInUseFromInfoSchema export for test.
// Go 测试会直接调用该函数；这里保留导出函数和 cache 扫描顺序。
pub fn CheckPlacementPolicyNotInUseFromInfoSchema(
    is: infoschema::InfoSchema,
    policy: &model::PolicyInfo,
) -> Result<(), errors::Error> {
    for dbInfo in is.AllSchemas() {
        if let Some(ref_) = dbInfo.PlacementPolicyRef {
            if ref_.ID == policy.ID {
                return Err(dbterror::ErrPlacementPolicyInUse.GenWithStackByArgs(policy.Name));
            }
        }
    }

    let schemaTables =
        is.ListTablesWithSpecialAttribute(infoschemacontext::AllPlacementPolicyAttribute);
    for schemaTable in schemaTables {
        for tblInfo in schemaTable.TableInfos {
            checkPlacementPolicyNotUsedByTable(tblInfo, policy)?;
        }
    }

    Ok(())
}

// checkPlacementPolicyNotInUseFromRange checks whether the placement policy is used by the special range.
fn checkPlacementPolicyNotInUseFromRange(policy: &model::PolicyInfo) -> Result<(), errors::Error> {
    let checkFn = |rangeBundleID: &str| -> Result<(), errors::Error> {
        let policyName = GetRangePlacementPolicyName(context::TODO(), rangeBundleID)?;
        if policyName == policy.Name.L {
            return Err(dbterror::ErrPlacementPolicyInUse.GenWithStackByArgs(policy.Name));
        }
        Ok(())
    };

    checkFn(placement::TiDBBundleRangePrefixForGlobal)?;
    checkFn(placement::TiDBBundleRangePrefixForMeta)
}

// getPlacementPolicyDependedObjectsIDs 对应 Go：扫描 DB、表和分区，收集依赖指定 policy 的对象 ID。
fn getPlacementPolicyDependedObjectsIDs(
    t: &mut meta::Mutator,
    policy: &model::PolicyInfo,
) -> Result<(Vec<i64>, Vec<i64>, Vec<Box<model::TableInfo>>), errors::Error> {
    let schemas = t.ListDatabases()?;
    let mut dbIDs: Vec<i64> = Vec::with_capacity(schemas.len());
    let mut partIDs: Vec<i64> = Vec::with_capacity(schemas.len());
    let mut tblInfos: Vec<Box<model::TableInfo>> = Vec::with_capacity(schemas.len());

    for dbInfo in schemas {
        if let Some(ref_) = dbInfo.PlacementPolicyRef {
            if ref_.ID == policy.ID {
                dbIDs.push(dbInfo.ID);
            }
        }
        let tables = meta::GetTableInfoWithAttributes(
            t,
            dbInfo.ID,
            meta::MustLoadFilterAttr { Attr: r#""partition":null"#, LoadIfMissing: true },
            meta::MustLoadFilterAttr { Attr: r#""policy_ref_info":null"#, LoadIfMissing: true },
        )?;
        for tblInfo in tables {
            if let Some(ref_) = tblInfo.PlacementPolicyRef {
                if ref_.ID == policy.ID {
                    tblInfos.push(tblInfo);
                }
            }
            if tblInfo.Partition.is_some() {
                for part in tblInfo.Partition.as_ref().unwrap().Definitions.iter() {
                    if let Some(partRef) = part.PlacementPolicyRef.as_ref() {
                        if partRef.ID == policy.ID {
                            partIDs.push(part.ID);
                        }
                    }
                }
            }
        }
    }
    Ok((dbIDs, partIDs, tblInfos))
}

// CheckPlacementPolicyNotInUseFromMeta export for test.
// Go 版本直接扫描 meta 中全部 DB 和表，适用于 cache 版本不匹配时的兜底检查。
pub fn CheckPlacementPolicyNotInUseFromMeta(
    t: &mut meta::Mutator,
    policy: &model::PolicyInfo,
) -> Result<(), errors::Error> {
    let schemas = t.ListDatabases()?;

    for dbInfo in schemas {
        if let Some(ref_) = dbInfo.PlacementPolicyRef {
            if ref_.ID == policy.ID {
                return Err(dbterror::ErrPlacementPolicyInUse.GenWithStackByArgs(policy.Name));
            }
        }

        let tables = t.ListTables(context::Background(), dbInfo.ID)?;
        for tblInfo in tables {
            checkPlacementPolicyNotUsedByTable(tblInfo, policy)?;
        }
    }
    Ok(())
}

// checkPlacementPolicyNotUsedByTable 对应 Go：检查表本身和分区是否引用同一个 policy。
fn checkPlacementPolicyNotUsedByTable(
    tblInfo: &model::TableInfo,
    policy: &model::PolicyInfo,
) -> Result<(), errors::Error> {
    if let Some(ref_) = tblInfo.PlacementPolicyRef.as_ref() {
        if ref_.ID == policy.ID {
            return Err(dbterror::ErrPlacementPolicyInUse.GenWithStackByArgs(policy.Name));
        }
    }

    if tblInfo.Partition.is_some() {
        for partition in tblInfo.Partition.as_ref().unwrap().Definitions.iter() {
            if let Some(ref_) = partition.PlacementPolicyRef.as_ref() {
                if ref_.ID == policy.ID {
                    return Err(dbterror::ErrPlacementPolicyInUse.GenWithStackByArgs(policy.Name));
                }
            }
        }
    }

    Ok(())
}

// GetRangePlacementPolicyName get the placement policy name used by range.
// rangeBundleID is limited to TiDBBundleRangePrefixForGlobal and TiDBBundleRangePrefixForMeta.
pub fn GetRangePlacementPolicyName(
    ctx: context::Context,
    rangeBundleID: &str,
) -> Result<String, errors::Error> {
    let bundle = infosync::GetRuleBundle(ctx, rangeBundleID)?;
    if bundle.is_none() || bundle.as_ref().unwrap().Rules.is_empty() {
        return Ok(String::new());
    }
    let rule = &bundle.unwrap().Rules[0];
    let pos = rule.ID.rfind("_rule_");
    if let Some(pos) = pos {
        if pos > 0 {
            return Ok(rule.ID[..pos].to_string());
        }
    }
    Ok(String::new())
}

// buildPolicyInfo 对应 Go：从 AST option 列表构造 PolicyInfo，并复用 SetDirectPlacementOpt 做字段赋值。
fn buildPolicyInfo(
    name: ast::CIStr,
    options: Vec<Box<ast::PlacementOption>>,
) -> Result<Box<model::PolicyInfo>, errors::Error> {
    let mut policyInfo = Box::new(model::PolicyInfo {
        PlacementSettings: Box::new(model::PlacementSettings::default()),
        ..Default::default()
    });
    policyInfo.Name = name;
    for opt in options {
        SetDirectPlacementOpt(
            &mut policyInfo.PlacementSettings,
            opt.Tp,
            opt.StrValue,
            opt.UintValue,
        )?;
    }
    Ok(policyInfo)
}

// removeTablePlacement 对应 Go：移除表和分区上的 placement ref，并返回是否真的发生改动。
fn removeTablePlacement(tbInfo: &mut model::TableInfo) -> bool {
    let mut hasPlacementSettings = false;
    if tbInfo.PlacementPolicyRef.is_some() {
        tbInfo.PlacementPolicyRef = None;
        hasPlacementSettings = true;
    }

    if removePartitionPlacement(tbInfo.Partition.as_mut()) {
        hasPlacementSettings = true;
    }

    hasPlacementSettings
}

// removePartitionPlacement 对应 Go：遍历分区定义并清空 PlacementPolicyRef。
fn removePartitionPlacement(partInfo: Option<&mut model::PartitionInfo>) -> bool {
    if partInfo.is_none() {
        return false;
    }

    let mut hasPlacementSettings = false;
    for def in partInfo.unwrap().Definitions.iter_mut() {
        if def.PlacementPolicyRef.is_some() {
            def.PlacementPolicyRef = None;
            hasPlacementSettings = true;
        }
    }
    hasPlacementSettings
}

// handleDatabasePlacement 对应 Go：在 ignore 模式下删除 DB placement，并把 note 追加到 StmtCtx。
fn handleDatabasePlacement(
    ctx: sessionctx::Context,
    dbInfo: &mut model::DBInfo,
) -> Result<(), errors::Error> {
    if dbInfo.PlacementPolicyRef.is_none() {
        return Ok(());
    }

    let sessVars = ctx.GetSessionVars();
    if sessVars.PlacementMode == vardef::PlacementModeIgnore {
        dbInfo.PlacementPolicyRef = None;
        sessVars.StmtCtx.AppendNote(errors::NewNoStackErrorf(format!(
            "Placement is ignored when TIDB_PLACEMENT_MODE is '{}'",
            vardef::PlacementModeIgnore
        )));
        return Ok(());
    }

    dbInfo.PlacementPolicyRef =
        checkAndNormalizePlacementPolicy(ctx, dbInfo.PlacementPolicyRef)?;
    Ok(())
}

// handleTablePlacement 对应 Go：处理表和分区 placement；ignore 模式会整体移除并追加 warning note。
fn handleTablePlacement(
    ctx: sessionctx::Context,
    tbInfo: &mut model::TableInfo,
) -> Result<(), errors::Error> {
    let sessVars = ctx.GetSessionVars();
    if sessVars.PlacementMode == vardef::PlacementModeIgnore && removeTablePlacement(tbInfo) {
        sessVars.StmtCtx.AppendNote(errors::NewNoStackErrorf(format!(
            "Placement is ignored when TIDB_PLACEMENT_MODE is '{}'",
            vardef::PlacementModeIgnore
        )));
        return Ok(());
    }

    tbInfo.PlacementPolicyRef =
        checkAndNormalizePlacementPolicy(ctx, tbInfo.PlacementPolicyRef)?;
    if tbInfo.Partition.is_some() {
        for partition in tbInfo.Partition.as_mut().unwrap().Definitions.iter_mut() {
            partition.PlacementPolicyRef =
                checkAndNormalizePlacementPolicy(ctx, partition.PlacementPolicyRef)?;
        }
    }
    Ok(())
}

// handlePartitionPlacement 对应 Go：仅处理分区列表上的 placement ref。
fn handlePartitionPlacement(
    ctx: sessionctx::Context,
    partInfo: &mut model::PartitionInfo,
) -> Result<(), errors::Error> {
    let sessVars = ctx.GetSessionVars();
    if sessVars.PlacementMode == vardef::PlacementModeIgnore
        && removePartitionPlacement(Some(partInfo))
    {
        sessVars.StmtCtx.AppendNote(errors::NewNoStackErrorf(format!(
            "Placement is ignored when TIDB_PLACEMENT_MODE is '{}'",
            vardef::PlacementModeIgnore
        )));
        return Ok(());
    }

    for partition in partInfo.Definitions.iter_mut() {
        partition.PlacementPolicyRef =
            checkAndNormalizePlacementPolicy(ctx, partition.PlacementPolicyRef)?;
    }
    Ok(())
}

// checkAndNormalizePlacementPolicy 对应 Go：把 policy 名称解析为 ID；名字为 default 时表示移除 placement 设置。
fn checkAndNormalizePlacementPolicy(
    ctx: sessionctx::Context,
    placementPolicyRef: Option<Box<model::PolicyRefInfo>>,
) -> Result<Option<Box<model::PolicyRefInfo>>, errors::Error> {
    if placementPolicyRef.is_none() {
        return Ok(None);
    }

    let mut placementPolicyRef = placementPolicyRef.unwrap();
    if placementPolicyRef.Name.L == defaultPlacementPolicyName {
        // Go 中 default policy 名字是删除 placement settings 的语义，而不是查找真实 policy。
        return Ok(None);
    }

    let (policy, ok) =
        sessiontxn::GetTxnManager(ctx).GetTxnInfoSchema().PolicyByName(placementPolicyRef.Name);
    if !ok {
        return Err(errors::Trace(
            infoschema::ErrPlacementPolicyNotExists.GenWithStackByArgs(placementPolicyRef.Name),
        ));
    }

    placementPolicyRef.ID = policy.ID;
    Ok(Some(placementPolicyRef))
}

// checkIgnorePlacementDDL 对应 Go：会话为 ignore 模式时追加 note 并阻止后续 placement DDL 逻辑。
fn checkIgnorePlacementDDL(ctx: sessionctx::Context) -> bool {
    let sessVars = ctx.GetSessionVars();
    if sessVars.PlacementMode == vardef::PlacementModeIgnore {
        sessVars.StmtCtx.AppendNote(errors::NewNoStackErrorf(format!(
            "Placement is ignored when TIDB_PLACEMENT_MODE is '{}'",
            vardef::PlacementModeIgnore
        )));
        return true;
    }
    false
}

// SetDirectPlacementOpt tries to make the PlacementSettings assignments generic for Schema/Table/Partition
// Go 通过 ast.PlacementOptionType 分派到 PlacementSettings 的各字段；未知 option 返回错误。
pub fn SetDirectPlacementOpt(
    placementSettings: &mut model::PlacementSettings,
    placementOptionType: ast::PlacementOptionType,
    stringVal: String,
    uintVal: u64,
) -> Result<(), errors::Error> {
    match placementOptionType {
        ast::PlacementOptionPrimaryRegion => placementSettings.PrimaryRegion = stringVal,
        ast::PlacementOptionRegions => placementSettings.Regions = stringVal,
        ast::PlacementOptionFollowerCount => placementSettings.Followers = uintVal,
        ast::PlacementOptionVoterCount => placementSettings.Voters = uintVal,
        ast::PlacementOptionLearnerCount => placementSettings.Learners = uintVal,
        ast::PlacementOptionSchedule => placementSettings.Schedule = stringVal,
        ast::PlacementOptionConstraints => placementSettings.Constraints = stringVal,
        ast::PlacementOptionLeaderConstraints => placementSettings.LeaderConstraints = stringVal,
        ast::PlacementOptionLearnerConstraints => placementSettings.LearnerConstraints = stringVal,
        ast::PlacementOptionFollowerConstraints => placementSettings.FollowerConstraints = stringVal,
        ast::PlacementOptionVoterConstraints => placementSettings.VoterConstraints = stringVal,
        ast::PlacementOptionSurvivalPreferences => placementSettings.SurvivalPreferences = stringVal,
        _ => return Err(errors::Trace(errors::New("unknown placement policy option"))),
    }
    Ok(())
}
*/

use std::collections::{BTreeMap, BTreeSet};

/// 放置策略在 DDL 状态机中的可见性状态。
///
/// 删除路径通常为 Public → WriteOnly → DeleteOnly → None（与 schema 对象软删除类似）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyState {
    /// 未公开（新建初始态）。
    None,
    /// 对外可见且可被引用。
    Public,
    /// 删除中间态：禁止新写入依赖。
    WriteOnly,
    /// 删除中间态：仅保留删除路径可见性。
    DeleteOnly,
}

/// 放置策略的具体配置项（区域、副本角色计数、约束字符串等）。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlacementSettings {
    /// 主区域（Primary Region）名称。
    pub primary_region: String,
    /// 参与调度的区域列表（逗号分隔）。
    pub regions: String,
    /// Follower 副本数。
    pub followers: u64,
    /// Voter 副本数。
    pub voters: u64,
    /// Learner 副本数（学习副本，不参与投票）。
    pub learners: u64,
    /// 调度策略名，如 EVEN / MAJORITY_IN_PRIMARY。
    pub schedule: String,
    /// 通用 label 约束字符串。
    pub constraints: String,
    /// Leader 约束。
    pub leader_constraints: String,
    /// Learner 约束。
    pub learner_constraints: String,
    /// Follower 约束。
    pub follower_constraints: String,
    /// Voter 约束。
    pub voter_constraints: String,
    /// 存活偏好（跨故障域容忍配置）。
    pub survival_preferences: String,
}

/// 一条已登记的放置策略元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyInfo {
    pub id: i64,
    pub name: String,
    pub state: PolicyState,
    pub settings: PlacementSettings,
}

/// 对某条放置策略的引用（按 ID + 名称）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyRef {
    pub id: i64,
    pub name: String,
}

/// 可绑定放置策略的对象（库/表/分区的简化模型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementObject {
    pub id: i64,
    pub policy_ref: Option<PolicyRef>,
    /// 分区子对象；表级对象可嵌套分区放置引用。
    pub partitions: Vec<PlacementObject>,
}

/// DDL/AST 侧放置选项类型，用于写入 `PlacementSettings` 对应字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementOptionType {
    PrimaryRegion,
    Regions,
    FollowerCount,
    VoterCount,
    LearnerCount,
    Schedule,
    Constraints,
    LeaderConstraints,
    LearnerConstraints,
    FollowerConstraints,
    VoterConstraints,
    SurvivalPreferences,
}

/// 放置策略操作错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyError {
    AlreadyExists,
    NotFound,
    InvalidState,
    InvalidOption,
    InvalidSettings,
    InUse,
    RangeBackend(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PolicyError {}

/// 内存中的放置策略目录：按 ID/名称索引，并记录库表引用与 range 规则占用。
#[derive(Default)]
pub struct PlacementPolicyCatalog {
    policies: BTreeMap<i64, PolicyInfo>,
    by_name: BTreeMap<String, i64>,
    /// Schema 版本号；每次成功变更递增。
    pub schema_version: u64,
    /// 数据库级放置对象列表。
    pub databases: Vec<PlacementObject>,
    /// 表级放置对象列表（可含分区）。
    pub tables: Vec<PlacementObject>,
    /// range 规则 ID → 策略名；用于检测系统 range 是否占用策略。
    pub range_policy_names: BTreeMap<String, String>,
}

impl PlacementPolicyCatalog {
    /// 创建策略；若同名已存在且 `replace_on_exist` 为真则仅替换 settings。
    pub fn create(
        &mut self,
        mut policy: PolicyInfo,
        replace_on_exist: bool,
    ) -> Result<u64, PolicyError> {
        // Go onCreatePlacementPolicy discards the caller-provided state before validation.
        policy.state = PolicyState::None;
        check_policy_validation(&policy.settings)?;
        let key = policy.name.to_ascii_lowercase();
        // OR REPLACE：复用原 ID，只更新 settings。
        if let Some(existing_id) = self.by_name.get(&key).copied() {
            if !replace_on_exist {
                return Err(PolicyError::AlreadyExists);
            }
            let old = self
                .policies
                .get_mut(&existing_id)
                .ok_or(PolicyError::NotFound)?;
            old.settings = policy.settings;
            self.schema_version = self.schema_version.saturating_add(1);
            return Ok(self.schema_version);
        }
        // 新建：None → Public 后写入目录。
        policy.state = PolicyState::Public;
        self.by_name.insert(key, policy.id);
        self.policies.insert(policy.id, policy);
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(self.schema_version)
    }

    /// 修改已存在且处于 Public 状态的策略配置。
    pub fn alter(
        &mut self,
        policy_id: i64,
        settings: PlacementSettings,
    ) -> Result<u64, PolicyError> {
        // Go resolves the old policy before validating the replacement settings.
        if !self.policies.contains_key(&policy_id) {
            return Err(PolicyError::NotFound);
        }
        check_policy_validation(&settings)?;
        let policy = self
            .policies
            .get_mut(&policy_id)
            .ok_or(PolicyError::NotFound)?;
        if policy.state != PolicyState::Public {
            return Err(PolicyError::InvalidState);
        }
        policy.settings = settings;
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(self.schema_version)
    }

    /// 推进删除状态机一步；到达 None 时从目录移除策略。
    pub fn drop_step(&mut self, policy_id: i64) -> Result<PolicyState, PolicyError> {
        self.check_not_in_use(policy_id)?;
        let policy = self
            .policies
            .get_mut(&policy_id)
            .ok_or(PolicyError::NotFound)?;
        let next = match policy.state {
            PolicyState::Public => PolicyState::WriteOnly,
            PolicyState::WriteOnly => PolicyState::DeleteOnly,
            PolicyState::DeleteOnly => PolicyState::None,
            PolicyState::None => return Err(PolicyError::InvalidState),
        };
        policy.state = next;
        // 最终态：清理 ID 与名称索引。
        if next == PolicyState::None {
            let policy = self.policies.remove(&policy_id).unwrap();
            self.by_name.remove(&policy.name.to_ascii_lowercase());
        }
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(next)
    }

    /// 按名称（大小写不敏感）查找策略。
    pub fn by_name(&self, name: &str) -> Option<&PolicyInfo> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|id| self.policies.get(id))
    }

    /// 将引用归一化为真实策略 ID；`default` 视为清除引用。
    pub fn normalize_ref(
        &self,
        reference: Option<PolicyRef>,
    ) -> Result<Option<PolicyRef>, PolicyError> {
        let Some(mut reference) = reference else {
            return Ok(None);
        };
        if reference.name.eq_ignore_ascii_case("default") {
            return Ok(None);
        }
        let policy = self.by_name(&reference.name).ok_or(PolicyError::NotFound)?;
        reference.id = policy.id;
        Ok(Some(reference))
    }

    /// 检查策略是否仍被库/表/分区或 range 规则引用。
    pub fn check_not_in_use(&self, policy_id: i64) -> Result<(), PolicyError> {
        let policy = self.policies.get(&policy_id).ok_or(PolicyError::NotFound)?;
        if self
            .databases
            .iter()
            .chain(&self.tables)
            .any(|object| object_uses_policy(object, policy_id))
        {
            return Err(PolicyError::InUse);
        }
        // 系统 key range 上的放置规则也可能引用该策略名。
        if self
            .range_policy_names
            .values()
            .any(|name| name.eq_ignore_ascii_case(&policy.name))
        {
            return Err(PolicyError::InUse);
        }
        Ok(())
    }

    /// 收集依赖该策略的库、分区、表 ID 列表（返回顺序：db、partition、table）。
    pub fn depended_object_ids(
        &self,
        policy_id: i64,
    ) -> Result<(Vec<i64>, Vec<i64>, Vec<i64>), PolicyError> {
        if !self.policies.contains_key(&policy_id) {
            return Err(PolicyError::NotFound);
        }
        let db_ids = self
            .databases
            .iter()
            .filter(|db| ref_matches(&db.policy_ref, policy_id))
            .map(|db| db.id)
            .collect();
        let table_ids = self
            .tables
            .iter()
            .filter(|table| ref_matches(&table.policy_ref, policy_id))
            .map(|table| table.id)
            .collect();
        let partition_ids = self
            .tables
            .iter()
            .flat_map(|table| &table.partitions)
            .filter(|part| ref_matches(&part.policy_ref, policy_id))
            .map(|part| part.id)
            .collect();
        Ok((db_ids, partition_ids, table_ids))
    }
}

/// 判断可选引用是否指向指定策略 ID。
fn ref_matches(reference: &Option<PolicyRef>, policy_id: i64) -> bool {
    reference
        .as_ref()
        .is_some_and(|reference| reference.id == policy_id)
}

/// 对象自身或其分区是否使用指定策略。
fn object_uses_policy(object: &PlacementObject, policy_id: i64) -> bool {
    ref_matches(&object.policy_ref, policy_id)
        || object
            .partitions
            .iter()
            .any(|part| ref_matches(&part.policy_ref, policy_id))
}

/// 校验放置配置合法性：followers/voters 互斥、主区域必须落在 regions、schedule 取值受限。
pub fn check_policy_validation(settings: &PlacementSettings) -> Result<(), PolicyError> {
    if settings.followers > 0 && settings.voters > 0 {
        return Err(PolicyError::InvalidSettings);
    }
    // primary_region 非空时，必须出现在 regions 列表中。
    if !settings.primary_region.is_empty()
        && settings
            .regions
            .split(',')
            .all(|region| region.trim() != settings.primary_region)
    {
        return Err(PolicyError::InvalidSettings);
    }
    if !settings.schedule.is_empty()
        && !matches!(settings.schedule.as_str(), "EVEN" | "MAJORITY_IN_PRIMARY")
    {
        return Err(PolicyError::InvalidSettings);
    }
    Ok(())
}

/// 根据选项列表构造初始状态为 None 的策略信息。
pub fn build_policy_info(
    id: i64,
    name: &str,
    options: &[(PlacementOptionType, String, u64)],
) -> Result<PolicyInfo, PolicyError> {
    let mut settings = PlacementSettings::default();
    for (kind, text, number) in options {
        set_direct_placement_opt(&mut settings, *kind, text, *number)?;
    }
    Ok(PolicyInfo {
        id,
        name: name.to_string(),
        state: PolicyState::None,
        settings,
    })
}

/// 按选项类型写入对应 settings 字段；schedule 统一转大写。
pub fn set_direct_placement_opt(
    settings: &mut PlacementSettings,
    kind: PlacementOptionType,
    text: &str,
    number: u64,
) -> Result<(), PolicyError> {
    match kind {
        PlacementOptionType::PrimaryRegion => settings.primary_region = text.to_string(),
        PlacementOptionType::Regions => settings.regions = text.to_string(),
        PlacementOptionType::FollowerCount => settings.followers = number,
        PlacementOptionType::VoterCount => settings.voters = number,
        PlacementOptionType::LearnerCount => settings.learners = number,
        PlacementOptionType::Schedule => settings.schedule = text.to_string(),
        PlacementOptionType::Constraints => settings.constraints = text.to_string(),
        PlacementOptionType::LeaderConstraints => settings.leader_constraints = text.to_string(),
        PlacementOptionType::LearnerConstraints => settings.learner_constraints = text.to_string(),
        PlacementOptionType::FollowerConstraints => {
            settings.follower_constraints = text.to_string()
        }
        PlacementOptionType::VoterConstraints => settings.voter_constraints = text.to_string(),
        PlacementOptionType::SurvivalPreferences => {
            settings.survival_preferences = text.to_string()
        }
    }
    Ok(())
}

/// 清除表及其分区上的策略引用，返回是否发生变更。
pub fn remove_table_placement(table: &mut PlacementObject) -> bool {
    let mut changed = table.policy_ref.take().is_some();
    for partition in &mut table.partitions {
        changed |= partition.policy_ref.take().is_some();
    }
    changed
}

/// 处理表级放置：ignore 时清除引用，否则归一化表与分区引用。
pub fn handle_table_placement(
    table: &mut PlacementObject,
    catalog: &PlacementPolicyCatalog,
    ignore: bool,
) -> Result<bool, PolicyError> {
    if ignore {
        return Ok(remove_table_placement(table));
    }
    table.policy_ref = catalog.normalize_ref(table.policy_ref.take())?;
    for partition in &mut table.partitions {
        partition.policy_ref = catalog.normalize_ref(partition.policy_ref.take())?;
    }
    Ok(false)
}

/// 从 PD range ID（形如 `policyName_rule_xxx`）提取策略名；无法解析则返回空串。
pub fn get_range_placement_policy_name(rule_id: Option<&str>) -> String {
    rule_id
        .and_then(|id| {
            id.rfind("_rule_")
                .filter(|position| *position > 0)
                .map(|position| id[..position].to_string())
        })
        .unwrap_or_default()
}

/// 收集对象及其分区上出现过的策略 ID 集合。
pub fn collect_policy_ids(objects: &[PlacementObject]) -> BTreeSet<i64> {
    objects
        .iter()
        .flat_map(|object| {
            object.policy_ref.iter().chain(
                object
                    .partitions
                    .iter()
                    .filter_map(|part| part.policy_ref.as_ref()),
            )
        })
        .map(|reference| reference.id)
        .collect()
}
