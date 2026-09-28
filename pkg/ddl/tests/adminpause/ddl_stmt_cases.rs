// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Admin Pause 测试用的 DDL 语句用例矩阵。
//
// 每个 [`StmtCase`] 描述一条目标 DDL、期望暂停时的 SchemaState
// （schema 状态机阶段，如 StateNone / StateWriteOnly / StatePublic 等）、
// 该状态下 `ADMIN PAUSE` 是否应成功，以及前后置 SQL。
// SchemaState 表示 DDL Job 推进过程中元数据对象所处阶段；
// Reorg（reorganization）表示需要回填存量数据的重组阶段。

/// 全局自增 ID 生成器（对应 Go 中带 Mutex 的版本；此处单线程可变借用即可）。
// AutoIncrsedID 对应 Go 中带 Mutex 的自增 ID 生成器。
pub struct AutoIncrsedID {
    /// 下一个可分配的 case 全局 ID。
    pub idx: i32,
}

impl AutoIncrsedID {
    /// 返回当前 `idx` 并自增，保证跨 case 数组的 ID 唯一。
    // globalID 在 Go 中加锁后返回当前 idx 并自增，保证并发读取 case 时 ID 唯一。
    pub fn global_id(&mut self) -> i32 {
        let id = self.idx;
        self.idx += 1;
        id
    }
}

/// Admin Pause 单条 DDL case 描述。
///
/// `schema_state` 是注入 pause 时期望的 SchemaState；
/// `is_job_pausable` 表示该状态下 pause 是否应成功。
// StmtCase 是 admin pause 测试的 DDL case 描述。
// global_id 跨不同 case 数组递增；stmt 是目标 DDL；schema_state 是期望暂停的状态；
// is_job_pausable 表示 admin pause 是否应成功；pre_condition_stmts/rollback_stmts 分别在目标语句前后执行。
pub struct StmtCase {
    /// 跨所有 case 数组唯一递增的编号。
    pub global_id: i32,
    /// 目标 DDL 语句。
    pub stmt: String,
    /// 期望暂停时的 SchemaState 名称。
    pub schema_state: &'static str,
    /// 该状态下 admin pause 是否应成功。
    pub is_job_pausable: bool,
    /// 目标语句执行前的准备 SQL。
    pub pre_condition_stmts: Vec<String>,
    /// 目标语句执行后的清理 / 回滚 SQL。
    pub rollback_stmts: Vec<String>,
}

/// 构造一条 [`StmtCase`]，自动分配 `global_id`。
fn stmt_case(
    ai: &mut AutoIncrsedID,
    stmt: String,
    schema_state: &'static str,
    is_job_pausable: bool,
    pre_condition_stmts: Vec<String>,
    rollback_stmts: Vec<String>,
) -> StmtCase {
    StmtCase {
        global_id: ai.global_id(),
        stmt,
        schema_state,
        is_job_pausable,
        pre_condition_stmts,
        rollback_stmts,
    }
}

/// 测试用 schema（database）名称。
pub const TEST_SCHEMA: &str = "test_create_db";
/// 创建测试 schema 的 DDL。
pub const CREATE_SCHEMA_STMT: &str = "create database test_create_db;";
/// 删除测试 schema 的 DDL。
pub const DROP_SCHEMA_STMT: &str = "drop database test_create_db;";

/// 普通建表测试表名。
pub const TABLE_NAME: &str = "test_create_tbl";
/// 创建普通测试表的 DDL。
pub const CREATE_TABLE_STMT: &str = r#"create table test_create_tbl (	id int(11) NOT NULL AUTO_INCREMENT,
    tenant varchar(128) NOT NULL,
    name varchar(128) NOT NULL,
    age int(11) NOT NULL,
    province varchar(32) NOT NULL DEFAULT '',
    city varchar(32) NOT NULL DEFAULT '',
    phone varchar(16) NOT NULL DEFAULT '',
    created_time datetime NOT NULL,
    updated_time datetime NOT NULL
);"#;
/// 删除普通测试表的 DDL。
pub const DROP_TABLE_STMT: &str = "drop table test_create_tbl;";

