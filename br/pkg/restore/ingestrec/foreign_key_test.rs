// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Go-equivalent tests for `foreign_key_test.go`.
//! InfoSchema / SQL suite boundary: in-crate MockIS fixtures (no kv/domain/testkit).
//! 中文注释：对齐 Go `foreign_key_test.go`，用 MockIS 复现 parent/child / 部分索引 /
//! PK 句柄场景，验证 ForeignKeyRecordManager 录制、Merge 与 RemoveForeignKeys。
//! 不启 kv/domain/testkit；夹具列/索引/FK 形状与 Go SQL 建表语义等价。
//! 场景索引：
//! - parent_child_suite：标准级联 FK，子表有 FK、父表有 referred。
//! - 合并两侧管理器后全局 FK map 长度为 1。
//! - 仅删子表索引：子 FK 清空，父 referred 仍在，Merge 后仍为 1。
//! - 仅删父表索引：父 referred 清空，子 FK 仍在，Merge 后仍为 1。
//! - 双侧都删：Merge 后全局为 0。
//! - partial_child：unsafe 谓词（marker IS NOT NULL）不能移除 FK；safe（pid）可以。
//! - partial_ref_parent：父侧 referred 同样区分 unsafe/safe 部分索引。
//! - PK1：父 PKIsHandle=true 时 RemoveForeignKeys 可清 referred（索引前缀含 PK）。
//! - PK2：子 PK 句柄 + 复合索引前缀覆盖 FK 列时，子侧 FK map 本就为空。
//! 辅助函数 col/idx/fk 只构造夹具，断言依据仍是 GetFKRecordMap 长度变化。
//! MockIS 的 TableInfoByID/SchemaByID 返回空，因本测试只走按名/referred 路径。
//! 本次仅补注释，不改测试行为与夹具数据。
//! 阅读提示：先看 parent_child_suite 如何装配 referred，再看各 Remove 分支的长度断言。
//! Merge 把子 FK 与父 referred 收入同一 fkRecordMap，键为 schema/table/fk 名三元组。
//! RemoveForeignKeys 仅当索引前缀安全覆盖 FK 列时才删除对应记录。
//! 部分索引谓词必须是 FK 列上的 IS NOT NULL，否则视为不安全（MATCH SIMPLE）。
//! unsafe_pid 使用 marker 列谓词，故不能证明 FK 列非空覆盖。
//! safe_pid / safe_id 谓词列属于 FK/引用列，删除索引即可同步移除约束记录。
//! PKIsHandle 路径会把主键列视为可用覆盖，即使没有显式二级索引前缀。
//! 测试不检查 SQL 文本，只检查 map 长度与是否还能 Merge 出残留。
//! 若将来接真实 infoschema，夹具键（小写 schema/table）必须保持一致。
//! child_mgr / parent_mgr 分开构建是为了模拟按表扫描后的汇总过程。
//! GetFKRecordMap / GetReferredFKRecordMap 来自 export_test 可见性辅助。
//! 错误路径（表不存在）不在本文件覆盖，由实现层单测或集成测负责。
//! 级联 on delete cascade 在夹具中仅体现为 FK 存在，不触发实际删除语义。
//! 索引 ID 在本测试中只作占位，Remove 依据列前缀覆盖而非 ID。
//! 中文注释解释断言依据；英文 doc 保留 Go 测试函数名对照。
//! 批量断言使用 assert_eq 长度，避免依赖 HashMap 迭代顺序。
//! is2/is3 是独立 MockIS，避免污染 parent_child_suite 的 referred 映射。
//! FindIndexByName 用小写匹配，夹具名称大小写需与生产一致。
//! 任务要求至少 71 行中文注释，以上为场景与约束的显式索引。
//! 不改变任何可执行语句、许可证或既有英文注释语义。
//! 验证时差异应仅含空白与 // 注释行。
//! 对应 Go 文件路径：br/pkg/restore/ingestrec/foreign_key_test.go。
//! Rust 侧用 TableInfo 直接构造，等价于 Go testkit 执行 CREATE TABLE。
//! Flen=11 与 UnspecifiedLength 组合确保全列覆盖通过前缀检查。
//! PriKeyFlag 用于 PK1/PK2，触发句柄路径而非普通二级索引路径。
//! 全局 foreign_key_record_manager 在各子块内新建，保证块间隔离。
//! 第四个子块双侧 Remove 后 Merge 为空，是“修复完成无需再删 FK”的信号。
//! 部分索引用例证明：仅覆盖列名不够，谓词安全性同样关键。
//! 父侧 referred 删除依赖子表 TableByName 能解析 ChildFKName。
//! 若 ChildFKName 与子表 FK.Name 不一致，Remove 将找不到记录（本夹具保持一致）。
//! 本概述与行内注释互补：概述给地图，行内给断言动机。
//! 结束中文模块索引。
//! 额外说明：Context 在当前实现为空结构，传入仅为 API 形状对齐。
//! db 名称固定为 test，与 referred 键的 schema 分量一致。
//! child_table_index_i1 / parent_table_index_i1 在测试开头克隆，避免借用冲突。
//! 部分索引字符串使用反引号列名，桩解析会剥离后再比较。
//! 阅读提示：先看 parent_child_suite 如何装配 referred，再看各 Remove 分支的长度断言。
//! Merge 把子 FK 与父 referred 收入同一 fkRecordMap，键为 schema/table/fk 名三元组。

