// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// SHOW 结果构造单元测试。
//
// 对应 Go `pkg/executor/test/showtest/show_test.go`。验证
// `ConstructResultOfShowCreateDatabase` 的 Go 输出契约，并对标识符中的反引号
// 做加倍转义。

static SHOW_SQL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct SerialTestKit {
    _guard: std::sync::MutexGuard<'static, ()>,
    testkit: astersql_testkit::TestKit,
}

impl std::ops::Deref for SerialTestKit {
    type Target = astersql_testkit::TestKit;

    fn deref(&self) -> &Self::Target {
        &self.testkit
    }
}

impl std::ops::DerefMut for SerialTestKit {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.testkit
    }
}

fn new_show_testkit() -> SerialTestKit {
    let guard = SHOW_SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    SerialTestKit {
        _guard: guard,
        testkit: tk,
    }
}

/// 校验 Go `ConstructResultOfShowCreateDatabase` 的 IF NOT EXISTS、字符集、
/// 默认校对规则和 placement policy 输出。
#[test]
fn show_create_database_uses_default_collation_and_escapes_names() {
    use astersql_executor::show::{ConstructResultOfShowCreateDatabase, CreateDatabaseInput};

    // Go 使用普通 IF NOT EXISTS 文本，placement 扩展的等号两侧没有空格。
    let sql = ConstructResultOfShowCreateDatabase(&CreateDatabaseInput {
        name: "sales`archive".to_owned(),
        if_not_exists: true,
        charset: "utf8mb4".to_owned(),
        collation: "utf8mb4_bin".to_owned(),
        placement_policy: Some("hot`data".to_owned()),
    })
    .expect("valid database metadata must format");

    assert_eq!(
        sql,
        "CREATE DATABASE IF NOT EXISTS `sales``archive` \
/*!40100 DEFAULT CHARACTER SET utf8mb4 */ \
/*T![placement] PLACEMENT POLICY=`hot``data` */"
    );
}

#[test]
fn show_create_database_uses_explicit_non_default_collation() {
    use astersql_executor::show::{ConstructResultOfShowCreateDatabase, CreateDatabaseInput};

    let sql = ConstructResultOfShowCreateDatabase(&CreateDatabaseInput {
        name: "analytics".to_owned(),
        if_not_exists: false,
        charset: "utf8mb4".to_owned(),
        collation: "utf8mb4_general_ci".to_owned(),
        placement_policy: None,
    })
    .expect("valid database metadata must format");

    assert_eq!(
        sql,
        "CREATE DATABASE `analytics` /*!40100 DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci */"
    );
}

/// 对应 Go `TestShowCreatePlacementPolicy` 的 SHOW CREATE 输出格式化。
#[test]
fn show_create_placement_policy_formats_settings() {
    use astersql_executor::show::{
        ConstructResultOfShowCreatePlacementPolicy, PlacementPolicyInfo,
    };

    let sql = ConstructResultOfShowCreatePlacementPolicy(&PlacementPolicyInfo {
        name: "show`policy".to_owned(),
        settings: "FOLLOWERS=4 REGIONS=\"us-east-1,us-east-2\"".to_owned(),
    });
    assert_eq!(
        sql,
        "CREATE PLACEMENT POLICY `show``policy` FOLLOWERS=4 REGIONS=\"us-east-1,us-east-2\""
    );
}

/// 对应 Go `TestShowEscape` 的标识符转义契约，覆盖数据库和 placement 名称。
#[test]
fn show_create_database_rejects_empty_name() {
    use astersql_executor::show::{ConstructResultOfShowCreateDatabase, CreateDatabaseInput};

    let error = ConstructResultOfShowCreateDatabase(&CreateDatabaseInput {
        name: String::new(),
        if_not_exists: false,
        charset: String::new(),
        collation: String::new(),
        placement_policy: None,
    })
    .expect_err("empty database names must be rejected");
    assert!(
        error
            .to_string()
            .contains("database name must not be empty")
    );
}

/// 对应 Go `TestShowStatsExtendedRemoved`：已移除的扩展统计 SHOW 必须返回
/// 固定错误，而不是空结果或通用语法错误。
#[test]
fn show_stats_extended_reports_feature_removed() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let tk = TestKit::new(store);
    let error = tk.QueryToErr("SHOW STATS_EXTENDED");
    assert_eq!(
        error.message(),
        "Extended statistics feature has been removed"
    );
}

