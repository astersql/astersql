// Copyright 2022 PingCAP, Inc.
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

// InfoStore（内存信息模式仓库）的单元测试。
//
// InfoStore 是 schema tracker（模式跟踪器）中用于存放库（schema/database）
// 与表（table）元数据的内存结构。本文件验证：
// - `lower_case_table_names` 模式对库名/表名大小写敏感度的影响；
// - 删除库、删除表时的存在性检查与级联清理行为。
//
// 术语：`lower_case_table_names` 对应 MySQL 同名系统变量——
// 值为 0 时名称区分大小写；值为 2 时存储保留原大小写、比较时转小写。

use crate::*;

struct FailingInfoSchema {
    schema: model::DBInfo,
}

impl InfoSchemaSource for FailingInfoSchema {
    fn AllSchemas(&self) -> Vec<model::DBInfo> {
        vec![self.schema.clone()]
    }

    fn SchemaTableInfos(&self, _schema: &ast::CIStr) -> Result<Vec<model::TableInfo>, Error> {
        Err(Error::Mismatch("table metadata unavailable".into()))
    }
}

#[test]
fn init_from_is_keeps_schema_inserted_when_table_loading_fails() {
    let schema_name = ast::NewCIStr("partially_loaded");
    let source = FailingInfoSchema {
        schema: model::DBInfo {
            Name: schema_name.clone(),
            ..Default::default()
        },
    };
    let mut store = NewInfoStore(0);

    assert!(store.InitFromIS(&source).is_err());
    assert!(
        store.SchemaByName(&schema_name).is_some(),
        "Go PutSchema runs before SchemaTableInfos and keeps the schema on error"
    );
}

/// 验证不同 `lower_case_table_names` 取值下，库表写入与按名查找的行为。
#[test]
fn test_info_store_lower_case_table_names() {
    let db = ast::NewCIStr("DBName");
    let lower_db = ast::NewCIStr("dbname");
    let table = ast::NewCIStr("TableName");
    let lower_table = ast::NewCIStr("tablename");
    let db_info = model::DBInfo {
        Name: db.clone(),
        ..Default::default()
    };
    let table_info = model::TableInfo {
        Name: table.clone(),
        ..Default::default()
    };
    // 模式 0：区分大小写，小写别名无法命中已写入的原名。
    let mut store = NewInfoStore(0);
    store.PutSchema(db_info.clone());
    assert!(store.SchemaByName(&db).is_some());
    assert!(store.SchemaByName(&lower_db).is_none());
    assert_eq!(
        store.PutTable(lower_db.clone(), table_info.clone()),
        Err(Error::DatabaseNotExists("dbname".into()))
    );
    store.PutTable(db.clone(), table_info.clone()).unwrap();
    assert!(store.TableByName(&db, &table).is_ok());
    assert!(matches!(
        store.TableByName(&lower_db, &table),
        Err(Error::DatabaseNotExists(name)) if name == "dbname"
    ));
    assert!(matches!(
        store.TableByName(&db, &lower_table),
        Err(Error::TableNotExists(..))
    ));
    assert_eq!(store.AllSchemaNames(), vec!["DBName"]);
    assert_eq!(
        store.AllTableNamesOfSchema(&ast::NewCIStr("wrong-db")),
        Err(Error::DatabaseNotExists("wrong-db".into()))
    );
    assert_eq!(store.AllTableNamesOfSchema(&db).unwrap(), vec!["TableName"]);

    // 模式 2：可用小写名查找，但列表返回的是内部规范化后的小写形式。
    let mut store = NewInfoStore(2);
    store.PutSchema(db_info);
    store.PutTable(lower_db.clone(), table_info).unwrap();
    assert_eq!(store.SchemaByName(&lower_db).unwrap().Name.O, "DBName");
    assert_eq!(
        store.TableByName(&lower_db, &lower_table).unwrap().Name.O,
        "TableName"
    );
    assert_eq!(store.AllSchemaNames(), vec!["dbname"]);
    assert_eq!(store.AllTableNamesOfSchema(&db).unwrap(), vec!["tablename"]);
}

/// 验证删除不存在的库/表会报错，以及删库后其下表不可再访问。
#[test]
fn test_info_store_delete_tables() {
    let db1 = ast::NewCIStr("DBName1");
    let db2 = ast::NewCIStr("DBName2");
    let t1 = ast::NewCIStr("TableName1");
    let t2 = ast::NewCIStr("TableName2");
    let mut store = NewInfoStore(0);
    store.PutSchema(model::DBInfo {
        Name: db1.clone(),
        ..Default::default()
    });
    store
        .PutTable(
            db1.clone(),
            model::TableInfo {
                Name: t1.clone(),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .PutTable(
            db1.clone(),
            model::TableInfo {
                Name: t2.clone(),
                ..Default::default()
            },
        )
        .unwrap();
    // 对尚未创建的 db2 执行删除应失败。
    assert!(!store.DeleteSchema(&db2));
    assert_eq!(
        store.PutTable(
            db2.clone(),
            model::TableInfo {
                Name: t1.clone(),
                ..Default::default()
            }
        ),
        Err(Error::DatabaseNotExists("DBName2".into()))
    );
    assert!(matches!(
        store.DeleteTable(&db2, &t1),
        Err(Error::DatabaseNotExists(_))
    ));
    store.PutSchema(model::DBInfo {
        Name: db2.clone(),
        ..Default::default()
    });
    store
        .PutTable(
            db2.clone(),
            model::TableInfo {
                Name: t1.clone(),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(matches!(
        store.DeleteTable(&db2, &t2),
        Err(Error::TableNotExists(..))
    ));
    store.DeleteTable(&db2, &t1).unwrap();
    assert!(store.AllTableNamesOfSchema(&db2).unwrap().is_empty());
    // 删除 db1 后，其下表按名查找应返回库不存在。
    assert!(store.DeleteSchema(&db1));
    assert!(matches!(
        store.TableByName(&db1, &t1),
        Err(Error::DatabaseNotExists(_))
    ));
    assert_eq!(store.AllSchemaNames(), vec!["DBName2"]);
}