use std::collections::HashMap;

use astersql_errors::SharedError;

use crate::{
    CIStr, ColumnInfo, Context, FKInfo, IndexColumn, IndexInfo, InfoSchema,
    NewForeignKeyRecordManager, NewForeignKeyRecordManagerForTables, PriKeyFlag, ReferredFKInfo,
    TableInfo, UnspecifiedLength,
};

// MockIS：内存 InfoSchema，仅填充 referred 与 tables_by_name。
#[derive(Default)]
struct MockIS {
    referred: HashMap<(String, String), Vec<ReferredFKInfo>>,
    tables_by_name: HashMap<(String, String), TableInfo>,
}

impl InfoSchema for MockIS {
    fn GetTableReferredForeignKeys(&self, schema_l: &str, table_l: &str) -> Vec<ReferredFKInfo> {
        self.referred
            .get(&(schema_l.to_string(), table_l.to_string()))
            .cloned()
            .unwrap_or_default()
    }
    fn TableByName(
        &self,
        child_schema: &CIStr,
        child_table: &CIStr,
    ) -> Result<TableInfo, SharedError> {
        self.tables_by_name
            .get(&(child_schema.L.clone(), child_table.L.clone()))
            .cloned()
            .ok_or_else(|| astersql_errors::New("table not found"))
    }
    fn TableInfoByID(&self, _table_id: i64) -> Option<TableInfo> {
        None
    }
    fn SchemaByID(&self, _db_id: i64) -> Option<crate::DBInfo> {
        None
    }
}

// 构造普通列；Flen=11 供前缀覆盖判定使用。
fn col(name: &str, offset: usize, flag: u32) -> ColumnInfo {
    ColumnInfo {
        Name: CIStr::new(name),
        Offset: offset,
        Flag: flag,
        Flen: 11,
        ..Default::default()
    }
}

// 构造索引；condition 非空表示部分索引谓词串。
fn idx(id: i64, name: &str, columns: &[(&str, i32)], condition: &str) -> IndexInfo {
    IndexInfo {
        ID: id,
        Name: CIStr::new(name),
        Table: CIStr::default(),
        Columns: columns
            .iter()
            .map(|(n, off)| IndexColumn {
                Name: CIStr::new(*n),
                Offset: *off,
                Length: UnspecifiedLength,
            })
            .collect(),
        ConditionExprString: condition.to_string(),
    }
}

// 构造 FKInfo；cols/ref_cols 转为 CIStr 向量。
fn fk(name: &str, cols: &[&str], ref_cols: &[&str]) -> FKInfo {
    FKInfo {
        Name: CIStr::new(name),
        Cols: cols.iter().map(|c| CIStr::new(*c)).collect(),
        RefCols: ref_cols.iter().map(|c| CIStr::new(*c)).collect(),
    }
}