/// `ALTER TABLE t_user` 前缀（部分用例拼接用）。
pub const ALTER_TABLE_PREFIX: &str = "alter table t_user";
/// 添加主键索引。
pub const ADD_PRIMARY_INDEX_STMT: &str = "alter table t_user add primary key idx_id (id);";
/// 删除主键。
pub const DROP_PRIMARY_INDEX_STMT: &str = "alter table t_user drop primary key;";
/// 添加唯一索引。
pub const ADD_UNIQUE_INDEX_STMT: &str = "alter table t_user add unique index idx_phone (phone);";
/// 删除唯一索引。
pub const DROP_UNIQUE_INDEX_STMT: &str = "alter table t_user drop index if exists idx_phone;";
/// 添加普通二级索引。
pub const ADD_INDEX_STMT: &str = "alter table t_user add index if not exists idx_name (name);";
/// 删除普通二级索引。
pub const DROP_INDEX_STMT: &str = "alter table t_user drop index if exists idx_name;";
/// 添加向量索引（HNSW + 余弦距离）。
pub const ADD_VECTOR_INDEX_STMT: &str =
    "alter table t_user_vec add vector index v_idx((VEC_COSINE_DISTANCE(vec))) USING HNSW;";
/// 删除向量索引。
pub const DROP_VECTOR_INDEX_STMT: &str = "alter table t_user_vec drop index if exists v_idx;";
/// 添加列存 inverted 索引（columnar index）。
pub const ADD_COLUMNAR_STMT: &str =
    "alter table t_user_vec add columnar index c_idx(age) using inverted;";
/// 删除列存索引。
pub const DROP_COLUMNAR_STMT: &str = "alter table t_user_vec drop index if exists c_idx;";

/// 添加测试列。
pub const ADD_COLUMN_STMT: &str = "alter table t_user add column t_col bigint default '1024';";
/// 删除测试列。
pub const DROP_COLUMN_STMT: &str = "alter table t_user drop column if exists t_col;";
/// 在测试列上建索引（用于带索引删列场景）。
pub const ADD_COLUMN_IDX_STMT: &str = "alter table t_user add index idx_t_col(t_col);";
/// `MODIFY COLUMN` 语句前缀。
pub const ALTER_COLUMN_PREFIX: &str = "alter table t_user modify column t_col ";

/// 分区表追加分区。
pub const ALTER_TABLE_PARTITION_ADD_PARTITION: &str =
    "alter table t_user_partition add partition (partition p7 values less than (200));";
/// 分区表删除分区。
pub const ALTER_TABLE_PARTITION_DROP_PARTITION: &str =
    "alter table t_user_partition drop partition p7;";
/// Placement Policy（放置策略）名称：控制数据副本落在哪些 Region。
pub const PLACEMENT_POLICY: &str = "placement_policy";
/// 创建 placement policy。
pub const CREATE_PLACEMENT_POLICY: &str =
    "create placement policy placement_policy PRIMARY_REGION=\"cn-east-1\", REGIONS=\"cn-east-1\";";
/// 删除 placement policy。
pub const DROP_PLACEMENT_POLICY: &str = "drop placement policy placement_policy";
/// 将 schema 绑定到 placement policy。
pub const ALTER_SCHEMA_POLICY: &str =
    "alter database test_create_db placement policy = 'placement_policy';";

/// 将 `&str` 切片转为拥有所有权的 `Vec<String>`。
fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// 创建 / 删除 schema 在各 SchemaState 下的 pause 状态矩阵。
// schemaDDLStmtCase 对应创建/删除 schema 的 pause 状态矩阵。
pub fn schema_ddl_stmt_case(ai: &mut AutoIncrsedID) -> Vec<StmtCase> {
    vec![
        // Create schema.
        stmt_case(
            ai,
            CREATE_SCHEMA_STMT.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[DROP_SCHEMA_STMT]),
        ),
        stmt_case(
            ai,
            CREATE_SCHEMA_STMT.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[DROP_SCHEMA_STMT]),
        ),
        // Drop schema.
        stmt_case(
            ai,
            DROP_SCHEMA_STMT.to_string(),
            "StatePublic",
            true,
            strs(&[CREATE_SCHEMA_STMT]),
            strs(&[DROP_SCHEMA_STMT]),
        ),
        stmt_case(
            ai,
            DROP_SCHEMA_STMT.to_string(),
            "StateWriteOnly",
            false,
            strs(&[CREATE_SCHEMA_STMT]),
            strs(&[DROP_SCHEMA_STMT]),
        ),
        stmt_case(
            ai,
            DROP_SCHEMA_STMT.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[CREATE_SCHEMA_STMT]),
            strs(&[DROP_SCHEMA_STMT]),
        ),
        stmt_case(
            ai,
            DROP_SCHEMA_STMT.to_string(),
            "StateNone",
            false,
            strs(&[CREATE_SCHEMA_STMT]),
            strs(&[DROP_SCHEMA_STMT]),
        ),
    ]
}

