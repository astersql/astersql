// Copyright 2026 AsterSQL.

#[derive(Clone, Copy, Debug)]
pub struct ParserContractCase {
    pub sql: &'static str,
    pub ok: bool,
    pub restored: &'static str,
}

const fn accepts(sql: &'static str) -> ParserContractCase {
    ParserContractCase {
        sql,
        ok: true,
        restored: sql,
    }
}

const fn rejects(sql: &'static str) -> ParserContractCase {
    ParserContractCase {
        sql,
        ok: false,
        restored: "",
    }
}

pub fn parser_contract_cases(name: &str) -> Vec<ParserContractCase> {
    let cases: &[ParserContractCase] = match name {
        "TestRecommendIndex" => &[
            accepts("recommend index run"),
            accepts("recommend index run with A = 1"),
            accepts("recommend index run with A = 1, B = 2"),
            accepts("recommend index run for 'select * from t where a=1'"),
            accepts("recommend index run for 'select * from t where a=1' with A = 1"),
            accepts("recommend index run for 'select * from t where a=1' with A = 1, B = 2"),
            accepts("recommend index show option"),
            accepts("recommend index apply 1"),
            accepts("recommend index ignore 1"),
            accepts("recommend index set A = 1"),
            accepts("recommend index set A = 1, B = 2"),
            accepts("recommend index set A = 1, B = 2, C = 3"),
        ],
        "TestAdminStmt" => &[accepts("ADMIN CHECK TABLE t")],
        "TestDMLStmt" => &[
            accepts("INSERT INTO t(a) VALUES (1)"),
            accepts("UPDATE t SET a = 2 WHERE a = 1"),
            accepts("DELETE FROM t WHERE a = 2"),
        ],
        "TestDBAStmt" => &[accepts("SHOW PROCESSLIST")],
        "TestExpression" => &[accepts("SELECT (1 + 2) * 3, NOT FALSE, 1 BETWEEN 0 AND 2")],
        "TestBuiltin" => &[accepts("SELECT COUNT(*), SUM(a), AVG(a) FROM t")],
        "TestIdentifier" => &[accepts("SELECT `select` FROM `table`")],
        "TestDDL" => &[
            accepts("CREATE TABLE t (id BIGINT PRIMARY KEY, v VARCHAR(32))"),
            accepts("ALTER TABLE t ADD COLUMN c INT"),
            accepts("DROP TABLE IF EXISTS t"),
        ],
        "TestType" => &[accepts(
            "CREATE TABLE t (a TINYINT, b BIGINT UNSIGNED, c DECIMAL(10,2), d DATETIME, e JSON)",
        )],
        "TestPrivilege" => &[accepts("GRANT SELECT ON db.* TO 'user'@'%'")],
        "TestComment" => &[accepts("SELECT /* parser contract */ 1 -- trailing\n")],
        "TestSetOperator" => &[accepts("SELECT 1 UNION ALL SELECT 2 EXCEPT SELECT 3")],
        "TestLikeEscape" => &[accepts("SELECT 'a_b' LIKE 'a\\_b' ESCAPE '\\\\'")],
        "TestLockUnlockTables" => &[accepts("LOCK TABLES t READ"), accepts("UNLOCK TABLES")],
        "TestWithRollup" => &[accepts("SELECT a, COUNT(*) FROM t GROUP BY a WITH ROLLUP")],
        "TestIndexHint" => &[accepts("SELECT * FROM t USE INDEX (idx_a)")],
        "TestSQLResult" => &[accepts("SELECT 1 AS one")],
        "TestEscape" => &[accepts("SELECT 'line\\nfeed', `a``b`")],
        "TestExplain" => &[accepts("EXPLAIN SELECT * FROM t WHERE a = 1")],
        "TestPrepare" => &[accepts("PREPARE stmt FROM 'SELECT 1'")],
        "TestDeallocate" => &[accepts("DEALLOCATE PREPARE stmt")],
        "TestExecute" => &[accepts("EXECUTE stmt USING @a")],
        "TestTrace" => &[accepts("TRACE SELECT * FROM t")],
        "TestSessionManage" => &[accepts("KILL 1")],
        "TestParseShowOpenTables" => &[accepts("SHOW OPEN TABLES FROM test LIKE 't%'")],
        "TestPrivilegeMariaDBEnabled" | "TestPrivilegeMariaDBDisabled" => {
            &[accepts("GRANT SELECT ON db.* TO 'user'@'%'")]
        }
        "TestSystemVersionedColumnMariaDBEnabled" | "TestSystemVersionedColumnMariaDBDisabled" => {
            &[accepts("SELECT 1")]
        }
        "TestSubquery" => &[accepts(
            "SELECT * FROM t WHERE EXISTS (SELECT 1 FROM s WHERE s.id = t.id)",
        )],
        "TestPriority" => &[accepts("SELECT HIGH_PRIORITY * FROM t")],
        "TestBinding" => &[accepts(
            "CREATE GLOBAL BINDING FOR SELECT * FROM t USING SELECT * FROM t USE INDEX(idx)",
        )],
        "TestView" => &[accepts("CREATE VIEW v AS SELECT a FROM t")],
        "TestTimestampDiffUnit" => &[accepts(
            "SELECT TIMESTAMPDIFF(DAY, '2020-01-01', '2020-01-02')",
        )],
        "TestAnalyze" => &[accepts("ANALYZE TABLE t")],
        "TestStartTransaction" => &[accepts("START TRANSACTION WITH CONSISTENT SNAPSHOT")],
        "TestBRIE" => &[accepts("BACKUP DATABASE test TO 'local:///tmp/backup'")],
        "TestCTE" => &[accepts("WITH cte AS (SELECT 1 AS a) SELECT a FROM cte")],
        "TestCTEMerge" => &[accepts(
            "WITH cte AS (SELECT 1) SELECT * FROM cte UNION SELECT * FROM cte",
        )],
        "TestAsOfClause" => &[accepts(
            "SELECT * FROM t AS OF TIMESTAMP '2020-01-01 00:00:00'",
        )],
        "TestTableSample" => &[accepts("SELECT * FROM t TABLESAMPLE REGIONS()")],
        "TestTablePartition" => &[accepts(
            "CREATE TABLE t (id INT) PARTITION BY RANGE (id) (PARTITION p0 VALUES LESS THAN (10))",
        )],
        "TestWindowFunctions" => &[accepts(
            "SELECT ROW_NUMBER() OVER (PARTITION BY a ORDER BY b), SUM(b) OVER (ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t",
        )],
        "TestStatisticsOps" => &[accepts("CREATE STATISTICS stats (CARDINALITY) ON t(a)")],
        "TestPartitionKeyAlgorithm" => &[accepts(
            "CREATE TABLE t (a VARCHAR(20)) PARTITION BY KEY ALGORITHM = 2 (a) PARTITIONS 4",
        )],
        "TestHelp" => &[accepts("HELP 'SELECT'")],
        "TestRestoreBinOpWithBrackets" => &[accepts("SELECT (a + b) * (c - d) FROM t")],
        "TestCTEBindings" => &[accepts("WITH cte AS (SELECT 1 AS a) SELECT a FROM cte")],
        "TestPlanReplayer" => &[accepts("PLAN REPLAYER DUMP EXPLAIN 'SELECT * FROM t'")],
        "TestTrafficStmt" => &[
            ParserContractCase {
                sql: "traffic capture to '/tmp' duration='1s' encryption_method='aes' compress=true",
                ok: true,
                restored: "TRAFFIC CAPTURE TO '/tmp' DURATION = '1s' ENCRYPTION_METHOD = 'aes' COMPRESS = TRUE",
            },
            ParserContractCase {
                sql: "traffic capture to '/tmp' duration '1s' encryption_method 'aes' compress true",
                ok: true,
                restored: "TRAFFIC CAPTURE TO '/tmp' DURATION = '1s' ENCRYPTION_METHOD = 'aes' COMPRESS = TRUE",
            },
            ParserContractCase {
                sql: "traffic capture to '/tmp' encryption_method='aes' duration='1s'",
                ok: true,
                restored: "TRAFFIC CAPTURE TO '/tmp' ENCRYPTION_METHOD = 'aes' DURATION = '1s'",
            },
            ParserContractCase {
                sql: "traffic capture to '/tmp' duration='1m'",
                ok: true,
                restored: "TRAFFIC CAPTURE TO '/tmp' DURATION = '1m'",
            },
            rejects("traffic capture to '/tmp' duration='1'"),
            rejects("traffic capture to '/tmp' duration=1s"),
            rejects("traffic capture to '/tmp' compress='true'"),
            rejects("traffic capture duration='1m'"),
            rejects("traffic capture"),
            ParserContractCase {
                sql: "traffic replay from '/tmp' user='root' password='123456' speed=1.0 read_only=true",
                ok: true,
                restored: "TRAFFIC REPLAY FROM '/tmp' USER = 'root' PASSWORD = '123456' SPEED = 1.0 READONLY = TRUE",
            },
            ParserContractCase {
                sql: "traffic replay from '/tmp' user 'root' password '123456' speed 1.0 read_only true",
                ok: true,
                restored: "TRAFFIC REPLAY FROM '/tmp' USER = 'root' PASSWORD = '123456' SPEED = 1.0 READONLY = TRUE",
            },
            ParserContractCase {
                sql: "traffic replay from '/tmp' speed 1.0 user='root'",
                ok: true,
                restored: "TRAFFIC REPLAY FROM '/tmp' SPEED = 1.0 USER = 'root'",
            },
            ParserContractCase {
                sql: "traffic replay from '/tmp' speed=1",
                ok: true,
                restored: "TRAFFIC REPLAY FROM '/tmp' SPEED = 1",
            },
            ParserContractCase {
                sql: "traffic replay from '/tmp' speed=0.5",
                ok: true,
                restored: "TRAFFIC REPLAY FROM '/tmp' SPEED = 0.5",
            },
            rejects("traffic replay from '/tmp' speed=-1"),
            rejects("traffic replay speed=1"),
            rejects("traffic replay"),
            ParserContractCase {
                sql: "show traffic jobs",
                ok: true,
                restored: "SHOW TRAFFIC JOBS",
            },
            rejects("show traffic jobs duration='1m'"),
            rejects("show traffic"),
            ParserContractCase {
                sql: "cancel traffic jobs",
                ok: true,
                restored: "CANCEL TRAFFIC JOBS",
            },
            rejects("cancel traffic jobs duration='1m'"),
            rejects("cancel traffic"),
            rejects("traffic test"),
            rejects("traffic"),
        ],
        "TestNonTransactionalDML" => &[accepts(
            "BATCH ON id LIMIT 100 INSERT INTO t2 SELECT * FROM t1",
        )],
        "TestIntervalPartition" => &[accepts(
            "CREATE TABLE t (dt DATETIME) PARTITION BY RANGE COLUMNS(dt) INTERVAL (1 DAY) FIRST PARTITION LESS THAN ('2024-01-01') LAST PARTITION LESS THAN ('2024-02-01')",
        )],
        "TestTTLTableOption" => &[accepts(
            "CREATE TABLE t (created_at TIMESTAMP) TTL = created_at + INTERVAL 1 DAY TTL_ENABLE = 'ON'",
        )],
        "TestCompatTypes" => &[accepts(
            "CREATE TABLE t (a BOOL, b MEDIUMINT, c LONGTEXT, d DOUBLE PRECISION)",
        )],
        "TestVector" => &[accepts("CREATE TABLE t (embedding VECTOR(3))")],
        "TestExplainExplore" => &[accepts("EXPLAIN EXPLORE REPLAYER '/tmp/replayer.zip'")],
        "TestCompatMariaDB" => &[accepts("SELECT 1")],
        "TestUUIDTypeMariaDBEnabled" | "TestUUIDTypeMariaDBDisabled" => &[accepts("SELECT 1")],
        "TestUUIDKeywordCompatibility" => &[accepts("SELECT uuid FROM t")],
        "TestSecondaryEngineAttribute" => &[accepts(
            "CREATE TABLE t (a INT) SECONDARY_ENGINE_ATTRIBUTE='engine=example'",
        )],
        "TestPartialIndex" => &[accepts("CREATE INDEX idx ON t(a) WHERE a > 0")],
        "TestTableAffinityOption" => &[accepts("CREATE TABLE t (a INT) AFFINITY='zone-a'")],
        "TestSplitPartition" => &[accepts(
            "ALTER TABLE t SPLIT MAXVALUE PARTITION LESS THAN (100)",
        )],
        other => panic!("missing Rust parser contract cases for {other}"),
    };
    cases.to_vec()
}

pub fn parser_contract_strings(name: &str, variable: &str) -> &'static [&'static str] {
    match (name, variable) {
        ("TestSimple", "reservedKws") => &["select", "table", "from"],
        ("TestSimple", "unreservedKws") => &["action", "admin", "value"],
        ("TestTableSample", "cases") => &[
            "SELECT * FROM t TABLESAMPLE REGIONS()",
            "SELECT * FROM t TABLESAMPLE REGIONS() REPEATABLE (42)",
        ],
        ("TestSignedInt64OutOfRange", "cases") => &[
            "recover table by job 18446744073709551612",
            "recover table t 18446744073709551612",
            "admin check index t idx (0, 18446744073709551612)",
            "create user abc@def with max_queries_per_hour 18446744073709551612",
        ],
        _ => panic!("missing Rust parser contract string list for {name}.{variable}"),
    }
}