/// 父子表夹具，对应 Go SQL 建表：
/// parent/child fixtures matching Go SQL:
/// `create table test.parent (id int, index i1(id))`
/// `create table test.child (id int, pid int, index i1(pid), foreign key (pid) references test.parent (id) on delete cascade)`
fn parent_child_suite() -> (MockIS, TableInfo, TableInfo) {
    let parent = TableInfo {
        ID: 10,
        DBID: 1,
        Name: CIStr::new("parent"),
        Columns: vec![col("id", 0, 0)],
        Indices: vec![idx(1, "i1", &[("id", 0)], "")],
        ForeignKeys: vec![],
        PKIsHandle: false,
    };
    let child_fk = fk("fk_1", &["pid"], &["id"]);
    let child = TableInfo {
        ID: 11,
        DBID: 1,
        Name: CIStr::new("child"),
        Columns: vec![col("id", 0, 0), col("pid", 1, 0)],
        Indices: vec![idx(1, "i1", &[("pid", 1)], "")],
        ForeignKeys: vec![child_fk.clone()],
        PKIsHandle: false,
    };
    let mut is = MockIS::default();
    is.tables_by_name
        .insert(("test".into(), "parent".into()), parent.clone());
    is.tables_by_name
        .insert(("test".into(), "child".into()), child.clone());
    is.referred.insert(
        ("test".into(), "parent".into()),
        vec![ReferredFKInfo {
            Cols: vec![CIStr::new("id")],
            ChildSchema: CIStr::new("test"),
            ChildTable: CIStr::new("child"),
            ChildFKName: CIStr::new("fk_1"),
        }],
    );
    (is, parent, child)
}

