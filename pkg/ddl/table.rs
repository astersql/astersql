// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// tiflashCheckTiDBHTTPAPIHalfInterval 对应 Go 常量：TiFlash HTTP API 半间隔。
pub const tiflashCheckTiDBHTTPAPIHalfInterval: Duration = Duration::from_millis(2500);

// repairTableOrViewWithCheck 对应 Go 辅助函数：先校验 TableInfo，再更新到 meta。
pub fn repairTableOrViewWithCheck(
    t: &mut meta::Mutator,
    job: &mut model::Job,
    schemaID: i64,
    tbInfo: &mut model::TableInfo,
) -> Result<(), errors::Error> {
    if let Err(err) = checkTableInfoValid(tbInfo) {
        job.State = model::JobStateCancelled;
        return Err(errors::Trace(err));
    }
    t.UpdateTable(schemaID, tbInfo)
}

impl worker {
    // onDropTableOrView 对应 Go 的 drop table/view 状态机：public -> write only -> delete only -> none。
    pub fn onDropTableOrView(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        let mut ver = 0;
        let mut args = match model::GetDropTableArgs(job) {
            Ok(args) => args,
            Err(err) => {
                job.State = model::JobStateCancelled;
                return Err(errors::Trace(err));
            }
        };
        jobCtx.jobArgs = args.clone();
        let mut tblInfo = checkTableExistAndCancelNonExistJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;

        let originalState = job.SchemaState;
        match tblInfo.State {
            model::StatePublic => {
                if job.Type == model::ActionDropTable {
                    checkDropTableHasForeignKeyReferredInOwner(jobCtx.infoCache, job, &args)?;
                }
                tblInfo.State = model::StateWriteOnly;
                ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, originalState != tblInfo.State).map_err(errors::Trace)?;
            }
            model::StateWriteOnly => {
                tblInfo.State = model::StateDeleteOnly;
                ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, originalState != tblInfo.State).map_err(errors::Trace)?;
            }
            model::StateDeleteOnly => {
                tblInfo.State = model::StateNone;
                let oldIDs = getPartitionIDs(&tblInfo);
                let mut ruleIDs = getPartitionRuleIDs(jobCtx.store.GetCodec(), &job.SchemaName, &tblInfo);
                ruleIDs.push(label::NewRuleID(jobCtx.store.GetCodec(), &job.SchemaName, &tblInfo.Name.L, ""));
                args.OldPartitionIDs = oldIDs.clone();
                ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, originalState != tblInfo.State).map_err(errors::Trace)?;

