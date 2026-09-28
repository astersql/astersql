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
// Schema 级 DDL 执行：创建、修改、删除与恢复 database 元数据。

#![allow(non_snake_case, non_camel_case_types, dead_code, unused_variables)]

// onCreateSchema 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 回滚分支会改变 DDL job 调度状态；继续前滚、取消和进入 rollingback 的路径均按 Go 结构展开。
fn onCreateSchema(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    // Function body follows Go call order.
        let mut schemaID = job.SchemaID;
        let mut args, err = model.GetCreateSchemaArgs(job);
        if err != None {
            // Invalid arguments, cancel this job.
            job.State = model.JobStateCancelled;
            return Err(errors.Trace(err));
        }
        let mut dbInfo = args.DBInfo;
        dbInfo.ID = schemaID;
        dbInfo.State = model.StateNone;
        err = checkSchemaNotExists(jobCtx.infoCache, schemaID, dbInfo);
        if err != None {
            if infoschema.ErrDatabaseExists.Equal(err) {
                // The database already exists, can't create it, we should cancel this job now.
                job.State = model.JobStateCancelled;
            }
            return Err(errors.Trace(err));
        }
        ver, err = updateSchemaVersion(jobCtx, job);
        if err != None {
            return Err(errors.Trace(err));
        }
        match dbInfo.State {
        // Go case model.StateNone:
            // none -> public
            dbInfo.State = model.StatePublic;
            err = jobCtx.metaMut.CreateDatabase(dbInfo);
            if err != None {
                return Err(errors.Trace(err));
            }
            // Finish this job.
            job.FinishDBJob(model.JobStateDone, model.StatePublic, ver, dbInfo);
            return Ok(ver);
        // Go default:
            // We can't enter here.
            return Err(errors.Errorf("invalid db state %v", dbInfo.State));
        }
}
// checkSchemaNotExists checks whether the database already exists.
// see checkTableNotExists for the rationale of why we check using info schema only.

// checkSchemaNotExists 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 自检失败沿用 Go 的 panic/强校验语义，方便后续人工对照。
fn checkSchemaNotExists(infoCache: &mut infoschema::InfoCache, schemaID: i64, dbInfo: &mut model::DBInfo) -> Result<(), errors::Error> {
    // Function body follows Go call order.
        let mut is = infoCache.GetLatest();
        // Check database exists by name.
        if is.SchemaExists(dbInfo.Name) {
            return infoschema.ErrDatabaseExists.GenWithStackByArgs(dbInfo.Name);
        }
        // Check database exists by ID.
        let mut if _, ok = is.SchemaByID(schemaID); ok {
            return infoschema.ErrDatabaseExists.GenWithStackByArgs(dbInfo.Name);
        }
        return Ok(());
}

// onModifySchemaCharsetAndCollate 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 回滚分支会改变 DDL job 调度状态；继续前滚、取消和进入 rollingback 的路径均按 Go 结构展开。
fn onModifySchemaCharsetAndCollate(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    // Function body follows Go call order.
        let mut args, err = model.GetModifySchemaArgs(job);
        if err != None {
            job.State = model.JobStateCancelled;
            return Err(errors.Trace(err));
        }
        let mut dbInfo, err = checkSchemaExistAndCancelNotExistJob(jobCtx.metaMut, job);
        if err != None {
            return Err(errors.Trace(err));
        }
        if dbInfo.Charset == args.ToCharset && dbInfo.Collate == args.ToCollate {
            job.FinishDBJob(model.JobStateDone, model.StatePublic, ver, dbInfo);
            return Ok(ver);
        }
        dbInfo.Charset = args.ToCharset;
        dbInfo.Collate = args.ToCollate;
        if err = jobCtx.metaMut.UpdateDatabase(dbInfo); err != None {
            return Err(errors.Trace(err));
        }
        if ver, err = updateSchemaVersion(jobCtx, job); err != None {
            return Err(errors.Trace(err));
        }
        job.FinishDBJob(model.JobStateDone, model.StatePublic, ver, dbInfo);
        return Ok(ver);
}

