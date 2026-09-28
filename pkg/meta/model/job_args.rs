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

// Job arg encoding between JobVersion1 array and JobVersion2 JSON.
// 访问元数据或连接数据库。Job、AST、PD HTTP 及同包模型均沿用 Go 名称，等待后续模块接线。

// DDL Job 参数类型与 V1/V2 编解码入口。
//
// V1 将参数保存为按位置排列的 JSON 数组；V2（自 8.4.0）保存单个有类型 JSON 对象。
// `getOrDecodeArgs*` 负责版本分流与 Job 内缓存；各 `*Args` 结构实现 `JobArgs` /
// `FinishedJobArgs`，并由 `Get*Args` 辅助函数解码。完成态参数仅部分动作需要回写。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// DynArg 对应 Go 的 any。V1 参数必须保持位置和数量，因此这里不把它收窄成业务类型。
pub type DynArg = Value;
/// Job 参数操作结果别名。
pub type JobArgResult<T> = Result<T, errors::Error>;

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}

// 将任意可序列化值转为 DynArg；失败时退回 Null。
fn arg<T: Serialize>(value: &T) -> DynArg {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

// getOrDecodeArgsV1 对应 Go 的泛型入口：V1 由每种参数自己的位置协议解码。
pub fn getOrDecodeArgsV1<T: JobArgs + Default>(mut args: T, job: &mut Job) -> JobArgResult<T> {
    intest::Assert(job.version == JobVersion1, "job version is not v1");
    args.decodeV1(job)?;
    let values: Vec<serde_json::Value> = if job.raw_args == b"null" {
        Vec::new()
    } else {
        serde_json::from_slice(&job.raw_args).map_err(errors::Trace)?
    };
    job.args = values;
    Ok(args)
}

// getOrDecodeArgsV2 对应 V2 JSON 解码及 Job 内缓存。缓存命中时必须只有一个完整参数对象。
pub fn getOrDecodeArgsV2<T>(job: &mut Job) -> JobArgResult<T>
where
    T: JobArgs + Clone + for<'de> Deserialize<'de> + 'static,
{
    intest::Assert(job.version == JobVersion2, "job version is not v2");
    if !job.args.is_empty() {
        intest::Assert(job.args.len() == 1, "job args length is not 1");
        return serde_json::from_value(job.args[0].clone()).map_err(errors::Trace);
    }
    let raw_value: serde_json::Value =
        serde_json::from_slice(&job.raw_args).map_err(errors::Trace)?;
    let value: T = serde_json::from_value(raw_value.clone()).map_err(errors::Trace)?;
    job.args.push(raw_value);
    Ok(value)
}

// getOrDecodeArgs 保留 Go 的版本分流；V1 不能误按 JSON 对象解释。
pub fn getOrDecodeArgs<T>(args: T, job: &mut Job) -> JobArgResult<T>
where
    T: JobArgs + Default + Clone + for<'de> Deserialize<'de> + 'static,
{
    if job.version == JobVersion1 {
        getOrDecodeArgsV1(args, job)
    } else {
        getOrDecodeArgsV2(job)
    }
}

// JobArgs 对应 Go 私有接口。调用方应经 Job.FillArgs，而不是直接拼 V1 数组。
pub trait JobArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg>;
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()>;
}

// FinishedJobArgs 对应完成态参数；多数任务结束后会清空参数，只有实现该 trait 的类型回写结果。
pub trait FinishedJobArgs: JobArgs {
    fn getFinishedArgsV1(&self, job: &Job) -> Vec<DynArg>;
}

// impl_simple_args 只消除“字段依次进入 V1 数组、再按同序解码”的重复样板；字段顺序仍在调用处显式可见。
macro_rules! impl_simple_args {
    ($ty:ty, $( $field:ident ),* $(,)?) => {
        impl JobArgs for $ty {
            fn getArgsV1(&self, _job: &Job) -> Vec<DynArg> { vec![$(arg(&self.$field)),*] }
            fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
                job.decodeArgs(($(&mut self.$field),*)).map_err(errors::Trace)
            }
        }
    };
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// 三类自增 ID：行 ID、自增列、auto_random。
pub struct AutoIDGroup {
    pub RowID: i64,
    pub IncrementID: i64,
    pub RandomID: i64,
}

// RecoverTableInfo 保存恢复单表所需快照、旧名称和三类自增 ID。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// 恢复单表所需快照、旧名称和三类自增 ID。
pub struct RecoverTableInfo {
    pub SchemaID: i64,
    pub TableInfo: Option<Box<TableInfo>>,
    pub DropJobID: i64,
    pub SnapshotTS: u64,
    pub AutoIDs: AutoIDGroup,
    pub OldSchemaName: String,
    pub OldTableName: String,
}

// RecoverSchemaInfo 对应嵌入 DBInfo 的 Go 结构；LoadTablesOnExecute 避免提交节点持久化过大的表列表。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// 恢复库：嵌入 DBInfo；LoadTablesOnExecute 避免提交节点持久化过大表列表。
pub struct RecoverSchemaInfo {
    #[serde(flatten)]
    pub DBInfo: Option<Box<DBInfo>>,
    pub RecoverTableInfos: Vec<RecoverTableInfo>,
    pub LoadTablesOnExecute: bool,
    pub DropJobID: i64,
    pub SnapshotTS: u64,
    pub OldSchemaName: ast::CIStr,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// 无参数动作使用的空参数类型。
pub struct EmptyArgs;
impl JobArgs for EmptyArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![]
    }
    fn decodeV1(&mut self, _: &Job) -> JobArgResult<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 创建库：携带待创建的 DBInfo。