                // Go 在 state none 后真正删除 table/view/sequence 元数据，并清理 auto id。
                if tblInfo.IsSequence() {
                    jobCtx.metaMut.DropSequence(job.SchemaID, job.TableID).map_err(errors::Trace)?;
                } else {
                    jobCtx.metaMut.DropTableOrView(job.SchemaID, job.TableID).map_err(errors::Trace)?;
                    jobCtx.metaMut.GetAutoIDAccessors(job.SchemaID, job.TableID).Del().map_err(errors::Trace)?;
                }
                if tblInfo.TiFlashReplica.is_some() {
                    if let Err(e) = infosync::DeleteTiFlashTableSyncProgress(&tblInfo) {
                        logutil::DDLLogger().Error("DeleteTiFlashTableSyncProgress fails", zap::Error(e));
                    }
                }
                // placement rule 延后到 GC worker，drop event 和 affinity/masking cleanup 仍按 Go 顺序保留。
                if !tblInfo.IsSequence() && !tblInfo.IsView() {
                    let dropTableEvent = notifier::NewDropTableEvent(&tblInfo);
                    asyncNotifyEvent(jobCtx, dropTableEvent, job, noSubJob, &self.sess).map_err(errors::Trace)?;
                }
                if let Err(err) = deleteTableAffinityGroupsInPD(jobCtx, &tblInfo, None) {
                    logutil::DDLLogger().Error("failed to delete affinity groups from PD", zap::Error(err), zap::Int64("tableID", tblInfo.ID));
                }
                self.dropMaskingPoliciesOnTable(jobCtx, tblInfo.ID)
                    .map_err(|err| errors::Wrapf(err, format!("failed to drop masking policies on table {}", tblInfo.ID)))?;
                job.FinishTableJob(model::JobStateDone, model::StateNone, ver, Some(tblInfo.clone()));
                let startKey = tablecodec::EncodeTablePrefix(job.TableID);
                job.FillFinishedArgs(&model::DropTableArgs { StartKey: startKey, OldPartitionIDs: oldIDs, OldRuleIDs: ruleIDs, ..Default::default() });
            }
            _ => return Err(errors::Trace(dbterror::ErrInvalidDDLState.GenWithStackByArgs("table", tblInfo.State))),
        }
        job.SchemaState = tblInfo.State;
        Ok(ver)
    }

    // onRecoverTable 对应 Go recover table 的两阶段流程：先记录 GC 状态，再恢复元数据并 finish。
    pub fn onRecoverTable(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        let mut ver = 0;
        let mut args = match model::GetRecoverArgs(job) {
            Ok(args) => args,
            Err(err) => {
                job.State = model::JobStateCancelled;
                return Err(errors::Trace(err));
            }
        };
        jobCtx.jobArgs = args.clone();
        let recoverInfo = args.RecoverTableInfos()[0].clone();
        let schemaID = recoverInfo.SchemaID;
        let mut tblInfo = recoverInfo.TableInfo.clone();
        if let Some(ttl) = tblInfo.TTLInfo.as_mut() {
            // 恢复表时强制关闭 TTL 调度，和 Go 的防护一致。
            ttl.Enable = false;
        }

        let gcEnable = checkGCEnable(self).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        checkTableNotExists(jobCtx.infoCache, schemaID, &tblInfo.Name.L).map_err(|err| {
            if infoschema::ErrDatabaseNotExists.Equal(&err) || infoschema::ErrTableExists.Equal(&err) {
                job.State = model::JobStateCancelled;
            }
            errors::Trace(err)
        })?;
        checkTableIDNotExists(jobCtx.metaMut, schemaID, tblInfo.ID).map_err(|err| {
            if infoschema::ErrDatabaseNotExists.Equal(&err) || infoschema::ErrTableExists.Equal(&err) {
                job.State = model::JobStateCancelled;
            }
            errors::Trace(err)
        })?;

        match tblInfo.State {
            model::StateNone => {
                // none -> write only：只记录 GC 检查结果，真正恢复留到下一轮。
                args.CheckFlag = if gcEnable { recoverCheckFlagEnableGC } else { recoverCheckFlagDisableGC };
                job.FillArgs(&args);
                job.SchemaState = model::StateWriteOnly;
                tblInfo.State = model::StateWriteOnly;
            }
            model::StateWriteOnly => {
                if gcEnable {
                    disableGC(self).map_err(|err| {
                        job.State = model::JobStateCancelled;
                        errors::Errorf(format!("disable gc failed, try again later. err: {}", err))
                    })?;
                }
                checkSafePoint(self, recoverInfo.SnapshotTS).map_err(|err| {
                    job.State = model::JobStateCancelled;
                    errors::Trace(err)
                })?;
                ver = self.recoverTable(jobCtx.stepCtx.clone(), jobCtx.metaMut, job, &recoverInfo).map_err(errors::Trace)?;
                let mut tableInfo = tblInfo.Clone();
                tableInfo.State = model::StatePublic;
                tableInfo.UpdateTS = jobCtx.metaMut.StartTS;
                args.AffectedPhysicalIDs = if recoverInfo.TableInfo.GetPartitionInfo().is_some() {
                    let mut tids = getPartitionIDs(&recoverInfo.TableInfo);
                    tids.push(recoverInfo.TableInfo.ID);
                    tids
                } else {
                    vec![recoverInfo.TableInfo.ID]
                };
                ver = updateVersionAndTableInfo(jobCtx, job, &mut tableInfo, true).map_err(errors::Trace)?;
                tblInfo.State = model::StatePublic;
                tblInfo.UpdateTS = jobCtx.metaMut.StartTS;
                job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
            }
            _ => return Err(dbterror::ErrInvalidDDLState.GenWithStackByArgs("table", tblInfo.State)),
        }
        Ok(ver)
    }

    // recoverTable 对应 Go 的内部恢复动作：移除 GC delete range、清 placement、重建 table/auto id、恢复 label rules。
    pub fn recoverTable(
        &mut self,
        ctx: context::Context,
        t: &mut meta::Mutator,
        job: &mut model::Job,
        recoverInfo: &model::RecoverTableInfo,
    ) -> Result<i64, errors::Error> {
        let ver = 0;
        let (tableRuleID, partRuleIDs, oldRuleIDs, oldRules) =
            getOldLabelRules(self.store.GetCodec(), &recoverInfo.TableInfo, &recoverInfo.OldSchemaName, &recoverInfo.OldTableName)
                .map_err(|err| {
                    job.State = model::JobStateCancelled;
                    errors::Wrapf(err, "failed to get old label rules from PD")
                })?;
        self.delRangeManager.removeFromGCDeleteRange(ctx.clone(), recoverInfo.DropJobID).map_err(errors::Trace)?;
        clearTablePlacementAndBundles(ctx, &mut recoverInfo.TableInfo.clone()).map_err(errors::Trace)?;

        let mut tableInfo = recoverInfo.TableInfo.Clone();
        tableInfo.State = model::StatePublic;
        tableInfo.UpdateTS = t.StartTS;
        t.CreateTableAndSetAutoID(recoverInfo.SchemaID, &mut tableInfo, recoverInfo.AutoIDs.clone()).map_err(errors::Trace)?;

        // Go failpoint mockRecoverTableCommitErr 只用于注入一次提交错误；保留注释，不执行注入。
        updateLabelRules(self.store.GetCodec(), &job.SchemaName, &recoverInfo.TableInfo, oldRules, tableRuleID, partRuleIDs, oldRuleIDs, recoverInfo.TableInfo.ID)
            .map_err(|err| {
                job.State = model::JobStateCancelled;
                errors::Wrapf(err, "failed to update the label rule to PD")
            })?;
        Ok(ver)
    }

    // onTruncateTable 对应 Go TRUNCATE TABLE：删除旧元数据，分配新 table/partition ID，重建 placement 和通知。
    pub fn onTruncateTable(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        let schemaID = job.SchemaID;
        let mut args = model::GetTruncateTableArgs(job).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        jobCtx.jobArgs = args.clone();
        let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, schemaID).map_err(errors::Trace)?;
        if tblInfo.IsView() || tblInfo.IsSequence() {
            job.State = model::JobStateCancelled;
            return Err(infoschema::ErrTableNotExists.GenWithStackByArgs(&job.SchemaName, &tblInfo.Name.O));
        }
        let oldTblInfo = tblInfo.Clone();
        checkTruncateTableHasForeignKeyReferredInOwner(jobCtx.infoCache, job, &tblInfo, args.FKCheck)?;
        jobCtx.metaMut.DropTableOrView(schemaID, tblInfo.ID).map_err(errors::Trace)?;
        jobCtx.metaMut.GetAutoIDAccessors(schemaID, tblInfo.ID).Del().map_err(errors::Trace)?;
        // Go failpoint truncateTableErr/mockTruncateTableUpdateVersionError 在这里只保留注入点语义。
        if tblInfo.TiFlashReplica.is_some() {
            if let Err(e) = infosync::DeleteTiFlashTableSyncProgress(&tblInfo) {
                logutil::DDLLogger().Error("DeleteTiFlashTableSyncProgress fails", zap::Error(e));
            }
        }
        let mut oldPartitionIDs = Vec::new();
        let mut newPartitionIDs = args.NewPartitionIDs.clone();
        if tblInfo.GetPartitionInfo().is_some() {
            oldPartitionIDs = getPartitionIDs(&tblInfo);
            newPartitionIDs = truncateTableByReassignPartitionIDs(jobCtx.metaMut, &mut tblInfo, newPartitionIDs).map_err(errors::Trace)?;
        }
        // 记录带 placement policy 的新旧 partition ID，供后续 finished args 使用。
        if let Some(pi) = tblInfo.GetPartitionInfo() {
            let mut oldIDs = Vec::new();
            let mut newIDs = Vec::new();
            for (i, oldID) in oldPartitionIDs.iter().enumerate() {
                let newDef = &pi.Definitions[i];
                if newDef.PlacementPolicyRef.is_some() {
                    oldIDs.push(*oldID);
                    newIDs.push(newDef.ID);
                }
            }
            args.OldPartIDsWithPolicy = oldIDs;
            args.NewPartIDsWithPolicy = newIDs;
        }

        let (tableRuleID, partRuleIDs, _, oldRules) =
            getOldLabelRules(jobCtx.store.GetCodec(), &tblInfo, &job.SchemaName, &tblInfo.Name.L)
                .map_err(|err| {
                    job.State = model::JobStateCancelled;
                    errors::Wrapf(err, "failed to get old label rules from PD")
                })?;
        updateLabelRules(jobCtx.store.GetCodec(), &job.SchemaName, &tblInfo, oldRules, tableRuleID, partRuleIDs, vec![], args.NewTableID)
            .map_err(|err| {
                job.State = model::JobStateCancelled;
                errors::Wrapf(err, "failed to update the label rule to PD")
            })?;

        // TiFlash replica、masking policy、placement bundle、pre-split、affinity 和异步事件按 Go 顺序保留。
        if tblInfo.TiFlashReplica.is_some() {
            tblInfo.TiFlashReplica.AvailablePartitionIDs = None;
            tblInfo.TiFlashReplica.Available = false;
        }
        tblInfo.ID = args.NewTableID;
        self.updateMaskingPolicyTableIDAfterTruncate(jobCtx, oldTblInfo.ID, tblInfo.ID).map_err(errors::Trace)?;
        let bundles = placement::NewFullTableBundles(jobCtx.metaMut, &tblInfo).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        infosync::PutRuleBundlesWithDefaultRetry(context::TODO(), bundles).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Wrapf(err, "failed to notify PD the placement rules")
        })?;
        jobCtx.metaMut.CreateTableOrView(schemaID, &mut tblInfo).map_err(errors::Trace)?;
        let scatterScope = job.GetSystemVars(vardef::TiDBScatterRegion).unwrap_or_default();
        preSplitAndScatterTable(self.sess.Context.clone(), jobCtx.store, &tblInfo, scatterScope);
        if tblInfo.Affinity.is_some() {
            createTableAffinityGroupsInPD(jobCtx, &tblInfo).map_err(errors::Trace)?;
        }
        if oldTblInfo.Affinity.is_some() {
            if let Err(err) = deleteTableAffinityGroupsInPD(jobCtx, &oldTblInfo, None) {
                logutil::DDLLogger().Error("failed to delete old affinity groups from PD", zap::Error(err), zap::Int64("tableID", oldTblInfo.ID));
            }
        }
        let ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
        let truncateTableEvent = notifier::NewTruncateTableEvent(&tblInfo, &oldTblInfo);
        asyncNotifyEvent(jobCtx, truncateTableEvent, job, noSubJob, &self.sess).map_err(errors::Trace)?;
        job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
        args.OldPartitionIDs = oldPartitionIDs;
        args.NewPartitionIDs = newPartitionIDs;
        job.FillFinishedArgs(&args);
        Ok(ver)
    }

    // onShardRowID 对应 Go 修改 shard_row_id_bits：减小直接改，增大需先校验 auto ID 溢出。
    pub fn onShardRowID(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        let args = model::GetShardRowIDArgs(job).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        let shardRowIDBits = args.ShardRowIDBits;
        let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
        if shardRowIDBits < tblInfo.ShardRowIDBits {
            tblInfo.ShardRowIDBits = shardRowIDBits;
        } else {
            let tbl = getTable(jobCtx.getAutoIDRequirement(), job.SchemaID, &tblInfo).map_err(errors::Trace)?;
            verifyNoOverflowShardBits(&mut self.sessPool, tbl, shardRowIDBits).map_err(|err| {
                job.State = model::JobStateCancelled;
                err
            })?;
            tblInfo.ShardRowIDBits = shardRowIDBits;
            tblInfo.MaxShardRowIDBits = shardRowIDBits;
        }
        let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true).map_err(errors::Trace)?;
        job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
        Ok(ver)
    }

    // onRenameTable 对应 Go 单表 rename，两阶段提交以满足 TiCDC 对 job state 和 schema reload 的要求。
    pub fn onRenameTable(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        let args = model::GetRenameTableArgs(job).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        jobCtx.jobArgs = args.clone();
        if job.SchemaState == model::StatePublic {
            return finishJobRenameTable(jobCtx, job);
        }
        checkTableNotExists(jobCtx.infoCache, job.SchemaID, &args.NewTableName.L).map_err(errors::Trace)?;
        let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, args.OldSchemaID).map_err(errors::Trace)?;
        let oldTableName = tblInfo.Name.clone();
        let mut ver = checkAndRenameTables(jobCtx, jobCtx.metaMut, job, &mut tblInfo, &args).map_err(errors::Trace)?;
        let mut fkh = newForeignKeyHelper();
        adjustForeignKeyChildTableInfoAfterRenameTable(jobCtx.infoCache, jobCtx.metaMut, job, &mut fkh, &mut tblInfo, args.OldSchemaName, oldTableName, args.NewTableName, job.SchemaID)?;
        let is = jobCtx.infoCache.GetLatest();
        let newDB = is.SchemaByID(job.SchemaID).ok_or_else(|| {
            job.State = model::JobStateCancelled;
            infoschema::ErrDatabaseNotExists.GenWithStackByArgs(format!("schema-ID: {}", job.SchemaID))
        })?;
        self.updateMaskingPolicyNamesAfterRename(jobCtx.stepCtx.clone(), tblInfo.ID, args.OldSchemaName, newDB.Name, oldTableName, args.NewTableName)
            .map_err(|err| errors::Wrapf(err, "failed to update masking policy names after table rename"))?;
        ver = updateSchemaVersion(jobCtx, job, fkh.getLoadedTables()).map_err(errors::Trace)?;
        job.SchemaState = model::StatePublic;
        Ok(ver)
    }

    // onRenameTables 对应 Go 批量 rename：逐表改名、修正外键子表、更新 masking policy，最后统一更新 schema version。
    pub fn onRenameTables(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        let args = model::GetRenameTablesArgs(job).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        jobCtx.jobArgs = args.clone();
        if job.SchemaState == model::StatePublic {
            return finishJobRenameTables(jobCtx, job);
        }
        let mut fkh = newForeignKeyHelper();
        let is = jobCtx.infoCache.GetLatest();
        for info in &args.RenameTableInfos {
            job.TableID = info.TableID;
            job.TableName = info.OldTableName.L.clone();
            let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, info.OldSchemaID).map_err(errors::Trace)?;
            checkAndRenameTables(jobCtx, jobCtx.metaMut, job, &mut tblInfo, info).map_err(errors::Trace)?;
            adjustForeignKeyChildTableInfoAfterRenameTable(jobCtx.infoCache, jobCtx.metaMut, job, &mut fkh, &mut tblInfo, info.OldSchemaName, info.OldTableName, info.NewTableName, info.NewSchemaID)?;
            let newDB = is.SchemaByID(info.NewSchemaID).ok_or_else(|| {
                job.State = model::JobStateCancelled;
                infoschema::ErrDatabaseNotExists.GenWithStackByArgs(format!("schema-ID: {}", info.NewSchemaID))
            })?;
            self.updateMaskingPolicyNamesAfterRename(jobCtx.stepCtx.clone(), tblInfo.ID, info.OldSchemaName, newDB.Name, info.OldTableName, info.NewTableName)
                .map_err(|err| errors::Wrapf(err, "failed to update masking policy names after table rename"))?;
        }
        let ver = updateSchemaVersion(jobCtx, job, fkh.getLoadedTables()).map_err(errors::Trace)?;
        job.SchemaState = model::StatePublic;
        Ok(ver)
    }

    // onSetTableFlashReplica 对应 Go 设置 TiFlash 副本数：先更新 PD rule，再修改 TableInfo。
    pub fn onSetTableFlashReplica(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        let args = model::GetSetTiFlashReplicaArgs(job).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        let replicaInfo = args.TiflashReplica;
        let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
        if metadef::IsMemOrSysDB(&job.SchemaName) {
            return Err(errors::Trace(dbterror::ErrUnsupportedTiFlashOperationForSysOrMemTable));
        }
        self.checkTiFlashReplicaCount(replicaInfo.Count).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        if let Some(pi) = tblInfo.GetPartitionInfo() {
            logutil::DDLLogger().Info("Set TiFlash replica pd rule for partitioned table", zap::Int64("tableID", tblInfo.ID));
            infosync::ConfigureTiFlashPDForPartitions(false, &pi.Definitions, replicaInfo.Count, &replicaInfo.Labels, tblInfo.ID).map_err(errors::Trace)?;
            infosync::ConfigureTiFlashPDForPartitions(true, &pi.AddingDefinitions, replicaInfo.Count, &replicaInfo.Labels, tblInfo.ID).map_err(errors::Trace)?;
        } else {
            logutil::DDLLogger().Info("Set TiFlash replica pd rule", zap::Int64("tableID", tblInfo.ID));
            infosync::ConfigureTiFlashPDForTable(tblInfo.ID, replicaInfo.Count, &replicaInfo.Labels).map_err(errors::Trace)?;
        }
        if replicaInfo.Count > 0 {
            let available = if args.ResetAvailable { false } else { tblInfo.TiFlashReplica.as_ref().map(|r| r.Available).unwrap_or(false) };
            tblInfo.TiFlashReplica = Some(model::TiFlashReplicaInfo { Count: replicaInfo.Count, LocationLabels: replicaInfo.Labels, Available: available, ..Default::default() });
        } else {
            if tblInfo.TiFlashReplica.is_some() {
                if let Err(err) = infosync::DeleteTiFlashTableSyncProgress(&tblInfo) {
                    logutil::DDLLogger().Error("DeleteTiFlashTableSyncProgress fails", zap::Error(err));
                }
            }
            tblInfo.TiFlashReplica = None;
        }
        let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true).map_err(errors::Trace)?;
        job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
        Ok(ver)
    }

    // checkTiFlashReplicaCount 对应 Go worker 方法：从 session pool 借 session 后校验 TiFlash store 数量。
    pub fn checkTiFlashReplicaCount(&mut self, replicaCount: u64) -> Result<(), errors::Error> {
        let ctx = self.sessPool.Get().map_err(errors::Trace)?;
        let result = checkTiFlashReplicaCount(ctx, replicaCount);
        self.sessPool.Put(ctx);
        result
    }

    // onAlterTableSetRegionSplitPolicy 对应 Go 设置表/索引 region split policy，并触发预切分。
    pub fn onAlterTableSetRegionSplitPolicy(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        let args = model::GetAlterTableSetRegionSplitPolicyArgs(job).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID)?;
        if args.IndexName.is_empty() {
            // Table-level split (SPLIT BETWEEN)
            tblInfo.TableSplitPolicy = args.Policy.Clone();
        } else {
            // Index split (SPLIT INDEX idx BETWEEN)
            let indexInfo = tblInfo.FindIndexByName(strings::ToLower(&args.IndexName)).ok_or_else(|| {
                job.State = model::JobStateCancelled;
                infoschema::ErrKeyNotExists.GenWithStackByArgs(&args.IndexName, &tblInfo.Name.O)
            })?;
            indexInfo.RegionSplitPolicy = args.Policy.Clone();
        }
        let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true).map_err(errors::Trace)?;
        let scatterScope = job.GetSystemVars(vardef::TiDBScatterRegion).unwrap_or(vardef::ScatterOff);
        preSplitAndScatterTable(self.sess.Context.clone(), jobCtx.store, &tblInfo, scatterScope);
        job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
        Ok(ver)
    }
}