// onModifySchemaDefaultPlacement 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 回滚分支会改变 DDL job 调度状态；继续前滚、取消和进入 rollingback 的路径均按 Go 结构展开。
fn onModifySchemaDefaultPlacement(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    // Function body follows Go call order.
        let mut args, err = model.GetModifySchemaArgs(job);
        if err != None {
            job.State = model.JobStateCancelled;
            return Err(errors.Trace(err));
        }
        let mut placementPolicyRef = args.PolicyRef;
        let mut metaMut = jobCtx.metaMut;
        let mut dbInfo, err = checkSchemaExistAndCancelNotExistJob(metaMut, job);
        if err != None {
            return Err(errors.Trace(err));
        }
        // Double Check if policy exits while ddl executing
        if _, err = checkPlacementPolicyRefValidAndCanNonValidJob(metaMut, job, placementPolicyRef); err != None {
            return Err(errors.Trace(err));
        }
        // Notice: dbInfo.DirectPlacementOpts and dbInfo.PlacementPolicyRef can not be both not nil, which checked before constructing ddl job.
        // So that we can just check the two situation that do not need ddl: 1. DB.DP == DDL.DP && nil == nil 2. nil == nil && DB.PP == DDL.PP
        if placementPolicyRef != None && dbInfo.PlacementPolicyRef != None && *dbInfo.PlacementPolicyRef == *placementPolicyRef {
            job.FinishDBJob(model.JobStateDone, model.StatePublic, ver, dbInfo);
            return Ok(ver);
        }
        // If placementPolicyRef and directPlacementOpts are both nil, And placement of dbInfo is not nil, it will remove all placement options.
        dbInfo.PlacementPolicyRef = placementPolicyRef;
        if err = metaMut.UpdateDatabase(dbInfo); err != None {
            return Err(errors.Trace(err));
        }
        if ver, err = updateSchemaVersion(jobCtx, job); err != None {
            return Err(errors.Trace(err));
        }
        job.FinishDBJob(model.JobStateDone, model.StatePublic, ver, dbInfo);
        return Ok(ver);
}

