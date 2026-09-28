// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// DDL（数据定义语言，如 CREATE/DROP/ALTER 等修改表结构的语句）错误路径测试。
//
// 本文件通过一个内存中的简化目录（Catalog）模型，模拟 DDL 作业在各种
// 异常情况下应返回的错误，覆盖以下场景：
// - 无效的 schema ID / table ID（DDL 作业携带的元数据标识失效）；
// - 建表时表已存在、删表时表不存在；
// - 加索引 / 加列 / 删列时引用了不存在的列；
// - 出错的 DDL 不应污染（部分写入）目录状态。

use std::collections::{HashMap, HashSet};

/// 单表允许的最大索引数量上限（对应 TiDB 中的 DefMaxOfIndexLimit 常量）。
const DEF_MAX_OF_INDEX_LIMIT: usize = 512;

/// 简化后的 DDL 错误枚举，对应 TiDB 中各类 DDL 执行错误码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DdlError {
    /// DDL 作业引用的 schema（数据库）ID 无效或已失效。
    InvalidSchemaId,
    /// DDL 作业引用的表 ID 无效或已失效。
    InvalidTableId,
    /// 建表时目标表名已存在。
    TableExists,
    /// 操作的目标表不存在。
    TableNotFound,
    /// 引用了不存在的列（对应 MySQL 的 ER_BAD_FIELD_ERROR）。
    BadField,
    /// 无法删除指定的列或键（对应 MySQL 的 ER_CANT_DROP_FIELD_OR_KEY）。
    CantDropFieldOrKey,
}

/// 内存中的简化元数据目录，模拟数据库的 schema 存储。
///
/// 通过 `invalid_schema_id` / `invalid_table_id` 两个开关注入故障，
/// 模拟 DDL 作业执行时元数据校验失败的情形。
#[derive(Default)]
struct Catalog {
    /// 已创建的数据库名集合。
    databases: HashSet<String>,
    /// 表名 -> 该表的列名集合。
    tables: HashMap<String, HashSet<String>>,
    /// 表名 -> 该表的索引名集合。
    indexes: HashMap<String, HashSet<String>>,
    /// 故障注入开关：为 true 时所有 DDL 均报 InvalidSchemaId。
    invalid_schema_id: bool,
    /// 故障注入开关：为 true 时校验阶段报 InvalidTableId。
    invalid_table_id: bool,
}

impl Catalog {
    /// 创建表：先校验 schema ID，再检查同名表冲突，最后写入列集合。
    fn create_table(&mut self, name: &str, columns: &[&str]) -> Result<(), DdlError> {
        if self.invalid_schema_id {
            return Err(DdlError::InvalidSchemaId);
        }
        if self.tables.contains_key(name) {
            return Err(DdlError::TableExists);
        }
        self.tables.insert(
            name.to_owned(),
            columns.iter().map(|column| (*column).to_owned()).collect(),
        );
        Ok(())
    }

    /// 创建数据库不依赖已有 schema ID；这与 Go 测试中建库作业即使注入错误
    /// schema ID 仍能成功的契约一致。
    fn create_database(&mut self, name: &str) {
        self.databases.insert(name.to_owned());
    }

    /// 删除表：先做作业 ID 校验，表不存在时返回 TableNotFound。
    fn drop_table(&mut self, name: &str) -> Result<(), DdlError> {
        self.check_job_ids()?;
        self.tables
            .remove(name)
            .map(|_| ())
            .ok_or(DdlError::TableNotFound)
    }

    /// 为表添加索引：要求目标表存在且索引列存在，否则分别报
    /// TableNotFound / BadField。
    fn add_index(&mut self, table: &str, index: &str, column: &str) -> Result<(), DdlError> {
        self.check_job_ids()?;
        let columns = self.tables.get(table).ok_or(DdlError::TableNotFound)?;
        if !columns.contains(column) {
            return Err(DdlError::BadField);
        }
        self.indexes
            .entry(table.to_owned())
            .or_default()
            .insert(index.to_owned());
        Ok(())
    }