// clearTablePlacementAndBundles 对应 Go：清除表和分区上的 placement policy ref，并把空 bundle 写回 PD。
pub fn clearTablePlacementAndBundles(ctx: context::Context, tblInfo: &mut model::TableInfo) -> Result<(), errors::Error> {
    // Go failpoint mockClearTablePlacementAndBundlesErr 在中只保留注释。
    let mut bundles: Vec<placement::Bundle> = Vec::new();
    if tblInfo.PlacementPolicyRef.is_some() {
        tblInfo.PlacementPolicyRef = None;
        bundles.push(placement::NewBundle(tblInfo.ID));
    }
    if let Some(partition) = tblInfo.Partition.as_mut() {
        for par in &mut partition.Definitions {
            if par.PlacementPolicyRef.is_some() {
                par.PlacementPolicyRef = None;
                bundles.push(placement::NewBundle(par.ID));
            }
        }
    }
    if bundles.is_empty() {
        return Ok(());
    }
    infosync::PutRuleBundlesWithDefaultRetry(ctx, bundles)
}

// mockRecoverTableCommitErrOnce 对应 Go 包级原子变量，确保 mockRecoverTableCommitErr 只注入一次。
pub static mockRecoverTableCommitErrOnce: AtomicU32 = AtomicU32::new(0);

// enableGC 对应 Go：借 session 调用 gcutil.EnableGC，并在结束时归还 session。
pub fn enableGC(w: &mut worker) -> Result<(), errors::Error> {
    let ctx = w.sessPool.Get().map_err(errors::Trace)?;
    let result = gcutil::EnableGC(ctx);
    w.sessPool.Put(ctx);
    result
}

// disableGC 对应 Go：借 session 调用 gcutil.DisableGC。
pub fn disableGC(w: &mut worker) -> Result<(), errors::Error> {
    let ctx = w.sessPool.Get().map_err(errors::Trace)?;
    let result = gcutil::DisableGC(ctx);
    w.sessPool.Put(ctx);
    result
}

// checkGCEnable 对应 Go：读取 GC 是否开启，session defer 归还改为显式收尾。
pub fn checkGCEnable(w: &mut worker) -> Result<bool, errors::Error> {
    let ctx = w.sessPool.Get().map_err(errors::Trace)?;
    let result = gcutil::CheckGCEnable(ctx);
    w.sessPool.Put(ctx);
    result
}

// checkSafePoint 对应 Go：校验 snapshotTS 是否仍可读。
pub fn checkSafePoint(w: &mut worker, snapshotTS: u64) -> Result<(), errors::Error> {
    let ctx = w.sessPool.Get().map_err(errors::Trace)?;
    let result = gcutil::ValidateSnapshot(ctx, snapshotTS);
    w.sessPool.Put(ctx);
    result
}

// getTable 对应 Go：从 TableInfo 创建 autoid allocators，再构造 table.Table。
pub fn getTable(r: autoid::Requirement, schemaID: i64, tblInfo: &model::TableInfo) -> Result<table::Table, errors::Error> {
    let allocs = autoid::NewAllocatorsFromTblInfo(r, schemaID, tblInfo);
    table::TableFromMeta(allocs, tblInfo).map_err(errors::Trace)
}

// GetTableInfoAndCancelFaultJob 对应 Go 导出函数：测试使用，要求表存在且状态为 public。
pub fn GetTableInfoAndCancelFaultJob(t: &mut meta::Mutator, job: &mut model::Job, schemaID: i64) -> Result<model::TableInfo, errors::Error> {
    let tblInfo = checkTableExistAndCancelNonExistJob(t, job, schemaID).map_err(errors::Trace)?;
    if tblInfo.State != model::StatePublic {
        job.State = model::JobStateCancelled;
        return Err(dbterror::ErrInvalidDDLState.GenWithStack(format!("table {} is not in public, but {}", tblInfo.Name, tblInfo.State)));
    }
    Ok(tblInfo)
}

// checkTableExistAndCancelNonExistJob 对应 Go：表不存在或库不存在时取消 job。
pub fn checkTableExistAndCancelNonExistJob(t: &mut meta::Mutator, job: &mut model::Job, schemaID: i64) -> Result<model::TableInfo, errors::Error> {
    match getTableInfo(t, job.TableID, schemaID) {
        Ok(tblInfo) => {
            if !job.TableName.is_empty() && tblInfo.Name.L != job.TableName && job.Type != model::ActionRepairTable {
                job.State = model::JobStateCancelled;
                return Err(infoschema::ErrTableNotExists.GenWithStackByArgs(&job.SchemaName, &job.TableName));
            }
            Ok(tblInfo)
        }
        Err(err) => {
            if infoschema::ErrDatabaseNotExists.Equal(&err) || infoschema::ErrTableNotExists.Equal(&err) {
                job.State = model::JobStateCancelled;
            }
            Err(err)
        }
    }
}

// getTableInfo 对应 Go：先查 schema 下的 table，再把 meta 层错误转换为 infoschema 错误。
pub fn getTableInfo(t: &mut meta::Mutator, tableID: i64, schemaID: i64) -> Result<model::TableInfo, errors::Error> {
    let tblInfo = match t.GetTable(schemaID, tableID) {
        Ok(info) => info,
        Err(err) => {
            if meta::ErrDBNotExists.Equal(&err) {
                return Err(errors::Trace(infoschema::ErrDatabaseNotExists.GenWithStackByArgs(format!("(Schema ID {})", schemaID))));
            }
            return Err(errors::Trace(err));
        }
    };
    tblInfo.ok_or_else(|| errors::Trace(infoschema::ErrTableNotExists.GenWithStackByArgs(format!("(Schema ID {})", schemaID), format!("(Table ID {})", tableID))))
}

// onRebaseAutoIncrementIDType 对应 Go wrapper：重置 AUTO_INCREMENT。
pub fn onRebaseAutoIncrementIDType(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    onRebaseAutoID(jobCtx, job, autoid::AutoIncrementType)
}

// onRebaseAutoRandomType 对应 Go wrapper：重置 AUTO_RANDOM。
pub fn onRebaseAutoRandomType(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    onRebaseAutoID(jobCtx, job, autoid::AutoRandomType)
}