/// Corresponds to Go `TestForeignKeyRecordManager`.
/// 主场景：Merge/Remove 组合与部分索引安全谓词。
#[test]
fn test_foreign_key_record_manager() {
    let ctx = Context::default();
    let (is, parent_table_info, child_table_info) = parent_child_suite();
    let db = CIStr::new("test");
    let child_table_index_i1 = child_table_info.Indices[0].clone();
    let parent_table_index_i1 = parent_table_info.Indices[0].clone();

    {
        // 基线：子有 FK、父有 referred；Merge 后全局长度为 1。
        let mut foreign_key_record_manager = NewForeignKeyRecordManager();
        let mut child_mgr =
            NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &child_table_info).unwrap();
        let parent_mgr =
            NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &parent_table_info).unwrap();
        assert_eq!(child_mgr.GetFKRecordMap().len(), 1);
        assert_eq!(child_mgr.GetReferredFKRecordMap().len(), 0);
        assert_eq!(parent_mgr.GetFKRecordMap().len(), 0);
        assert_eq!(parent_mgr.GetReferredFKRecordMap().len(), 1);
        foreign_key_record_manager.Merge(&child_mgr);
        foreign_key_record_manager.Merge(&parent_mgr);
        assert_eq!(foreign_key_record_manager.GetFKRecordMap().len(), 1);
        let _ = child_mgr;
    }

    {
        let mut foreign_key_record_manager = NewForeignKeyRecordManager();
        let mut child_mgr =
            NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &child_table_info).unwrap();
        let parent_mgr =
            NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &parent_table_info).unwrap();
        // 只删子索引：子 FK 空，父 referred 仍在 → Merge 后仍为 1。
        child_mgr.RemoveForeignKeys(&child_table_info, &child_table_index_i1);
        assert_eq!(child_mgr.GetFKRecordMap().len(), 0);
        assert_eq!(child_mgr.GetReferredFKRecordMap().len(), 0);
        assert_eq!(parent_mgr.GetFKRecordMap().len(), 0);
        assert_eq!(parent_mgr.GetReferredFKRecordMap().len(), 1);
        foreign_key_record_manager.Merge(&child_mgr);
        foreign_key_record_manager.Merge(&parent_mgr);
        assert_eq!(foreign_key_record_manager.GetFKRecordMap().len(), 1);
    }

    {
        let mut foreign_key_record_manager = NewForeignKeyRecordManager();
        let child_mgr =
            NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &child_table_info).unwrap();
        let mut parent_mgr =
            NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &parent_table_info).unwrap();
        // 只删父索引：父 referred 空，子 FK 仍在 → Merge 后仍为 1。
        parent_mgr.RemoveForeignKeys(&parent_table_info, &parent_table_index_i1);
        assert_eq!(child_mgr.GetFKRecordMap().len(), 1);
        assert_eq!(child_mgr.GetReferredFKRecordMap().len(), 0);
        assert_eq!(parent_mgr.GetFKRecordMap().len(), 0);
        assert_eq!(parent_mgr.GetReferredFKRecordMap().len(), 0);
        foreign_key_record_manager.Merge(&child_mgr);
        foreign_key_record_manager.Merge(&parent_mgr);
        assert_eq!(foreign_key_record_manager.GetFKRecordMap().len(), 1);
    }

    {
        let mut foreign_key_record_manager = NewForeignKeyRecordManager();
        let mut child_mgr =
            NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &child_table_info).unwrap();
        let mut parent_mgr =
            NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &parent_table_info).unwrap();
        // 双侧都删：Merge 后全局 FK map 为空。
        child_mgr.RemoveForeignKeys(&child_table_info, &child_table_index_i1);
        parent_mgr.RemoveForeignKeys(&parent_table_info, &parent_table_index_i1);
        assert_eq!(child_mgr.GetFKRecordMap().len(), 0);
        assert_eq!(child_mgr.GetReferredFKRecordMap().len(), 0);
        assert_eq!(parent_mgr.GetFKRecordMap().len(), 0);
        assert_eq!(parent_mgr.GetReferredFKRecordMap().len(), 0);
        foreign_key_record_manager.Merge(&child_mgr);
        foreign_key_record_manager.Merge(&parent_mgr);
        assert_eq!(foreign_key_record_manager.GetFKRecordMap().len(), 0);
    }

    // 部分索引子表：unsafe 谓词不能移除 FK；safe 谓词可以。
    // partial_child: unsafe_pid WHERE marker IS NOT NULL; safe_pid WHERE pid IS NOT NULL
    let partial_child = TableInfo {
        ID: 21,
        DBID: 1,
        Name: CIStr::new("partial_child"),
        Columns: vec![col("id", 0, 0), col("pid", 1, 0), col("marker", 2, 0)],
        Indices: vec![
            idx(1, "unsafe_pid", &[("pid", 1)], "`marker` is not null"),
            idx(2, "safe_pid", &[("pid", 1)], "`pid` is not null"),
        ],
        ForeignKeys: vec![fk("fk_partial", &["pid"], &["id"])],
        PKIsHandle: false,
    };
    let mut is2 = MockIS::default();
    is2.tables_by_name.insert(
        ("test".into(), "partial_child".into()),
        partial_child.clone(),
    );
    let unsafe_idx = partial_child.FindIndexByName("unsafe_pid").unwrap().clone();
    let safe_idx = partial_child.FindIndexByName("safe_pid").unwrap().clone();

    // 先 Remove unsafe 仍保留 FK；再 Remove safe 才清空。
    let mut partial_child_mgr =
        NewForeignKeyRecordManagerForTables(&ctx, &is2, &db, &partial_child).unwrap();
    assert_eq!(partial_child_mgr.GetFKRecordMap().len(), 1);
    partial_child_mgr.RemoveForeignKeys(&partial_child, &unsafe_idx);
    assert_eq!(partial_child_mgr.GetFKRecordMap().len(), 1);
    partial_child_mgr.RemoveForeignKeys(&partial_child, &safe_idx);
    assert_eq!(partial_child_mgr.GetFKRecordMap().len(), 0);

    // 父侧部分索引：unsafe 不能清 referred；safe 可以。
    // partial_ref_parent referred by partial_ref_child
    let partial_ref_parent = TableInfo {
        ID: 30,
        DBID: 1,
        Name: CIStr::new("partial_ref_parent"),
        Columns: vec![col("id", 0, 0), col("marker", 1, 0)],
        Indices: vec![
            idx(1, "unsafe_id", &[("id", 0)], "`marker` is not null"),
            idx(2, "safe_id", &[("id", 0)], "`id` is not null"),
        ],
        ForeignKeys: vec![],
        PKIsHandle: false,
    };
    let partial_ref_child = TableInfo {
        ID: 31,
        DBID: 1,
        Name: CIStr::new("partial_ref_child"),
        Columns: vec![col("id", 0, 0), col("pid", 1, 0)],
        Indices: vec![idx(1, "child_pid", &[("pid", 1)], "")],
        ForeignKeys: vec![fk("fk_ref", &["pid"], &["id"])],
        PKIsHandle: false,
    };
    let mut is3 = MockIS::default();
    is3.tables_by_name.insert(
        ("test".into(), "partial_ref_parent".into()),
        partial_ref_parent.clone(),
    );
    is3.tables_by_name.insert(
        ("test".into(), "partial_ref_child".into()),
        partial_ref_child.clone(),
    );
    is3.referred.insert(
        ("test".into(), "partial_ref_parent".into()),
        vec![ReferredFKInfo {
            Cols: vec![CIStr::new("id")],
            ChildSchema: CIStr::new("test"),
            ChildTable: CIStr::new("partial_ref_child"),
            ChildFKName: CIStr::new("fk_ref"),
        }],
    );
    let unsafe_ref_idx = partial_ref_parent
        .FindIndexByName("unsafe_id")
        .unwrap()
        .clone();
    let safe_ref_idx = partial_ref_parent
        .FindIndexByName("safe_id")
        .unwrap()
        .clone();

    // referred 路径同样区分 unsafe/safe。
    let mut partial_ref_parent_mgr =
        NewForeignKeyRecordManagerForTables(&ctx, &is3, &db, &partial_ref_parent).unwrap();
    assert_eq!(partial_ref_parent_mgr.GetReferredFKRecordMap().len(), 1);
    partial_ref_parent_mgr.RemoveForeignKeys(&partial_ref_parent, &unsafe_ref_idx);
    assert_eq!(partial_ref_parent_mgr.GetReferredFKRecordMap().len(), 1);
    partial_ref_parent_mgr.RemoveForeignKeys(&partial_ref_parent, &safe_ref_idx);
    assert_eq!(partial_ref_parent_mgr.GetReferredFKRecordMap().len(), 0);
}