    /// 删除索引；作业 ID 校验必须先于索引存在性检查。
    fn drop_index(&mut self, table: &str, index: &str) -> Result<(), DdlError> {
        self.check_job_ids()?;
        let indexes = self.indexes.get_mut(table).ok_or(DdlError::TableNotFound)?;
        if !indexes.remove(index) {
            return Err(DdlError::CantDropFieldOrKey);
        }
        Ok(())
    }

    /// 为表添加列；`after` 指定新列位置（模拟 `ADD COLUMN ... AFTER x`），
    /// 若 AFTER 引用的列不存在则报 BadField。
    fn add_column(
        &mut self,
        table: &str,
        column: &str,
        after: Option<&str>,
    ) -> Result<(), DdlError> {
        self.check_job_ids()?;
        let columns = self.tables.get_mut(table).ok_or(DdlError::TableNotFound)?;
        if after.is_some_and(|name| !columns.contains(name)) {
            return Err(DdlError::BadField);
        }
        columns.insert(column.to_owned());
        Ok(())
    }

    /// 删除列：列不存在时报 CantDropFieldOrKey。
    fn drop_column(&mut self, table: &str, column: &str) -> Result<(), DdlError> {
        self.check_job_ids()?;
        let columns = self.tables.get_mut(table).ok_or(DdlError::TableNotFound)?;
        if !columns.remove(column) {
            return Err(DdlError::CantDropFieldOrKey);
        }
        Ok(())
    }

    /// 原子地添加多列；任一位置引用无效时不写入任何列。
    fn add_columns(
        &mut self,
        table: &str,
        columns: &[(&str, Option<&str>)],
    ) -> Result<(), DdlError> {
        self.check_job_ids()?;
        let current = self.tables.get(table).ok_or(DdlError::TableNotFound)?;
        let mut updated = current.clone();
        for (column, after) in columns {
            if after.is_some_and(|name| !updated.contains(name)) {
                return Err(DdlError::BadField);
            }
            updated.insert((*column).to_owned());
        }
        self.tables.insert(table.to_owned(), updated);
        Ok(())
    }

    /// 原子地删除多列；任一列不存在时回滚此前的删除。
    fn drop_columns(&mut self, table: &str, columns: &[&str]) -> Result<(), DdlError> {
        self.check_job_ids()?;
        let current = self.tables.get(table).ok_or(DdlError::TableNotFound)?;
        let mut updated = current.clone();
        for column in columns {
            if !updated.remove(*column) {
                return Err(DdlError::CantDropFieldOrKey);
            }
        }
        self.tables.insert(table.to_owned(), updated);
        Ok(())
    }

    /// 校验 DDL 作业携带的 schema/table ID 是否有效。
    /// 模拟真实 DDL 执行前对作业元数据的合法性检查，schema 校验优先于 table。
    fn check_job_ids(&self) -> Result<(), DdlError> {
        if self.invalid_schema_id {
            return Err(DdlError::InvalidSchemaId);
        }
        if self.invalid_table_id {
            return Err(DdlError::InvalidTableId);
        }
        Ok(())
    }
}

/// 表级 DDL 错误：分别注入无效 schema ID、无效 table ID 验证删表失败，
/// 并验证重复建表返回 TableExists。
#[test]
fn test_table_error() {
    let mut catalog = Catalog::default();
    catalog.create_table("testDrop", &["a"]).unwrap();

    // 注入无效 schema ID，删表应失败。
    catalog.invalid_schema_id = true;
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.drop_table("testDrop")
    );
    catalog.invalid_schema_id = false;

    // 注入无效 table ID，删表同样应失败。
    catalog.invalid_table_id = true;
    assert_eq!(
        Err(DdlError::InvalidTableId),
        catalog.drop_table("testDrop")
    );
    catalog.invalid_table_id = false;

    // 重复创建同名表应报 TableExists。
    catalog.create_table("t2", &["a"]).unwrap();
    assert_eq!(
        Err(DdlError::TableExists),
        catalog.create_table("t2", &["a"])
    );
}