// onRebaseAutoID 对应 Go：按 force 参数调整 newBase，并同步 TableInfo 与 allocator。
pub fn onRebaseAutoID(jobCtx: &mut jobContext, job: &mut model::Job, tp: autoid::AllocatorType) -> Result<i64, errors::Error> {
    let args = model::GetRebaseAutoIDArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut newBase = args.NewBase;
    let force = args.Force;
    if job.MultiSchemaInfo.is_some() && job.MultiSchemaInfo.Revertible {
        job.MarkNonRevertible();
        return Ok(0);
    }
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    let tbl = getTable(jobCtx.getAutoIDRequirement(), job.SchemaID, &tblInfo).map_err(errors::Trace)?;
    if !force {
        let newBaseTemp = adjustNewBaseToNextGlobalID(None, &tbl, tp, newBase).map_err(errors::Trace)?;
        if newBase != newBaseTemp {
            job.Warning = toTError(fmt::Errorf(format!("Can't reset AUTO_INCREMENT to {} without FORCE option, using {} instead", newBase, newBaseTemp)));
        }
        newBase = newBaseTemp;
    }
    if tp == autoid::AutoIncrementType {
        tblInfo.AutoIncID = newBase;
    } else {
        tblInfo.AutoRandID = newBase;
    }
    if let Some(alloc) = tbl.Allocators(None).Get(tp) {
        let newEnd = newBase - 1;
        let err = if force { alloc.ForceRebase(newEnd) } else { alloc.Rebase(context::Background(), newEnd, false) };
        if let Err(err) = err {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    }
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}
*/

// 表级 DDL 状态机与目录操作的可运行简化模型。
//
// 覆盖建表/删表（Public → WriteOnly → DeleteOnly → None 多阶段 schema 状态）、
// 恢复、截断、跨库重命名、外键引用修正，以及 auto_increment / TiFlash 副本 /
// placement / affinity / Region split policy 等表属性变更。
// 文件中的大块注释保留了 Go 侧完整 worker 流程的迁移草稿。

use std::collections::{BTreeMap, BTreeSet};

/// 表在在线 DDL 中的 schema 可见性状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableState {
    /// 对所有读写公开可见。
    Public,
    /// 仅写入阶段：旧数据只读，新写入走新 schema。
    WriteOnly,
    /// 仅删除阶段：只允许删除操作看到新 schema。
    DeleteOnly,
    /// 元数据已删除，表不再可见。
    None,
}

/// 外键引用的目标库表名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignKeyReference {
    pub schema: String,
    pub table: String,
}

/// TiFlash 列存副本配置：副本数、位置标签与已就绪的物理分区。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiFlashReplica {
    pub count: u64,
    pub location_labels: Vec<String>,
    pub available_partition_ids: BTreeSet<i64>,
}

/// 简化的表元信息，对应 Go `model.TableInfo` 中与表 DDL 相关的字段子集。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    pub id: i64,
    pub schema_id: i64,
    pub name: String,
    pub state: TableState,
    pub partition_ids: Vec<i64>,
    pub auto_increment_id: i64,
    pub auto_random_id: i64,
    pub auto_id_cache: u64,
    /// The original schema that still owns this table's auto-ID allocator.
    ///
    /// Go leaves this as zero while a table stays in its creation schema. A
    /// cross-schema rename records that schema ID, and moving back clears it.
    /// 仍持有本表 auto-ID 分配器的原始 schema；跨库 rename 时记录，迁回则清零。
    pub auto_id_schema_id: i64,
    pub shard_row_id_bits: u8,
    pub max_shard_row_id_bits: u8,
    pub comment: String,
    pub charset: String,
    pub collation: String,
    pub version: u64,
    pub foreign_keys: Vec<ForeignKeyReference>,
    pub tiflash_replica: Option<TiFlashReplica>,
    pub placement_policy: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub cached: bool,
    pub affinity: Option<String>,
    pub split_policy: Option<String>,
}

/// 表目录与属性变更过程中的错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableError {
    AlreadyExists,
    NotFound,
    SchemaNotFound,
    NameTooLong,
    RecoveryConflict,
    InvalidAutoId,
    ShardBitsOverflow,
    InvalidCharsetCollation,
    InvalidReplicaCount,
    PartitionNotFound,
    InvalidVersion,
    InvalidPlacement,
    InvalidAffinity,
    GcSafePointTooNew,
}

/// 重命名语义：`RENAME TABLE` 与 `ALTER TABLE ... RENAME` 的校验差异。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameMode {
    RenameTable,
    AlterTable,
}

impl std::fmt::Display for TableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for TableError {}

/// 已删除但仍可被 recover 的表快照及其 drop 时间戳。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedTable {
    pub table: TableInfo,
    pub drop_ts: u64,
}

/// GC（垃圾回收，清理 MVCC 历史版本）开关与 safe point 控制器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GcController {
    pub enabled: bool,
    pub safe_point: u64,
}

impl GcController {
    /// 恢复前校验 snapshot 不早于 safe point，并临时关闭 GC；返回先前是否开启。
    pub fn disable_for_recovery(&mut self, snapshot_ts: u64) -> Result<bool, TableError> {
        if snapshot_ts < self.safe_point {
            return Err(TableError::GcSafePointTooNew);
        }
        let was_enabled = self.enabled;
        self.enabled = false;
        Ok(was_enabled)
    }
    /// 按恢复前记录恢复 GC 开关。
    pub fn restore(&mut self, was_enabled: bool) {
        self.enabled = was_enabled;
    }
}

/// 内存表目录：按 schema 存放表，并保留已删除表供 recover。
#[derive(Clone, Default)]
pub struct TableCatalog {
    schemas: BTreeMap<i64, BTreeMap<String, TableInfo>>,
    dropped: BTreeMap<i64, DroppedTable>,
}

impl TableCatalog {
    /// 创建空 schema；已存在则返回 false。
    pub fn create_schema(&mut self, schema_id: i64) -> bool {
        if self.schemas.contains_key(&schema_id) {
            return false;
        }
        self.schemas.insert(schema_id, BTreeMap::new());
        true
    }

    /// 删除 schema 并返回其中所有表。
    pub fn drop_schema(&mut self, schema_id: i64) -> Result<Vec<TableInfo>, TableError> {
        self.schemas
            .remove(&schema_id)
            .map(|tables| tables.into_values().collect())
            .ok_or(TableError::SchemaNotFound)
    }

    /// 判断 schema 是否存在。
    pub fn schema_exists(&self, schema_id: i64) -> bool {
        self.schemas.contains_key(&schema_id)
    }

    /// 列出 schema 下所有表名。
    pub fn table_names(&self, schema_id: i64) -> Result<Vec<String>, TableError> {
        let schema = self
            .schemas
            .get(&schema_id)
            .ok_or(TableError::SchemaNotFound)?;
        Ok(schema.values().map(|table| table.name.clone()).collect())
    }

    /// 插入表；同名已存在则报 AlreadyExists。
    pub fn insert(&mut self, table: TableInfo) -> Result<(), TableError> {
        let schema = self.schemas.entry(table.schema_id).or_default();
        let key = table.name.to_ascii_lowercase();
        if schema.contains_key(&key) {
            return Err(TableError::AlreadyExists);
        }
        schema.insert(key, table);
        Ok(())
    }

    /// 按 schema 与表名查找表（大小写不敏感）。
    pub fn get(&self, schema_id: i64, name: &str) -> Result<&TableInfo, TableError> {
        self.schemas
            .get(&schema_id)
            .and_then(|schema| schema.get(&name.to_ascii_lowercase()))
            .ok_or(TableError::NotFound)
    }

    /// 按 schema 与表名可变查找表。
    pub fn get_mut(&mut self, schema_id: i64, name: &str) -> Result<&mut TableInfo, TableError> {
        self.schemas
            .get_mut(&schema_id)
            .and_then(|schema| schema.get_mut(&name.to_ascii_lowercase()))
            .ok_or(TableError::NotFound)
    }

    /// 推进删表状态机一步；到达 None 时移入 dropped 并记录 drop_ts。
    pub fn drop_table_step(
        &mut self,
        schema_id: i64,
        name: &str,
        drop_ts: u64,
    ) -> Result<TableState, TableError> {
        let key = name.to_ascii_lowercase();
        let table = self
            .schemas
            .get_mut(&schema_id)
            .and_then(|schema| schema.get_mut(&key))
            .ok_or(TableError::NotFound)?;
        // Public → WriteOnly → DeleteOnly → None，与在线 DDL 删表状态机一致。
        let next = match table.state {
            TableState::Public => TableState::WriteOnly,
            TableState::WriteOnly => TableState::DeleteOnly,
            TableState::DeleteOnly | TableState::None => TableState::None,
        };
        table.state = next;
        if next == TableState::None {
            let table = self
                .schemas
                .get_mut(&schema_id)
                .unwrap()
                .remove(&key)
                .unwrap();
            self.dropped
                .insert(table.id, DroppedTable { table, drop_ts });
        }
        Ok(next)
    }

    /// 从 dropped 恢复表：临时关 GC，冲突时回滚 GC 状态。
    pub fn recover_table(
        &mut self,
        table_id: i64,
        gc: &mut GcController,
    ) -> Result<bool, TableError> {
        let dropped = self
            .dropped
            .get(&table_id)
            .ok_or(TableError::NotFound)?
            .clone();
        let was_enabled = gc.disable_for_recovery(dropped.drop_ts)?;
        let schema = self.schemas.entry(dropped.table.schema_id).or_default();
        let key = dropped.table.name.to_ascii_lowercase();
        // 目标名已存在则恢复失败，并还原 GC 开关。
        if schema.contains_key(&key) {
            gc.restore(was_enabled);
            return Err(TableError::RecoveryConflict);
        }
        let mut table = self.dropped.remove(&table_id).unwrap().table;
        table.state = TableState::Public;
        schema.insert(key, table);
        gc.restore(was_enabled);
        Ok(true)
    }

    /// 截断表：换新 table/partition ID，重置 auto ID 与 TiFlash 可用分区。
    pub fn truncate_table(
        &mut self,
        schema_id: i64,
        name: &str,
        new_table_id: i64,
        new_partition_ids: Vec<i64>,
    ) -> Result<Vec<i64>, TableError> {
        let table = self.get_mut(schema_id, name)?;
        let mut old_ids = vec![table.id];
        old_ids.extend(table.partition_ids.iter().copied());
        table.id = new_table_id;
        table.partition_ids = new_partition_ids;
        table.auto_increment_id = 0;
        table.auto_random_id = 0;
        if let Some(replica) = table.tiflash_replica.as_mut() {
            replica.available_partition_ids.clear();
        }
        table.version = table.version.saturating_add(1);
        Ok(old_ids)
    }