/// 对应 Go `TestShowCreatePlacementPolicy`：覆盖创建、修改、删除和不存在错误。
#[test]
fn show_create_placement_policy_executes_sql_contract() {
    let mut tk = new_show_testkit();
    tk.MustExec(
        "CREATE PLACEMENT POLICY xyz PRIMARY_REGION='us-east-1' \
         REGIONS='us-east-1,us-east-2' FOLLOWERS=4",
        Vec::new(),
    );
    tk.MustQuery("SHOW CREATE PLACEMENT POLICY xyz", Vec::new())
        .Check(astersql_testkit::RowsWithSep(
            "|",
            &[
                "xyz|CREATE PLACEMENT POLICY `xyz` PRIMARY_REGION=\"us-east-1\" \
             REGIONS=\"us-east-1,us-east-2\" FOLLOWERS=4",
            ],
        ));

    tk.MustExec(
        "CREATE PLACEMENT POLICY xyz2 FOLLOWERS=1 \
         SURVIVAL_PREFERENCES=\"[zone, dc, host]\"",
        Vec::new(),
    );
    tk.MustQuery("SHOW CREATE PLACEMENT POLICY xyz2", Vec::new())
        .Check(astersql_testkit::RowsWithSep(
            "|",
            &["xyz2|CREATE PLACEMENT POLICY `xyz2` FOLLOWERS=1 \
             SURVIVAL_PREFERENCES=\"[zone, dc, host]\""],
        ));
    tk.MustExec("DROP PLACEMENT POLICY xyz2", Vec::new());

    let error = tk.QueryToErr("SHOW CREATE PLACEMENT POLICY doesnotexist");
    assert!(
        error.message().contains("doesnotexist")
            && error
                .message()
                .to_ascii_lowercase()
                .contains("placement policy"),
        "unexpected error: {error}"
    );

    tk.MustExec("ALTER PLACEMENT POLICY xyz FOLLOWERS=4", Vec::new());
    tk.MustQuery("SHOW CREATE PLACEMENT POLICY xyz", Vec::new())
        .Check(astersql_testkit::RowsWithSep(
            "|",
            &["xyz|CREATE PLACEMENT POLICY `xyz` FOLLOWERS=4"],
        ));
    tk.MustExec(
        "ALTER PLACEMENT POLICY xyz FOLLOWERS=4 \
         SURVIVAL_PREFERENCES=\"[zone, dc, host]\"",
        Vec::new(),
    );
    tk.MustQuery("SHOW CREATE PLACEMENT POLICY xyz", Vec::new())
        .Check(astersql_testkit::RowsWithSep(
            "|",
            &["xyz|CREATE PLACEMENT POLICY `xyz` FOLLOWERS=4 \
             SURVIVAL_PREFERENCES=\"[zone, dc, host]\""],
        ));
    tk.MustExec("DROP PLACEMENT POLICY xyz", Vec::new());
}