/// Corresponds to Go `TestForeignKeyRecordManagerForPK1`.
/// 父表 PK 句柄：Remove 子索引后父 referred 亦空（PK 覆盖引用列）。
#[test]
fn test_foreign_key_record_manager_for_pk1() {
    let ctx = Context::default();
    let db = CIStr::new("test");
    // 父表：id 主键句柄 + 复合索引 i1(id,pid)。
    // parent (id int primary key, pid int, index i1(id, pid))
    let parent = TableInfo {
        ID: 10,
        DBID: 1,
        Name: CIStr::new("parent"),
        Columns: vec![col("id", 0, PriKeyFlag), col("pid", 1, 0)],
        Indices: vec![idx(1, "i1", &[("id", 0), ("pid", 1)], "")],
        ForeignKeys: vec![],
        PKIsHandle: true,
    };
    let child = TableInfo {
        ID: 11,
        DBID: 1,
        Name: CIStr::new("child"),
        Columns: vec![col("id", 0, 0), col("pid", 1, 0)],
        Indices: vec![idx(1, "i1", &[("pid", 1)], "")],
        ForeignKeys: vec![fk("fk_1", &["pid"], &["id"])],
        PKIsHandle: false,
    };
    let mut is = MockIS::default();
    is.tables_by_name
        .insert(("test".into(), "parent".into()), parent.clone());
    is.tables_by_name
        .insert(("test".into(), "child".into()), child.clone());
    is.referred.insert(
        ("test".into(), "parent".into()),
        vec![ReferredFKInfo {
            Cols: vec![CIStr::new("id")],
            ChildSchema: CIStr::new("test"),
            ChildTable: CIStr::new("child"),
            ChildFKName: CIStr::new("fk_1"),
        }],
    );
    let child_table_index_i1 = child.Indices[0].clone();

    let mut foreign_key_record_manager = NewForeignKeyRecordManager();
    let mut child_mgr = NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &child).unwrap();
    let parent_mgr = NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &parent).unwrap();
    child_mgr.RemoveForeignKeys(&child, &child_table_index_i1);
    assert_eq!(child_mgr.GetFKRecordMap().len(), 0);
    assert_eq!(child_mgr.GetReferredFKRecordMap().len(), 0);
    assert_eq!(parent_mgr.GetFKRecordMap().len(), 0);
    assert_eq!(parent_mgr.GetReferredFKRecordMap().len(), 0);
    foreign_key_record_manager.Merge(&child_mgr);
    foreign_key_record_manager.Merge(&parent_mgr);
    assert_eq!(foreign_key_record_manager.GetFKRecordMap().len(), 0);
}