    /// 无额外校验的单表重命名（内部直接调用 `rename_table_inner`）。
    pub fn rename_table(
        &mut self,
        old_schema: i64,
        old_name: &str,
        new_schema: i64,
        new_name: &str,
    ) -> Result<(), TableError> {
        self.rename_table_inner(old_schema, old_name, new_schema, new_name)
    }

    /// 按 RenameMode 校验后再重命名；AlterTable 允许同库同名仅改大小写。
    pub fn rename_table_checked(
        &mut self,
        mode: RenameMode,
        old_schema: i64,
        old_name: &str,
        new_schema: i64,
        new_name: &str,
    ) -> Result<(), TableError> {
        if new_name.chars().count() > 64 {
            return Err(TableError::NameTooLong);
        }
        let old_key = old_name.to_ascii_lowercase();
        let new_key = new_name.to_ascii_lowercase();

        match mode {
            RenameMode::RenameTable => {
                if self
                    .schemas
                    .get(&new_schema)
                    .is_some_and(|destination| destination.contains_key(&new_key))
                {
                    return Err(TableError::AlreadyExists);
                }
                let source_exists = self
                    .schemas
                    .get(&old_schema)
                    .is_some_and(|schema| schema.contains_key(&old_key));
                if !source_exists {
                    return Err(TableError::NotFound);
                }
                if !self.schemas.contains_key(&new_schema) {
                    return Err(TableError::SchemaNotFound);
                }
            }
            RenameMode::AlterTable => {
                if !self
                    .schemas
                    .get(&old_schema)
                    .is_some_and(|schema| schema.contains_key(&old_key))
                {
                    return Err(TableError::NotFound);
                }
                let destination = self
                    .schemas
                    .get(&new_schema)
                    .ok_or(TableError::SchemaNotFound)?;
                // 同库仅大小写变化：就地改名并 bump version。
                if old_schema == new_schema && old_key == new_key {
                    let table = self.get_mut(old_schema, old_name)?;
                    table.name = new_name.to_owned();
                    table.version = table.version.saturating_add(1);
                    return Ok(());
                }
                if destination.contains_key(&new_key) {
                    return Err(TableError::AlreadyExists);
                }
            }
        }
        self.rename_table_inner(old_schema, old_name, new_schema, new_name)
    }

    /// 真正执行搬迁：维护 auto_id_schema_id，并修正其它表上的外键引用表名。
    fn rename_table_inner(
        &mut self,
        old_schema: i64,
        old_name: &str,
        new_schema: i64,
        new_name: &str,
    ) -> Result<(), TableError> {
        let old_key = old_name.to_ascii_lowercase();
        let new_key = new_name.to_ascii_lowercase();
        if self
            .schemas
            .get(&new_schema)
            .is_some_and(|schema| schema.contains_key(&new_key))
        {
            return Err(TableError::AlreadyExists);
        }
        let mut table = self
            .schemas
            .get_mut(&old_schema)
            .and_then(|schema| schema.remove(&old_key))
            .ok_or(TableError::NotFound)?;
        let old_table_name = table.name.clone();
        // 首次跨库搬走时记录原 schema 作为 auto-ID 归属；迁回则清零。
        if table.auto_id_schema_id == 0 && new_schema != old_schema {
            table.auto_id_schema_id = old_schema;
        }
        if new_schema == table.auto_id_schema_id {
            table.auto_id_schema_id = 0;
        }
        table.schema_id = new_schema;
        table.name = new_name.to_string();
        table.version = table.version.saturating_add(1);
        self.schemas
            .entry(new_schema)
            .or_default()
            .insert(new_key, table);
        // 同步更新其它表外键中对本表旧名的引用。
        for schema in self.schemas.values_mut() {
            for dependent in schema.values_mut() {
                for reference in &mut dependent.foreign_keys {
                    if reference.table.eq_ignore_ascii_case(&old_table_name) {
                        reference.table = new_name.to_string();
                    }
                }
            }
        }
        Ok(())
    }

    /// 批量重命名：先在克隆目录上试跑，成功后再提交，保证原子性。
    pub fn rename_tables(
        &mut self,
        renames: &[(i64, String, i64, String)],
    ) -> Result<(), TableError> {
        let mut staged = self.clone();
        staged.rename_tables_inner(renames)?;
        *self = staged;
        Ok(())
    }

    /// 批量重命名前校验源存在、目标 schema 存在且新名长度合法。
    pub fn rename_tables_checked(
        &mut self,
        renames: &[(i64, String, i64, String)],
    ) -> Result<(), TableError> {
        for (old_schema, old_name, _, _) in renames {
            if !self
                .schemas
                .get(old_schema)
                .is_some_and(|schema| schema.contains_key(&old_name.to_ascii_lowercase()))
            {
                return Err(TableError::NotFound);
            }
        }
        for (_, _, new_schema, new_name) in renames {
            if new_name.chars().count() > 64 {
                return Err(TableError::NameTooLong);
            }
            if !self.schema_exists(*new_schema) {
                return Err(TableError::SchemaNotFound);
            }
        }
        self.rename_tables(renames)
    }

    /// 批量搬迁实现：先全部取出再写回，并处理循环换名与外键引用。
    fn rename_tables_inner(
        &mut self,
        renames: &[(i64, String, i64, String)],
    ) -> Result<(), TableError> {
        let destinations: BTreeSet<(i64, String)> = renames
            .iter()
            .map(|(_, _, schema, name)| (*schema, name.to_ascii_lowercase()))
            .collect();
        // 目标 (schema, name) 不可重复。
        if destinations.len() != renames.len() {
            return Err(TableError::AlreadyExists);
        }
        for (_, _, schema, name) in renames {
            let is_source = renames.iter().any(|(source_schema, source_name, _, _)| {
                source_schema == schema && source_name.eq_ignore_ascii_case(name)
            });
            // 目标名若不是某次 rename 的源，则目录中不能已占用。
            if !is_source
                && self
                    .schemas
                    .get(schema)
                    .is_some_and(|tables| tables.contains_key(&name.to_ascii_lowercase()))
            {
                return Err(TableError::AlreadyExists);
            }
        }
        let mut moved = Vec::new();
        for (schema_id, name, _, _) in renames {
            let table = self
                .schemas
                .get_mut(schema_id)
                .and_then(|tables| tables.remove(&name.to_ascii_lowercase()))
                .ok_or(TableError::NotFound)?;
            moved.push(table);
        }
        for (mut table, (_, old_name, new_schema, new_name)) in moved.into_iter().zip(renames) {
            for schema in self.schemas.values_mut() {
                for dependent in schema.values_mut() {
                    for reference in &mut dependent.foreign_keys {
                        if reference.table.eq_ignore_ascii_case(old_name) {
                            reference.table = new_name.clone();
                        }
                    }
                }
            }
            if table.auto_id_schema_id == 0 && *new_schema != table.schema_id {
                table.auto_id_schema_id = table.schema_id;
            }
            if *new_schema == table.auto_id_schema_id {
                table.auto_id_schema_id = 0;
            }
            table.schema_id = *new_schema;
            table.name = new_name.clone();
            table.version = table.version.saturating_add(1);
            self.schemas
                .entry(*new_schema)
                .or_default()
                .insert(new_name.to_ascii_lowercase(), table);
        }
        Ok(())
    }
}

/// 调整 AUTO_INCREMENT 基准；非 force 时不得小于等于当前值。
pub fn rebase_auto_increment(
    table: &mut TableInfo,
    new_base: i64,
    force: bool,
) -> Result<bool, TableError> {
    if new_base < 0 {
        return Err(TableError::InvalidAutoId);
    }
    if !force && new_base <= table.auto_increment_id {
        return Ok(false);
    }
    table.auto_increment_id = new_base;
    Ok(true)
}

/// 调整 AUTO_RANDOM 基准；不得回退或为负。
pub fn rebase_auto_random(table: &mut TableInfo, new_base: i64) -> Result<bool, TableError> {
    if new_base < table.auto_random_id || new_base < 0 {
        return Err(TableError::InvalidAutoId);
    }
    let changed = new_base != table.auto_random_id;
    table.auto_random_id = new_base;
    Ok(changed)
}

/// 修改 auto ID 缓存大小，返回是否发生变化。
pub fn alter_auto_id_cache(table: &mut TableInfo, cache: u64) -> bool {
    let changed = table.auto_id_cache != cache;
    table.auto_id_cache = cache;
    changed
}

/// 修改 shard_row_id_bits（行 ID 高位分片位数，用于打散写入热点）。
///
/// 与 Go `onShardRowID` 一致：调低当前位数时保留历史最大值，只有调高时才推进该值。
pub fn alter_shard_row_id_bits(table: &mut TableInfo, bits: u8) -> Result<bool, TableError> {
    if bits > 15 {
        return Err(TableError::ShardBitsOverflow);
    }
    let changed = table.shard_row_id_bits != bits;
    table.shard_row_id_bits = bits;
    table.max_shard_row_id_bits = table.max_shard_row_id_bits.max(bits);
    Ok(changed)
}

/// 修改表注释。
pub fn alter_comment(table: &mut TableInfo, comment: impl Into<String>) -> bool {
    let comment = comment.into();
    let changed = table.comment != comment;
    table.comment = comment;
    changed
}

/// 修改字符集与校对规则；校对名须匹配字符集前缀（binary 特例除外）。
pub fn alter_charset_and_collation(
    table: &mut TableInfo,
    charset: &str,
    collation: &str,
) -> Result<bool, TableError> {
    let charset = charset.to_ascii_lowercase();
    let collation = collation.to_ascii_lowercase();
    if !collation.starts_with(&(charset.clone() + "_"))
        && !(charset == "binary" && collation == "binary")
    {
        return Err(TableError::InvalidCharsetCollation);
    }
    let changed = table.charset != charset || table.collation != collation;
    table.charset = charset;
    table.collation = collation;
    Ok(changed)
}

/// 设置或清除 TiFlash 副本；count 为 0 时移除配置。
pub fn set_tiflash_replica(
    table: &mut TableInfo,
    count: u64,
    location_labels: Vec<String>,
) -> Result<(), TableError> {
    if count == 0 {
        table.tiflash_replica = None;
        return Ok(());
    }
    // Go preserves the existing availability when changing a non-zero replica
    // count; only the explicit ResetAvailable job argument clears it.
    let available_partition_ids = table
        .tiflash_replica
        .take()
        .map(|replica| replica.available_partition_ids)
        .unwrap_or_default();
    table.tiflash_replica = Some(TiFlashReplica {
        count,
        location_labels,
        available_partition_ids,
    });
    Ok(())
}