pub struct CreateSchemaArgs {
    #[serde(rename = "db_info", skip_serializing_if = "is_default")]
    pub DBInfo: Option<Box<DBInfo>>,
}
impl JobArgs for CreateSchemaArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(&self.DBInfo)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.DBInfo = Some(Box::default());
        job.decodeArgs(self.DBInfo.as_mut().unwrap())
            .map_err(errors::Trace)
    }
}
/// 解码创建库参数。
pub fn GetCreateSchemaArgs(job: &mut Job) -> JobArgResult<CreateSchemaArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 删除库：外键检查标志与完成态相关字段。
pub struct DropSchemaArgs {
    #[serde(rename = "fk_check", skip_serializing_if = "is_default")]
    pub FKCheck: bool,
    #[serde(rename = "all_dropped_table_ids", skip_serializing_if = "is_default")]
    pub AllDroppedTableIDs: Vec<i64>,
}
impl JobArgs for DropSchemaArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(&self.FKCheck)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        job.decodeArgs(&mut self.FKCheck).map_err(errors::Trace)
    }
}
impl FinishedJobArgs for DropSchemaArgs {
    fn getFinishedArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(&self.AllDroppedTableIDs)]
    }
}
/// 解码删除库普通参数。
pub fn GetDropSchemaArgs(job: &mut Job) -> JobArgResult<DropSchemaArgs> {
    getOrDecodeArgs(Default::default(), job)
}
/// 解码删除库完成态参数。
pub fn GetFinishedDropSchemaArgs(job: &mut Job) -> JobArgResult<DropSchemaArgs> {
    if job.version == JobVersion1 {
        let mut ids = vec![];
        job.decodeArgs(&mut ids).map_err(errors::Trace)?;
        return Ok(DropSchemaArgs {
            AllDroppedTableIDs: ids,
            ..Default::default()
        });
    }
    getOrDecodeArgsV2(job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改库字符集/排序规则/默认 Placement 等。
pub struct ModifySchemaArgs {
    #[serde(rename = "to_charset", skip_serializing_if = "is_default")]
    pub ToCharset: String,
    #[serde(rename = "to_collate", skip_serializing_if = "is_default")]
    pub ToCollate: String,
    #[serde(rename = "policy_ref", skip_serializing_if = "is_default")]
    pub PolicyRef: Option<Box<PolicyRefInfo>>,
}
impl JobArgs for ModifySchemaArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionModifySchemaCharsetAndCollate {
            vec![arg(&self.ToCharset), arg(&self.ToCollate)]
        } else {
            vec![arg(&self.PolicyRef)]
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        if job.tp == ActionModifySchemaCharsetAndCollate {
            job.decodeArgs((&mut self.ToCharset, &mut self.ToCollate))
                .map_err(errors::Trace)
        } else {
            job.decodeArgs(&mut self.PolicyRef).map_err(errors::Trace)
        }
    }
}
/// 解码修改库参数。
pub fn GetModifySchemaArgs(job: &mut Job) -> JobArgResult<ModifySchemaArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 创建表/视图/序列：表信息、外键检查、替换视图等。
pub struct CreateTableArgs {
    #[serde(rename = "table_info", skip_serializing_if = "is_default")]
    pub TableInfo: Option<Box<TableInfo>>,
    #[serde(rename = "on_exist_replace", skip_serializing_if = "is_default")]
    pub OnExistReplace: bool,
    #[serde(rename = "old_view_tbl_id", skip_serializing_if = "is_default")]
    pub OldViewTblID: i64,
    #[serde(rename = "fk_check", skip_serializing_if = "is_default")]
    pub FKCheck: bool,
}
impl JobArgs for CreateTableArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        match job.tp {
            ActionCreateTable => vec![arg(&self.TableInfo), arg(&self.FKCheck)],
            ActionCreateView => vec![
                arg(&self.TableInfo),
                arg(&self.OnExistReplace),
                arg(&self.OldViewTblID),
            ],
            ActionCreateSequence => vec![arg(&self.TableInfo)],
            _ => vec![],
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.TableInfo = Some(Box::default());
        match job.tp {
            ActionCreateTable => {
                job.decodeArgs((self.TableInfo.as_mut().unwrap(), &mut self.FKCheck))
            }
            ActionCreateView => job.decodeArgs((
                self.TableInfo.as_mut().unwrap(),
                &mut self.OnExistReplace,
                &mut self.OldViewTblID,
            )),
            ActionCreateSequence => job.decodeArgs(self.TableInfo.as_mut().unwrap()),
            _ => Ok(()),
        }
        .map_err(errors::Trace)
    }
}
/// 解码创建表类参数。
pub fn GetCreateTableArgs(job: &mut Job) -> JobArgResult<CreateTableArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 批量建表：多张表共享外键检查策略。
pub struct BatchCreateTableArgs {
    #[serde(rename = "tables", skip_serializing_if = "is_default")]
    pub Tables: Vec<CreateTableArgs>,
}
impl JobArgs for BatchCreateTableArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        let infos: Vec<_> = self.Tables.iter().map(|t| &t.TableInfo).collect();
        vec![arg(&infos), arg(&self.Tables[0].FKCheck)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        let (mut infos, mut fk_check): (Vec<Box<TableInfo>>, bool) = (vec![], false);
        job.decodeArgs((&mut infos, &mut fk_check))
            .map_err(errors::Trace)?;
        self.Tables = infos
            .into_iter()
            .map(|i| CreateTableArgs {
                TableInfo: Some(i),
                FKCheck: fk_check,
                ..Default::default()
            })
            .collect();
        Ok(())
    }
}
/// 解码批量建表参数。
pub fn GetBatchCreateTableArgs(job: &mut Job) -> JobArgResult<BatchCreateTableArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 删除表/视图/序列参数。
pub struct DropTableArgs {
    #[serde(rename = "identifiers", skip_serializing_if = "is_default")]
    pub Identifiers: Vec<ast::Ident>,
    #[serde(rename = "fk_check", skip_serializing_if = "is_default")]
    pub FKCheck: bool,
    #[serde(rename = "start_key", skip_serializing_if = "is_default")]
    pub StartKey: Vec<u8>,
    #[serde(rename = "old_partition_ids", skip_serializing_if = "is_default")]
    pub OldPartitionIDs: Vec<i64>,
    #[serde(rename = "old_rule_ids", skip_serializing_if = "is_default")]
    pub OldRuleIDs: Vec<String>,
}
impl JobArgs for DropTableArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionDropTable {
            vec![arg(&self.Identifiers), arg(&self.FKCheck)]
        } else {
            vec![]
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        if job.tp == ActionDropTable {
            job.decodeArgs((&mut self.Identifiers, &mut self.FKCheck))
                .map_err(errors::Trace)
        } else {
            Ok(())
        }
    }
}
impl FinishedJobArgs for DropTableArgs {
    fn getFinishedArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![
            arg(&self.StartKey),
            arg(&self.OldPartitionIDs),
            arg(&self.OldRuleIDs),
        ]
    }
}
/// 解码删表普通参数。
pub fn GetDropTableArgs(job: &mut Job) -> JobArgResult<DropTableArgs> {
    getOrDecodeArgs(Default::default(), job)
}
/// 解码删表完成态参数。
pub fn GetFinishedDropTableArgs(job: &mut Job) -> JobArgResult<DropTableArgs> {
    if job.version == JobVersion1 {
        let mut out = DropTableArgs::default();
        job.decodeArgs((
            &mut out.StartKey,
            &mut out.OldPartitionIDs,
            &mut out.OldRuleIDs,
        ))
        .map_err(errors::Trace)?;
        return Ok(out);
    }
    getOrDecodeArgsV2(job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 截断表：新旧表 ID、分区与外键检查等。
pub struct TruncateTableArgs {
    #[serde(rename = "fk_check", skip_serializing_if = "is_default")]
    pub FKCheck: bool,
    #[serde(rename = "new_table_id", skip_serializing_if = "is_default")]
    pub NewTableID: i64,
    #[serde(rename = "new_partition_ids", skip_serializing_if = "is_default")]
    pub NewPartitionIDs: Vec<i64>,
    #[serde(rename = "old_partition_ids", skip_serializing_if = "is_default")]
    pub OldPartitionIDs: Vec<i64>,
    #[serde(skip)]
    pub NewPartIDsWithPolicy: Vec<i64>,
    #[serde(skip)]
    pub OldPartIDsWithPolicy: Vec<i64>,
    #[serde(skip)]
    pub ShouldUpdateAffectedPartitions: bool,
}
impl JobArgs for TruncateTableArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionTruncateTable {
            // 第四项只供提交端计算新 ID 数量；执行端仍只解码前三项，保持历史布局。
            vec![
                arg(&self.NewTableID),
                arg(&self.FKCheck),
                arg(&self.NewPartitionIDs),
                arg(&self.OldPartitionIDs.len()),
            ]
        } else {
            vec![arg(&self.OldPartitionIDs), arg(&self.NewPartitionIDs)]
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        if job.tp == ActionTruncateTable {
            job.decodeArgs((
                &mut self.NewTableID,
                &mut self.FKCheck,
                &mut self.NewPartitionIDs,
            ))
            .map_err(errors::Trace)
        } else {
            job.decodeArgs((&mut self.OldPartitionIDs, &mut self.NewPartitionIDs))
                .map_err(errors::Trace)
        }
    }
}
impl FinishedJobArgs for TruncateTableArgs {
    fn getFinishedArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionTruncateTable {
            vec![arg(&Vec::<u8>::new()), arg(&self.OldPartitionIDs)]
        } else {
            vec![arg(&self.OldPartitionIDs)]
        }
    }
}
/// 解码截断表普通参数。
pub fn GetTruncateTableArgs(job: &mut Job) -> JobArgResult<TruncateTableArgs> {
    getOrDecodeArgs(Default::default(), job)
}
/// 解码截断表完成态参数。
pub fn GetFinishedTruncateTableArgs(job: &mut Job) -> JobArgResult<TruncateTableArgs> {
    if job.version == JobVersion1 {
        let mut out = TruncateTableArgs::default();
        if job.tp == ActionTruncateTable {
            let mut unused_start_key: Vec<u8> = vec![];
            job.decodeArgs((&mut unused_start_key, &mut out.OldPartitionIDs))
                .map_err(errors::Trace)?;
        } else {
            job.decodeArgs(&mut out.OldPartitionIDs)
                .map_err(errors::Trace)?;
        }
        return Ok(out);
    }
    getOrDecodeArgsV2(job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 表 ID 与索引 ID 对。
pub struct TableIDIndexID {
    #[serde(rename = "table_id", skip_serializing_if = "is_default")]
    pub TableID: i64,
    #[serde(rename = "index_id", skip_serializing_if = "is_default")]
    pub IndexID: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 分区增删改相关参数（含新旧分区定义）。
pub struct TablePartitionArgs {
    #[serde(rename = "part_names", skip_serializing_if = "is_default")]
    pub PartNames: Vec<String>,
    #[serde(rename = "part_info", skip_serializing_if = "is_default")]
    pub PartInfo: Option<Box<PartitionInfo>>,
    #[serde(rename = "old_physical_tbl_ids", skip_serializing_if = "is_default")]
    pub OldPhysicalTblIDs: Vec<i64>,
    #[serde(rename = "old_global_indexes", skip_serializing_if = "is_default")]
    pub OldGlobalIndexes: Vec<TableIDIndexID>,
    #[serde(skip)]
    pub NewPartitionIDs: Vec<i64>,
}
impl JobArgs for TablePartitionArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionAddTablePartition {
            vec![arg(&self.PartInfo)]
        } else if job.tp == ActionDropTablePartition {
            vec![arg(&self.PartNames)]
        } else {
            vec![arg(&self.PartNames), arg(&self.PartInfo)]
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        let mut info = Box::<PartitionInfo>::default();
        if job.tp == ActionAddTablePartition {
            if job.state == JobStateRollingback {
                job.decodeArgs(&mut self.PartNames).map_err(errors::Trace)?;
            } else {
                job.decodeArgs(&mut info).map_err(errors::Trace)?;
            }
        } else if job.tp == ActionDropTablePartition {
            job.decodeArgs(&mut self.PartNames).map_err(errors::Trace)?;
        } else {
            job.decodeArgs((&mut self.PartNames, &mut info))
                .map_err(errors::Trace)?;
        }
        self.PartInfo = Some(info);
        Ok(())
    }
}
impl FinishedJobArgs for TablePartitionArgs {
    fn getFinishedArgsV1(&self, job: &Job) -> Vec<DynArg> {
        intest::Assert(
            job.tp != ActionAddTablePartition || job.state == JobStateRollbackDone,
            "add table partition job should not call getFinishedArgsV1 if not rollback",
        );
        vec![arg(&self.OldPhysicalTblIDs), arg(&self.OldGlobalIndexes)]
    }
}
/// 解码分区操作普通参数。
pub fn GetTablePartitionArgs(job: &mut Job) -> JobArgResult<TablePartitionArgs> {
    let mut out: TablePartitionArgs = getOrDecodeArgs(Default::default(), job)?;
    // Drop partition 与 add-partition 回滚路径要求 PartInfo 非空，即使 V1 没持久化它。
    if out.PartInfo.is_none() {
        out.PartInfo = Some(Box::default());
    }
    Ok(out)
}
/// 解码分区操作完成态参数。
pub fn GetFinishedTablePartitionArgs(job: &mut Job) -> JobArgResult<TablePartitionArgs> {
    if job.version == JobVersion1 {
        let mut out = TablePartitionArgs::default();
        job.decodeArgs((&mut out.OldPhysicalTblIDs, &mut out.OldGlobalIndexes))
            .map_err(errors::Trace)?;
        return Ok(out);
    }
    getOrDecodeArgsV2(job)
}
/// 为加分区失败回滚填充必要参数。
pub fn FillRollbackArgsForAddPartition(job: &mut Job, args: &TablePartitionArgs) {
    intest::Assert(
        job.tp == ActionAddTablePartition,
        "only for add partition job",
    );
    let mut fake = Job {
        version: job.version,
        tp: ActionDropTablePartition,
        ..Default::default()
    };
    fake.FillArgs(TablePartitionArgs {
        PartNames: args.PartNames.clone(),
        ..Default::default()
    });
    job.args = if job.version == JobVersion1 {
        vec![arg(&args.PartNames)]
    } else {
        fake.args
    };
    job.raw_args = fake.raw_args;
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 交换分区与普通表的参数。
pub struct ExchangeTablePartitionArgs {
    #[serde(rename = "partition_id", skip_serializing_if = "is_default")]
    pub PartitionID: i64,
    #[serde(rename = "pt_schema_id", skip_serializing_if = "is_default")]
    pub PTSchemaID: i64,
    #[serde(rename = "pt_table_id", skip_serializing_if = "is_default")]
    pub PTTableID: i64,
    #[serde(rename = "partition_name", skip_serializing_if = "is_default")]
    pub PartitionName: String,
    #[serde(rename = "with_validation", skip_serializing_if = "is_default")]
    pub WithValidation: bool,
}
impl_simple_args!(
    ExchangeTablePartitionArgs,
    PartitionID,
    PTSchemaID,
    PTTableID,
    PartitionName,
    WithValidation
);
/// 解码交换分区参数。
pub fn GetExchangeTablePartitionArgs(job: &mut Job) -> JobArgResult<ExchangeTablePartitionArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改分区属性/放置策略等。
pub struct AlterTablePartitionArgs {
    #[serde(rename = "partition_id", skip_serializing_if = "is_default")]
    pub PartitionID: i64,
    #[serde(rename = "label_rule", skip_serializing_if = "is_default")]
    pub LabelRule: Option<Box<pdhttp::LabelRule>>,
    #[serde(rename = "policy_ref_info", skip_serializing_if = "is_default")]
    pub PolicyRefInfo: Option<Box<PolicyRefInfo>>,
}
impl JobArgs for AlterTablePartitionArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionAlterTablePartitionAttributes {
            vec![arg(&self.PartitionID), arg(&self.LabelRule)]
        } else {
            vec![arg(&self.PartitionID), arg(&self.PolicyRefInfo)]
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        if job.tp == ActionAlterTablePartitionAttributes {
            job.decodeArgs((&mut self.PartitionID, &mut self.LabelRule))
                .map_err(errors::Trace)
        } else {
            job.decodeArgs((&mut self.PartitionID, &mut self.PolicyRefInfo))
                .map_err(errors::Trace)
        }
    }
}
/// 解码修改分区参数。
pub fn GetAlterTablePartitionArgs(job: &mut Job) -> JobArgResult<AlterTablePartitionArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 单表重命名：旧库 ID 与新表名等。
pub struct RenameTableArgs {
    #[serde(rename = "old_schema_id", skip_serializing_if = "is_default")]
    pub OldSchemaID: i64,
    #[serde(rename = "old_schema_name", skip_serializing_if = "is_default")]
    pub OldSchemaName: ast::CIStr,
    #[serde(rename = "new_table_name", skip_serializing_if = "is_default")]
    pub NewTableName: ast::CIStr,
    #[serde(rename = "old_table_name", skip_serializing_if = "is_default")]
    pub OldTableName: ast::CIStr,
    #[serde(rename = "new_schema_id", skip_serializing_if = "is_default")]
    pub NewSchemaID: i64,
    #[serde(rename = "table_id", skip_serializing_if = "is_default")]
    pub TableID: i64,
    #[serde(skip)]
    pub OldSchemaIDForSchemaDiff: i64,
}
impl_simple_args!(RenameTableArgs, OldSchemaID, NewTableName, OldSchemaName);
/// 解码单表重命名参数。
pub fn GetRenameTableArgs(job: &mut Job) -> JobArgResult<RenameTableArgs> {
    let mut out: RenameTableArgs = getOrDecodeArgs(Default::default(), job)?;
    out.NewSchemaID = job.schema_id;
    Ok(out)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 资源组创建/修改/删除参数。
pub struct ResourceGroupArgs {
    #[serde(rename = "rg_info", skip_serializing_if = "is_default")]
    pub RGInfo: Option<Box<ResourceGroupInfo>>,
}
impl JobArgs for ResourceGroupArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        let info = self.RGInfo.as_ref().unwrap();
        match job.tp {
            ActionCreateResourceGroup => vec![arg(info), arg(&false)], // 第二项已无用途，仅为旧任务兼容。
            ActionAlterResourceGroup => vec![arg(info)],
            ActionDropResourceGroup => vec![arg(&info.Name)],
            _ => vec![],
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.RGInfo = Some(Box::default());
        let info = self.RGInfo.as_mut().unwrap();
        match job.tp {
            ActionCreateResourceGroup | ActionAlterResourceGroup => job.decodeArgs(info),
            ActionDropResourceGroup => job.decodeArgs(&mut info.Name),
            _ => Ok(()),
        }
        .map_err(errors::Trace)
    }
}
/// 解码资源组参数。
pub fn GetResourceGroupArgs(job: &mut Job) -> JobArgResult<ResourceGroupArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 重置自增/auto_random 基线。
pub struct RebaseAutoIDArgs {
    #[serde(rename = "new_base", skip_serializing_if = "is_default")]
    pub NewBase: i64,
    #[serde(rename = "force", skip_serializing_if = "is_default")]
    pub Force: bool,
}
impl_simple_args!(RebaseAutoIDArgs, NewBase, Force);
/// 解码 rebase auto ID 参数。
pub fn GetRebaseAutoIDArgs(job: &mut Job) -> JobArgResult<RebaseAutoIDArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改表注释。
pub struct ModifyTableCommentArgs {
    #[serde(rename = "comment", skip_serializing_if = "is_default")]
    pub Comment: String,
}
impl_simple_args!(ModifyTableCommentArgs, Comment);
/// 解码修改表注释参数。
pub fn GetModifyTableCommentArgs(job: &mut Job) -> JobArgResult<ModifyTableCommentArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改表字符集与排序规则。
pub struct ModifyTableCharsetAndCollateArgs {
    #[serde(rename = "to_charset", skip_serializing_if = "is_default")]
    pub ToCharset: String,
    #[serde(rename = "to_collate", skip_serializing_if = "is_default")]
    pub ToCollate: String,
    #[serde(rename = "needs_overwrite_cols", skip_serializing_if = "is_default")]
    pub NeedsOverwriteCols: bool,
}
impl_simple_args!(
    ModifyTableCharsetAndCollateArgs,
    ToCharset,
    ToCollate,
    NeedsOverwriteCols
);
/// 解码修改表字符集参数。
pub fn GetModifyTableCharsetAndCollateArgs(
    job: &mut Job,
) -> JobArgResult<ModifyTableCharsetAndCollateArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改索引可见性。
pub struct AlterIndexVisibilityArgs {
    #[serde(rename = "index_name", skip_serializing_if = "is_default")]
    pub IndexName: ast::CIStr,
    #[serde(rename = "invisible", skip_serializing_if = "is_default")]
    pub Invisible: bool,
}
impl_simple_args!(AlterIndexVisibilityArgs, IndexName, Invisible);
/// 解码索引可见性参数。
pub fn GetAlterIndexVisibilityArgs(job: &mut Job) -> JobArgResult<AlterIndexVisibilityArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 添加外键。
pub struct AddForeignKeyArgs {
    #[serde(rename = "fk_info", skip_serializing_if = "is_default")]
    pub FkInfo: Option<Box<FKInfo>>,
    #[serde(rename = "fk_check", skip_serializing_if = "is_default")]
    pub FkCheck: bool,
}
impl_simple_args!(AddForeignKeyArgs, FkInfo, FkCheck);
/// 解码加外键参数。
pub fn GetAddForeignKeyArgs(job: &mut Job) -> JobArgResult<AddForeignKeyArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 删除外键。
pub struct DropForeignKeyArgs {
    #[serde(rename = "fk_name", skip_serializing_if = "is_default")]
    pub FkName: ast::CIStr,
}
impl_simple_args!(DropForeignKeyArgs, FkName);
/// 解码删外键参数。
pub fn GetDropForeignKeyArgs(job: &mut Job) -> JobArgResult<DropForeignKeyArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 增删改列相关参数（含列位置、多列等）。
pub struct TableColumnArgs {
    #[serde(rename = "column_info", skip_serializing_if = "is_default")]
    pub Col: Option<Box<ColumnInfo>>,
    #[serde(rename = "position", skip_serializing_if = "is_default")]
    pub Pos: Option<Box<ast::ColumnPosition>>,
    #[serde(rename = "offset", skip_serializing_if = "is_default")]
    pub Offset: i32,
    #[serde(rename = "ignore_existence_err", skip_serializing_if = "is_default")]
    pub IgnoreExistenceErr: bool,
    #[serde(rename = "index_ids", skip_serializing_if = "is_default")]
    pub IndexIDs: Vec<i64>,
    #[serde(rename = "partition_ids", skip_serializing_if = "is_default")]
    pub PartitionIDs: Vec<i64>,
}
impl JobArgs for TableColumnArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionDropColumn {
            let col = self.Col.as_ref().unwrap();
            if !self.IndexIDs.is_empty() {
                vec![
                    arg(&col.Name),
                    arg(&self.IgnoreExistenceErr),
                    arg(&self.IndexIDs),
                    arg(&self.PartitionIDs),
                ]
            } else {
                vec![arg(&col.Name), arg(&self.IgnoreExistenceErr)]
            }
        } else {
            vec![
                arg(&self.Col),
                arg(&self.Pos),
                arg(&self.Offset),
                arg(&self.IgnoreExistenceErr),
            ]
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.Col = Some(Box::default());
        self.Pos = Some(Box::default());
        if job.tp == ActionDropColumn || job.state == JobStateRollingback {
            job.decodeArgs((
                &mut self.Col.as_mut().unwrap().Name,
                &mut self.IgnoreExistenceErr,
                &mut self.IndexIDs,
                &mut self.PartitionIDs,
            ))
            .map_err(errors::Trace)
        } else {
            job.decodeArgs((
                self.Col.as_mut().unwrap(),
                self.Pos.as_mut().unwrap(),
                &mut self.Offset,
                &mut self.IgnoreExistenceErr,
            ))
            .map_err(errors::Trace)
        }
    }
}
/// 为加列回滚填充参数。
pub fn FillRollBackArgsForAddColumn(job: &mut Job, args: TableColumnArgs) {
    intest::Assert(job.tp == ActionAddColumn, "only for add column job");
    let mut fake = Job {
        version: job.version,
        tp: ActionDropColumn,
        ..Default::default()
    };
    fake.FillArgs(args);
    job.args = fake.args;
    job.raw_args = fake.raw_args;
}
/// 解码列变更参数。
pub fn GetTableColumnArgs(job: &mut Job) -> JobArgResult<TableColumnArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 批量重命名表：多组并行字段。
pub struct RenameTablesArgs {
    #[serde(rename = "rename_table_infos", skip_serializing_if = "is_default")]
    pub RenameTableInfos: Vec<RenameTableArgs>,
}
impl JobArgs for RenameTablesArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        let mut old_schema_ids = vec![];
        let mut old_schema_names = vec![];
        let mut old_table_names = vec![];
        let mut new_schema_ids = vec![];
        let mut new_table_names = vec![];
        let mut table_ids = vec![];
        for info in &self.RenameTableInfos {
            old_schema_ids.push(info.OldSchemaID);
            old_schema_names.push(info.OldSchemaName.clone());
            old_table_names.push(info.OldTableName.clone());
            new_schema_ids.push(info.NewSchemaID);
            new_table_names.push(info.NewTableName.clone());
            table_ids.push(info.TableID);
        }
        vec![
            arg(&old_schema_ids),
            arg(&new_schema_ids),
            arg(&new_table_names),
            arg(&table_ids),
            arg(&old_schema_names),
            arg(&old_table_names),
        ]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        let (mut a, mut b, mut c, mut d, mut e, mut f) =
            (vec![], vec![], vec![], vec![], vec![], vec![]);
        job.decodeArgs((&mut a, &mut d, &mut e, &mut f, &mut b, &mut c))
            .map_err(errors::Trace)?;
        // 8.1 及更早节点可能错误删掉 oldTableNames 尾参数；补等长空值以维持索引对齐。
        if c.is_empty() && !a.is_empty() {
            c.resize(a.len(), ast::CIStr::default());
        }
        self.RenameTableInfos = GetRenameTablesArgsFromV1(a, b, c, d, e, f);
        Ok(())
    }
}
/// 从 V1 并行数组字段组装 RenameTables 条目。
pub fn GetRenameTablesArgsFromV1(
    old_schema_ids: Vec<i64>,
    old_schema_names: Vec<ast::CIStr>,
    old_table_names: Vec<ast::CIStr>,
    new_schema_ids: Vec<i64>,
    new_table_names: Vec<ast::CIStr>,
    table_ids: Vec<i64>,
) -> Vec<RenameTableArgs> {
    old_schema_ids
        .into_iter()
        .enumerate()
        .map(|(i, id)| RenameTableArgs {
            OldSchemaID: id,
            OldSchemaName: old_schema_names[i].clone(),
            OldTableName: old_table_names[i].clone(),
            NewSchemaID: new_schema_ids[i],
            NewTableName: new_table_names[i].clone(),
            TableID: table_ids[i],
            ..Default::default()
        })
        .collect()
}
/// 解码批量重命名参数。
pub fn GetRenameTablesArgs(job: &mut Job) -> JobArgResult<RenameTablesArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改序列选项。
pub struct AlterSequenceArgs {
    #[serde(rename = "ident", skip_serializing_if = "is_default")]
    pub Ident: ast::Ident,
    #[serde(rename = "seq_options", skip_serializing_if = "is_default")]
    pub SeqOptions: Vec<ast::SequenceOption>,
}
impl_simple_args!(AlterSequenceArgs, Ident, SeqOptions);
/// 解码修改序列参数。
pub fn GetAlterSequenceArgs(job: &mut Job) -> JobArgResult<AlterSequenceArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改表 auto ID 缓存大小。
pub struct ModifyTableAutoIDCacheArgs {
    #[serde(rename = "new_cache", skip_serializing_if = "is_default")]
    pub NewCache: i64,
}
impl_simple_args!(ModifyTableAutoIDCacheArgs, NewCache);
/// 解码 auto ID cache 参数。
pub fn GetModifyTableAutoIDCacheArgs(job: &mut Job) -> JobArgResult<ModifyTableAutoIDCacheArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 设置 row ID 分片位数。
pub struct ShardRowIDArgs {
    #[serde(rename = "shard_row_id_bits", skip_serializing_if = "is_default")]
    pub ShardRowIDBits: u64,
}
impl_simple_args!(ShardRowIDArgs, ShardRowIDBits);
/// 解码 shard row ID 参数。
pub fn GetShardRowIDArgs(job: &mut Job) -> JobArgResult<ShardRowIDArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改或移除表 TTL 配置。
pub struct AlterTTLInfoArgs {
    #[serde(rename = "ttl_info", skip_serializing_if = "is_default")]
    pub TTLInfo: Option<Box<TTLInfo>>,
    #[serde(rename = "ttl_enable", skip_serializing_if = "is_default")]
    pub TTLEnable: Option<bool>,
    #[serde(rename = "ttl_cron_job_schedule", skip_serializing_if = "is_default")]
    pub TTLCronJobSchedule: Option<String>,
}
impl_simple_args!(AlterTTLInfoArgs, TTLInfo, TTLEnable, TTLCronJobSchedule);
/// 解码 TTL 变更参数。
pub fn GetAlterTTLInfoArgs(job: &mut Job) -> JobArgResult<AlterTTLInfoArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 删除/修改 CHECK 约束的通用字段。
pub struct CheckConstraintArgs {
    #[serde(rename = "constraint_name", skip_serializing_if = "is_default")]
    pub ConstraintName: ast::CIStr,
    #[serde(rename = "enforced", skip_serializing_if = "is_default")]
    pub Enforced: bool,
}
impl_simple_args!(CheckConstraintArgs, ConstraintName, Enforced);
/// 解码 CHECK 约束参数。
pub fn GetCheckConstraintArgs(job: &mut Job) -> JobArgResult<CheckConstraintArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 添加 CHECK 约束。
pub struct AddCheckConstraintArgs {
    #[serde(rename = "constraint_info")]
    pub Constraint: Option<Box<ConstraintInfo>>,
}
impl JobArgs for AddCheckConstraintArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(&self.Constraint)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.Constraint = Some(Box::default());
        job.decodeArgs(&mut self.Constraint).map_err(errors::Trace)
    }
}
/// 解码加 CHECK 约束参数。
pub fn GetAddCheckConstraintArgs(job: &mut Job) -> JobArgResult<AddCheckConstraintArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改表 Placement Policy 引用。
pub struct AlterTablePlacementArgs {
    #[serde(rename = "placement_policy_ref", skip_serializing_if = "is_default")]
    pub PlacementPolicyRef: Option<Box<PolicyRefInfo>>,
}
impl_simple_args!(AlterTablePlacementArgs, PlacementPolicyRef);
/// 解码表 Placement 参数。
pub fn GetAlterTablePlacementArgs(job: &mut Job) -> JobArgResult<AlterTablePlacementArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 设置 TiFlash 副本数与标签。
pub struct SetTiFlashReplicaArgs {
    #[serde(rename = "tiflash_replica", skip_serializing_if = "is_default")]
    pub TiflashReplica: ast::TiFlashReplicaSpec,
    #[serde(rename = "reset_available", skip_serializing_if = "is_default")]
    pub ResetAvailable: bool,
}
// ResetAvailable 仅存在于 V2，V1 位置数组仍只包含副本规格。
impl_simple_args!(SetTiFlashReplicaArgs, TiflashReplica);
/// 解码设置 TiFlash 副本参数。
pub fn GetSetTiFlashReplicaArgs(job: &mut Job) -> JobArgResult<SetTiFlashReplicaArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 更新 TiFlash 副本可用状态。
pub struct UpdateTiFlashReplicaStatusArgs {
    #[serde(rename = "available", skip_serializing_if = "is_default")]
    pub Available: bool,
    #[serde(rename = "physical_id", skip_serializing_if = "is_default")]
    pub PhysicalID: i64,
}
impl_simple_args!(UpdateTiFlashReplicaStatusArgs, Available, PhysicalID);
/// 解码 TiFlash 状态更新参数。
pub fn GetUpdateTiFlashReplicaStatusArgs(
    job: &mut Job,
) -> JobArgResult<UpdateTiFlashReplicaStatusArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 锁表/解锁：会话与锁类型列表。
pub struct LockTablesArgs {
    #[serde(rename = "lock_tables", skip_serializing_if = "is_default")]
    pub LockTables: Vec<TableLockTpInfo>,
    #[serde(rename = "index_of_lock", skip_serializing_if = "is_default")]
    pub IndexOfLock: i32,
    #[serde(rename = "unlock_tables", skip_serializing_if = "is_default")]
    pub UnlockTables: Vec<TableLockTpInfo>,
    #[serde(rename = "index_of_unlock", skip_serializing_if = "is_default")]
    pub IndexOfUnlock: i32,
    #[serde(rename = "session_info", skip_serializing_if = "is_default")]
    pub SessionInfo: SessionInfo,
    #[serde(rename = "is_cleanup:omitempty")]
    pub IsCleanup: bool,
}
impl JobArgs for LockTablesArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(self)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        job.decodeArgs(self).map_err(errors::Trace)
    }
}
/// 解码锁表参数。
pub fn GetLockTablesArgs(job: &mut Job) -> JobArgResult<LockTablesArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改表模式（Normal/Import/Restore 等）。
pub struct AlterTableModeArgs {
    #[serde(rename = "table_mode", skip_serializing_if = "is_default")]
    pub TableMode: TableMode,
    #[serde(rename = "schema_id", skip_serializing_if = "is_default")]
    pub SchemaID: i64,
    #[serde(rename = "table_id", skip_serializing_if = "is_default")]
    pub TableID: i64,
}
impl JobArgs for AlterTableModeArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(self)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        job.decodeArgs(self).map_err(errors::Trace)
    }
}
/// 解码改表模式参数。
pub fn GetAlterTableModeArgs(job: &mut Job) -> JobArgResult<AlterTableModeArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修复表：用新 TableInfo 覆盖损坏元数据。
pub struct RepairTableArgs {
    #[serde(rename = "table_info")]
    pub TableInfo: Option<Box<TableInfo>>,
}
impl_simple_args!(RepairTableArgs, TableInfo);
/// 解码修复表参数。
pub fn GetRepairTableArgs(job: &mut Job) -> JobArgResult<RepairTableArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改表属性（含 PD label rule 等）。
pub struct AlterTableAttributesArgs {
    #[serde(rename = "label_rule", skip_serializing_if = "is_default")]
    pub LabelRule: Option<Box<pdhttp::LabelRule>>,
}
impl JobArgs for AlterTableAttributesArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(&self.LabelRule)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.LabelRule = Some(Box::default());
        job.decodeArgs(self.LabelRule.as_mut().unwrap())
            .map_err(errors::Trace)
    }
}
/// 解码表属性参数。
pub fn GetAlterTableAttributesArgs(job: &mut Job) -> JobArgResult<AlterTableAttributesArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 恢复表/库：快照 TS、自增 ID 与旧名称等。
pub struct RecoverArgs {
    #[serde(rename = "recover_info", skip_serializing_if = "is_default")]
    pub RecoverInfo: Option<Box<RecoverSchemaInfo>>,
    #[serde(rename = "check_flag", skip_serializing_if = "is_default")]
    pub CheckFlag: i64,
    #[serde(skip)]
    pub AffectedPhysicalIDs: Vec<i64>,
}
impl JobArgs for RecoverArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionRecoverTable {
            vec![arg(&self.RecoverTableInfos()[0]), arg(&self.CheckFlag)]
        } else {
            vec![arg(&self.RecoverInfo), arg(&self.CheckFlag)]
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        let mut schema = RecoverSchemaInfo::default();
        if job.tp == ActionRecoverTable {
            let mut table = RecoverTableInfo::default();
            job.decodeArgs((&mut table, &mut self.CheckFlag))
                .map_err(errors::Trace)?;
            schema.RecoverTableInfos = vec![table];
        } else {
            job.decodeArgs((&mut schema, &mut self.CheckFlag))
                .map_err(errors::Trace)?;
        }
        self.RecoverInfo = Some(Box::new(schema));
        Ok(())
    }
}
impl RecoverArgs {
    /// 返回待恢复表信息切片。
    pub fn RecoverTableInfos(&self) -> &[RecoverTableInfo] {
        &self.RecoverInfo.as_ref().unwrap().RecoverTableInfos
    }
}
/// 解码恢复参数。
pub fn GetRecoverArgs(job: &mut Job) -> JobArgResult<RecoverArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// Placement Policy 创建/修改/删除参数。
pub struct PlacementPolicyArgs {
    #[serde(rename = "policy", skip_serializing_if = "is_default")]
    pub Policy: Option<Box<PolicyInfo>>,
    #[serde(rename = "replace_on_exist", skip_serializing_if = "is_default")]
    pub ReplaceOnExist: bool,
    #[serde(rename = "policy_name", skip_serializing_if = "is_default")]
    pub PolicyName: ast::CIStr,
    #[serde(rename = "policy_id")]
    pub PolicyID: i64,
}
impl JobArgs for PlacementPolicyArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        match job.tp {
            ActionCreatePlacementPolicy => vec![arg(&self.Policy), arg(&self.ReplaceOnExist)],
            ActionAlterPlacementPolicy => vec![arg(&self.Policy)],
            _ => vec![arg(&self.PolicyName)],
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.PolicyID = job.schema_id;
        match job.tp {
            ActionCreatePlacementPolicy => {
                job.decodeArgs((&mut self.Policy, &mut self.ReplaceOnExist))
            }
            ActionAlterPlacementPolicy => job.decodeArgs(&mut self.Policy),
            _ => job.decodeArgs(&mut self.PolicyName),
        }
        .map_err(errors::Trace)
    }
}
/// 解码 Placement Policy 参数。
pub fn GetPlacementPolicyArgs(job: &mut Job) -> JobArgResult<PlacementPolicyArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 脱敏策略创建/修改/删除参数。
pub struct MaskingPolicyArgs {
    #[serde(rename = "policy", skip_serializing_if = "is_default")]
    pub Policy: Option<Box<MaskingPolicyInfo>>,
    #[serde(rename = "replace_on_exist", skip_serializing_if = "is_default")]
    pub ReplaceOnExist: bool,
    #[serde(rename = "policy_name", skip_serializing_if = "is_default")]
    pub PolicyName: ast::CIStr,
    #[serde(rename = "policy_id")]
    pub PolicyID: i64,
}
impl JobArgs for MaskingPolicyArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        match job.tp {
            ActionCreateMaskingPolicy => vec![arg(&self.Policy), arg(&self.ReplaceOnExist)],
            ActionAlterMaskingPolicy => vec![arg(&self.Policy)],
            _ => vec![arg(&self.PolicyName)],
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.PolicyID = job.schema_id;
        match job.tp {
            ActionCreateMaskingPolicy => {
                job.decodeArgs((&mut self.Policy, &mut self.ReplaceOnExist))
            }
            ActionAlterMaskingPolicy => job.decodeArgs(&mut self.Policy),
            _ => job.decodeArgs(&mut self.PolicyName),
        }
        .map_err(errors::Trace)
    }
}
/// 解码脱敏策略参数。
pub fn GetMaskingPolicyArgs(job: &mut Job) -> JobArgResult<MaskingPolicyArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 设置列默认值。
pub struct SetDefaultValueArgs {
    #[serde(rename = "column_info", skip_serializing_if = "is_default")]
    pub Col: Option<Box<ColumnInfo>>,
}
impl JobArgs for SetDefaultValueArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(&self.Col)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        self.Col = Some(Box::default());
        job.decodeArgs(self.Col.as_mut().unwrap())
            .map_err(errors::Trace)
    }
}
/// 解码设置默认值参数。
pub fn GetSetDefaultValueArgs(job: &mut Job) -> JobArgResult<SetDefaultValueArgs> {
    getOrDecodeArgs(Default::default(), job)
}