// onDropSchema 对应 w *worker 的方法，保留源文件中的声明顺序、主要分支和错误返回语义。
// 回滚分支会改变 DDL job 调度状态；继续前滚、取消和进入 rollingback 的路径均按 Go 结构展开。
impl worker {
    pub fn onDropSchema(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        // Go receiver mapped to self.
            let mut metaMut = jobCtx.metaMut;
            let mut dbInfo, err = checkSchemaExistAndCancelNotExistJob(metaMut, job);
            if err != None {
                return Err(errors.Trace(err));
            }
            if dbInfo.State == model.StatePublic {
                err = checkDatabaseHasForeignKeyReferredInOwner(jobCtx, job);
                if err != None {
                    return Err(errors.Trace(err));
                }
            }
            ver, err = updateSchemaVersion(jobCtx, job);
            if err != None {
                return Err(errors.Trace(err));
            }
            match dbInfo.State {
            // Go case model.StatePublic:
                // public -> write only
                dbInfo.State = model.StateWriteOnly;
                err = metaMut.UpdateDatabase(dbInfo);
                if err != None {
                    return Err(errors.Trace(err));
                }
                let mut tables []*model.TableInfo;
                tables, err = metaMut.ListTables(jobCtx.stepCtx, job.SchemaID);
                if err != None {
                    return Err(errors.Trace(err));
                }
                let mut ruleIDs []string;
                for tblInfo in tables  {
                    let mut rules = append(getPartitionRuleIDs(jobCtx.store.GetCodec(), job.SchemaName, tblInfo), label.NewRuleID(jobCtx.store.GetCodec(), job.SchemaName, tblInfo.Name.L, ""));
                    ruleIDs = append(ruleIDs, rules...);
                }
                let mut patch = label.NewRulePatch([]*label.Rule{}, ruleIDs);
                err = infosync.UpdateLabelRules(context.TODO(), patch);
                if err != None {
                    job.State = model.JobStateCancelled;
                    return Err(errors.Trace(err));
                }
            // Go case model.StateWriteOnly:
                // write only -> delete only
                dbInfo.State = model.StateDeleteOnly;
                err = metaMut.UpdateDatabase(dbInfo);
                if err != None {
                    return Err(errors.Trace(err));
                }
            // Go case model.StateDeleteOnly:
                dbInfo.State = model.StateNone;
                let mut tables []*model.TableInfo;
                tables, err = metaMut.ListTables(jobCtx.stepCtx, job.SchemaID);
                if err != None {
                    return Err(errors.Trace(err));
                }
                // Best-effort cleanup - log errors but continue with DROP DATABASE
                let mut if err = batchDeleteTableAffinityGroups(jobCtx, tables); err != None {
                    logutil.DDLLogger().Warn("failed to delete affinity groups for batch tables, but operation will continue",
                        zap.Error(err),
                        zap.Int64("databaseID", dbInfo.ID));
                }
                // Clean up masking policies for all tables in the dropped database.
                let mut if err = self.dropMaskingPoliciesByDBName(jobCtx, job.SchemaName); err != None {
                    logutil.DDLLogger().Warn("failed to delete masking policies for database, but operation will continue",
                        zap.Error(err),
                        zap.String("dbName", job.SchemaName));
                }
                err = metaMut.UpdateDatabase(dbInfo);
                if err != None {
                    return Err(errors.Trace(err));
                }
                // we only drop meta key of database, but not drop tables' meta keys.
                if err = metaMut.DropDatabase(dbInfo.ID); err != None {
                    break;
                }
                // Split tables into multiple jobs to avoid too big records in the notifier.
                const tooManyTablesThreshold = 100000;
                let mut tablesPerJob = 100;
                if len(tables) > tooManyTablesThreshold {
                    tablesPerJob = 500;
                }
                let mut for i = 0; i < len(tables); i += tablesPerJob {
                    let mut end = min(i+tablesPerJob, len(tables));
                    let mut dropSchemaEvent = notifier.NewDropSchemaEvent(dbInfo, tables[i:end]);
                    err = asyncNotifyEvent(jobCtx, dropSchemaEvent, job, int64(i/tablesPerJob), self.sess);
                    if err != None {
                        return Err(errors.Trace(err));
                    }
                }
                // Finish this job.
                job.FillFinishedArgs(&model.DropSchemaArgs{
                    AllDroppedTableIDs: getIDs(tables),
                });
                job.FinishDBJob(model.JobStateDone, model.StateNone, ver, dbInfo);
            // Go default:
                // We can't enter here.
                return Err(errors.Trace(errors.Errorf("invalid db state %v", dbInfo.State)));
            }
            job.SchemaState = dbInfo.State;
            return Err(errors.Trace(err));
    }
}