/// 对应 Go `TestShowEscape`：反引号与 ANSI_QUOTES 模式必须分别正确转义。
#[test]
fn show_create_table_escapes_identifiers_for_sql_mode() {
    let mut tk = new_show_testkit();
    tk.MustExec("drop table if exists `t``abl\"e`", Vec::new());
    tk.MustExec(
        "create table `t``abl\"e`(`c``olum\"n` int(11) primary key)",
        Vec::new(),
    );
    tk.MustQuery("show create table `t``abl\"e`", Vec::new())
        .Check(astersql_testkit::RowsWithSep(
            "|",
            &[concat!(
                "t`abl\"e|CREATE TABLE `t``abl\"e` (\n",
                "  `c``olum\"n` int(11) NOT NULL,\n",
                "  PRIMARY KEY (`c``olum\"n`) /*T![clustered_index] CLUSTERED */\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            )],
        ));

    tk.MustExec("set @old_sql_mode=@@sql_mode", Vec::new());
    tk.MustExec("set sql_mode=ansi_quotes", Vec::new());
    tk.MustQuery("show create table \"t`abl\"\"e\"", Vec::new())
        .Check(astersql_testkit::RowsWithSep(
            "|",
            &[concat!(
                "t`abl\"e|CREATE TABLE \"t`abl\"\"e\" (\n",
                "  \"c`olum\"\"n\" int(11) NOT NULL,\n",
                "  PRIMARY KEY (\"c`olum\"\"n\") /*T![clustered_index] CLUSTERED */\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            )],
        ));
    tk.MustExec("rename table \"t`abl\"\"e\" to t", Vec::new());
    tk.MustExec("set sql_mode=@old_sql_mode", Vec::new());
}

/// 对应 Go `TestShowLimitReturnRow`：会话行数限制同样约束 SHOW 结果。
#[test]
fn show_and_select_respect_sql_select_limit_and_filters() {
    let mut tk = new_show_testkit();
    tk.MustExec("drop table if exists t1, t2", Vec::new());
    tk.MustExec(
        "create table t1(a int, b int, c int, d int, \
         index idx_a(a), index idx_b(b))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2(a int, b int, c int, d int, \
         index idx_a(a), index idx_b(b))",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO t1 VALUES(1,2,3,4),(4,3,1,2)", Vec::new());
    tk.MustExec("SET @@sql_select_limit=1", Vec::new());

    tk.MustExec("PREPARE stmt FROM \"SHOW COLUMNS FROM t1\"", Vec::new());
    assert_eq!(tk.MustQuery("EXECUTE stmt", Vec::new()).len(), 1);
    tk.MustExec("PREPARE stmt FROM \"select * FROM t1\"", Vec::new());
    assert_eq!(tk.MustQuery("EXECUTE stmt", Vec::new()).len(), 1);
    assert_eq!(tk.MustQuery("SHOW ENGINES", Vec::new()).len(), 1);
    tk.MustQuery("SHOW DATABASES like '%SCHEMA'", Vec::new())
        .Check(astersql_testkit::Rows(&["INFORMATION_SCHEMA"]));
    tk.MustQuery("SHOW TABLES where tables_in_test='t2'", Vec::new())
        .Check(astersql_testkit::Rows(&["t2"]));
    assert_eq!(
        tk.MustQuery("SHOW TABLE STATUS where name='t2'", Vec::new())
            .Rows()[0][0],
        "t2"
    );
    tk.MustQuery("SHOW COLUMNS FROM t1 where Field ='d'", Vec::new())
        .Check(astersql_testkit::Rows(&["d int(11) YES  <nil> "]));
    tk.MustQuery("Show Charset where charset='gbk'", Vec::new())
        .Check(vec![vec![
            "gbk",
            "Chinese Internal Code Specification",
            "gbk_chinese_ci",
            "2",
        ]]);
    assert_eq!(
        tk.MustQuery("SHOW status where variable_name ='server_id'", Vec::new())
            .Rows()[0][0],
        "server_id"
    );
    tk.MustQuery("Show Collation where collation='utf8_bin'", Vec::new())
        .Check(vec![vec![
            "utf8_bin",
            "utf8",
            "83",
            "Yes",
            "Yes",
            "1",
            "PAD SPACE",
        ]]);
    assert_eq!(
        tk.MustQuery("show index from t1 where key_name='idx_b'", Vec::new())
            .Rows()[0][2],
        "idx_b"
    );
}

/// 对应 Go `TestShowConfig`：`tidb_config` 必须返回实际配置 JSON。
#[test]
fn show_config_variable_contains_runtime_configuration() {
    let tk = new_show_testkit();
    let rows = tk
        .MustQuery("show variables like '%config%'", Vec::new())
        .Rows();
    assert_eq!(rows.len(), 1);
    let config = &rows[0][1];
    assert!(
        config.contains("\t\t\"copr-cache\": {\n\t\t\t\"capacity-mb\": 1000\n\t\t},\n"),
        "missing copr-cache config: {config}"
    );
    assert!(config.contains("\"enable-global-kill\": true"));
}

/// 对应 Go `TestShowVar`：按注册表检查 session/global 可见性与大小写 LIKE。
#[test]
fn show_variables_follow_registered_scopes_and_case_insensitive_like() {
    let mut tk = new_show_testkit();
    let variables = astersql_sessionctx_variable::GetSysVars();
    let mut session_only = Vec::new();
    let mut session_visible_global = Vec::new();
    let mut global_visible = Vec::new();
    for variable in variables.values() {
        if variable.Scope == astersql_sessionctx_vardef::ScopeSession {
            session_only.push(variable.Name.clone());
        } else if !variable.HasSessionScope() && !variable.InternalSessionVariable {
            session_visible_global.push(variable.Name.clone());
        } else {
            global_visible.push(variable.Name.clone());
        }
    }

    let in_list = |names: &[String]| names.join("','");
    let sql = format!(
        "show variables where variable_name in('{}')",
        in_list(&session_only)
    );
    assert_eq!(tk.MustQuery(&sql, Vec::new()).len(), session_only.len());
    let sql = format!(
        "show global variables where variable_name in('{}')",
        in_list(&session_only)
    );
    assert!(tk.MustQuery(&sql, Vec::new()).is_empty());

    let sql = format!(
        "show variables where variable_name in('{}')",
        in_list(&session_visible_global)
    );
    assert_eq!(
        tk.MustQuery(&sql, Vec::new()).len(),
        session_visible_global.len()
    );
    let sql = format!(
        "show global variables where variable_name in('{}')",
        in_list(&global_visible)
    );
    assert_eq!(tk.MustQuery(&sql, Vec::new()).len(), global_visible.len());

    let version_rows = tk
        .MustQuery("show variables like 'version%'", Vec::new())
        .Rows();
    for row in version_rows {
        match row[0].as_str() {
            "version" => assert_eq!(row[1], astersql_parser_mysql::r#const::ServerVersion()),
            "version_comment" => assert_eq!(row[1], "AsterSQL Server"),
            _ => {}
        }
    }

    tk.MustExec("SET @@SQL_MODE='NO_BACKSLASH_ESCAPES'", Vec::new());
    tk.MustQuery("SHOW SESSION VARIABLES like 'sql_mode'", Vec::new())
        .Check(vec![vec!["sql_mode", "NO_BACKSLASH_ESCAPES"]]);
    tk.MustQuery("SHOW SESSION VARIABLES like 'SQL_MODE'", Vec::new())
        .Check(vec![vec!["sql_mode", "NO_BACKSLASH_ESCAPES"]]);
}

/// 对应 Go `TestShowTableStatus`：固定列、分区注释和库名大小写均保持兼容。
#[test]
fn show_table_status_reports_stable_fields_and_partition_comment() {
    let mut tk = new_show_testkit();
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t(a bigint)", Vec::new());

    let rows = tk.MustQuery("show table status", Vec::new()).Rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(&rows[0][0..4], ["t", "InnoDB", "10", "Compact"]);

    tk.MustExec("drop table if exists tp", Vec::new());
    tk.MustExec(
        "create table tp (a int) partition by range(a) \
         (partition p0 values less than (10), \
          partition p1 values less than (20), \
          partition p2 values less than (maxvalue))",
        Vec::new(),
    );
    let rows = tk
        .MustQuery("show table status from test like 'tp'", Vec::new())
        .Rows();
    assert_eq!(rows[0][16], "partitioned");

    tk.MustExec("create database UPPER_CASE", Vec::new());
    tk.MustExec("use UPPER_CASE", Vec::new());
    tk.MustExec("create table t (i int)", Vec::new());
    assert_eq!(tk.MustQuery("show table status", Vec::new()).len(), 1);
    tk.MustExec("use upper_case", Vec::new());
    assert_eq!(tk.MustQuery("show table status", Vec::new()).len(), 1);
    tk.MustExec("drop database UPPER_CASE", Vec::new());
}

/// 对应 Go `TestShowCreateTableWithIntegerDisplayLengthWarnings` 的全部类型组合。
#[test]
fn show_create_table_strips_deprecated_integer_widths_and_reports_warnings() {
    static STRICT_WIDTH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct RestoreStrictWidth;
    impl Drop for RestoreStrictWidth {
        fn drop(&mut self) {
            unsafe {
                astersql_parser_types::TiDBStrictIntegerDisplayWidth = false;
            }
        }
    }
    let mut tk = new_show_testkit();
    let warning = vec![
        "Warning".to_owned(),
        "1681".to_owned(),
        "Integer display width is deprecated and will be removed in a future release.".to_owned(),
    ];
    let zerofill_warning = vec![
        "Warning".to_owned(),
        "1681".to_owned(),
        "The ZEROFILL attribute is deprecated and will be removed in a future release. Use the LPAD function to zero-pad numbers, or store the formatted numbers in a CHAR column.".to_owned(),
    ];
    let cases = [
        (
            "create table t(a int(2), b varchar(2))",
            vec![warning.clone()],
            concat!(
                "CREATE TABLE `t` (\n",
                "  `a` int DEFAULT NULL,\n",
                "  `b` varchar(2) DEFAULT NULL\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            ),
        ),
        (
            "create table t(a bigint(10), b bigint)",
            vec![warning.clone()],
            concat!(
                "CREATE TABLE `t` (\n",
                "  `a` bigint DEFAULT NULL,\n",
                "  `b` bigint DEFAULT NULL\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            ),
        ),
        (
            "create table t(a tinyint(5), b tinyint(2), c tinyint)",
            vec![warning.clone(), warning.clone()],
            concat!(
                "CREATE TABLE `t` (\n",
                "  `a` tinyint DEFAULT NULL,\n",
                "  `b` tinyint DEFAULT NULL,\n",
                "  `c` tinyint DEFAULT NULL\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            ),
        ),
        (
            "create table t(a smallint(5), b smallint)",
            vec![warning.clone()],
            concat!(
                "CREATE TABLE `t` (\n",
                "  `a` smallint DEFAULT NULL,\n",
                "  `b` smallint DEFAULT NULL\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            ),
        ),
        (
            "create table t(a mediumint(5), b mediumint)",
            vec![warning.clone()],
            concat!(
                "CREATE TABLE `t` (\n",
                "  `a` mediumint DEFAULT NULL,\n",
                "  `b` mediumint DEFAULT NULL\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            ),
        ),
        (
            "create table t(a int1(1), b int2(2), c int3, d int4, e int8)",
            vec![warning.clone()],
            concat!(
                "CREATE TABLE `t` (\n",
                "  `a` tinyint(1) DEFAULT NULL,\n",
                "  `b` smallint DEFAULT NULL,\n",
                "  `c` mediumint DEFAULT NULL,\n",
                "  `d` int DEFAULT NULL,\n",
                "  `e` bigint DEFAULT NULL\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            ),
        ),
        (
            "create table t(id int primary key, c1 bool, c2 int(10) zerofill)",
            vec![warning, zerofill_warning],
            concat!(
                "CREATE TABLE `t` (\n",
                "  `id` int NOT NULL,\n",
                "  `c1` tinyint(1) DEFAULT NULL,\n",
                "  `c2` int(10) unsigned zerofill DEFAULT NULL,\n",
                "  PRIMARY KEY (`id`) /*T![clustered_index] CLUSTERED */\n",
                ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            ),
        ),
    ];

    for (ddl, expected_warnings, expected_create) in cases {
        tk.MustExec("drop table if exists t", Vec::new());
        let _lock = STRICT_WIDTH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            astersql_parser_types::TiDBStrictIntegerDisplayWidth = true;
        }
        let restore = RestoreStrictWidth;
        tk.MustExec(ddl, Vec::new());
        assert_eq!(
            tk.MustQuery("show warnings", Vec::new()).Rows(),
            expected_warnings
        );
        tk.MustQuery("show create table t", Vec::new())
            .Check(vec![vec!["t", expected_create]]);
        drop(restore);
    }
}

/// 对应 Go `TestShowWarnings`：Warning/Error/Note 级别及计数生命周期。
#[test]
fn show_warnings_preserves_previous_statement_and_resets_counts() {
    let mut tk = new_show_testkit();
    tk.MustExec(
        "create table if not exists show_warnings (a int)",
        Vec::new(),
    );
    tk.MustExec("set @@sql_mode=''", Vec::new());
    tk.MustQuery("select @@sql_mode", Vec::new())
        .Check(astersql_testkit::Rows(&[""]));
    tk.MustExec("insert show_warnings values ('a')", Vec::new());
    tk.MustQuery("show warnings", Vec::new()).Check(vec![vec![
        "Warning",
        "1366",
        "Incorrect int value: 'a' for column 'a' at row 1",
    ]]);
    tk.MustQuery("show warnings", Vec::new()).Check(vec![vec![
        "Warning",
        "1366",
        "Incorrect int value: 'a' for column 'a' at row 1",
    ]]);

    let error = tk
        .Exec("create table show_warnings (a int)", Vec::new())
        .expect_err("duplicate CREATE TABLE must fail");
    assert!(error.message().contains("already exists"));
    tk.MustQuery("show warnings", Vec::new()).Check(vec![vec![
        "Error",
        "1050",
        "Table 'test.show_warnings' already exists",
    ]]);
    tk.MustQuery("select @@error_count", Vec::new())
        .Check(astersql_testkit::Rows(&["1"]));

    tk.MustExec("create table show_warnings_2 (a int)", Vec::new());
    tk.MustExec(
        "create table if not exists show_warnings_2 like show_warnings",
        Vec::new(),
    );
    tk.MustQuery("show warnings", Vec::new()).Check(vec![vec![
        "Note",
        "1050",
        "Table 'test.show_warnings_2' already exists",
    ]]);
    tk.MustQuery("select @@warning_count", Vec::new())
        .Check(astersql_testkit::Rows(&["1"]));
    tk.MustQuery("select @@warning_count", Vec::new())
        .Check(astersql_testkit::Rows(&["0"]));
}

/// 对应 Go `TestShowCreateTablePlacement`：表级及各分区类型的 placement
/// 信息必须在 SHOW CREATE TABLE 中完整恢复。
#[test]
fn show_create_table_restores_placement_for_tables_and_partitions() {
    let mut tk = new_show_testkit();
    tk.MustExec(
        "create placement policy x followers=2 constraints='[+disk=ssd]'",
        Vec::new(),
    );
    tk.MustExec("create table t(a int) placement policy='x'", Vec::new());
    let table_create = tk.MustQuery("show create table t", Vec::new()).Rows()[0][1].clone();
    assert!(
        table_create.ends_with("/*T![placement] PLACEMENT POLICY=`x` */"),
        "actual SHOW CREATE: {table_create}"
    );

    let cases = [
        (
            "partition by list (a) (partition pLow values in (1,2,3) comment 'a comment' placement policy 'x', partition pMax values in (10,11))",
            "PARTITION BY LIST (`a`)\n(PARTITION `pLow` VALUES IN (1,2,3) COMMENT 'a comment' /*T![placement] PLACEMENT POLICY=`x` */,\n PARTITION `pMax` VALUES IN (10,11))",
        ),
        (
            "partition by list columns (b) (partition pLow values in ('1','2') placement policy 'x', partition pMax values in ('10'))",
            "PARTITION BY LIST COLUMNS(`b`)\n(PARTITION `pLow` VALUES IN ('1','2') /*T![placement] PLACEMENT POLICY=`x` */,\n PARTITION `pMax` VALUES IN ('10'))",
        ),
        (
            "partition by range (a) (partition pLow values less than (10) placement policy 'x', partition pMax values less than (maxvalue))",
            "PARTITION BY RANGE (`a`)\n(PARTITION `pLow` VALUES LESS THAN (10) /*T![placement] PLACEMENT POLICY=`x` */,\n PARTITION `pMax` VALUES LESS THAN (MAXVALUE))",
        ),
        (
            "partition by range columns (a,b) (partition pLow values less than (10,'10') placement policy 'x', partition pMax values less than (maxvalue,'x'))",
            "PARTITION BY RANGE COLUMNS(`a`,`b`)\n(PARTITION `pLow` VALUES LESS THAN (10,'10') /*T![placement] PLACEMENT POLICY=`x` */,\n PARTITION `pMax` VALUES LESS THAN (MAXVALUE,'x'))",
        ),
        (
            "partition by hash (a) (partition pLow comment 'a comment' placement policy 'x', partition pMax)",
            "PARTITION BY HASH (`a`)\n(PARTITION `pLow` COMMENT 'a comment' /*T![placement] PLACEMENT POLICY=`x` */,\n PARTITION `pMax`)",
        ),
    ];
    for (partition, expected_suffix) in cases {
        tk.MustExec("drop table t", Vec::new());
        tk.MustExec(
            &format!("create table t(a int, b varchar(255)) placement policy='x' {partition}"),
            Vec::new(),
        );
        let create = &tk.MustQuery("show create table t", Vec::new()).Rows()[0][1];
        assert!(
            create.ends_with(expected_suffix),
            "actual SHOW CREATE: {create}"
        );
        tk.MustQuery("show warnings", Vec::new())
            .Check(astersql_testkit::Rows(&[]));
    }
}

/// 对应 Go `TestShowVisibility`：库表可见性随授权与回收实时变化。
#[test]
fn show_visibility_tracks_database_and_table_privileges() {
    use astersql_parser_auth::parser::auth::auth::UserIdentity;
    use astersql_testkit::TestKit;

    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut root = TestKit::new(store.clone());
    root.MustExec("create database showdatabase", Vec::new());
    root.MustExec("create table showdatabase.t1 (id int)", Vec::new());
    root.MustExec("create table showdatabase.t2 (id int)", Vec::new());
    root.MustExec("create user 'show'@'%'", Vec::new());
    let mut show = TestKit::new(store);
    show.Session()
        .AuthenticateUserForTest(&UserIdentity {
            username: "show".to_owned(),
            hostname: "127.0.0.1".to_owned(),
            ..Default::default()
        })
        .expect("authenticate show user");
    show.MustQuery("show databases", Vec::new())
        .Check(astersql_testkit::Rows(&["INFORMATION_SCHEMA"]));
    root.MustExec("grant select on showdatabase.t1 to 'show'@'%'", Vec::new());
    show.MustQuery("show databases", Vec::new())
        .Check(astersql_testkit::Rows(&[
            "INFORMATION_SCHEMA",
            "showdatabase",
        ]));
    show.MustExec("use showdatabase", Vec::new());
    show.MustQuery("show tables", Vec::new())
        .Check(astersql_testkit::Rows(&["t1"]));
    root.MustExec(
        "revoke select on showdatabase.t1 from 'show'@'%'",
        Vec::new(),
    );
    show.MustQuery("show databases", Vec::new())
        .Check(astersql_testkit::Rows(&["INFORMATION_SCHEMA"]));
    root.MustExec("grant create on *.* to 'show'@'%'", Vec::new());
    assert!(show.MustQuery("show databases", Vec::new()).Rows().len() >= 2);
}

/// 对应 Go `TestShowGrantsPrivilege`、`TestIssue18878`、`TestIssue17794`：SHOW GRANTS
/// 使用认证账户，并严格区分显式目标的 host。
#[test]
fn show_grants_enforces_target_privileges_and_authenticated_host() {
    use astersql_parser_auth::parser::auth::auth::UserIdentity;
    use astersql_testkit::TestKit;

    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut root = TestKit::new(store.clone());
    root.MustExec("create user show_grants", Vec::new());
    root.MustQuery("show grants for show_grants", Vec::new());
    let unprivileged = TestKit::new(store.clone());
    unprivileged
        .Session()
        .AuthenticateUserForTest(&UserIdentity {
            username: "show_grants".to_owned(),
            hostname: "127.0.0.1".to_owned(),
            ..Default::default()
        })
        .expect("authenticate show_grants");
    let error = unprivileged.QueryToErr("show grants for root");
    assert!(error.message().contains("Access denied"));
    unprivileged.MustQuery("show grants", Vec::new());

    root.Session()
        .AuthenticateUserForTest(&UserIdentity {
            username: "root".to_owned(),
            hostname: "127.0.0.1".to_owned(),
            ..Default::default()
        })
        .expect("authenticate root by client host");
    root.MustQuery("select user()", Vec::new())
        .Check(astersql_testkit::Rows(&["root@127.0.0.1"]));
    for host in ["127.0.0.1", "localhost", "1.1.1.1"] {
        let error = root.QueryToErr(&format!("show grants for root@'{host}'"));
        assert!(error.message().contains("There is no such grant defined"));
    }
    root.MustExec("create user 'root'@'8.8.%'", Vec::new());
    let root_pattern = TestKit::new(store);
    root_pattern
        .Session()
        .AuthenticateUserForTest(&UserIdentity {
            username: "root".to_owned(),
            hostname: "8.8.8.8".to_owned(),
            ..Default::default()
        })
        .expect("authenticate root host pattern");
    root_pattern
        .MustQuery("show grants", Vec::new())
        .Check(astersql_testkit::RowsWithSep(
            "|",
            &["GRANT USAGE ON *.* TO 'root'@'8.8.%'"],
        ));
}

/// 对应 Go `TestShowStatsPrivilege`：统计 SHOW 需要 mysql 统计表读取权限。
#[test]
fn show_stats_requires_mysql_statistics_privileges() {
    use astersql_parser_auth::parser::auth::auth::UserIdentity;
    use astersql_testkit::TestKit;

    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut root = TestKit::new(store.clone());
    root.MustExec("create user show_stats", Vec::new());
    let mut stats = TestKit::new(store);
    stats
        .Session()
        .AuthenticateUserForTest(&UserIdentity {
            username: "show_stats".to_owned(),
            hostname: "127.0.0.1".to_owned(),
            ..Default::default()
        })
        .expect("authenticate stats user");
    for sql in [
        "show stats_meta",
        "show stats_buckets",
        "show stats_histograms",
        "show stats_healthy",
    ] {
        let error = stats.QueryToErr(sql);
        assert!(
            error.message().contains("denied") || error.message().contains("Access denied"),
            "{sql}: {error}"
        );
    }
    root.MustExec("grant select on mysql.* to show_stats", Vec::new());
    for sql in [
        "show stats_meta",
        "show stats_buckets",
        "show stats_histograms",
        "show stats_healthy",
    ] {
        stats.MustExec(sql, Vec::new());
    }
}

/// 对应 Go `TestIssue10549` / `TestIssue11165`：默认角色影响当前身份的可见库与授权，
/// 且 SET DEFAULT ROLE 的 NONE/ALL/显式角色路径均可执行。
#[test]
fn show_grants_and_databases_include_active_default_roles() {
    use astersql_parser_auth::parser::auth::auth::UserIdentity;
    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut root = astersql_testkit::TestKit::new(store.clone());
    root.MustExec("create database newdb", Vec::new());
    root.MustExec("create role app_developer", Vec::new());
    root.MustExec("grant all on newdb.* to app_developer", Vec::new());
    root.MustExec("create user dev", Vec::new());
    root.MustExec("grant 'app_developer' to 'dev'", Vec::new());
    root.MustExec("set default role 'app_developer' to 'dev'", Vec::new());
    let dev = astersql_testkit::TestKit::new(store);
    dev.Session()
        .AuthenticateUserForTest(&UserIdentity {
            username: "dev".to_owned(),
            hostname: "127.0.0.1".to_owned(),
            ..Default::default()
        })
        .expect("authenticate dev");
    dev.MustQuery("show databases", Vec::new())
        .Check(astersql_testkit::Rows(&["INFORMATION_SCHEMA", "newdb"]));
    let grants = dev.MustQuery("show grants", Vec::new()).Rows();
    assert!(grants.iter().any(|row| row[0].contains("`newdb`.*")));

    root.MustExec("create role r_manager", Vec::new());
    root.MustExec("create user 'manager'@'localhost'", Vec::new());
    root.MustExec("grant 'r_manager' to 'manager'@'localhost'", Vec::new());
    root.MustExec("set default role all to 'manager'@'localhost'", Vec::new());
    root.MustExec("set default role none to 'manager'@'localhost'", Vec::new());
    root.MustExec(
        "set default role 'r_manager' to 'manager'@'localhost'",
        Vec::new(),
    );
}

/// 对应 Go `TestShowCreateUser`：认证插件、密码摘要、锁定与属性须恢复为 SQL。
#[test]
fn show_create_user_restores_authentication_and_account_options() {
    let mut tk = new_show_testkit();
    tk.MustExec(
        "create user 'test_show_create_user'@'%' identified by 'root'",
        Vec::new(),
    );
    tk.MustQuery("show create user 'test_show_create_user'@'%'", Vec::new())
        .Check(astersql_testkit::RowsWithSep("|", &[concat!(
            "CREATE USER `test_show_create_user`@`%` IDENTIFIED WITH 'mysql_native_password' AS ",
            "'*81F5E21E35407D884A6CD4A731AEBFB6AF209E1B' REQUIRE NONE PASSWORD EXPIRE DEFAULT ",
            "ACCOUNT UNLOCK PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT"
        )]));
    tk.MustExec("set sql_mode=ansi_quotes", Vec::new());
    tk.MustQuery("show create user 'test_show_create_user'@'%'", Vec::new())
        .Check(astersql_testkit::RowsWithSep("|", &[concat!(
            "CREATE USER \"test_show_create_user\"@\"%\" IDENTIFIED WITH 'mysql_native_password' AS ",
            "'*81F5E21E35407D884A6CD4A731AEBFB6AF209E1B' REQUIRE NONE PASSWORD EXPIRE DEFAULT ",
            "ACCOUNT UNLOCK PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT"
        )]));
    tk.MustExec("set sql_mode=default", Vec::new());
    let error = tk.QueryToErr("show create user 'missing'@'localhost'");
    assert!(error.message().contains("SHOW CREATE USER failed"));
    tk.MustExec(
        "create user 'lockness'@'%' identified by 'monster' account lock",
        Vec::new(),
    );
    assert!(
        tk.MustQuery("show create user 'lockness'@'%'", Vec::new())
            .Rows()[0][0]
            .contains("ACCOUNT LOCK")
    );
    tk.MustExec("create user commentUser comment '1234'", Vec::new());
    assert!(
        tk.MustQuery("show create user commentUser", Vec::new())
            .Rows()[0][0]
            .ends_with("ATTRIBUTE '{\"comment\": \"1234\"}'")
    );
}

/// 对应 Go `TestShow2` / `TestCollation`：完整列、序列/视图分类与 collation
/// 字段值必须由真实元数据路径返回。
#[test]
fn show_full_columns_tables_and_collations_use_metadata_contract() {
    let mut tk = new_show_testkit();
    tk.MustExec(
        "create table full_cols(c_int int, c_bool bool, c_char char(1) charset ascii collate ascii_bin, c_json json)",
        Vec::new(),
    );
    tk.MustQuery("show full columns from full_cols", Vec::new())
        .Check(vec![
            vec![
                "c_int",
                "int(11)",
                "",
                "YES",
                "",
                "<nil>",
                "",
                "select,insert,update,references",
                "",
            ],
            vec![
                "c_bool",
                "tinyint(1)",
                "",
                "YES",
                "",
                "<nil>",
                "",
                "select,insert,update,references",
                "",
            ],
            vec![
                "c_char",
                "char(1)",
                "ascii_bin",
                "YES",
                "",
                "<nil>",
                "",
                "select,insert,update,references",
                "",
            ],
            vec![
                "c_json",
                "json",
                "",
                "YES",
                "",
                "<nil>",
                "",
                "select,insert,update,references",
                "",
            ],
        ]);
    tk.MustExec("create sequence seq", Vec::new());
    tk.MustExec("create view v as select c_int from full_cols", Vec::new());
    tk.MustQuery("show full tables", Vec::new()).Check(vec![
        vec!["full_cols", "BASE TABLE"],
        vec!["seq", "SEQUENCE"],
        vec!["v", "VIEW"],
    ]);
    let collations = tk.MustQuery("show collation", Vec::new()).Rows();
    assert!(collations.iter().all(|row| row.len() == 7));
    tk.MustQuery(
        "show collation where Charset='utf8' and Collation='utf8_bin'",
        Vec::new(),
    )
    .Check(vec![vec![
        "utf8_bin",
        "utf8",
        "83",
        "Yes",
        "Yes",
        "1",
        "PAD SPACE",
    ]]);
}

/// 对应 Go `TestUnprivilegedShow`：SHOW TABLE STATUS 只泄露有表权限的对象。
#[test]
fn unprivileged_show_table_status_filters_invisible_tables() {
    use astersql_parser_auth::parser::auth::auth::UserIdentity;
    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut root = astersql_testkit::TestKit::new(store.clone());
    root.MustExec("create database testshow", Vec::new());
    root.MustExec("create table testshow.t1(a int)", Vec::new());
    root.MustExec("create table testshow.t2(a int)", Vec::new());
    root.MustExec("create user lowprivuser", Vec::new());
    let low = astersql_testkit::TestKit::new(store);
    low.Session()
        .AuthenticateUserForTest(&UserIdentity {
            username: "lowprivuser".to_owned(),
            hostname: "127.0.0.1".to_owned(),
            ..Default::default()
        })
        .expect("authenticate low privilege user");
    low.MustQuery("show table status from testshow", Vec::new())
        .Check(astersql_testkit::Rows(&[]));
    root.MustExec("grant all on testshow.t1 to lowprivuser", Vec::new());
    let rows = low
        .MustQuery("show table status from testshow", Vec::new())
        .Rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "t1");
}

/// 对应 Go `TestShowWarningsForExprPushdown`：TiFlash 不支持表达式会生成稳定 warning。
#[test]
fn show_warnings_reports_tiflash_expression_pushdown_rejections() {
    let mut tk = new_show_testkit();
    tk.MustExec(
        "create table show_warnings_expr_pushdown(a int, value date)",
        Vec::new(),
    );
    tk.MustExec(
        "alter table show_warnings_expr_pushdown set tiflash replica 1",
        Vec::new(),
    );
    tk.MustExec("set tidb_allow_mpp=0", Vec::new());
    tk.MustExec(
        "explain format='brief' select * from show_warnings_expr_pushdown where md5(value)='2020-01-01'",
        Vec::new(),
    );
    tk.MustQuery("show warnings", Vec::new()).Check(vec![vec![
        "Warning",
        "1105",
        "Scalar function 'md5'(signature: MD5, return type: var_string(32)) is not supported to push down to tiflash now.",
    ]]);
    tk.MustExec(
        "explain format='brief' select /*+ read_from_storage(tiflash[show_warnings_expr_pushdown]) */ max(md5(value)) from show_warnings_expr_pushdown group by a",
        Vec::new(),
    );
    tk.MustQuery("show warnings", Vec::new()).Check(vec![
        vec![
            "Warning",
            "1105",
            "Scalar function 'md5'(signature: MD5, return type: var_string(32)) is not supported to push down to tiflash now.",
        ],
        vec![
            "Warning",
            "1105",
            "Aggregation can not be pushed to tiflash because arguments of AggFunc `max` contains unsupported exprs",
        ],
    ]);
    tk.MustExec(
        "explain format='brief' select /*+ read_from_storage(tiflash[show_warnings_expr_pushdown]) */ max(a) from show_warnings_expr_pushdown group by md5(value)",
        Vec::new(),
    );
    tk.MustQuery("show warnings", Vec::new()).Check(vec![
        vec![
            "Warning",
            "1105",
            "Scalar function 'md5'(signature: MD5, return type: var_string(32)) is not supported to push down to tiflash now.",
        ],
        vec![
            "Warning",
            "1105",
            "Aggregation can not be pushed to tiflash because groupByItems contain unsupported exprs",
        ],
    ]);
    tk.MustExec("set tidb_opt_distinct_agg_push_down=0", Vec::new());
    tk.MustExec(
        "explain format='brief' select max(distinct a) from show_warnings_expr_pushdown group by value",
        Vec::new(),
    );
    tk.MustQuery("select @@warning_count", Vec::new())
        .Check(astersql_testkit::Rows(&["0"]));
}

/// 对应 Go `TestAutoRandomBase` / `TestAutoRandomWithLargeSignedShowTableRegions`。
#[test]
fn show_auto_random_bases_and_unsigned_region_keys_are_stable() {
    let mut tk = new_show_testkit();
    tk.MustExec("set @@allow_auto_random_explicit_insert=true", Vec::new());
    tk.MustExec(
        "create table ar(a bigint primary key auto_random(5), b int unique key auto_increment) auto_random_base=100, auto_increment=100",
        Vec::new(),
    );
    let create = tk.MustQuery("show create table ar", Vec::new()).Rows()[0][1].clone();
    assert!(create.contains("AUTO_INCREMENT=100"));
    assert!(create.contains("AUTO_RANDOM_BASE=100"));
    tk.MustExec(
        "create table region_t(a bigint unsigned auto_random primary key clustered)",
        Vec::new(),
    );
    tk.MustQuery(
        "split table region_t between (18446744073709541615) and (18446744073709551615) regions 2",
        Vec::new(),
    )
    .Check(astersql_testkit::Rows(&["1 1"]));
    let regions = tk
        .MustQuery("show table region_t regions", Vec::new())
        .Rows();
    assert!(regions.len() >= 2);
    let marker = regions[1][1].find("_r_").expect("record key marker");
    assert_ne!(regions[1][1].as_bytes()[marker + 3], b'-');
}

/// 对应 Go `TestShowClusterConfig`：SQL 路径至少返回标准四列且 WHERE 生效。
#[test]
fn show_cluster_config_has_filterable_four_column_contract() {
    let _guard = SHOW_SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let tk = astersql_testkit::TestKit::new(store);
    let rows = vec![
        vec!["tidb", "127.0.0.1:1111", "log.level", "info"],
        vec!["pd", "127.0.0.1:2222", "log.level", "info"],
        vec!["tikv", "127.0.0.1:3333", "log.level", "info"],
    ]
    .into_iter()
    .map(|row| row.into_iter().map(str::to_owned).collect())
    .collect();
    astersql_session::runtime::SetShowClusterConfigForTest(&domain, Ok(rows));
    tk.MustQuery("show config", Vec::new()).Check(vec![
        vec!["tidb", "127.0.0.1:1111", "log.level", "info"],
        vec!["pd", "127.0.0.1:2222", "log.level", "info"],
        vec!["tikv", "127.0.0.1:3333", "log.level", "info"],
    ]);
    tk.MustQuery("show config where type='tidb'", Vec::new())
        .Check(vec![vec!["tidb", "127.0.0.1:1111", "log.level", "info"]]);
    tk.MustQuery("show config where type like '%ti%'", Vec::new())
        .Check(vec![
            vec!["tidb", "127.0.0.1:1111", "log.level", "info"],
            vec!["tikv", "127.0.0.1:3333", "log.level", "info"],
        ]);
    astersql_session::runtime::SetShowClusterConfigForTest(
        &domain,
        Err("something unknown error".to_owned()),
    );
    assert_eq!(
        tk.QueryToErr("show config").message(),
        "something unknown error"
    );
}
