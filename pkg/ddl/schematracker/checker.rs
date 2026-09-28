// Copyright 2024 PingCAP, Inc.
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

// SchemaTracker 与真实 DDL 执行器的一致性检查包装。
//
// `Checker` 在每次 DDL 命令执行后，同步更新内存中的 `SchemaTracker`，
// 并核对库/表是否仍可在跟踪器中查到，用于发现真实执行器与内存模型漂移。
// `closed` 标志可临时关闭核对（Disable/Enable）。

use crate::{
    AlterTableSpec, CreateIndexSpec, CreateTableSpec, Error, InfoSchemaSource, NewSchemaTracker,
    SchemaTracker, ast, model,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// 占位初始化入口，与 Go 侧包 init 对齐。
pub fn init() {}

#[derive(Clone)]
/// 可投递给执行器与跟踪器的 DDL 命令枚举。
pub enum DdlCommand {
    /// 创建库；bool 为 IF NOT EXISTS。
    CreateSchema(ast::CIStr, bool),
    /// 使用完整 DBInfo 创建库；bool 为已存在时是否忽略。
    CreateSchemaWithInfo(model::DBInfo, bool),
    /// 修改库字符集与排序规则。
    AlterSchema(ast::CIStr, String, String),
    /// 删除库；bool 为 IF EXISTS。
    DropSchema(ast::CIStr, bool),
    /// 创建表。
    CreateTable(CreateTableSpec),
    /// 创建视图。
    CreateView(CreateTableSpec),
    /// 删除多表；末参为 IF EXISTS。
    DropTable(ast::CIStr, Vec<ast::CIStr>, bool),
    /// 删除多视图；末参为 IF EXISTS。
    DropView(ast::CIStr, Vec<ast::CIStr>, bool),
    /// 创建索引。
    CreateIndex(CreateIndexSpec),
    /// 删除索引；末参为 IF EXISTS。
    DropIndex(ast::CIStr, ast::CIStr, ast::CIStr, bool),
    /// 修改表（列/索引/分区等）。
    AlterTable(AlterTableSpec),
    /// 重命名表：每项为 (旧库, 旧表, 新库, 新表)。
    RenameTable(Vec<(ast::CIStr, ast::CIStr, ast::CIStr, ast::CIStr)>),
    /// 跟踪器侧无操作的占位命令（如 Recover/Flashback 等未建模动作）。
    Noop(&'static str),
}

/// 真实 DDL 执行器抽象；Checker 先调执行器再同步 SchemaTracker。
pub trait DdlExecutor: Send {
    /// 执行一条 DDL 命令。
    fn Execute(&mut self, command: DdlCommand) -> Result<(), Error>;
    /// 返回真实 DDL 当前的库元数据；不存在时返回 None。
    fn SchemaByName(&self, _name: &ast::CIStr) -> Result<Option<model::DBInfo>, Error> {
        Ok(None)
    }
    /// 返回真实 DDL 当前的表元数据；不存在时返回 None。
    fn TableByName(
        &self,
        _schema: &ast::CIStr,
        _table: &ast::CIStr,
    ) -> Result<Option<model::TableInfo>, Error> {
        Ok(None)
    }
    /// 按真实执行器的 SHOW CREATE 规则渲染库元数据。
    fn ConstructResultOfShowCreateDatabase(&self, _info: &model::DBInfo) -> Result<String, Error> {
        Err(Error::Unsupported("ConstructResultOfShowCreateDatabase"))
    }
    /// 按真实执行器的 SHOW CREATE 规则渲染表元数据。
    fn ConstructResultOfShowCreateTable(&self, _info: &model::TableInfo) -> Result<String, Error> {
        Err(Error::Unsupported("ConstructResultOfShowCreateTable"))
    }
    /// 会话启用 shard/pre-split 选项时忽略对应 SHOW CREATE 注释。
    fn IgnoreShardRowIdAndPreSplitComments(&self) -> bool {
        false
    }
    fn Start(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn Stats(&self) -> HashMap<String, String> {
        HashMap::new()
    }
    fn GetScope(&self, _status: &str) -> u64 {
        0
    }
    fn Stop(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn RegisterStatsHandle(&mut self) {}
    fn SchemaSyncer(&self) -> Option<String> {
        None
    }
    fn StateSyncer(&self) -> Option<String> {
        None
    }
    fn OwnerManager(&self) -> Option<String> {
        None
    }
    fn GetID(&self) -> String {
        String::new()
    }
    fn DoDDLJob(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn GetMinJobIDRefresher(&self) -> Option<String> {
        None
    }
    fn DoDDLJobWrapper(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

/// 包装真实执行器与 SchemaTracker 的一致性检查器。
pub struct Checker {
    /// 被包装的真实 DDL 执行器。
    realExecutor: Box<dyn DdlExecutor>,
    /// 内存 schema 跟踪器，镜像执行结果。
    pub tracker: SchemaTracker,
    /// 为 true 时跳过库/表存在性核对。
    closed: AtomicBool,
}
/// 用给定执行器与大小写模式构造 Checker。
pub fn NewChecker(real_executor: Box<dyn DdlExecutor>, lower_case_table_names: i32) -> Checker {
    Checker {
        realExecutor: real_executor,
        tracker: NewSchemaTracker(lower_case_table_names),
        closed: AtomicBool::new(false),
    }
}

impl Checker {
    /// 关闭跟踪核对。
    pub fn Disable(&self) {
        self.closed.store(true, Ordering::Release);
    }
    /// 重新开启跟踪核对。
    pub fn Enable(&self) {
        self.closed.store(false, Ordering::Release);
    }
    /// 在跟踪器中确保存在名为 test 的库。
    pub fn CreateTestDB(&mut self) {
        self.tracker.CreateTestDB();
    }
    /// 核对真实执行器与跟踪器中的库元数据。
    fn checkDBInfo(&self, name: &ast::CIStr) -> Result<(), Error> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let real = self.realExecutor.SchemaByName(name)?;
        let tracked = self.tracker.InfoStore.SchemaByName(name);
        match (real.as_ref(), tracked) {
            (None, None) => Ok(()),
            (Some(_), None) => Err(metadata_presence_mismatch("database", &name.O, true)),
            (None, Some(_)) => Err(metadata_presence_mismatch("database", &name.O, false)),
            (Some(real), Some(tracked)) => {
                let real = self
                    .realExecutor
                    .ConstructResultOfShowCreateDatabase(real)?;
                let tracked = self
                    .realExecutor
                    .ConstructResultOfShowCreateDatabase(tracked)?;
                compare_rendered_metadata("database", &name.O, real, tracked)
            }
        }
    }
    /// 核对真实执行器与跟踪器中的表元数据。
    fn checkTableInfo(&self, schema: &ast::CIStr, table: &ast::CIStr) -> Result<(), Error> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        if schema.L == "mysql" {
            return Ok(());
        }
        let real = self.realExecutor.TableByName(schema, table)?;
        let tracked = self.tracker.InfoStore.TableByName(schema, table).ok();
        let name = format!("{}.{}", schema.O, table.O);
        match (real.as_ref(), tracked) {
            (None, None) => Ok(()),
            (Some(_), None) => Err(metadata_presence_mismatch("table", &name, true)),
            (None, Some(_)) => Err(metadata_presence_mismatch("table", &name, false)),
            (Some(real), Some(tracked)) => {
                let real = self.realExecutor.ConstructResultOfShowCreateTable(real)?;
                let tracked = self
                    .realExecutor
                    .ConstructResultOfShowCreateTable(tracked)?;
                let real = normalize_show_create_table(
                    real,
                    self.realExecutor.IgnoreShardRowIdAndPreSplitComments(),
                );
                let tracked = normalize_show_create_table(
                    tracked,
                    self.realExecutor.IgnoreShardRowIdAndPreSplitComments(),
                );
                compare_rendered_metadata("table", &name, real, tracked)
            }
        }
    }
    /// 先交给真实执行器，再按命令类型同步更新 SchemaTracker。
    fn execute(&mut self, command: DdlCommand) -> Result<(), Error> {
        // 镜像同一命令到内存跟踪器。
        self.realExecutor.Execute(command.clone())?;
        match command {
            DdlCommand::CreateSchema(name, ignore) => self.tracker.CreateSchema(name, ignore),
            DdlCommand::CreateSchemaWithInfo(info, ignore) => {
                self.tracker.CreateSchemaWithInfo(info, ignore)
            }
            DdlCommand::AlterSchema(name, charset, collate) => {
                self.tracker.AlterSchema(&name, charset, collate)
            }
            DdlCommand::DropSchema(name, ignore) => self.tracker.DropSchema(&name, ignore),
            DdlCommand::CreateTable(spec) => self.tracker.CreateTable(spec),
            DdlCommand::CreateView(spec) => self.tracker.CreateView(spec),
            DdlCommand::DropTable(schema, tables, ignore) => {
                self.tracker.DropTable(&schema, &tables, ignore)
            }
            DdlCommand::DropView(schema, tables, ignore) => {
                self.tracker.DropView(&schema, &tables, ignore)
            }
            DdlCommand::CreateIndex(spec) => self.tracker.CreateIndex(spec),
            DdlCommand::DropIndex(schema, table, index, ignore) => {
                self.tracker.DropIndex(&schema, &table, &index, ignore)
            }
            DdlCommand::AlterTable(spec) => self.tracker.AlterTable(spec),
            DdlCommand::RenameTable(pairs) => self.tracker.RenameTable(pairs),
            DdlCommand::Noop(_) => Ok(()),
        }
    }
    /// 创建库并核对跟踪结果。
    pub fn CreateSchema(&mut self, name: ast::CIStr, ignore: bool) -> Result<(), Error> {
        self.execute(DdlCommand::CreateSchema(name.clone(), ignore))?;
        self.checkDBInfo(&name)
    }
    /// 修改库字符集/排序规则并核对。
    pub fn AlterSchema(
        &mut self,
        name: ast::CIStr,
        charset: String,
        collate: String,
    ) -> Result<(), Error> {
        self.execute(DdlCommand::AlterSchema(name.clone(), charset, collate))?;
        self.checkDBInfo(&name)
    }
    /// 删除库（删除后无需再核对存在性）。
    pub fn DropSchema(&mut self, name: ast::CIStr, ignore: bool) -> Result<(), Error> {
        self.execute(DdlCommand::DropSchema(name.clone(), ignore))?;
        self.checkDBInfo(&name)
    }
    /// 创建表并核对跟踪结果。
    pub fn CreateTable(&mut self, spec: CreateTableSpec) -> Result<(), Error> {
        let schema = spec.schema.clone();
        let table = spec.table.Name.clone();
        let command = DdlCommand::CreateTable(spec);
        self.realExecutor.Execute(command.clone())?;
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let DdlCommand::CreateTable(spec) = command else {
            unreachable!()
        };
        self.tracker.CreateTable(spec)?;
        self.checkTableInfo(&schema, &table)
    }
    /// 创建视图：当前与建表走同一路径。
    pub fn CreateView(&mut self, spec: CreateTableSpec) -> Result<(), Error> {
        let schema = spec.schema.clone();
        let table = spec.table.Name.clone();
        self.execute(DdlCommand::CreateView(spec))?;
        self.checkTableInfo(&schema, &table)
    }
    /// 删除表。
    pub fn DropTable(
        &mut self,
        schema: ast::CIStr,
        tables: Vec<ast::CIStr>,
        ignore: bool,
    ) -> Result<(), Error> {
        let real_result = self.realExecutor.Execute(DdlCommand::DropTable(
            schema.clone(),
            tables.clone(),
            ignore,
        ));
        let _ = self.tracker.DropTable(&schema, &tables, ignore);
        for table in &tables {
            self.checkTableInfo(&schema, table)?;
        }
        real_result
    }
    /// 删除视图：复用删表逻辑。
    pub fn DropView(
        &mut self,
        schema: ast::CIStr,
        tables: Vec<ast::CIStr>,
        ignore: bool,
    ) -> Result<(), Error> {
        self.realExecutor
            .Execute(DdlCommand::DropView(schema.clone(), tables.clone(), ignore))?;
        self.tracker.DropView(&schema, &tables, ignore)?;
        for table in &tables {
            self.checkTableInfo(&schema, table)?;
        }
        Ok(())
    }
    /// 创建索引并核对表仍可查。
    pub fn CreateIndex(&mut self, spec: CreateIndexSpec) -> Result<(), Error> {
        let schema = spec.schema.clone();
        let table = spec.table.clone();
        self.execute(DdlCommand::CreateIndex(spec))?;
        self.checkTableInfo(&schema, &table)
    }
    /// 删除索引并核对表仍可查。
    pub fn DropIndex(
        &mut self,
        schema: ast::CIStr,
        table: ast::CIStr,
        index: ast::CIStr,
        ignore: bool,
    ) -> Result<(), Error> {
        self.execute(DdlCommand::DropIndex(
            schema.clone(),
            table.clone(),
            index,
            ignore,
        ))?;
        self.checkTableInfo(&schema, &table)
    }
    /// 修改表并核对跟踪结果。
    pub fn AlterTable(&mut self, spec: AlterTableSpec) -> Result<(), Error> {
        let schema = spec.schema.clone();
        let table = spec.table.clone();
        let command = DdlCommand::AlterTable(spec);
        self.realExecutor.Execute(command.clone())?;
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let DdlCommand::AlterTable(spec) = command else {
            unreachable!()
        };
        self.tracker.AlterTable(spec)?;
        self.checkTableInfo(&schema, &table)
    }
    /// 批量重命名表。
    pub fn RenameTable(
        &mut self,
        pairs: Vec<(ast::CIStr, ast::CIStr, ast::CIStr, ast::CIStr)>,
    ) -> Result<(), Error> {
        let checked_pairs = pairs.clone();
        self.execute(DdlCommand::RenameTable(pairs))?;
        for (old_schema, old_table, new_schema, new_table) in checked_pairs {
            self.checkTableInfo(&old_schema, &old_table)?;
            self.checkTableInfo(&new_schema, &new_table)?;
        }
        Ok(())
    }
    /// 用完整 DBInfo 创建库并核对跟踪结果。
    pub fn CreateSchemaWithInfo(&mut self, info: model::DBInfo, ignore: bool) -> Result<(), Error> {
        let name = info.Name.clone();
        self.execute(DdlCommand::CreateSchemaWithInfo(info, ignore))?;
        self.checkDBInfo(&name)
    }
    /// 用已有 TableInfo 创建表。
    pub fn CreateTableWithInfo(
        &mut self,
        _schema: ast::CIStr,
        _table: model::TableInfo,
        _ignore: bool,
    ) -> Result<(), Error> {
        panic!("implement me")
    }
    /// 批量创建多张表。
    pub fn BatchCreateTableWithInfo(
        &mut self,
        _schema: ast::CIStr,
        _tables: Vec<model::TableInfo>,
        _ignore: bool,
    ) -> Result<(), Error> {
        panic!("implement me")
    }
    /// 从 InfoSchema 数据源初始化跟踪器。
    pub fn InitFromIS(&mut self, source: &dyn InfoSchemaSource) -> Result<(), Error> {
        self.tracker.InfoStore.InitFromIS(source)
    }
    /// 转发到真实执行器的 Start。
    pub fn Start(&mut self) -> Result<(), Error> {
        self.realExecutor.Start()
    }
    /// 转发统计信息。
    pub fn Stats(&self) -> HashMap<String, String> {
        self.realExecutor.Stats()
    }
    /// 转发 scope 查询。
    pub fn GetScope(&self, status: &str) -> u64 {
        self.realExecutor.GetScope(status)
    }
    /// 转发停止。
    pub fn Stop(&mut self) -> Result<(), Error> {
        self.realExecutor.Stop()
    }
    /// 转发统计句柄注册。
    pub fn RegisterStatsHandle(&mut self) {
        self.realExecutor.RegisterStatsHandle()
    }
    /// 转发 schema syncer 标识。
    pub fn SchemaSyncer(&self) -> Option<String> {
        self.realExecutor.SchemaSyncer()
    }
    /// 转发 state syncer 标识。
    pub fn StateSyncer(&self) -> Option<String> {
        self.realExecutor.StateSyncer()
    }
    /// 转发 owner manager 标识。
    pub fn OwnerManager(&self) -> Option<String> {
        self.realExecutor.OwnerManager()
    }
    /// 转发执行器 ID。
    pub fn GetID(&self) -> String {
        self.realExecutor.GetID()
    }
    /// 转发执行 DDL job。
    pub fn DoDDLJob(&mut self) -> Result<(), Error> {
        self.realExecutor.DoDDLJob()
    }
    /// 转发最小 job ID 刷新器标识。
    pub fn GetMinJobIDRefresher(&self) -> Option<String> {
        self.realExecutor.GetMinJobIDRefresher()
    }
    /// 转发带包装的 DDL job 执行。
    pub fn DoDDLJobWrapper(&mut self) -> Result<(), Error> {
        self.realExecutor.DoDDLJobWrapper()
    }
}

fn metadata_presence_mismatch(kind: &str, name: &str, exists_in_real: bool) -> Error {
    let location = if exists_in_real {
        "real DDL but not in schema tracker"
    } else {
        "schema tracker but not in real DDL"
    };
    Error::Mismatch(format!("{kind} {name} exists in {location}"))
}

fn compare_rendered_metadata(
    kind: &str,
    name: &str,
    real: String,
    tracked: String,
) -> Result<(), Error> {
    if real == tracked {
        Ok(())
    } else {
        Err(Error::Mismatch(format!(
            "{kind} {name} metadata differs:\n{real}\n!=\n{tracked}"
        )))
    }
}

fn normalize_show_create_table(mut value: String, remove_shard_pre_split: bool) -> String {
    for comment in [
        " /*T![clustered_index] NONCLUSTERED */",
        " /*T![clustered_index] CLUSTERED */",
    ] {
        value = value.replace(comment, "");
    }
    if remove_shard_pre_split {
        value = remove_tidb_comment(value, " /*T! SHARD_ROW_ID_BITS=");
        value = remove_tidb_comment(value, " /*T! PRE_SPLIT_REGIONS=");
    }
    value
}

fn remove_tidb_comment(mut value: String, prefix: &str) -> String {
    while let Some(start) = value.find(prefix) {
        let Some(relative_end) = value[start + prefix.len()..].find("*/") else {
            break;
        };
        let end = start + prefix.len() + relative_end + 2;
        value.replace_range(start..end, "");
    }
    value
}

/// 未在 SchemaTracker 中建模的 DDL 入口：以 Noop 驱动真实执行器。
impl Checker {
    pub fn RecoverSchema(&mut self) -> Result<(), Error> {
        Ok(())
    }
    pub fn RecoverTable(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn FlashbackCluster(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn TruncateTable(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn LockTables(&mut self) -> Result<(), Error> {
        self.execute(DdlCommand::Noop("LockTables"))
    }
    pub fn UnlockTables(&mut self) -> Result<(), Error> {
        self.execute(DdlCommand::Noop("UnlockTables"))
    }
    pub fn AlterTableMode(&mut self) -> Result<(), Error> {
        self.execute(DdlCommand::Noop("AlterTableMode"))
    }
    pub fn RefreshMeta(&mut self) -> Result<(), Error> {
        self.execute(DdlCommand::Noop("RefreshMeta"))
    }
    pub fn CleanupTableLock(&mut self) -> Result<(), Error> {
        self.execute(DdlCommand::Noop("CleanupTableLock"))
    }
    pub fn UpdateTableReplicaInfo(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn RepairTable(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn CreateSequence(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn DropSequence(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn AlterSequence(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn CreateMaskingPolicy(&mut self) -> Result<(), Error> {
        self.execute(DdlCommand::Noop("CreateMaskingPolicy"))
    }
    pub fn CreatePlacementPolicy(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn DropPlacementPolicy(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn AlterPlacementPolicy(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
    pub fn AddResourceGroup(&mut self) -> Result<(), Error> {
        Ok(())
    }
    pub fn DropResourceGroup(&mut self) -> Result<(), Error> {
        Ok(())
    }
    pub fn AlterResourceGroup(&mut self) -> Result<(), Error> {
        Ok(())
    }
    pub fn CreatePlacementPolicyWithInfo(&mut self) -> Result<(), Error> {
        panic!("implement me")
    }
}

/// 将 DDL 能力注入到存储对象的包装类型（泛型透传）。
pub struct StorageDDLInjector<T> {
    /// 被包装的底层存储。
    pub storage: T,
}
/// 构造 `StorageDDLInjector`。
pub fn NewStorageDDLInjector<T>(storage: T) -> StorageDDLInjector<T> {
    StorageDDLInjector { storage }
}
/// 取出被包装的底层存储。
pub fn UnwrapStorage<T>(storage: StorageDDLInjector<T>) -> T {
    storage.storage
}