// onRecoverSchema 对应 w *worker 的方法，保留源文件中的声明顺序、主要分支和错误返回语义。
// 回滚分支会改变 DDL job 调度状态；继续前滚、取消和进入 rollingback 的路径均按 Go 结构展开。
impl worker {
    pub fn onRecoverSchema(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
        // Go receiver mapped to self.
            let mut args, err = model.GetRecoverArgs(job);
            if err != None {
                // Invalid arguments, cancel this job.
                job.State = model.JobStateCancelled;
                return Err(errors.Trace(err));
            }
            let mut recoverSchemaInfo = args.RecoverInfo;
            let mut schemaInfo = recoverSchemaInfo.DBInfo;
            // check GC and safe point
            let mut gcEnable, err = checkGCEnable(w);
            if err != None {
                job.State = model.JobStateCancelled;
                return Err(errors.Trace(err));
            }
            match schemaInfo.State {
            // Go case model.StateNone:
                // none -> write only
                // check GC enable and update flag.
                if gcEnable {
                    args.CheckFlag = recoverCheckFlagEnableGC;
                } else {
                    args.CheckFlag = recoverCheckFlagDisableGC;
                }
                job.FillArgs(args);
                schemaInfo.State = model.StateWriteOnly;
                job.SchemaState = model.StateWriteOnly;
            // Go case model.StateWriteOnly:
                // write only -> public
                // do recover schema and tables.
                if gcEnable {
                    err = disableGC(w);
                    if err != None {
                        job.State = model.JobStateCancelled;
                        return Err(errors.Errorf("disable gc failed, try again later. err: %v", err));
                    }
                }
                let mut recoverTbls = recoverSchemaInfo.RecoverTableInfos;
                if recoverSchemaInfo.LoadTablesOnExecute {
                    let mut sid = recoverSchemaInfo.DBInfo.ID;
                    let mut snap = self.store.GetSnapshot(kv.NewVersion(recoverSchemaInfo.SnapshotTS));
                    let mut snapMeta = meta.NewReader(snap);
                    let mut tables, err2 = snapMeta.ListTables(jobCtx.stepCtx, sid);
                    if err2 != None {
                        job.State = model.JobStateCancelled;
                        return Err(errors.Trace(err2));
                    }
                    recoverTbls = make([]*model.RecoverTableInfo, 0, len(tables));
                    for tblInfo in tables  {
                        let mut autoIDs, err3 = snapMeta.GetAutoIDAccessors(sid, tblInfo.ID).Get();
                        if err3 != None {
                            job.State = model.JobStateCancelled;
                            return Err(errors.Trace(err3));
                        }
                        recoverTbls = append(recoverTbls, &model.RecoverTableInfo{
                            SchemaID:      sid,
                            TableInfo:     tblInfo,
                            DropJobID:     recoverSchemaInfo.DropJobID,
                            SnapshotTS:    recoverSchemaInfo.SnapshotTS,
                            AutoIDs:       autoIDs,
                            OldSchemaName: recoverSchemaInfo.OldSchemaName.L,
                            OldTableName:  tblInfo.Name.L,
                        });
                    }
                }
                let mut dbInfo = schemaInfo.Clone();
                dbInfo.State = model.StatePublic;
                err = jobCtx.metaMut.CreateDatabase(dbInfo);
                if err != None {
                    return Err(errors.Trace(err));
                }
                // check GC safe point
                err = checkSafePoint(w, recoverSchemaInfo.SnapshotTS);
                if err != None {
                    job.State = model.JobStateCancelled;
                    return Err(errors.Trace(err));
                }
                for recoverInfo in recoverTbls  {
                    if recoverInfo.TableInfo.TTLInfo != None {
                        // force disable TTL job schedule for recovered table
                        recoverInfo.TableInfo.TTLInfo.Enable = false;
                    }
                    ver, err = self.recoverTable(jobCtx.stepCtx, jobCtx.metaMut, job, recoverInfo);
                    if err != None {
                        return Err(errors.Trace(err));
                    }
                }
                schemaInfo.State = model.StatePublic;
                // use to update InfoSchema
                job.SchemaID = schemaInfo.ID;
                ver, err = updateSchemaVersion(jobCtx, job);
                if err != None {
                    return Err(errors.Trace(err));
                }
                // Finish this job.
                job.FinishDBJob(model.JobStateDone, model.StatePublic, ver, schemaInfo);
                return Ok(ver);
            // Go default:
                // We can't enter here.
                return Err(errors.Errorf("invalid db state %v", schemaInfo.State));
            }
            return Err(errors.Trace(err));
    }
}

// checkSchemaExistAndCancelNotExistJob 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 回滚分支会改变 DDL job 调度状态；继续前滚、取消和进入 rollingback 的路径均按 Go 结构展开。
// 自检失败沿用 Go 的 panic/强校验语义，方便后续人工对照。
fn checkSchemaExistAndCancelNotExistJob(t: &mut meta::Mutator, job: &mut model::Job) -> Result<&mut model::DBInfo, errors::Error> {
    // Function body follows Go call order.
        let mut dbInfo, err = t.GetDatabase(job.SchemaID);
        if err != None {
            return Ok(());
        }
        if dbInfo == None {
            job.State = model.JobStateCancelled;
            return Ok(());
        }
        return Ok(dbInfo);
}

// getIDs 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
fn getIDs(tables: Vec<&mut model::TableInfo>) -> Vec<i64> {
    // Function body follows Go call order.
        let mut ids = make([]int64, 0, len(tables));
        for t in tables  {
            ids = append(ids, t.ID);
            if t.GetPartitionInfo() != None {
                ids = append(ids, getPartitionIDs(t)...);
            }
        }
        return ids;
}
*/