// KeyRange 从 kv.KeyRange 复制，以避免 model 与 kv 形成循环依赖。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 键范围：[StartKey, EndKey)。
pub struct KeyRange {
    #[serde(rename = "start_key", skip_serializing_if = "is_default")]
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 集群 Flashback：目标 TS 与 GC/分析/TTL 等开关快照。
pub struct FlashbackClusterArgs {
    #[serde(rename = "flashback_ts", skip_serializing_if = "is_default")]
    pub FlashbackTS: u64,
    #[serde(rename = "pd_schedule_value", skip_serializing_if = "is_default")]
    pub PDScheduleValue: HashMap<String, DynArg>,
    #[serde(rename = "enable_gc", skip_serializing_if = "is_default")]
    pub EnableGC: bool,
    #[serde(rename = "enable_auto_analyze", skip_serializing_if = "is_default")]
    pub EnableAutoAnalyze: bool,
    #[serde(rename = "enable_ttl_job", skip_serializing_if = "is_default")]
    pub EnableTTLJob: bool,
    #[serde(rename = "super_read_only", skip_serializing_if = "is_default")]
    pub SuperReadOnly: bool,
    #[serde(rename = "locked_region_cnt", skip_serializing_if = "is_default")]
    pub LockedRegionCnt: u64,
    #[serde(rename = "start_ts", skip_serializing_if = "is_default")]
    pub StartTS: u64,
    #[serde(rename = "commit_ts", skip_serializing_if = "is_default")]
    pub CommitTS: u64,
    #[serde(rename = "key_ranges", skip_serializing_if = "is_default")]
    pub FlashbackKeyRanges: Vec<KeyRange>,
}
impl JobArgs for FlashbackClusterArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        let on_off = |v| if v { "ON" } else { "OFF" };
        vec![
            arg(&self.FlashbackTS),
            arg(&self.PDScheduleValue),
            arg(&self.EnableGC),
            arg(&on_off(self.EnableAutoAnalyze)),
            arg(&on_off(self.SuperReadOnly)),
            arg(&self.LockedRegionCnt),
            arg(&self.StartTS),
            arg(&self.CommitTS),
            arg(&on_off(self.EnableTTLJob)),
            arg(&self.FlashbackKeyRanges),
        ]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        let (mut analyze, mut read_only, mut ttl) = (String::new(), String::new(), String::new());
        job.decodeArgs((
            &mut self.FlashbackTS,
            &mut self.PDScheduleValue,
            &mut self.EnableGC,
            &mut analyze,
            &mut read_only,
            &mut self.LockedRegionCnt,
            &mut self.StartTS,
            &mut self.CommitTS,
            &mut ttl,
            &mut self.FlashbackKeyRanges,
        ))
        .map_err(errors::Trace)?;
        self.EnableAutoAnalyze = analyze == "ON";
        self.SuperReadOnly = read_only == "ON";
        self.EnableTTLJob = ttl == "ON";
        Ok(())
    }
}
/// 解码 Flashback 集群参数。
pub fn GetFlashbackClusterArgs(job: &mut Job) -> JobArgResult<FlashbackClusterArgs> {
    getOrDecodeArgs(Default::default(), job)
}