/// 创建 / 删除普通表在各 SchemaState 下的 pause 状态矩阵。
// tableDDLStmt 对应创建/删除普通表的 pause 状态矩阵。
pub fn table_ddl_stmt(ai: &mut AutoIncrsedID) -> Vec<StmtCase> {
    vec![
        stmt_case(
            ai,
            CREATE_TABLE_STMT.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[DROP_TABLE_STMT]),
        ),
        stmt_case(
            ai,
            CREATE_TABLE_STMT.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[DROP_TABLE_STMT]),
        ),
        stmt_case(
            ai,
            DROP_TABLE_STMT.to_string(),
            "StatePublic",
            true,
            strs(&[CREATE_TABLE_STMT]),
            strs(&[DROP_TABLE_STMT]),
        ),
        stmt_case(
            ai,
            DROP_TABLE_STMT.to_string(),
            "StateWriteOnly",
            false,
            strs(&[CREATE_TABLE_STMT]),
            strs(&[DROP_TABLE_STMT]),
        ),
        stmt_case(
            ai,
            DROP_TABLE_STMT.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[CREATE_TABLE_STMT]),
            strs(&[DROP_TABLE_STMT]),
        ),
        stmt_case(
            ai,
            DROP_TABLE_STMT.to_string(),
            "StateNone",
            false,
            strs(&[CREATE_TABLE_STMT]),
            strs(&[DROP_TABLE_STMT]),
        ),
    ]
}