// Schema（数据库）级 DDL 的内存目录实现。
//
// 提供创建/修改字符集与排序规则、修改 Placement Policy（放置策略）、
// 分阶段删除（Public → WriteOnly → DeleteOnly → None）以及从 dropped
// 缓存恢复库的能力。文件前半的大块注释保留了对应 Go 实现的迁移草稿，
// 下方为可编译的简化 `SchemaCatalog`。

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Schema（库）在 DDL 状态机中的可见性阶段。
///
/// 在线删除库时按 Public → WriteOnly → DeleteOnly → None 推进：
/// WriteOnly 禁止新写入，DeleteOnly 仅允许删除，None 表示元数据已移除。
pub enum SchemaState {
    Public,
    WriteOnly,
    DeleteOnly,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 内存中的数据库元信息。
pub struct DatabaseInfo {
    /// 库的全局唯一 ID。
    pub id: i64,
    /// 库名（展示用原始大小写）。
    pub name: String,
    /// 默认字符集。
    pub charset: String,
    /// 默认排序规则（collation）。
    pub collation: String,
    /// 默认 Placement Policy 名称；None 表示未绑定放置策略。
    pub placement_policy: Option<String>,
    /// 当前 schema 状态。
    pub state: SchemaState,
    /// 库内表摘要列表（含分区物理 ID）。
    pub tables: Vec<SchemaTable>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 库目录中登记的表摘要，用于收集物理 ID（表 ID 与分区 ID）。
pub struct SchemaTable {
    /// 逻辑表 ID。
    pub id: i64,
    /// 分区表各分区的物理 ID；非分区表为空。
    pub partition_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// SchemaCatalog 操作失败原因。
pub enum SchemaError {
    /// 库已存在且未指定 IF NOT EXISTS。
    AlreadyExists,
    /// 目标库不存在。
    NotFound,
    /// 字符集与排序规则不匹配或不受支持。
    InvalidCharsetCollation,
    /// Placement Policy 名称为空串等非法值。
    InvalidPlacementPolicy,
    /// 恢复时发现同名库已存在。
    RecoveryConflict,
}

/// 以 Debug 形式展示错误，便于测试断言。
impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SchemaError {}

#[derive(Default)]
/// 内存 schema 目录：维护存活库与已删除库缓存。
pub struct SchemaCatalog {
    /// 下一个可分配的库 ID。
    next_id: i64,
    /// 小写库名 → 存活库信息。
    schemas: BTreeMap<String, DatabaseInfo>,
    /// 已删除库按 ID 缓存，供 recover 使用。
    dropped: BTreeMap<i64, DatabaseInfo>,
}

impl SchemaCatalog {
    /// 创建库；`if_not_exists` 为 true 时已存在则返回 Ok(None)。
    pub fn create_schema(
        &mut self,
        name: &str,
        charset: &str,
        collation: &str,
        placement_policy: Option<String>,
        if_not_exists: bool,
    ) -> Result<Option<i64>, SchemaError> {
        // 库名按 ASCII 小写做唯一键，兼容大小写不敏感匹配。
        let key = name.to_ascii_lowercase();
        if self.schemas.contains_key(&key) {
            return if if_not_exists {
                Ok(None)
            } else {
                Err(SchemaError::AlreadyExists)
            };
        }
        validate_charset_and_collation(charset, collation)?;
        validate_placement_policy(placement_policy.as_deref())?;
        // 分配从 1 起的单调库 ID。
        self.next_id = self.next_id.saturating_add(1).max(1);
        let id = self.next_id;
        self.schemas.insert(
            key,
            DatabaseInfo {
                id,
                name: name.to_string(),
                charset: charset.to_ascii_lowercase(),
                collation: collation.to_ascii_lowercase(),
                placement_policy,
                state: SchemaState::Public,
                tables: Vec::new(),
            },
        );
        Ok(Some(id))
    }

    /// 修改库的默认字符集与排序规则；返回是否实际发生变更。
    pub fn modify_charset_and_collation(
        &mut self,
        name: &str,
        charset: &str,
        collation: &str,
    ) -> Result<bool, SchemaError> {
        let info = self
            .schemas
            .get_mut(&name.to_ascii_lowercase())
            .ok_or(SchemaError::NotFound)?;
        // Go onModifySchemaCharsetAndCollate resolves the database before it
        // examines or applies the requested values, so preserve that error order.
        validate_charset_and_collation(charset, collation)?;
        let changed = !info.charset.eq_ignore_ascii_case(charset)
            || !info.collation.eq_ignore_ascii_case(collation);
        info.charset = charset.to_ascii_lowercase();
        info.collation = collation.to_ascii_lowercase();
        Ok(changed)
    }