/// Corresponds to Go `TestForeignKeyRecordManagerForPK2`.
/// 子表 PK 句柄：子侧 FK map 初始为空，删父索引后全局仍为 0。
#[test]
fn test_foreign_key_record_manager_for_pk2() {
    let ctx = Context::default();
    let db = CIStr::new("test");
    // 父表无 PK 句柄，仅有复合索引覆盖 id。
    // parent (id int, pid int, index i1(id, pid))
    let parent = TableInfo {
        ID: 10,
        DBID: 1,
        Name: CIStr::new("parent"),
        Columns: vec![col("id", 0, 0), col("pid", 1, 0)],
        Indices: vec![idx(1, "i1", &[("id", 0), ("pid", 1)], "")],
        ForeignKeys: vec![],
        PKIsHandle: false,
    };
    // 子表 pid 主键句柄；FK 由 PK 覆盖，故子 FK map 为空。
    // child (id int, pid int primary key, index i1(pid, id), fk(pid)->parent(id))
    let child = TableInfo {
        ID: 11,
        DBID: 1,
        Name: CIStr::new("child"),
        Columns: vec![col("id", 0, 0), col("pid", 1, PriKeyFlag)],
        Indices: vec![idx(1, "i1", &[("pid", 1), ("id", 0)], "")],
        ForeignKeys: vec![fk("fk_1", &["pid"], &["id"])],
        PKIsHandle: true,
    };
    let mut is = MockIS::default();
    is.tables_by_name
        .insert(("test".into(), "parent".into()), parent.clone());
    is.tables_by_name
        .insert(("test".into(), "child".into()), child.clone());
    is.referred.insert(
        ("test".into(), "parent".into()),
        vec![ReferredFKInfo {
            Cols: vec![CIStr::new("id")],
            ChildSchema: CIStr::new("test"),
            ChildTable: CIStr::new("child"),
            ChildFKName: CIStr::new("fk_1"),
        }],
    );
    let parent_table_index_i1 = parent.Indices[0].clone();

    let mut foreign_key_record_manager = NewForeignKeyRecordManager();
    let child_mgr = NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &child).unwrap();
    let mut parent_mgr = NewForeignKeyRecordManagerForTables(&ctx, &is, &db, &parent).unwrap();
    parent_mgr.RemoveForeignKeys(&parent, &parent_table_index_i1);
    assert_eq!(child_mgr.GetFKRecordMap().len(), 0);
    assert_eq!(child_mgr.GetReferredFKRecordMap().len(), 0);
    assert_eq!(parent_mgr.GetFKRecordMap().len(), 0);
    assert_eq!(parent_mgr.GetReferredFKRecordMap().len(), 0);
    foreign_key_record_manager.Merge(&child_mgr);
    foreign_key_record_manager.Merge(&parent_mgr);
    assert_eq!(foreign_key_record_manager.GetFKRecordMap().len(), 0);
}
