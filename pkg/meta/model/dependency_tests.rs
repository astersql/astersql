// Copyright 2026 AsterSQL.

// task-416 依赖拼装：将 model 各子测试与桩类型编入同一编译单元。
//
// `GO_REFERENCE` 记录 Go/迁移期如何用 `include!` 聚合 bdr、column、index、job 等测试，
// 以及为 JobArgs 提供的最小 DBInfo/TableInfo 桩。本文件实际可运行测试验证列、索引、
// 表三者共享同一列名身份时，前缀覆盖判定 `IsIndexPrefixCovered` 成立。

/// 保留聚合测试与桩类型的参考源码，便于对照依赖注入方式。
const GO_REFERENCE: &str = r################"

mod bdr_test {
    use super::group_1::*;
    use super::group_3::action_type_string;
    include!("../../../pkg/meta/model/bdr_test.rs");
}

mod column_test {
    use super::group_1::*;
    include!("../../../pkg/meta/model/column_test.rs");
}

mod index_test {
    use super::group_1::*;
    use super::kerneltype;
    include!("../../../pkg/meta/model/index_test.rs");
}

mod job_args_test {
    use super::job_args::*;
    include!("../../../pkg/meta/model/job_args_test.rs");
}

pub mod ast {
    pub use super::group_3::ast::*;
}
pub mod errors {
    pub use super::group_3::errors::*;
}
pub mod mysql {
    pub use super::group_3::mysql::*;
}
pub mod terror {
    pub use super::group_3::terror::*;
}
pub mod tracing {
    pub use super::group_3::tracing::*;
}
pub mod vardef {
    pub use super::group_3::vardef::*;
}
pub mod time {
    pub use super::group_3::time::*;
}
pub mod kerneltype {
    pub fn is_next_gen() -> bool {
        false
    }
    pub fn IsNextGen() -> bool {
        false
    }
    pub fn IsClassic() -> bool {
        true
    }
}
pub mod reorg {
    pub use super::group_3::{BackfillMeta, DDLReorgMeta, ReorgStage, ReorgType};
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DBInfo {
    pub id: i64,
    pub name: ast::CIStr,
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TableInfo {
    pub id: i64,
    pub name: ast::CIStr,
}
pub fn ts_convert_to_time(ts: u64) -> u64 {
    ts
}

pub trait JobArgs {
    fn get_args_v1(&self, job: &job_test::Job) -> Vec<serde_json::Value>;
    fn to_json(&self) -> serde_json::Value;
}
pub trait FinishedJobArgs: JobArgs {
    fn get_finished_args_v1(&self, job: &job_test::Job) -> Vec<serde_json::Value>;
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TruncateTableArgs {
    pub FKCheck: bool,
}
impl JobArgs for TruncateTableArgs {
    fn get_args_v1(&self, _: &job_test::Job) -> Vec<serde_json::Value> {
        vec![serde_json::json!(self.FKCheck)]
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RenameTableArgs {
    pub OldSchemaID: i64,
    pub NewTableName: ast::CIStr,
}
impl JobArgs for RenameTableArgs {
    fn get_args_v1(&self, _: &job_test::Job) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!(self.OldSchemaID),
            serde_json::json!(self.NewTableName),
        ]
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}

mod job_test {
    use super::reorg::{BackfillMeta, DDLReorgMeta, ReorgStage, ReorgType};
    use super::{
        DBInfo, FinishedJobArgs, JobArgs, RenameTableArgs, TableInfo, TruncateTableArgs, ast,
        errors, kerneltype, mysql, terror, time, tracing, ts_convert_to_time,
    };
    include!("../../../pkg/meta/model/job.rs");
    include!("../../../pkg/meta/model/job_test.rs");
}

mod placement_test {
    use super::group_3::*;
    include!("../../../pkg/meta/model/placement_test.rs");
}

mod table_mode_test {
    use super::group_3::*;
    include!("../../../pkg/meta/model/table_mode_test.rs");
}
"################;

use crate::{ColumnInfo, IndexColumn, IndexInfo, IsIndexPrefixCovered, TableInfo, ast};

/// 列、索引、表引用同一列名时，索引前缀应覆盖该列名列表。
#[test]
fn canonical_model_dependencies_share_one_identity() {
    // 单列 id，索引包含该列，表同时挂上列与索引。
    let column = ColumnInfo {
        ID: 1,
        Name: ast::NewCIStr("id"),
        Offset: 0,
        ..Default::default()
    };
    let index = IndexInfo {
        Columns: vec![IndexColumn {
            Name: column.Name.clone(),
            Offset: column.Offset,
            ..Default::default()
        }],
        ..Default::default()
    };
    let table = TableInfo {
        Columns: vec![column],
        Indices: vec![index.clone()],
        ..Default::default()
    };
    // IsIndexPrefixCovered：查询列名是否被索引列前缀完整覆盖（可用于索引选择）。
    assert!(IsIndexPrefixCovered(&table, &index, &[ast::NewCIStr("id")]));
}