    /// 修改或清除库的默认 Placement Policy；返回是否实际发生变更。
    pub fn modify_placement(
        &mut self,
        name: &str,
        placement: Option<String>,
    ) -> Result<bool, SchemaError> {
        let info = self
            .schemas
            .get_mut(&name.to_ascii_lowercase())
            .ok_or(SchemaError::NotFound)?;
        // Go onModifySchemaDefaultPlacement checks schema existence before
        // validating the referenced placement policy.
        validate_placement_policy(placement.as_deref())?;
        let changed = info.placement_policy != placement;
        info.placement_policy = placement;
        Ok(changed)
    }

    /// 推进删除库的一个状态步；到达 None 时移入 dropped 缓存。
    pub fn drop_schema_step(&mut self, name: &str) -> Result<SchemaState, SchemaError> {
        let key = name.to_ascii_lowercase();
        let state = self.schemas.get(&key).ok_or(SchemaError::NotFound)?.state;
        // 状态机：Public → WriteOnly → DeleteOnly → None。
        let next = match state {
            SchemaState::Public => SchemaState::WriteOnly,
            SchemaState::WriteOnly => SchemaState::DeleteOnly,
            SchemaState::DeleteOnly => SchemaState::None,
            SchemaState::None => SchemaState::None,
        };
        // 最终步：从存活目录移除并记入 dropped，便于后续 recover。
        if next == SchemaState::None {
            let mut info = self.schemas.remove(&key).ok_or(SchemaError::NotFound)?;
            info.state = SchemaState::None;
            self.dropped.insert(info.id, info);
        } else if let Some(info) = self.schemas.get_mut(&key) {
            info.state = next;
        }
        Ok(next)
    }

    /// 按 ID 从 dropped 缓存恢复库；若同名库已存在则冲突。
    pub fn recover_schema(&mut self, schema_id: i64) -> Result<(), SchemaError> {
        let mut info = self
            .dropped
            .remove(&schema_id)
            .ok_or(SchemaError::NotFound)?;
        let key = info.name.to_ascii_lowercase();
        // 同名库已存在：把记录放回 dropped，避免丢弃可恢复数据。
        if self.schemas.contains_key(&key) {
            self.dropped.insert(schema_id, info);
            return Err(SchemaError::RecoveryConflict);
        }
        info.state = SchemaState::Public;
        self.schemas.insert(key, info);
        Ok(())
    }

    /// 按名查找存活库（大小写不敏感）。
    pub fn schema(&self, name: &str) -> Option<&DatabaseInfo> {
        self.schemas.get(&name.to_ascii_lowercase())
    }
}

/// 校验字符集与排序规则前缀是否匹配（如 utf8mb4 对应 utf8mb4_*）。
pub fn validate_charset_and_collation(charset: &str, collation: &str) -> Result<(), SchemaError> {
    let charset = charset.to_ascii_lowercase();
    let collation = collation.to_ascii_lowercase();
    let valid = match charset.as_str() {
        "utf8mb4" => collation.starts_with("utf8mb4_"),
        "utf8" => collation.starts_with("utf8_") && !collation.starts_with("utf8mb4_"),
        "latin1" => collation.starts_with("latin1_"),
        "ascii" => collation.starts_with("ascii_"),
        "binary" => collation == "binary",
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(SchemaError::InvalidCharsetCollation)
    }
}

/// 校验 Placement Policy：允许 None，但不允许空白字符串。
pub fn validate_placement_policy(policy: Option<&str>) -> Result<(), SchemaError> {
    if policy.is_some_and(|value| value.trim().is_empty()) {
        Err(SchemaError::InvalidPlacementPolicy)
    } else {
        Ok(())
    }
}

/// 收集表 ID 及其分区物理 ID，供 DropSchema 等清理路径使用。
pub fn schema_physical_ids(tables: &[SchemaTable]) -> Vec<i64> {
    let mut ids = Vec::new();
    for table in tables {
        ids.push(table.id);
        ids.extend(table.partition_ids.iter().copied());
    }
    ids
}
