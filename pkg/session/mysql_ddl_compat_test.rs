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

// MySQL 常用 DDL 的端到端兼容性测试。
//
// 测试通过真实会话连续执行建库、建表、变更、重命名、截断与删除，并交叉检查
// `information_schema` 元数据和表中已有数据，确保目录状态与物理存储始终同步。

use crate::runtime::{ConcreteRecordSet, ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

// 结果集按批次拉取；统一收集可让后续断言同时覆盖完整结果与读取错误。
fn collect(mut result: ConcreteRecordSet) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read DDL compatibility row") {
        rows.push(row);
    }
    rows
}

// 执行不需要结果的 DDL/DML，并在失败信息中保留原始 SQL 便于定位。
fn execute(session: &ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("DDL statement failed: {sql}: {error}"));
}

// 执行校验查询并取出首个结果集，供目录与数据断言复用。
fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let result = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("DDL verification query failed: {sql}: {error}"))
        .remove(0);
    collect(result)
}

#[test]
fn vector_index_kind_is_persisted_for_create_and_alter() {
    let (domain, session) = CreateAnalyzeSession().expect("canonical DDL session");

    execute(
        &session,
        "create table test.vector_kind_create (v vector(3), \
         vector index ((vec_cosine_distance(v))) using hnsw)",
    );
    let created = domain
        .table_by_name("test", "vector_kind_create")
        .expect("created vector-index table metadata");
    let created_info = created.Indices[0]
        .VectorInfo
        .as_ref()
        .expect("CREATE TABLE vector metadata");
    assert_eq!(created_info.Kind, astersql_meta_model::VectorIndexKindHNSW);

    execute(
        &session,
        "create table test.vector_kind_alter (v vector(3))",
    );
    execute(
        &session,
        "alter table test.vector_kind_alter add vector index idx \
         ((vec_l2_distance(v))) using hnsw",
    );
    let altered = domain
        .table_by_name("test", "vector_kind_alter")
        .expect("altered vector-index table metadata");
    let altered_info = altered.Indices[0]
        .VectorInfo
        .as_ref()
        .expect("ALTER TABLE vector metadata");
    assert_eq!(altered_info.Kind, astersql_meta_model::VectorIndexKindHNSW);
}