/// 更新某个物理表/分区的 TiFlash 副本就绪状态。
pub fn update_tiflash_replica_status(
    table: &mut TableInfo,
    physical_id: i64,
    available: bool,
) -> Result<bool, TableError> {
    let replica = table
        .tiflash_replica
        .as_mut()
        .ok_or(TableError::InvalidReplicaCount)?;
    if physical_id != table.id && !table.partition_ids.contains(&physical_id) {
        return Err(TableError::PartitionNotFound);
    }
    Ok(if available {
        replica.available_partition_ids.insert(physical_id)
    } else {
        replica.available_partition_ids.remove(&physical_id)
    })
}

/// 单调推进表 version。
pub fn update_table_version(table: &mut TableInfo, version: u64) -> Result<bool, TableError> {
    if version < table.version {
        return Err(TableError::InvalidVersion);
    }
    let changed = version != table.version;
    table.version = version;
    Ok(changed)
}

/// 修改 placement policy（数据放置策略）；空字符串非法。
pub fn alter_placement(
    table: &mut TableInfo,
    placement: Option<String>,
) -> Result<bool, TableError> {
    if placement
        .as_ref()
        .is_some_and(|policy| policy.trim().is_empty())
    {
        return Err(TableError::InvalidPlacement);
    }
    let changed = table.placement_policy != placement;
    table.placement_policy = placement;
    Ok(changed)
}

/// 整体替换表 attributes。
pub fn alter_attributes(table: &mut TableInfo, attributes: BTreeMap<String, String>) -> bool {
    let changed = table.attributes != attributes;
    table.attributes = attributes;
    changed
}

/// 开启或关闭表缓存（cache table）。
pub fn alter_cache(table: &mut TableInfo, cached: bool) -> bool {
    let changed = table.cached != cached;
    table.cached = cached;
    changed
}

/// 修改 affinity（亲和性调度配置）；空字符串非法。
pub fn alter_affinity(table: &mut TableInfo, affinity: Option<String>) -> Result<bool, TableError> {
    if affinity
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(TableError::InvalidAffinity);
    }
    let changed = table.affinity != affinity;
    table.affinity = affinity;
    Ok(changed)
}

/// 修改 Region 预切分策略字符串。
pub fn alter_region_split_policy(table: &mut TableInfo, policy: Option<String>) -> bool {
    let changed = table.split_policy != policy;
    table.split_policy = policy;
    changed
}

/// 返回表的物理 ID 列表：无分区时为 table id，否则为各 partition id。
pub fn table_physical_ids(table: &TableInfo) -> Vec<i64> {
    if table.partition_ids.is_empty() {
        vec![table.id]
    } else {
        table.partition_ids.clone()
    }
}

