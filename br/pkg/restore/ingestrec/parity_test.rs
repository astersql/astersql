// Copyright 2026 AsterSQL.

//! ingestrec Go/Rust 契约对等测试：串联 TryAddJob → RewriteTableID → UpdateIndexInfo
//! → Iterate / IterateForeignKeys，断言与 Go 同路径可观察行为一致。
//! MockIS 为内存 InfoSchema 替身，不启 kv/domain；覆盖边界跳过、正常录制、缺 args
//! 报错、表 ID 重写、列列表填充、外键合并，以及子 job Done 与缺库硬错误。
//! 场景索引：
//! - 边界：None / LitMerge 非 ingest → items 仍空。
//! - 成功录制：Synced+Ingest+IndexArgs → 重写后表 1100 可见。
//! - 缺参：finished_index_args=None → TryAddJob 报错。
//! - UpdateIndexInfo：ColumnList=`%n`，ColumnArgs=`id`，Schema/Table 名正确。
//! - IterateForeignKeys：子表 fk1 恰好一次。
//! - 子 job：Done+isSubJob=true 可录制；缺 SchemaByID 硬失败。
//! - 形状：ForeignKeyRecordKey 字段可构造，保证公开契约可见。
//! 本文件只解释断言意图，不改测试逻辑或夹具数据。
//! 夹具中的 FK 列名 `c` 与索引列 `id` 刻意不同，用于验证 FK 录制不依赖列名相等。
//! PriKeyFlag 仅标记夹具列，不改变本测试的主路径断言。

use std::collections::HashMap;

use astersql_errors::SharedError;

use crate::{
    ActionType, CIStr, ColumnInfo, Context, DBInfo, DDLReorgMeta, FKInfo, FinishedModifyIndexArgs,
    ForeignKeyRecordKey, IndexArg, IndexColumn, IndexInfo, InfoSchema, IngestRecorder, Job,
    JobState, PriKeyFlag, ReferredFKInfo, ReorgType, TableInfo, UnspecifiedLength,
};

/// 内存 InfoSchema：按 ID/名索引表与库，并提供 referred FK 映射。
#[derive(Default)]
struct MockIS {
    tables_by_id: HashMap<i64, TableInfo>,
    schemas_by_id: HashMap<i64, DBInfo>,
    referred: HashMap<(String, String), Vec<ReferredFKInfo>>,
    tables_by_name: HashMap<(String, String), TableInfo>,
}

impl InfoSchema for MockIS {
    // 按 schema/table 小写键取被引用外键；缺省空切片。
    fn GetTableReferredForeignKeys(&self, schema_l: &str, table_l: &str) -> Vec<ReferredFKInfo> {
        self.referred
            .get(&(schema_l.to_string(), table_l.to_string()))
            .cloned()
            .unwrap_or_default()
    }
    // TableByName 找不到时返回 "table not found"，对齐 FK 解析失败路径。
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
    fn TableInfoByID(&self, table_id: i64) -> Option<TableInfo> {
        self.tables_by_id.get(&table_id).cloned()
    }
    fn SchemaByID(&self, db_id: i64) -> Option<DBInfo> {
        self.schemas_by_id.get(&db_id).cloned()
    }
}