/// 主键 / 唯一 / 普通 / vector / columnar 索引 add·drop 的 pause 状态矩阵。
///
/// `StateWriteReorganization` / `StateDeleteReorganization` 对应 reorg 回填阶段。
// indexDDLStmtCase 覆盖主键、唯一索引、普通索引、vector index 和 columnar index 的 add/drop pause 状态。
pub fn index_ddl_stmt_case(ai: &mut AutoIncrsedID) -> Vec<StmtCase> {
    vec![
        stmt_case(
            ai,
            ADD_PRIMARY_INDEX_STMT.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_PRIMARY_INDEX_STMT.to_string(),
            "StateDeleteOnly",
            true,
            vec![],
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_PRIMARY_INDEX_STMT.to_string(),
            "StateWriteOnly",
            true,
            vec![],
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_PRIMARY_INDEX_STMT.to_string(),
            "StateWriteReorganization",
            true,
            vec![],
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_PRIMARY_INDEX_STMT.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_PRIMARY_INDEX_STMT.to_string(),
            "StatePublic",
            true,
            strs(&[ADD_PRIMARY_INDEX_STMT]),
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_PRIMARY_INDEX_STMT.to_string(),
            "StateDeleteReorganization",
            false,
            strs(&[ADD_PRIMARY_INDEX_STMT]),
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_PRIMARY_INDEX_STMT.to_string(),
            "StateWriteOnly",
            false,
            strs(&[ADD_PRIMARY_INDEX_STMT]),
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_PRIMARY_INDEX_STMT.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[ADD_PRIMARY_INDEX_STMT]),
            strs(&[DROP_PRIMARY_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_UNIQUE_INDEX_STMT.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[DROP_UNIQUE_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_UNIQUE_INDEX_STMT.to_string(),
            "StateDeleteOnly",
            true,
            vec![],
            strs(&[DROP_UNIQUE_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_UNIQUE_INDEX_STMT.to_string(),
            "StateWriteOnly",
            true,
            vec![],
            strs(&[DROP_UNIQUE_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_UNIQUE_INDEX_STMT.to_string(),
            "StateWriteReorganization",
            true,
            vec![],
            strs(&[DROP_UNIQUE_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_UNIQUE_INDEX_STMT.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[DROP_UNIQUE_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_INDEX_STMT.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[DROP_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_INDEX_STMT.to_string(),
            "StateDeleteOnly",
            true,
            vec![],
            strs(&[DROP_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_INDEX_STMT.to_string(),
            "StateWriteOnly",
            true,
            vec![],
            strs(&[DROP_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_INDEX_STMT.to_string(),
            "StateWriteReorganization",
            true,
            vec![],
            strs(&[DROP_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_INDEX_STMT.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[DROP_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_VECTOR_INDEX_STMT.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[DROP_VECTOR_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_VECTOR_INDEX_STMT.to_string(),
            "StateDeleteOnly",
            true,
            vec![],
            strs(&[DROP_VECTOR_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_VECTOR_INDEX_STMT.to_string(),
            "StateWriteOnly",
            true,
            vec![],
            strs(&[DROP_VECTOR_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_VECTOR_INDEX_STMT.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[DROP_VECTOR_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_VECTOR_INDEX_STMT.to_string(),
            "StatePublic",
            true,
            strs(&[ADD_VECTOR_INDEX_STMT]),
            strs(&[DROP_VECTOR_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_VECTOR_INDEX_STMT.to_string(),
            "StateWriteOnly",
            false,
            strs(&[ADD_VECTOR_INDEX_STMT]),
            strs(&[DROP_VECTOR_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_VECTOR_INDEX_STMT.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[ADD_VECTOR_INDEX_STMT]),
            strs(&[DROP_VECTOR_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_VECTOR_INDEX_STMT.to_string(),
            "StateDeleteReorganization",
            false,
            strs(&[ADD_VECTOR_INDEX_STMT]),
            strs(&[DROP_VECTOR_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            ADD_COLUMNAR_STMT.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[DROP_COLUMNAR_STMT]),
        ),
        stmt_case(
            ai,
            ADD_COLUMNAR_STMT.to_string(),
            "StateDeleteOnly",
            true,
            vec![],
            strs(&[DROP_COLUMNAR_STMT]),
        ),
        stmt_case(
            ai,
            ADD_COLUMNAR_STMT.to_string(),
            "StateWriteOnly",
            true,
            vec![],
            strs(&[DROP_COLUMNAR_STMT]),
        ),
        stmt_case(
            ai,
            ADD_COLUMNAR_STMT.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[DROP_COLUMNAR_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMNAR_STMT.to_string(),
            "StatePublic",
            true,
            strs(&[ADD_COLUMNAR_STMT]),
            strs(&[DROP_COLUMNAR_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMNAR_STMT.to_string(),
            "StateWriteOnly",
            false,
            strs(&[ADD_COLUMNAR_STMT]),
            strs(&[DROP_COLUMNAR_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMNAR_STMT.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[ADD_COLUMNAR_STMT]),
            strs(&[DROP_COLUMNAR_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMNAR_STMT.to_string(),
            "StateDeleteReorganization",
            false,
            strs(&[ADD_COLUMNAR_STMT]),
            strs(&[DROP_COLUMNAR_STMT]),
        ),
        stmt_case(
            ai,
            DROP_INDEX_STMT.to_string(),
            "StatePublic",
            true,
            strs(&[ADD_INDEX_STMT]),
            strs(&[DROP_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_INDEX_STMT.to_string(),
            "StateDeleteReorganization",
            false,
            strs(&[ADD_INDEX_STMT]),
            strs(&[DROP_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_INDEX_STMT.to_string(),
            "StateWriteOnly",
            false,
            strs(&[ADD_INDEX_STMT]),
            strs(&[DROP_INDEX_STMT]),
        ),
        stmt_case(
            ai,
            DROP_INDEX_STMT.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[ADD_INDEX_STMT]),
            strs(&[DROP_INDEX_STMT]),
        ),
    ]
}

/// 列 add / drop，以及无 reorg / 有 reorg 的 modify column pause 矩阵。
// columnDDLStmtCase 覆盖 add/drop column，以及 no-reorg/reorg modify column。
pub fn column_ddl_stmt_case(ai: &mut AutoIncrsedID) -> Vec<StmtCase> {
    // 无 reorg：类型兼容缩小/还原；有 reorg：改成 char 触发数据重组。
    let modify_mediumint = format!("{}mediumint;", ALTER_COLUMN_PREFIX);
    let modify_int = format!("{}int;", ALTER_COLUMN_PREFIX);
    let modify_char = format!("{}char(10);", ALTER_COLUMN_PREFIX);
    vec![
        stmt_case(
            ai,
            ADD_COLUMN_STMT.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            ADD_COLUMN_STMT.to_string(),
            "StateDeleteOnly",
            true,
            vec![],
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            ADD_COLUMN_STMT.to_string(),
            "StateWriteOnly",
            true,
            vec![],
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            ADD_COLUMN_STMT.to_string(),
            "StateWriteReorganization",
            true,
            vec![],
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            ADD_COLUMN_STMT.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StatePublic",
            true,
            strs(&[ADD_COLUMN_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[ADD_COLUMN_STMT]),
            vec![],
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StateWriteOnly",
            false,
            strs(&[ADD_COLUMN_STMT]),
            vec![],
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StateDeleteReorganization",
            false,
            strs(&[ADD_COLUMN_STMT]),
            vec![],
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StateNone",
            false,
            strs(&[ADD_COLUMN_STMT]),
            vec![],
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StatePublic",
            true,
            strs(&[ADD_COLUMN_STMT, ADD_COLUMN_IDX_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[ADD_COLUMN_STMT, ADD_COLUMN_IDX_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StateWriteOnly",
            false,
            strs(&[ADD_COLUMN_STMT, ADD_COLUMN_IDX_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StateDeleteReorganization",
            false,
            strs(&[ADD_COLUMN_STMT, ADD_COLUMN_IDX_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            DROP_COLUMN_STMT.to_string(),
            "StateNone",
            false,
            strs(&[ADD_COLUMN_STMT, ADD_COLUMN_IDX_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            modify_mediumint,
            "StateNone",
            true,
            strs(&[ADD_COLUMN_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            modify_int,
            "StatePublic",
            false,
            strs(&[ADD_COLUMN_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            modify_char.clone(),
            "StateNone",
            true,
            strs(&[ADD_COLUMN_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            modify_char.clone(),
            "StateDeleteOnly",
            true,
            strs(&[ADD_COLUMN_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            modify_char.clone(),
            "StateWriteOnly",
            true,
            strs(&[ADD_COLUMN_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            modify_char.clone(),
            "StateWriteReorganization",
            true,
            strs(&[ADD_COLUMN_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
        stmt_case(
            ai,
            modify_char,
            "StatePublic",
            false,
            strs(&[ADD_COLUMN_STMT]),
            strs(&[DROP_COLUMN_STMT]),
        ),
    ]
}

/// Exchange / add / drop partition 的 pause 状态矩阵。
// tablePartitionDDLStmtCase 覆盖 exchange/add/drop partition 的 pause 状态。
pub fn table_partition_ddl_stmt_case(ai: &mut AutoIncrsedID) -> Vec<StmtCase> {
    // exchange partition：把分区与普通表互换数据；需开启 session 开关。
    let exchange = "alter table t_user_partition exchange partition p6 with table t_user;";
    let enable_exchange = "set @@tidb_enable_exchange_partition=1;";
    vec![
        stmt_case(
            ai,
            exchange.to_string(),
            "StateNone",
            true,
            strs(&[enable_exchange]),
            vec![],
        ),
        stmt_case(
            ai,
            exchange.to_string(),
            "StatePublic",
            false,
            strs(&[enable_exchange]),
            vec![],
        ),
        stmt_case(
            ai,
            ALTER_TABLE_PARTITION_ADD_PARTITION.to_string(),
            "StateNone",
            true,
            vec![],
            strs(&[ALTER_TABLE_PARTITION_DROP_PARTITION]),
        ),
        stmt_case(
            ai,
            ALTER_TABLE_PARTITION_ADD_PARTITION.to_string(),
            "StateReplicaOnly",
            true,
            vec![],
            strs(&[ALTER_TABLE_PARTITION_DROP_PARTITION]),
        ),
        stmt_case(
            ai,
            ALTER_TABLE_PARTITION_ADD_PARTITION.to_string(),
            "StatePublic",
            false,
            vec![],
            strs(&[ALTER_TABLE_PARTITION_DROP_PARTITION]),
        ),
        stmt_case(
            ai,
            ALTER_TABLE_PARTITION_DROP_PARTITION.to_string(),
            "StatePublic",
            true,
            strs(&[ALTER_TABLE_PARTITION_ADD_PARTITION]),
            strs(&[ALTER_TABLE_PARTITION_DROP_PARTITION]),
        ),
        stmt_case(
            ai,
            ALTER_TABLE_PARTITION_DROP_PARTITION.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[ALTER_TABLE_PARTITION_ADD_PARTITION]),
            strs(&[ALTER_TABLE_PARTITION_DROP_PARTITION]),
        ),
        // Go 源码这里保留了两个 StateDeleteOnly case；同样不去重。
        stmt_case(
            ai,
            ALTER_TABLE_PARTITION_DROP_PARTITION.to_string(),
            "StateDeleteOnly",
            false,
            strs(&[ALTER_TABLE_PARTITION_ADD_PARTITION]),
            strs(&[ALTER_TABLE_PARTITION_DROP_PARTITION]),
        ),
        stmt_case(
            ai,
            ALTER_TABLE_PARTITION_DROP_PARTITION.to_string(),
            "StateDeleteReorganization",
            false,
            strs(&[ALTER_TABLE_PARTITION_ADD_PARTITION]),
            strs(&[ALTER_TABLE_PARTITION_DROP_PARTITION]),
        ),
        stmt_case(
            ai,
            ALTER_TABLE_PARTITION_DROP_PARTITION.to_string(),
            "StateNone",
            false,
            strs(&[ALTER_TABLE_PARTITION_ADD_PARTITION]),
            strs(&[ALTER_TABLE_PARTITION_DROP_PARTITION]),
        ),
    ]
}

/// 修改 database placement policy 的 pause 状态矩阵（保留 Go 名 `placeRul` 拼写）。
// placeRulDDLStmtCase 保留 Go 源变量名拼写，覆盖 alter database placement policy。
pub fn place_rul_ddl_stmt_case(ai: &mut AutoIncrsedID) -> Vec<StmtCase> {
    vec![
        stmt_case(
            ai,
            ALTER_SCHEMA_POLICY.to_string(),
            "StateNone",
            true,
            strs(&[CREATE_PLACEMENT_POLICY, CREATE_SCHEMA_STMT]),
            strs(&[DROP_SCHEMA_STMT, DROP_PLACEMENT_POLICY]),
        ),
        stmt_case(
            ai,
            ALTER_SCHEMA_POLICY.to_string(),
            "StatePublic",
            false,
            strs(&[CREATE_PLACEMENT_POLICY, CREATE_SCHEMA_STMT]),
            strs(&[DROP_SCHEMA_STMT, DROP_PLACEMENT_POLICY]),
        ),
    ]
}

/// 简易执行：先跑前置 SQL，再跑目标 DDL，最后尽力执行回滚 SQL。
// simpleRunStmt 对应 Go 方法：依次执行前置 SQL、目标 DDL，再尽力执行回滚 SQL。
pub fn simple_run_stmt<E: crate::ddl_data_generation::SqlExecutor>(
    stmt_case: &StmtCase,
    stmt_kit: &mut E,
) -> Result<(), String> {
    for prepare_stmt in &stmt_case.pre_condition_stmts {
        // Go 使用 MustExec，前置条件失败会直接终止当前测试。
        stmt_kit.must_exec(prepare_stmt)?;
    }

    stmt_kit.must_exec(&stmt_case.stmt)?;

    for rollback_stmt in &stmt_case.rollback_stmts {
        // Go 回滚阶段使用 stmtKit.Exec 并忽略错误，避免清理失败覆盖目标 case 结果。
        let _ = stmt_kit.exec(rollback_stmt);
    }
    Ok(())
}