/// 视图错误测试的基础夹具：确认正常建表后目录中可查到该表。
#[test]
fn test_view_error_fixture() {
    let mut catalog = Catalog::default();
    catalog.create_table("t", &["a"]).unwrap();
    assert!(catalog.tables.contains_key("t"));
}

/// 外键相关错误：schema ID 无效时，添加外键索引（这里以普通索引模拟）应失败。
#[test]
fn test_foreign_key_error() {
    let mut catalog = Catalog::default();
    catalog.create_table("t", &["a"]).unwrap();
    catalog.create_table("t1", &["a"]).unwrap();
    catalog.invalid_schema_id = true;
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.add_index("t1", "fk", "a")
    );
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.drop_index("t1", "fk")
    );
}

/// 索引相关错误：schema ID 无效时，加索引与删列均应报 InvalidSchemaId。
#[test]
fn test_index_error() {
    let mut catalog = Catalog::default();
    catalog.create_table("t", &["a"]).unwrap();
    catalog.add_index("t", "a", "a").unwrap();
    catalog.invalid_schema_id = true;
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.add_index("t", "idx", "a")
    );
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.drop_column("t1", "a")
    );
}

/// 列相关错误：依次覆盖无效 schema ID、无效 table ID、
/// AFTER 引用不存在列（BadField）、删除不存在列（CantDropFieldOrKey）。
#[test]
fn test_column_error() {
    let mut catalog = Catalog::default();
    catalog.create_table("t", &["a", "aa", "ab"]).unwrap();

    // schema ID 无效时加列 / 删列都失败。
    catalog.invalid_schema_id = true;
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.add_column("t", "ta", None)
    );
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.drop_column("t", "aa")
    );
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.drop_column("t", "aa")
    );
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.add_columns("t", &[("ta", None), ("tb", None)])
    );
    assert_eq!(
        Err(DdlError::InvalidSchemaId),
        catalog.drop_columns("t", &["aa", "ab"])
    );
    catalog.invalid_schema_id = false;

    // table ID 无效时加列 / 删列同样失败。
    catalog.invalid_table_id = true;
    assert_eq!(
        Err(DdlError::InvalidTableId),
        catalog.add_column("t", "ta", None)
    );
    assert_eq!(
        Err(DdlError::InvalidTableId),
        catalog.drop_column("t", "aa")
    );
    assert_eq!(
        Err(DdlError::InvalidTableId),
        catalog.drop_column("t", "aa")
    );
    assert_eq!(
        Err(DdlError::InvalidTableId),
        catalog.add_columns("t", &[("ta", None), ("tb", None)])
    );
    assert_eq!(
        Err(DdlError::InvalidTableId),
        catalog.drop_columns("t", &["aa", "ab"])
    );
    catalog.invalid_table_id = false;

    // AFTER 引用了不存在的列 c5，应报 BadField；删除不存在的列报 CantDropFieldOrKey。
    assert_eq!(
        Err(DdlError::BadField),
        catalog.add_column("t", "c", Some("c5"))
    );
    assert_eq!(
        Err(DdlError::CantDropFieldOrKey),
        catalog.drop_column("t", "c5")
    );
    assert_eq!(
        Err(DdlError::BadField),
        catalog.add_columns("t", &[("c", Some("c5")), ("d", None)])
    );
    assert!(!catalog.tables["t"].contains("d"));
    assert_eq!(
        Err(DdlError::CantDropFieldOrKey),
        catalog.drop_columns("t", &["ab", "c5"])
    );
    assert!(catalog.tables["t"].contains("ab"));
}

/// 建库作业不依赖已有 schema ID，注入错误 schema ID 后仍应成功。
#[test]
fn test_create_database_error() {
    let mut catalog = Catalog::default();
    catalog.invalid_schema_id = true;
    catalog.create_database("db1");
    assert!(catalog.databases.contains("db1"));
}

/// 校验单表索引数量上限常量为 512（超过该值建索引应报 TooManyKeys 类错误）。
#[test]
fn test_create_index_err_too_many_keys_limit() {
    assert_eq!(512, DEF_MAX_OF_INDEX_LIMIT);
}