/*

// onModifyTableAutoIDCache 对应 Go：更新 AutoIDCache 并完成 job。
pub fn onModifyTableAutoIDCache(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetModifyTableAutoIDCacheArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    tblInfo.AutoIDCache = args.NewCache;
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// verifyNoOverflowShardBits 对应 Go：检查下一次全局 auto ID 在新 shard bits 下是否溢出。
pub fn verifyNoOverflowShardBits(s: &mut sess::Pool, tbl: table::Table, shardRowIDBits: u64) -> Result<(), errors::Error> {
    if shardRowIDBits == 0 {
        return Ok(());
    }
    let ctx = s.Get().map_err(errors::Trace)?;
    let autoIncID = tbl.Allocators(ctx.GetTableCtx()).Get(autoid::RowIDAllocType).NextGlobalAutoID().map_err(errors::Trace)?;
    s.Put(ctx);
    if tables::OverflowShardBits(autoIncID, shardRowIDBits, autoid::RowIDBitLength, true) {
        return Err(autoid::ErrAutoincReadFailed.GenWithStack(format!("shard_row_id_bits {} will cause next global auto ID {} overflow", shardRowIDBits, autoIncID)));
    }
    Ok(())
}

// checkAndRenameTables 对应 Go：先 drop 旧 schema 记录，再以新名新 schema 创建，并迁移 label rules。
pub fn checkAndRenameTables(
    jobCtx: &mut jobContext,
    t: &mut meta::Mutator,
    job: &mut model::Job,
    tblInfo: &mut model::TableInfo,
    args: &model::RenameTableArgs,
) -> Result<i64, errors::Error> {
    let ver = 0;
    t.DropTableOrView(args.OldSchemaID, tblInfo.ID).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    // Go failpoint renameTableErr 根据新表名注入错误；仅保留注释。
    let oldTableName = tblInfo.Name.clone();
    let (tableRuleID, partRuleIDs, oldRuleIDs, oldRules) =
        getOldLabelRules(jobCtx.store.GetCodec(), tblInfo, &args.OldSchemaName.L, &oldTableName.L)
            .map_err(|err| {
                job.State = model::JobStateCancelled;
                errors::Wrapf(err, "failed to get old label rules from PD")
            })?;
    if tblInfo.AutoIDSchemaID == 0 && args.NewSchemaID != args.OldSchemaID {
        tblInfo.AutoIDSchemaID = args.OldSchemaID;
    }
    if args.NewSchemaID == tblInfo.AutoIDSchemaID {
        tblInfo.AutoIDSchemaID = 0;
    }
    tblInfo.Name = args.NewTableName.clone();
    t.CreateTableOrView(args.NewSchemaID, tblInfo).map_err(errors::Trace)?;
    let newDBInfo = t.GetDatabase(args.NewSchemaID).map_err(errors::Trace)?;
    updateLabelRules(jobCtx.store.GetCodec(), &newDBInfo.Name.L, tblInfo, oldRules, tableRuleID, partRuleIDs, oldRuleIDs, tblInfo.ID)
        .map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Wrapf(err, "failed to update the label rule to PD")
        })?;
    Ok(ver)
}

// adjustForeignKeyChildTableInfoAfterRenameTable 对应 Go：父表 rename 后同步子表外键引用。
pub fn adjustForeignKeyChildTableInfoAfterRenameTable(
    infoCache: &infoschema::InfoCache,
    t: &mut meta::Mutator,
    job: &mut model::Job,
    fkh: &mut foreignKeyHelper,
    tblInfo: &mut model::TableInfo,
    oldSchemaName: ast::CIStr,
    oldTableName: ast::CIStr,
    newTableName: ast::CIStr,
    newSchemaID: i64,
) -> Result<(), errors::Error> {
    if !vardef::EnableForeignKey.Load() || newTableName.L == oldTableName.L {
        return Ok(());
    }
    let is = infoCache.GetLatest();
    let newDB = is.SchemaByID(newSchemaID).ok_or_else(|| {
        job.State = model::JobStateCancelled;
        infoschema::ErrDatabaseNotExists.GenWithStackByArgs(format!("schema-ID: {}", newSchemaID))
    })?;
    let referredFKs = is.GetTableReferredForeignKeys(&oldSchemaName.L, &oldTableName.L);
    if referredFKs.is_empty() {
        return Ok(());
    }
    fkh.addLoadedTable(&oldSchemaName.L, &oldTableName.L, newDB.ID, tblInfo);
    for referredFK in referredFKs {
        let childTableInfo = match fkh.getTableFromStorage(&is, t, &referredFK.ChildSchema, &referredFK.ChildTable) {
            Ok(info) => info,
            Err(err) if infoschema::ErrTableNotExists.Equal(&err) || infoschema::ErrDatabaseNotExists.Equal(&err) => continue,
            Err(err) => return Err(err),
        };
        if let Some(childFKInfo) = model::FindFKInfoByName(&mut childTableInfo.tblInfo.ForeignKeys, &referredFK.ChildFKName.L) {
            childFKInfo.RefSchema = newDB.Name.clone();
            childFKInfo.RefTable = newTableName.clone();
        }
    }
    for info in &fkh.loaded {
        updateTable(t, info.schemaID, &mut info.tblInfo.clone(), false)?;
    }
    Ok(())
}

// finishJobRenameTable 对应 Go rename 第二阶段：更新 schema version 后把 job 标记 done。
pub fn finishJobRenameTable(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let tblInfo = getTableInfo(jobCtx.metaMut, job.TableID, job.SchemaID).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let args = jobCtx.jobArgs.as_mut().downcast_mut::<model::RenameTableArgs>().unwrap();
    args.OldSchemaIDForSchemaDiff = job.SchemaID;
    let ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// finishJobRenameTables 对应 Go 批量 rename 第二阶段：收集所有新 schema 下的 tableInfo 后 finish。
pub fn finishJobRenameTables(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = jobCtx.jobArgs.as_mut().downcast_mut::<model::RenameTablesArgs>().unwrap();
    let infos = args.RenameTableInfos.clone();
    let mut tblSchemaIDs = HashMap::new();
    for info in &infos {
        tblSchemaIDs.insert(info.TableID, info.NewSchemaID);
    }
    let mut tblInfos = Vec::with_capacity(infos.len());
    for info in &infos {
        let tblInfo = getTableInfo(jobCtx.metaMut, info.TableID, tblSchemaIDs[&info.TableID]).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Trace(err)
        })?;
        tblInfos.push(tblInfo);
    }
    for info in &mut args.RenameTableInfos {
        info.OldSchemaIDForSchemaDiff = info.NewSchemaID;
    }
    let ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
    job.FinishMultipleTableJob(model::JobStateDone, model::StatePublic, ver, tblInfos);
    Ok(ver)
}

// onModifyTableComment 对应 Go：修改表注释，多 schema 可回滚阶段先标记不可回滚。
pub fn onModifyTableComment(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetModifyTableCommentArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    if job.MultiSchemaInfo.is_some() && job.MultiSchemaInfo.Revertible {
        job.MarkNonRevertible();
        return Ok(0);
    }
    tblInfo.Comment = args.Comment;
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// onModifyTableCharsetAndCollate 对应 Go：校验并更新表/列 charset 与 collate。
pub fn onModifyTableCharsetAndCollate(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetModifyTableCharsetAndCollateArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let dbInfo = checkSchemaExistAndCancelNotExistJob(jobCtx.metaMut, job).map_err(errors::Trace)?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    checkAlterTableCharset(&tblInfo, &dbInfo, &args.ToCharset, &args.ToCollate, args.NeedsOverwriteCols).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    if job.MultiSchemaInfo.is_some() && job.MultiSchemaInfo.Revertible {
        job.MarkNonRevertible();
        return Ok(0);
    }
    tblInfo.Charset = args.ToCharset;
    tblInfo.Collate = args.ToCollate;
    if args.NeedsOverwriteCols {
        for col in &mut tblInfo.Columns {
            if field_types::HasCharset(&col.FieldType) {
                col.SetCharset(&tblInfo.Charset);
                col.SetCollate(&tblInfo.Collate);
            } else {
                col.SetCharset(charset::CharsetBin);
                col.SetCollate(charset::CharsetBin);
            }
        }
    }
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// onUpdateTiFlashReplicaStatus 对应 Go：更新表或分区 TiFlash available 状态。
pub fn onUpdateTiFlashReplicaStatus(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetUpdateTiFlashReplicaStatusArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    if tblInfo.TiFlashReplica.is_none()
        || (tblInfo.ID == args.PhysicalID && tblInfo.TiFlashReplica.Available == args.Available)
        || (tblInfo.ID != args.PhysicalID && args.Available == tblInfo.TiFlashReplica.IsPartitionAvailable(args.PhysicalID))
    {
        job.State = model::JobStateCancelled;
        return Err(errors::Errorf(format!("the replica available status of table {} is already updated", tblInfo.Name.String())));
    }
    if tblInfo.ID == args.PhysicalID {
        tblInfo.TiFlashReplica.Available = args.Available;
    } else if let Some(pi) = tblInfo.GetPartitionInfo() {
        if args.Available {
            let mut allAvailable = true;
            for p in &pi.Definitions {
                if p.ID == args.PhysicalID {
                    tblInfo.TiFlashReplica.AvailablePartitionIDs.push(args.PhysicalID);
                }
                allAvailable = allAvailable && tblInfo.TiFlashReplica.IsPartitionAvailable(p.ID);
            }
            tblInfo.TiFlashReplica.Available = allAvailable;
        } else {
            tblInfo.TiFlashReplica.AvailablePartitionIDs.retain(|id| *id != args.PhysicalID);
            tblInfo.TiFlashReplica.Available = false;
            logutil::DDLLogger().Info("TiFlash replica become unavailable", zap::Int64("tableID", tblInfo.ID), zap::Int64("partitionID", args.PhysicalID));
        }
    } else {
        job.State = model::JobStateCancelled;
        return Err(errors::Errorf(format!("unknown physical ID {} in table {}", args.PhysicalID, tblInfo.Name.O)));
    }
    if tblInfo.TiFlashReplica.Available {
        logutil::DDLLogger().Info("TiFlash replica available", zap::Int64("tableID", tblInfo.ID));
    }
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// checkTableNotExists 对应 Go：用缓存 infoschema 检查同 schema 下表名不存在。
pub fn checkTableNotExists(infoCache: &infoschema::InfoCache, schemaID: i64, tableName: &str) -> Result<(), errors::Error> {
    let is = infoCache.GetLatest();
    checkTableNotExistsFromInfoSchema(is, schemaID, tableName)
}

// checkConstraintNamesNotExists 对应 Go：扫描 schema 下已有表，避免 check constraint 重名。
pub fn checkConstraintNamesNotExists(t: &mut meta::Mutator, schemaID: i64, constraints: &[model::ConstraintInfo]) -> Result<(), errors::Error> {
    if constraints.is_empty() {
        return Ok(());
    }
    let tbInfos = t.ListTables(context::Background(), schemaID)?;
    for tb in tbInfos {
        for constraint in constraints {
            if constraint.State != model::StateWriteOnly && tb.FindConstraintInfoByName(&constraint.Name.L).is_some() {
                return Err(infoschema::ErrCheckConstraintDupName.GenWithStackByArgs(&constraint.Name.L));
            }
        }
    }
    Ok(())
}

// checkTableIDNotExists 对应 Go：按 table ID 检查待恢复表不会撞到现有表。
pub fn checkTableIDNotExists(t: &mut meta::Mutator, schemaID: i64, tableID: i64) -> Result<(), errors::Error> {
    match t.GetTable(schemaID, tableID) {
        Ok(Some(tbl)) => Err(infoschema::ErrTableExists.GenWithStackByArgs(tbl.Name)),
        Ok(None) => Ok(()),
        Err(err) if meta::ErrDBNotExists.Equal(&err) => Err(infoschema::ErrDatabaseNotExists.GenWithStackByArgs("")),
        Err(err) => Err(errors::Trace(err)),
    }
}

// checkTableNotExistsFromInfoSchema 对应 Go：先查库，再查表名。
pub fn checkTableNotExistsFromInfoSchema(is: infoschema::InfoSchema, schemaID: i64, tableName: &str) -> Result<(), errors::Error> {
    let schema = is.SchemaByID(schemaID).ok_or_else(|| infoschema::ErrDatabaseNotExists.GenWithStackByArgs(""))?;
    if is.TableExists(schema.Name, ast::NewCIStr(tableName)) {
        return Err(infoschema::ErrTableExists.GenWithStackByArgs(tableName));
    }
    Ok(())
}

// updateVersionAndTableInfoWithCheck 对应 Go：校验一个或多个 TableInfo 后再更新 schema version/table info。
pub fn updateVersionAndTableInfoWithCheck(
    jobCtx: &mut jobContext,
    job: &mut model::Job,
    tblInfo: &mut model::TableInfo,
    shouldUpdateVer: bool,
    multiInfos: Vec<schemaIDAndTableInfo>,
) -> Result<i64, errors::Error> {
    if let Err(err) = checkTableInfoValid(tblInfo) {
        job.State = model::JobStateCancelled;
        return Err(errors::Trace(err));
    }
    for info in &multiInfos {
        if let Err(err) = checkTableInfoValid(&info.tblInfo) {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    }
    updateVersionAndTableInfo(jobCtx, job, tblInfo, shouldUpdateVer, multiInfos)
}

// updateVersionAndTableInfo 对应 Go：必要时更新 schema version，再写 table info 和附加表信息。
pub fn updateVersionAndTableInfo(
    jobCtx: &mut jobContext,
    job: &mut model::Job,
    tblInfo: &mut model::TableInfo,
    shouldUpdateVer: bool,
    multiInfos: Vec<schemaIDAndTableInfo>,
) -> Result<i64, errors::Error> {
    // Go failpoint mockUpdateVersionAndTableInfoErr 用于测试错误包装；保留注释。
    let mut ver = 0;
    if shouldUpdateVer && (job.MultiSchemaInfo.is_none() || !job.MultiSchemaInfo.SkipVersion) {
        ver = updateSchemaVersion(jobCtx, job, multiInfos.clone()).map_err(errors::Trace)?;
    }
    let needUpdateTs = tblInfo.State == model::StatePublic
        && job.Type != model::ActionTruncateTable
        && job.Type != model::ActionTruncateTablePartition
        && job.Type != model::ActionRenameTable
        && job.Type != model::ActionRenameTables
        && job.Type != model::ActionExchangeTablePartition;
    updateTable(jobCtx.metaMut, job.SchemaID, tblInfo, needUpdateTs).map_err(errors::Trace)?;
    for info in multiInfos {
        updateTable(jobCtx.metaMut, info.schemaID, &mut info.tblInfo.clone(), needUpdateTs).map_err(errors::Trace)?;
    }
    Ok(ver)
}

// updateTable 对应 Go：按 needUpdateTs 更新 TableInfo.UpdateTS，然后写 meta。
pub fn updateTable(t: &mut meta::Mutator, schemaID: i64, tblInfo: &mut model::TableInfo, needUpdateTs: bool) -> Result<(), errors::Error> {
    if needUpdateTs {
        tblInfo.UpdateTS = t.StartTS;
    }
    t.UpdateTable(schemaID, tblInfo)
}

// schemaIDAndTableInfo 对应 Go 小结构体：携带额外需要更新的 schema/table 信息。
#[derive(Clone)]
pub struct schemaIDAndTableInfo {
    pub schemaID: i64,
    pub tblInfo: model::TableInfo,
}

// onRepairTable 对应 Go repair table：先推进 schema version，再把修复后的表从 none 改 public。
pub fn onRepairTable(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut args = model::GetRepairTableArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = args.TableInfo;
    tblInfo.State = model::StateNone;
    GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    let ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
    match tblInfo.State {
        model::StateNone => {
            tblInfo.State = model::StatePublic;
            tblInfo.UpdateTS = jobCtx.metaMut.StartTS;
            repairTableOrViewWithCheck(jobCtx.metaMut, job, job.SchemaID, &mut tblInfo).map_err(errors::Trace)?;
            job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
            Ok(ver)
        }
        _ => Err(dbterror::ErrInvalidDDLState.GenWithStackByArgs("table", tblInfo.State)),
    }
}

// onAlterTableAttributes 对应 Go：写入/删除表级 label rule，然后更新表版本。
pub fn onAlterTableAttributes(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetAlterTableAttributesArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID)?;
    let err = if args.LabelRule.Labels.is_empty() {
        let patch = label::NewRulePatch(vec![], vec![args.LabelRule.ID.clone()]);
        infosync::UpdateLabelRules(jobCtx.stepCtx.clone(), patch)
    } else {
        let labelRule = label::Rule::from(args.LabelRule.clone());
        infosync::PutLabelRule(jobCtx.stepCtx.clone(), &labelRule)
    };
    if let Err(err) = err {
        job.State = model::JobStateCancelled;
        return Err(errors::Wrapf(err, "failed to notify PD the label rules"));
    }
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true, vec![]).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// onAlterTablePartitionAttributes 对应 Go：按 partitionID 修改分区 label rule。
pub fn onAlterTablePartitionAttributes(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetAlterTablePartitionArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID)?;
    let ptInfo = tblInfo.GetPartitionInfo();
    if ptInfo.GetNameByID(args.PartitionID).is_empty() {
        job.State = model::JobStateCancelled;
        return Err(errors::Trace(table::ErrUnknownPartition.GenWithStackByArgs("drop?", tblInfo.Name.O)));
    }
    let err = if args.LabelRule.Labels.is_empty() {
        infosync::UpdateLabelRules(context::TODO(), label::NewRulePatch(vec![], vec![args.LabelRule.ID.clone()]))
    } else {
        infosync::PutLabelRule(context::TODO(), &label::Rule::from(args.LabelRule.clone()))
    };
    if let Err(err) = err {
        job.State = model::JobStateCancelled;
        return Err(errors::Wrapf(err, "failed to notify PD the label rules"));
    }
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true, vec![]).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// onAlterTablePartitionPlacement 对应 Go：修改单个分区 placement policy，并把 bundle 通知 PD。
pub fn onAlterTablePartitionPlacement(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetAlterTablePartitionArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID)?;
    let mut partitionDef = None;
    let mut oldPartitionEnablesPlacement = false;
    for def in &mut tblInfo.GetPartitionInfo().Definitions {
        if args.PartitionID == def.ID {
            oldPartitionEnablesPlacement = def.PlacementPolicyRef.is_some();
            def.PlacementPolicyRef = args.PolicyRefInfo.clone();
            partitionDef = Some(def.clone());
            break;
        }
    }
    let partitionDef = partitionDef.ok_or_else(|| {
        job.State = model::JobStateCancelled;
        errors::Trace(table::ErrUnknownPartition.GenWithStackByArgs("drop?", tblInfo.Name.O))
    })?;
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true, vec![]).map_err(errors::Trace)?;
    checkPlacementPolicyRefValidAndCanNonValidJob(jobCtx.metaMut, job, partitionDef.PlacementPolicyRef.clone()).map_err(errors::Trace)?;
    let mut bundle = placement::NewPartitionBundle(jobCtx.metaMut, partitionDef.clone()).map_err(errors::Trace)?;
    if bundle.is_none() && oldPartitionEnablesPlacement {
        bundle = Some(placement::NewBundle(partitionDef.ID));
    }
    if let Some(bundle) = bundle {
        infosync::PutRuleBundlesWithDefaultRetry(context::TODO(), vec![bundle]).map_err(|err| {
            job.State = model::JobStateCancelled;
            errors::Wrapf(err, "failed to notify PD the placement rules")
        })?;
    }
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// onAlterTablePlacement 对应 Go：修改表级 placement policy，并处理旧 bundle 清理。
pub fn onAlterTablePlacement(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetAlterTablePlacementArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID)?;
    checkPlacementPolicyRefValidAndCanNonValidJob(jobCtx.metaMut, job, args.PlacementPolicyRef.clone()).map_err(errors::Trace)?;
    let oldTableEnablesPlacement = tblInfo.PlacementPolicyRef.is_some();
    tblInfo.PlacementPolicyRef = args.PlacementPolicyRef;
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true, vec![]).map_err(errors::Trace)?;
    let mut bundle = placement::NewTableBundle(jobCtx.metaMut, &tblInfo).map_err(errors::Trace)?;
    if bundle.is_none() && oldTableEnablesPlacement {
        bundle = Some(placement::NewBundle(tblInfo.ID));
    }
    if let Some(bundle) = bundle {
        infosync::PutRuleBundlesWithDefaultRetry(context::TODO(), vec![bundle]).map_err(errors::Trace)?;
    }
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}

// getOldLabelRules 对应 Go：根据旧库表名收集表和分区 rule ID，并从 PD 读取旧规则。
pub fn getOldLabelRules(
    codec: tikv::Codec,
    tblInfo: &model::TableInfo,
    oldSchemaName: &str,
    oldTableName: &str,
) -> Result<(String, Vec<String>, Vec<String>, HashMap<String, label::Rule>), errors::Error> {
    let tableRuleID = label::NewRuleID(codec, oldSchemaName, oldTableName, "");
    let mut partRuleIDs = Vec::new();
    let mut oldRuleIDs = vec![tableRuleID.clone()];
    if let Some(pi) = tblInfo.GetPartitionInfo() {
        for def in &pi.Definitions {
            partRuleIDs.push(label::NewRuleID(codec, oldSchemaName, oldTableName, &def.Name.L));
        }
    }
    oldRuleIDs.extend(partRuleIDs.clone());
    let oldRules = infosync::GetLabelRules(context::TODO(), oldRuleIDs.clone())?;
    Ok((tableRuleID, partRuleIDs, oldRuleIDs, oldRules))
}

// updateLabelRules 对应 Go：用旧 rule clone/reset 生成新 rule patch，并删除旧 rule IDs。
pub fn updateLabelRules(
    codec: tikv::Codec,
    newSchemaName: &str,
    tblInfo: &model::TableInfo,
    oldRules: HashMap<String, label::Rule>,
    tableRuleID: String,
    partRuleIDs: Vec<String>,
    oldRuleIDs: Vec<String>,
    tID: i64,
) -> Result<(), errors::Error> {
    if oldRules.is_empty() {
        return Ok(());
    }
    let mut newRules = Vec::new();
    if let Some(pi) = tblInfo.GetPartitionInfo() {
        for (idx, def) in pi.Definitions.iter().enumerate() {
            if let Some(r) = oldRules.get(&partRuleIDs[idx]) {
                newRules.push(r.Clone().Reset(codec, newSchemaName, &tblInfo.Name.L, &def.Name.L, def.ID));
            }
        }
    }
    let mut ids = vec![tID];
    if let Some(r) = oldRules.get(&tableRuleID) {
        if let Some(pi) = tblInfo.GetPartitionInfo() {
            for def in &pi.Definitions {
                ids.push(def.ID);
            }
        }
        newRules.push(r.Clone().Reset(codec, newSchemaName, &tblInfo.Name.L, "", ids));
    }
    let patch = label::NewRulePatch(newRules, oldRuleIDs);
    infosync::UpdateLabelRules(context::TODO(), patch)
}

// onAlterCacheTable 对应 Go：disable -> switching -> enable 的表缓存状态机。
pub fn onAlterCacheTable(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut tbInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    let mut ver = 0;
    if tbInfo.TableCacheStatusType == model::TableCacheStatusEnable {
        job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tbInfo));
        return Ok(ver);
    }
    if tbInfo.TempTableType != model::TempTableNone {
        return Err(errors::Trace(dbterror::ErrOptOnTemporaryTable.GenWithStackByArgs("alter temporary table cache")));
    }
    if tbInfo.Partition.is_some() {
        return Err(errors::Trace(dbterror::ErrOptOnCacheTable.GenWithStackByArgs("partition mode")));
    }
    match tbInfo.TableCacheStatusType {
        model::TableCacheStatusDisable => {
            tbInfo.TableCacheStatusType = model::TableCacheStatusSwitching;
            ver = updateVersionAndTableInfoWithCheck(jobCtx, job, &mut tbInfo, true, vec![])?;
        }
        model::TableCacheStatusSwitching => {
            tbInfo.TableCacheStatusType = model::TableCacheStatusEnable;
            ver = updateVersionAndTableInfoWithCheck(jobCtx, job, &mut tbInfo, true, vec![])?;
            job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tbInfo));
        }
        _ => {
            job.State = model::JobStateCancelled;
            return Err(dbterror::ErrInvalidDDLState.GenWithStackByArgs("alter table cache", tbInfo.TableCacheStatusType.String()));
        }
    }
    Ok(ver)
}

// onAlterNoCacheTable 对应 Go：enable -> switching -> disable 的表缓存状态机。
pub fn onAlterNoCacheTable(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut tbInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    let mut ver = 0;
    if tbInfo.TableCacheStatusType == model::TableCacheStatusDisable {
        job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tbInfo));
        return Ok(ver);
    }
    match tbInfo.TableCacheStatusType {
        model::TableCacheStatusEnable => {
            tbInfo.TableCacheStatusType = model::TableCacheStatusSwitching;
            ver = updateVersionAndTableInfoWithCheck(jobCtx, job, &mut tbInfo, true, vec![])?;
        }
        model::TableCacheStatusSwitching => {
            tbInfo.TableCacheStatusType = model::TableCacheStatusDisable;
            ver = updateVersionAndTableInfoWithCheck(jobCtx, job, &mut tbInfo, true, vec![])?;
            job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tbInfo));
        }
        _ => {
            job.State = model::JobStateCancelled;
            return Err(dbterror::ErrInvalidDDLState.GenWithStackByArgs("alter table no cache", tbInfo.TableCacheStatusType.String()));
        }
    }
    Ok(ver)
}

// onRefreshMeta 对应 Go：只更新 schema version，然后把 job/schema state 标记完成。
pub fn onRefreshMeta(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    model::GetRefreshMetaArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
    job.State = model::JobStateDone;
    job.SchemaState = model::StatePublic;
    Ok(ver)
}

// onAlterTableAffinity 对应 Go：校验 affinity、创建新 PD group、按需清理旧 group，再更新表信息。
pub fn onAlterTableAffinity(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let args = model::GetAlterTableAffinityArgs(job).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    let mut tblInfo = GetTableInfoAndCancelFaultJob(jobCtx.metaMut, job, job.SchemaID).map_err(errors::Trace)?;
    let oldTblInfo = tblInfo.Clone();
    validateTableAffinity(&tblInfo, args.Affinity.clone()).map_err(|err| {
        job.State = model::JobStateCancelled;
        errors::Trace(err)
    })?;
    tblInfo.Affinity = args.Affinity;
    if tblInfo.Affinity.is_some() {
        createTableAffinityGroupsInPD(jobCtx, &tblInfo).map_err(errors::Trace)?;
    }
    if oldTblInfo.Affinity.is_some()
        && (tblInfo.Affinity.is_none() || oldTblInfo.Affinity.Level != tblInfo.Affinity.Level)
    {
        if let Err(err) = deleteTableAffinityGroupsInPD(jobCtx, &oldTblInfo, None) {
            logutil::DDLLogger().Error("failed to delete old affinity groups from PD", zap::Error(err), zap::Int64("tableID", oldTblInfo.ID));
        }
    }
    let ver = updateVersionAndTableInfo(jobCtx, job, &mut tblInfo, true, vec![]).map_err(errors::Trace)?;
    job.FinishTableJob(model::JobStateDone, model::StatePublic, ver, Some(tblInfo));
    Ok(ver)
}
*/