/// 索引操作类型：加索引 / 删索引 / 回滚加索引。
pub type IndexOp = u8;
/// 加索引操作。
pub const OpAddIndex: IndexOp = 0;
/// 删索引操作。
pub const OpDropIndex: IndexOp = 1;
/// 回滚加索引操作。
pub const OpRollbackAddIndex: IndexOp = 2;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 单个索引参数：名称、唯一性、全局性、列式索引类型等。
pub struct IndexArg {
    #[serde(skip)]
    pub Global: bool,
    #[serde(rename = "unique", skip_serializing_if = "is_default")]
    pub Unique: bool,
    #[serde(rename = "index_name", skip_serializing_if = "is_default")]
    pub IndexName: ast::CIStr,
    #[serde(rename = "index_part_specifications")]
    pub IndexPartSpecifications: Vec<ast::IndexPartSpecification>,
    #[serde(rename = "index_option", skip_serializing_if = "is_default")]
    pub IndexOption: Option<Box<ast::IndexOption>>,
    #[serde(rename = "hidden_cols", skip_serializing_if = "is_default")]
    pub HiddenCols: Vec<ColumnInfo>,
    #[serde(rename = "func_expr", skip_serializing_if = "is_default")]
    pub FuncExpr: String,
    #[serde(rename = "is_vector", skip_serializing_if = "is_default")]
    pub IsColumnar: bool,
    #[serde(rename = "columnar_index_type", skip_serializing_if = "is_default")]
    pub ColumnarIndexType: ColumnarIndexType,
    #[serde(rename = "is_pk", skip_serializing_if = "is_default")]
    pub IsPK: bool,
    #[serde(rename = "sql_mode", skip_serializing_if = "is_default")]
    pub SQLMode: mysql::SQLMode,
    #[serde(rename = "index_id", skip_serializing_if = "is_default")]
    pub IndexID: i64,
    #[serde(rename = "if_exist", skip_serializing_if = "is_default")]
    pub IfExist: bool,
    #[serde(rename = "is_global", skip_serializing_if = "is_default")]
    pub IsGlobal: bool,
    #[serde(rename = "split_opt", skip_serializing_if = "is_default")]
    pub SplitOpt: Option<Box<IndexArgSplitOpt>>,
    #[serde(rename = "condition_string", skip_serializing_if = "is_default")]
    pub ConditionString: String,
}
impl IndexArg {
    // 未显式写类型但旧字段 IsColumnar=true 时按向量索引解释，保持历史 JSON 兼容。
    pub fn GetColumnarIndexType(&self) -> ColumnarIndexType {
        if self.ColumnarIndexType == ColumnarIndexTypeNA && self.IsColumnar {
            ColumnarIndexTypeVector
        } else {
            self.ColumnarIndexType
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 索引构建时的 Region 预分裂选项。
pub struct IndexArgSplitOpt {
    #[serde(rename = "lower", skip_serializing_if = "is_default")]
    pub Lower: Vec<String>,
    #[serde(rename = "upper", skip_serializing_if = "is_default")]
    pub Upper: Vec<String>,
    #[serde(rename = "num", skip_serializing_if = "is_default")]
    pub Num: i64,
    #[serde(rename = "value_lists", skip_serializing_if = "is_default")]
    pub ValueLists: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 加/删/回滚索引的参数集合，含分区 ID 与操作类型。
pub struct ModifyIndexArgs {
    #[serde(rename = "index_args", skip_serializing_if = "is_default")]
    pub IndexArgs: Vec<IndexArg>,
    #[serde(rename = "partition_ids", skip_serializing_if = "is_default")]
    pub PartitionIDs: Vec<i64>,
    #[serde(skip)]
    pub OpType: IndexOp,
}
impl JobArgs for ModifyIndexArgs {
    fn getArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if job.tp == ActionRenameIndex {
            return vec![
                arg(&self.IndexArgs[0].IndexName),
                arg(&self.IndexArgs[1].IndexName),
            ];
        }
        if job.tp == ActionDropIndex || job.tp == ActionDropPrimaryKey {
            if self.IndexArgs.len() == 1 {
                return vec![
                    arg(&self.IndexArgs[0].IndexName),
                    arg(&self.IndexArgs[0].IfExist),
                ];
            }
            return vec![
                arg(&self
                    .IndexArgs
                    .iter()
                    .map(|a| &a.IndexName)
                    .collect::<Vec<_>>()),
                arg(&self.IndexArgs.iter().map(|a| a.IfExist).collect::<Vec<_>>()),
            ];
        }
        if job.tp == ActionAddColumnarIndex {
            let a = &self.IndexArgs[0];
            return vec![
                arg(&a.IndexName),
                arg(&a.IndexPartSpecifications[0]),
                arg(&a.IndexOption),
                arg(&a.FuncExpr),
                arg(&a.ColumnarIndexType),
            ];
        }
        if job.tp == ActionAddPrimaryKey {
            let a = &self.IndexArgs[0];
            // 第六项历史上被设置但从未读取，固定写 null 以兼容旧任务。
            return vec![
                arg(&a.Unique),
                arg(&a.IndexName),
                arg(&a.IndexPartSpecifications),
                arg(&a.IndexOption),
                arg(&a.SQLMode),
                Value::Null,
                arg(&a.Global),
            ];
        }
        let mut unique = Vec::with_capacity(self.IndexArgs.len());
        let mut names = Vec::with_capacity(self.IndexArgs.len());
        let mut parts = Vec::with_capacity(self.IndexArgs.len());
        let mut options = Vec::with_capacity(self.IndexArgs.len());
        let mut hidden = Vec::with_capacity(self.IndexArgs.len());
        let mut global = Vec::with_capacity(self.IndexArgs.len());
        for arg in &self.IndexArgs {
            unique.push(arg.Unique);
            names.push(arg.IndexName.clone());
            parts.push(arg.IndexPartSpecifications.clone());
            options.push(arg.IndexOption.clone());
            hidden.push(arg.HiddenCols.clone());
            global.push(arg.Global);
        }
        if self.IndexArgs.len() == 1 {
            vec![
                arg(&unique[0]),
                arg(&names[0]),
                arg(&parts[0]),
                arg(&options[0]),
                arg(&hidden[0]),
                arg(&global[0]),
            ]
        } else {
            vec![
                arg(&unique),
                arg(&names),
                arg(&parts),
                arg(&options),
                arg(&hidden),
                arg(&global),
            ]
        }
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        match job.tp {
            ActionRenameIndex => self.decodeRenameIndexV1(job),
            ActionAddIndex => self.decodeAddIndexV1(job),
            ActionAddColumnarIndex => self.decodeAddColumnarIndexV1(job),
            ActionAddPrimaryKey => self.decodeAddPrimaryKeyV1(job),
            _ => Err(errors::Errorf(format!(
                "Invalid job type for decoding {}",
                job.tp
            ))),
        }
    }
}
impl ModifyIndexArgs {
    fn decodeRenameIndexV1(&mut self, job: &Job) -> JobArgResult<()> {
        let (mut from, mut to) = Default::default();
        job.decodeArgs((&mut from, &mut to))
            .map_err(errors::Trace)?;
        self.IndexArgs = vec![
            IndexArg {
                IndexName: from,
                ..Default::default()
            },
            IndexArg {
                IndexName: to,
                ..Default::default()
            },
        ];
        Ok(())
    }
    fn decodeDropIndexV1(&mut self, job: &Job) -> JobArgResult<()> {
        let (mut names, mut exists) = (vec![ast::CIStr::default()], vec![false]);
        if job.decodeArgs((&mut names[0], &mut exists[0])).is_err() {
            job.decodeArgs((&mut names, &mut exists))
                .map_err(errors::Trace)?;
        }
        self.IndexArgs = names
            .into_iter()
            .enumerate()
            .map(|(i, n)| IndexArg {
                IndexName: n,
                IfExist: exists[i],
                ..Default::default()
            })
            .collect();
        Ok(())
    }
    fn decodeAddIndexV1(&mut self, job: &Job) -> JobArgResult<()> {
        let mut unique: Vec<bool> = vec![false];
        let mut names: Vec<ast::CIStr> = vec![Default::default()];
        let mut parts: Vec<Vec<ast::IndexPartSpecification>> = vec![vec![]];
        let mut options: Vec<Option<Box<ast::IndexOption>>> = vec![None];
        let mut hidden: Vec<Vec<ColumnInfo>> = vec![vec![]];
        let mut global: Vec<bool> = vec![false];
        if job
            .decodeArgs((
                &mut unique,
                &mut names,
                &mut parts,
                &mut options,
                &mut hidden,
                &mut global,
            ))
            .is_err()
        {
            job.decodeArgs((
                &mut unique[0],
                &mut names[0],
                &mut parts[0],
                &mut options[0],
                &mut hidden[0],
                &mut global[0],
            ))
            .map_err(errors::Trace)?;
        }
        self.IndexArgs = unique
            .into_iter()
            .enumerate()
            .map(|(i, u)| IndexArg {
                Unique: u,
                IndexName: names[i].clone(),
                IndexPartSpecifications: parts[i].clone(),
                IndexOption: options[i].clone(),
                HiddenCols: hidden[i].clone(),
                Global: global[i],
                ..Default::default()
            })
            .collect();
        Ok(())
    }
    fn decodeAddPrimaryKeyV1(&mut self, job: &Job) -> JobArgResult<()> {
        let mut a = IndexArg {
            IsPK: true,
            ..Default::default()
        };
        let mut unused = Value::Null;
        job.decodeArgs((
            &mut a.Unique,
            &mut a.IndexName,
            &mut a.IndexPartSpecifications,
            &mut a.IndexOption,
            &mut a.SQLMode,
            &mut unused,
            &mut a.Global,
        ))
        .map_err(errors::Trace)?;
        self.IndexArgs = vec![a];
        Ok(())
    }
    fn decodeAddColumnarIndexV1(&mut self, job: &Job) -> JobArgResult<()> {
        let (mut name, mut part, mut option, mut expr, mut kind) = Default::default();
        job.decodeArgs((&mut name, &mut part, &mut option, &mut expr, &mut kind))
            .map_err(errors::Trace)?;
        self.IndexArgs = vec![IndexArg {
            IndexName: name,
            IndexPartSpecifications: vec![part],
            IndexOption: option,
            FuncExpr: expr,
            IsColumnar: true,
            ColumnarIndexType: kind,
            ..Default::default()
        }];
        Ok(())
    }
    /// 返回重命名索引的旧名与新名。
    pub fn GetRenameIndexes(&self) -> (ast::CIStr, ast::CIStr) {
        (
            self.IndexArgs[0].IndexName.clone(),
            self.IndexArgs[1].IndexName.clone(),
        )
    }
}
impl FinishedJobArgs for ModifyIndexArgs {
    fn getFinishedArgsV1(&self, job: &Job) -> Vec<DynArg> {
        if self.OpType == OpAddIndex {
            if job.tp == ActionAddColumnarIndex {
                let a = &self.IndexArgs[0];
                return vec![
                    arg(&a.IndexID),
                    arg(&a.IfExist),
                    arg(&self.PartitionIDs),
                    arg(&a.IsGlobal),
                ];
            }
            return vec![
                arg(&self.IndexArgs.iter().map(|a| a.IndexID).collect::<Vec<_>>()),
                arg(&self.IndexArgs.iter().map(|a| a.IfExist).collect::<Vec<_>>()),
                arg(&self.PartitionIDs),
                arg(&self.IndexArgs.iter().map(|a| a.Global).collect::<Vec<_>>()),
            ];
        }
        if self.OpType == OpRollbackAddIndex {
            return vec![
                arg(&self
                    .IndexArgs
                    .iter()
                    .map(|a| &a.IndexName)
                    .collect::<Vec<_>>()),
                arg(&self.IndexArgs.iter().map(|a| a.IfExist).collect::<Vec<_>>()),
                arg(&self.PartitionIDs),
            ];
        }
        let a = &self.IndexArgs[0];
        vec![
            arg(&a.IndexName),
            arg(&a.IfExist),
            arg(&a.IndexID),
            arg(&self.PartitionIDs),
            arg(&a.IsColumnar),
        ]
    }
}
/// 解码加索引类参数。
pub fn GetModifyIndexArgs(job: &mut Job) -> JobArgResult<ModifyIndexArgs> {
    getOrDecodeArgs(Default::default(), job)
}
/// 解码删索引参数（复用 ModifyIndexArgs 布局）。
pub fn GetDropIndexArgs(job: &mut Job) -> JobArgResult<ModifyIndexArgs> {
    if job.version == JobVersion2 {
        return getOrDecodeArgsV2(job);
    }
    let mut out = ModifyIndexArgs::default();
    out.decodeDropIndexV1(job)?;
    Ok(out)
}
/// 解码索引变更完成态参数。
pub fn GetFinishedModifyIndexArgs(job: &mut Job) -> JobArgResult<ModifyIndexArgs> {
    if job.version == JobVersion2 {
        return getOrDecodeArgsV2(job);
    }
    let mut out = ModifyIndexArgs::default();
    if job.IsRollingback() || job.tp == ActionDropIndex || job.tp == ActionDropPrimaryKey {
        let (mut names, mut exists, mut ids) = (vec![ast::CIStr::default()], vec![false], vec![0]);
        let (mut partitions, mut columnar) = (vec![], false);
        if job.IsRollingback() {
            job.decodeArgs((&mut names, &mut exists, &mut partitions, &mut columnar))
        } else {
            job.decodeArgs((
                &mut names[0],
                &mut exists[0],
                &mut ids[0],
                &mut partitions,
                &mut columnar,
            ))
        }
        .map_err(errors::Trace)?;
        out.PartitionIDs = partitions;
        out.IndexArgs = names
            .into_iter()
            .enumerate()
            .map(|(i, n)| IndexArg {
                IndexName: n,
                IfExist: exists[i],
                IsColumnar: columnar,
                ..Default::default()
            })
            .collect();
        // V1 drop-index 目前只支持一个索引 ID。
        out.IndexArgs[0].IndexID = ids[0];
        return Ok(out);
    }
    let (mut ids, mut exists, mut globals, mut partitions) =
        (vec![0], vec![false], vec![false], vec![]);
    if job
        .decodeArgs((
            &mut ids[0],
            &mut exists[0],
            &mut partitions,
            &mut globals[0],
        ))
        .is_err()
    {
        job.decodeArgs((&mut ids, &mut exists, &mut partitions, &mut globals))
            .map_err(errors::Trace)?;
    }
    out.PartitionIDs = partitions;
    out.IndexArgs = ids
        .into_iter()
        .enumerate()
        .map(|(i, id)| IndexArg {
            IndexID: id,
            IfExist: exists[i],
            IsGlobal: globals[i],
            ..Default::default()
        })
        .collect();
    Ok(out)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改列：新旧列信息、重组类型与检查约束等。
pub struct ModifyColumnArgs {
    #[serde(rename = "column", skip_serializing_if = "is_default")]
    pub Column: Option<Box<ColumnInfo>>,
    #[serde(rename = "old_column_id", skip_serializing_if = "is_default")]
    pub OldColumnID: i64,
    #[serde(rename = "old_column_name", skip_serializing_if = "is_default")]
    pub OldColumnName: ast::CIStr,
    #[serde(rename = "position", skip_serializing_if = "is_default")]
    pub Position: Option<Box<ast::ColumnPosition>>,
    #[serde(rename = "modify_column_type", skip_serializing_if = "is_default")]
    pub ModifyColumnType: u8,
    #[serde(rename = "new_shard_bits", skip_serializing_if = "is_default")]
    pub NewShardBits: u64,
    #[serde(rename = "changing_column", skip_serializing_if = "is_default")]
    pub ChangingColumn: Option<Box<ColumnInfo>>,
    #[serde(skip)]
    pub ChangingIdxs: Vec<IndexInfo>,
    #[serde(rename = "removed_idxs", skip_serializing_if = "is_default")]
    pub RedundantIdxs: Vec<i64>,
    #[serde(rename = "index_ids", skip_serializing_if = "is_default")]
    pub IndexIDs: Vec<i64>,
    #[serde(rename = "new_index_ids", skip_serializing_if = "is_default")]
    pub NewIndexIDs: Vec<i64>,
    #[serde(rename = "partition_ids", skip_serializing_if = "is_default")]
    pub PartitionIDs: Vec<i64>,
}
impl JobArgs for ModifyColumnArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        let mut args = vec![
            arg(&self.Column),
            arg(&self.OldColumnName),
            arg(&self.Position),
            arg(&self.ModifyColumnType),
            arg(&self.NewShardBits),
        ];
        // 升级兼容：旧节点先期只认识五项；临时列存在时才追加运行期生成的四项。
        if self.ChangingColumn.is_some() {
            args.extend([
                arg(&self.ChangingColumn),
                arg(&self.ChangingIdxs),
                arg(&self.RedundantIdxs),
                arg(&self.OldColumnID),
            ]);
        }
        args
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        job.decodeArgs((
            &mut self.Column,
            &mut self.OldColumnName,
            &mut self.Position,
            &mut self.ModifyColumnType,
            &mut self.NewShardBits,
            &mut self.ChangingColumn,
            &mut self.ChangingIdxs,
            &mut self.RedundantIdxs,
            &mut self.OldColumnID,
        ))
        .map_err(errors::Trace)
    }
}
impl FinishedJobArgs for ModifyColumnArgs {
    fn getFinishedArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![
            arg(&self.IndexIDs),
            arg(&self.PartitionIDs),
            arg(&self.NewIndexIDs),
        ]
    }
}
/// 解码改列普通参数。
pub fn GetModifyColumnArgs(job: &mut Job) -> JobArgResult<ModifyColumnArgs> {
    getOrDecodeArgs(Default::default(), job)
}
/// 解码改列完成态参数。
pub fn GetFinishedModifyColumnArgs(job: &mut Job) -> JobArgResult<ModifyColumnArgs> {
    if job.version == JobVersion1 {
        let mut out = ModifyColumnArgs::default();
        job.decodeArgs((
            &mut out.IndexIDs,
            &mut out.PartitionIDs,
            &mut out.NewIndexIDs,
        ))
        .map_err(errors::Trace)?;
        return Ok(out);
    }
    getOrDecodeArgsV2(job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 刷新元信息：强制 infoschema 重新加载指定对象。
pub struct RefreshMetaArgs {
    #[serde(rename = "schema_id", skip_serializing_if = "is_default")]
    pub SchemaID: i64,
    #[serde(rename = "table_id", skip_serializing_if = "is_default")]
    pub TableID: i64,
    #[serde(rename = "involved_db", skip_serializing_if = "is_default")]
    pub InvolvedDB: String,
    #[serde(rename = "involved_table", skip_serializing_if = "is_default")]
    pub InvolvedTable: String,
}
impl JobArgs for RefreshMetaArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(self)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        job.decodeArgs(self).map_err(errors::Trace)
    }
}
/// 解码刷新元信息参数。
pub fn GetRefreshMetaArgs(job: &mut Job) -> JobArgResult<RefreshMetaArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
/// 修改表亲和性配置。
pub struct AlterTableAffinityArgs {
    #[serde(rename = "affinity", skip_serializing_if = "is_default")]
    pub Affinity: Option<Box<TableAffinityInfo>>,
}
impl JobArgs for AlterTableAffinityArgs {
    fn getArgsV1(&self, _: &Job) -> Vec<DynArg> {
        vec![arg(self)]
    }
    fn decodeV1(&mut self, job: &Job) -> JobArgResult<()> {
        job.decodeArgs(self).map_err(errors::Trace)
    }
}
/// 解码表亲和性参数。
pub fn GetAlterTableAffinityArgs(job: &mut Job) -> JobArgResult<AlterTableAffinityArgs> {
    getOrDecodeArgs(Default::default(), job)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// 设置表 Region 分裂策略。
pub struct AlterTableSetRegionSplitPolicyArgs {
    pub IndexName: String,
    pub Policy: Option<Box<RegionSplitPolicy>>,
}
impl_simple_args!(AlterTableSetRegionSplitPolicyArgs, IndexName, Policy);
/// 解码 Region 分裂策略参数。
pub fn GetAlterTableSetRegionSplitPolicyArgs(
    job: &mut Job,
) -> JobArgResult<AlterTableSetRegionSplitPolicyArgs> {
    getOrDecodeArgs(Default::default(), job)
}