/// 端到端契约：覆盖过滤、录制、重写、更新、迭代与缺库错误。
#[test]
fn go_rust_public_contract_matches() {
    let mut rec = IngestRecorder::New();

    // 边界：nil / 非 ingest / 未同步 job 均应被跳过，不写入 items。
    rec.TryAddJob(None, false).unwrap();
    // LitMerge 虽 Synced 但非 Ingest，必须忽略。
    let non_ingest = Job {
        Type: ActionType::AddIndex,
        State: JobState::Synced,
        ReorgMeta: Some(DDLReorgMeta {
            ReorgTp: ReorgType::LitMerge,
        }),
        ..Default::default()
    };
    rec.TryAddJob(Some(&non_ingest), false).unwrap();

    // 正常：录制 ingest 加索引 job（Synced + Ingest + finished args）。
    let job = Job {
        TableID: 100,
        Type: ActionType::AddIndex,
        State: JobState::Synced,
        ReorgMeta: Some(DDLReorgMeta {
            ReorgTp: ReorgType::Ingest,
        }),
        finished_index_args: Some(FinishedModifyIndexArgs {
            IndexArgs: vec![IndexArg { IndexID: 7 }],
        }),
        ..Default::default()
    };
    rec.TryAddJob(Some(&job), false).unwrap();

    // 错误：缺 finished_index_args 必须失败，防止静默丢索引。
    let bad = Job {
        TableID: 101,
        Type: ActionType::AddIndex,
        State: JobState::Synced,
        ReorgMeta: Some(DDLReorgMeta {
            ReorgTp: ReorgType::Ingest,
        }),
        finished_index_args: None,
        ..Default::default()
    };
    assert!(rec.TryAddJob(Some(&bad), false).is_err());

    // 重写：表 ID +1000，skip=false 保留条目。
    rec.RewriteTableID(|id| Ok((id + 1000, false))).unwrap();

    // 夹具：单列索引 + 子表 FK，供 UpdateIndexInfo / IterateForeignKeys 消费。
    let col = ColumnInfo {
        Name: CIStr::new("id"),
        Offset: 0,
        Flag: PriKeyFlag,
        ..Default::default()
    };
    let idx = IndexInfo {
        ID: 7,
        Name: CIStr::new("idx"),
        Table: CIStr::new("t"),
        Columns: vec![IndexColumn {
            Name: CIStr::new("id"),
            Offset: 0,
            Length: UnspecifiedLength,
        }],
        ConditionExprString: String::new(),
    };
    let tbl = TableInfo {
        ID: 1100,
        DBID: 1,
        Name: CIStr::new("t"),
        Columns: vec![col],
        Indices: vec![idx],
        ForeignKeys: vec![FKInfo {
            Name: CIStr::new("fk1"),
            Cols: vec![CIStr::new("c")],
            RefCols: vec![CIStr::new("id")],
        }],
        PKIsHandle: false,
    };
    let mut is = MockIS::default();
    // 重写后表 ID 为 1100，必须按新键注册。
    is.tables_by_id.insert(1100, tbl.clone());
    is.schemas_by_id.insert(
        1,
        DBInfo {
            ID: 1,
            Name: CIStr::new("test"),
        },
    );

    // 资源：UpdateIndexInfo 填充 ColumnList 与 FK 管理器；Iterate 只产出 Updated。
    rec.UpdateIndexInfo(&Context::default(), &is).unwrap();
    let mut seen = 0;
    rec.Iterate(|tid, iid, info| {
        // 断言重写后的表 ID、列占位与库表名均与 Go 期望一致。
        assert_eq!(tid, 1100);
        assert_eq!(iid, 7);
        assert!(info.Updated);
        assert_eq!(info.ColumnList, "%n");
        assert_eq!(info.ColumnArgs, vec!["id".to_string()]);
        assert_eq!(info.SchemaName.O, "test");
        assert_eq!(info.TableName.O, "t");
        seen += 1;
        Ok(())
    })
    .unwrap();
    // 恰好一条已更新索引。
    assert_eq!(seen, 1);

    // 外键：子表 FK 应出现在 IterateForeignKeys 中恰好一次。
    let mut fks = 0;
    rec.IterateForeignKeys(|fk| {
        assert_eq!(fk.FKInfo.Name.O, "fk1");
        fks += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(fks, 1);

    // 缺库：表存在但 SchemaByID 缺失 → 硬错误，消息含 cannot find database。
    let mut rec2 = IngestRecorder::New();
    let job2 = Job {
        TableID: 200,
        Type: ActionType::AddPrimaryKey,
        State: JobState::Done,
        ReorgMeta: Some(DDLReorgMeta {
            ReorgTp: ReorgType::Ingest,
        }),
        finished_index_args: Some(FinishedModifyIndexArgs {
            IndexArgs: vec![IndexArg { IndexID: 1 }],
        }),
        ..Default::default()
    };
    // 子 job Done 应被接受（与主 job Synced 规则不同）。
    rec2.TryAddJob(Some(&job2), true).unwrap(); // subjob Done accepted
    let mut is2 = MockIS::default();
    // 仅注册表，故意不注册 DBID=9 的库。
    is2.tables_by_id.insert(
        200,
        TableInfo {
            ID: 200,
            DBID: 9,
            Name: CIStr::new("x"),
            ..Default::default()
        },
    );
    let err = rec2.UpdateIndexInfo(&Context::default(), &is2).unwrap_err();
    assert!(err.to_string().contains("cannot find database"), "{err}");

    // 类型形状：ForeignKeyRecordKey 字段可构造，保证公开契约编译期可见。
    let _ = ForeignKeyRecordKey {
        ChildSchemaNameO: "a".into(),
        ChildTableNameO: "b".into(),
        FKNameO: "c".into(),
    };
}

/// Go dereferences `foreignKeyRecordManager` directly, so an uninitialized recorder panics.
#[test]
#[should_panic(expected = "foreignKeyRecordManager is nil")]
fn ingest_recorder_export_panics_without_foreign_key_manager() {
    let rec = IngestRecorder::New();
    let _ = rec.GetFKRecordMap();
}