#[test]
fn common_mysql_ddl_round_trips_through_catalog_and_storage() {
    let (_domain, session) = CreateAnalyzeSession().expect("canonical DDL session");

    // 数据库名不区分大小写，`IF NOT EXISTS` 也不得重复创建目录项。
    execute(&session, "create database ddl_compat");
    execute(&session, "create database if not exists DDL_COMPAT");
    assert_eq!(
        rows(
            &session,
            "select schema_name from information_schema.schemata \
             where schema_name='ddl_compat'",
        ),
        vec![vec!["ddl_compat".to_owned()]],
    );
    execute(&session, "use DDL_COMPAT");

    // 建表后从目录表核对表注释、列属性、默认值与索引等 MySQL 元数据。
    execute(
        &session,
        "create table Work_Items (\
         id bigint not null auto_increment comment 'identifier',\
         code varchar(32) not null comment 'business code',\
         quantity int null default 7 comment 'quantity',\
         note varchar(64) null default 'draft' comment 'note',\
         primary key (id),\
         unique key uk_work_code (code),\
         key idx_work_quantity (quantity)) \
         comment='work items'",
    );
    execute(
        &session,
        "create table if not exists work_items (ignored int)",
    );
    assert_eq!(
        rows(
            &session,
            "select table_name, table_comment from information_schema.tables \
             where table_schema='ddl_compat' and table_name='work_items'",
        ),
        vec![vec!["work_items".to_owned(), "work items".to_owned()]],
    );
    assert_eq!(
        rows(
            &session,
            "select column_name, is_nullable, column_default, extra, column_comment \
             from information_schema.columns \
             where table_schema='ddl_compat' and table_name='work_items' \
             order by ordinal_position",
        ),
        vec![
            vec![
                "id".to_owned(),
                "NO".to_owned(),
                String::new(),
                "auto_increment".to_owned(),
                "identifier".to_owned(),
            ],
            vec![
                "code".to_owned(),
                "NO".to_owned(),
                String::new(),
                String::new(),
                "business code".to_owned(),
            ],
            vec![
                "quantity".to_owned(),
                "YES".to_owned(),
                "7".to_owned(),
                String::new(),
                "quantity".to_owned(),
            ],
            vec![
                "note".to_owned(),
                "YES".to_owned(),
                "draft".to_owned(),
                String::new(),
                "note".to_owned(),
            ],
        ],
    );
    // 先写入数据，再执行后续 ALTER，以验证目录变更会正确迁移已有物理行。
    execute(
        &session,
        "insert into work_items (code, quantity, note) values \
         ('alpha', 10, 'first'), ('beta', 20, 'draft')",
    );

    // 新增非空列时旧行应回填默认值；重复的条件式加列、加索引必须保持幂等。
    execute(
        &session,
        "alter table work_items add column rating int not null default 5 comment 'rating'",
    );
    execute(
        &session,
        "alter table work_items add column if not exists rating int not null default 9",
    );
    execute(
        &session,
        "alter table work_items add index idx_work_rating (rating)",
    );
    execute(
        &session,
        "alter table work_items add index if not exists idx_work_rating (rating)",
    );
    assert_eq!(
        rows(
            &session,
            "select id, code, quantity, note, rating from work_items order by id",
        ),
        vec![
            vec![
                "1".to_owned(),
                "alpha".to_owned(),
                "10".to_owned(),
                "first".to_owned(),
                "5".to_owned(),
            ],
            vec![
                "2".to_owned(),
                "beta".to_owned(),
                "20".to_owned(),
                "draft".to_owned(),
                "5".to_owned(),
            ],
        ],
    );

    // 修改类型、改名与重命名索引后，既要保留旧数据，也要更新目录中的列定义。
    execute(
        &session,
        "alter table work_items modify column quantity bigint not null default 7 \
         comment 'quantity widened'",
    );
    execute(
        &session,
        "alter table work_items change column note description varchar(80) null \
         default 'draft' comment 'description'",
    );
    execute(
        &session,
        "alter table work_items rename column rating to priority",
    );
    execute(
        &session,
        "alter table work_items rename index idx_work_rating to idx_work_priority",
    );
    assert_eq!(
        rows(
            &session,
            "select code, quantity, description, priority from work_items order by id",
        ),
        vec![
            vec![
                "alpha".to_owned(),
                "10".to_owned(),
                "first".to_owned(),
                "5".to_owned(),
            ],
            vec![
                "beta".to_owned(),
                "20".to_owned(),
                "draft".to_owned(),
                "5".to_owned(),
            ],
        ],
        "MODIFY/CHANGE/RENAME COLUMN must retain and convert old rows",
    );
    assert_eq!(
        rows(
            &session,
            "select column_name, data_type, is_nullable, column_default, column_comment \
             from information_schema.columns \
             where table_schema='ddl_compat' and table_name='work_items' \
             and column_name in ('quantity', 'description', 'priority') \
             order by ordinal_position",
        ),
        vec![
            vec![
                "quantity".to_owned(),
                "bigint".to_owned(),
                "NO".to_owned(),
                "7".to_owned(),
                "quantity widened".to_owned(),
            ],
            vec![
                "description".to_owned(),
                "varchar".to_owned(),
                "YES".to_owned(),
                "draft".to_owned(),
                "description".to_owned(),
            ],
            vec![
                "priority".to_owned(),
                "integer".to_owned(),
                "NO".to_owned(),
                "5".to_owned(),
                "rating".to_owned(),
            ],
        ],
    );
    assert_eq!(
        rows(
            &session,
            "select index_name, non_unique, column_name \
             from information_schema.statistics \
             where table_schema='ddl_compat' and table_name='work_items' \
             order by index_name, seq_in_index",
        ),
        vec![
            vec!["PRIMARY".to_owned(), "0".to_owned(), "id".to_owned()],
            vec![
                "idx_work_priority".to_owned(),
                "1".to_owned(),
                "priority".to_owned(),
            ],
            vec![
                "idx_work_quantity".to_owned(),
                "1".to_owned(),
                "quantity".to_owned(),
            ],
            vec!["uk_work_code".to_owned(), "0".to_owned(), "code".to_owned(),],
        ],
    );

    // 删除索引和列后再次使用 `IF EXISTS`，用于覆盖不存在对象上的兼容语义。
    execute(
        &session,
        "alter table work_items drop index idx_work_quantity",
    );
    execute(
        &session,
        "alter table work_items drop index if exists idx_work_quantity",
    );
    execute(&session, "alter table work_items drop column quantity");
    execute(
        &session,
        "alter table work_items drop column if exists quantity",
    );
    assert_eq!(
        rows(
            &session,
            "select column_name from information_schema.columns \
             where table_schema='ddl_compat' and table_name='work_items' \
             order by ordinal_position",
        ),
        vec![
            vec!["id".to_owned()],
            vec!["code".to_owned()],
            vec!["description".to_owned()],
            vec!["priority".to_owned()],
        ],
    );

    // 两种表重命名语法应更新目录名称，同时通过稳定的物理表 ID 保留原有行。
    execute(&session, "rename table work_items to Work_Items_Renamed");
    execute(
        &session,
        "alter table work_items_renamed rename to work_items_archive",
    );
    assert_eq!(
        rows(
            &session,
            "select table_name from information_schema.tables \
             where table_schema='ddl_compat' order by table_name",
        ),
        vec![vec!["work_items_archive".to_owned()]],
    );
    assert_eq!(
        rows(
            &session,
            "select code, description, priority from work_items_archive order by id",
        ),
        vec![
            vec!["alpha".to_owned(), "first".to_owned(), "5".to_owned(),],
            vec!["beta".to_owned(), "draft".to_owned(), "5".to_owned(),],
        ],
        "RENAME TABLE must retain rows under the stable physical table ID",
    );

    // TRUNCATE 应切断旧物理行的可见性，但继续保留完整表结构。
    execute(&session, "truncate table work_items_archive");
    assert!(
        rows(&session, "select id from work_items_archive").is_empty(),
        "TRUNCATE must make old physical rows unreachable",
    );
    assert_eq!(
        rows(
            &session,
            "select count(*) from information_schema.columns \
             where table_schema='ddl_compat' and table_name='work_items_archive'",
        ),
        vec![vec!["4".to_owned()]],
        "TRUNCATE must retain the table definition",
    );

    // 最终清理同时验证条件式 DROP 的幂等性，以及目录项确实被移除。
    execute(&session, "drop table work_items_archive");
    execute(&session, "drop table if exists work_items_archive");
    assert!(
        rows(
            &session,
            "select table_name from information_schema.tables \
             where table_schema='ddl_compat'",
        )
        .is_empty(),
    );
    execute(&session, "drop database ddl_compat");
    execute(&session, "drop database if exists DDL_COMPAT");
    assert!(
        rows(
            &session,
            "select schema_name from information_schema.schemata \
             where schema_name='ddl_compat'",
        )
        .is_empty(),
    );
}
