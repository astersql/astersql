// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 表达式集成测试 Part3：时间内置、行校验和与计划缓存。
//
// 覆盖 DATE/DATETIME/DURATION、TIMESTAMPDIFF/EXTRACT、
// `tidb_row_checksum`（按列字节 CRC32）、会话变量与返回类型深拷贝。

#![allow(non_snake_case)]

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};
use astersql_types::time::{
    BasicTimeContext, ExtractDatetimeNum, ExtractDurationNum, GetFsp, ParseDate, ParseDateFormat,
    ParseDatetime, ParseDuration, TimestampDiff,
};

fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk
}

#[test]
fn TestTimeBuiltin() {
    let mut tk = new_testkit();
    tk.MustQuery(
        "SELECT DATE('2019-09-12'), DATE('2019-09-12 12:12:09'), \
         DATE('2019-09-12 12:12:09.121212'), DATE('aa'), DATE(NULL)",
        Vec::new(),
    )
    .Check(Rows(&["2019-09-12 2019-09-12 2019-09-12 <nil> <nil>"]));
    tk.MustQuery(
        "SELECT DATE('0000-00-00'),DATE('0000-00-00 12:12:09'),\
         DATE('0000-00-00 00:00:00.121212'),DATE('0000-00-00 00:00:00.000000')",
        Vec::new(),
    )
    .Check(Rows(&["<nil> <nil> <nil> <nil>"]));
    tk.MustQuery(
        "SELECT YEAR('2013-01-09'), MONTH('2013-01-09'), \
         QUARTER('2012-08-24'), DAYOFMONTH('2017-08-12'), \
         DAYOFYEAR('2017-08-12'), WEEKDAY('2012-12-20'), \
         DAYOFWEEK('2012-12-20')",
        Vec::new(),
    )
    .Check(Rows(&["2013 1 3 12 224 3 5"]));
    tk.MustQuery(
        "SELECT TIME('2003-12-31 01:02:03'), TIME('01:02:03.000123'), \
         TIME('-838:59:59.000001'), TIME('-839:59:59'), TIME(NULL)",
        Vec::new(),
    )
    .Check(Rows(&[
        "01:02:03 01:02:03.000123 -838:59:59.000001 <nil> <nil>",
    ]));
    tk.MustQuery(
        "SELECT HOUR('12:13:14.123456'), HOUR('272:59:55'), \
         MINUTE('12:13:14.123456'), SECOND('12:13:14.123456'), \
         MICROSECOND('12:00:00.000010')",
        Vec::new(),
    )
    .Check(Rows(&["12 272 13 14 10"]));
    tk.MustQuery(
        "SELECT DATEDIFF('2007-12-31 23:59:59','2007-12-30'), \
         LAST_DAY('2003-02-05'), LAST_DAY('2004-02-05')",
        Vec::new(),
    )
    .Check(Rows(&["1 2003-02-28 2004-02-29"]));
    tk.MustQuery(
        "SELECT TIMESTAMPDIFF(MONTH,'2003-02-01','2003-05-01'), \
         TIMESTAMPDIFF(YEAR,'2002-05-01','2003-01-01'), \
         TIMESTAMPDIFF(SECOND,'2024-01-01 00:00:00','2024-01-02 01:02:03')",
        Vec::new(),
    )
    .Check(Rows(&["3 0 90123"]));
    tk.MustQuery(
        "SELECT DATE_FORMAT('2017-06-15','%W %M %e %Y %r %y')",
        Vec::new(),
    )
    .Check(vec![vec!["Thursday June 15 2017 12:00:00 AM 17"]]);
    tk.MustQuery(
        "SELECT PERIOD_ADD(200807,2), PERIOD_ADD(200807,-2), \
         PERIOD_DIFF(200807,200705), PERIOD_DIFF(200807,200908)",
        Vec::new(),
    )
    .Check(Rows(&["200809 200805 14 -13"]));
}

#[test]
/// 小数秒 fsp、DATE_FORMAT 与 ParseDateFormat 分片。
fn TestTimeFractionAndFormatting() {
    let context = BasicTimeContext::default();
    let time = ParseDatetime(&context, "2024-06-13 12:12:12.123456").unwrap();
    assert_eq!(GetFsp("12:12:12.123456"), 6);
    assert_eq!(time.Fsp(), 6);
    assert_eq!(
        time.DateFormat("%Y/%m/%d %H:%i:%s.%f").unwrap(),
        "2024/06/13 12:12:12.123456"
    );
    assert_eq!(ParseDateFormat("2024-06-13"), ["2024", "06", "13"]);
}

#[test]
/// DURATION 上限 838:59:59；超出范围拒绝。
fn TestDurationBuiltin() {
    let context = BasicTimeContext::default();
    let duration = ParseDuration(&context, "838:59:59.999999", 6).unwrap().0;
    assert_eq!(duration.String(), "838:59:59.999999");
    assert_eq!(ExtractDurationNum(&duration, "HOUR").unwrap(), 838);
    assert!(ParseDuration(&context, "839:00:00", 0).is_err());
}

fn calculateChecksum(columns: &[ChecksumValue<'_>]) -> u32 {
    let mut digest = crc32fast::Hasher::new();
    for value in columns {
        match value {
            ChecksumValue::Int(value) => digest.update(&value.to_le_bytes()),
            ChecksumValue::String(bytes) => {
                digest.update(&(bytes.len() as u32).to_le_bytes());
                digest.update(bytes);
            }
        }
    }
    digest.finalize()
}

enum ChecksumValue<'a> {
    Int(u64),
    String(&'a [u8]),
}

#[test]
fn TestTiDBRowChecksumBuiltin() {
    let mut tk = new_testkit();
    tk.MustExec("SET GLOBAL tidb_enable_row_level_checksum = 1", Vec::new());
    tk.MustExec("CREATE TABLE t (id INT PRIMARY KEY, c INT)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (1, 10)", Vec::new());
    tk.MustExec("ALTER TABLE t CHANGE COLUMN c c VARCHAR(10)", Vec::new());
    let checksum1 =
        calculateChecksum(&[ChecksumValue::Int(1), ChecksumValue::String(b"10")]).to_string();

    tk.MustExec("SET SESSION tidb_enable_row_level_checksum = 1", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (2, '20')", Vec::new());
    let checksum2 =
        calculateChecksum(&[ChecksumValue::Int(2), ChecksumValue::String(b"20")]).to_string();
    let checksum3 =
        calculateChecksum(&[ChecksumValue::Int(3), ChecksumValue::String(b"30")]).to_string();
    tk.MustExec("SET SESSION tidb_enable_row_level_checksum = 0", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (3, '30')", Vec::new());

    tk.MustQuery("SELECT TIDB_ROW_CHECKSUM() FROM t WHERE id = 1", Vec::new())
        .Check(Rows(&[&checksum1]));
    tk.MustQuery(
        "SELECT id, c, TIDB_ROW_CHECKSUM() FROM t WHERE id = 1",
        Vec::new(),
    )
    .Check(Rows(&[&format!("1 10 {checksum1}")]));
    tk.MustQuery(
        "SELECT id, TIDB_ROW_CHECKSUM(), c FROM t WHERE id = 2",
        Vec::new(),
    )
    .Check(Rows(&[&format!("2 {checksum2} 20")]));
    tk.MustQuery(
        "SELECT TIDB_ROW_CHECKSUM(), id, c FROM t WHERE id = 3",
        Vec::new(),
    )
    .Check(Rows(&[&format!("{checksum3} 3 30")]));
    tk.MustQuery(
        "SELECT TIDB_ROW_CHECKSUM() FROM t WHERE id IN (1, 2, 3)",
        Vec::new(),
    )
    .Check(Rows(&[&checksum1, &checksum2, &checksum3]));
    tk.MustQuery(
        "SELECT id, c, TIDB_ROW_CHECKSUM() FROM t WHERE id IN (1, 2, 3)",
        Vec::new(),
    )
    .Check(Rows(&[
        &format!("1 10 {checksum1}"),
        &format!("2 20 {checksum2}"),
        &format!("3 30 {checksum3}"),
    ]));

    for sql in [
        "SELECT LENGTH(TIDB_ROW_CHECKSUM()) FROM t WHERE id = 1",
        "SELECT c FROM t WHERE id = 1 AND TIDB_ROW_CHECKSUM() IS NOT NULL",
        "SELECT LENGTH(TIDB_ROW_CHECKSUM()) FROM t WHERE id IN (1, 2, 3)",
        "SELECT c FROM t WHERE id IN (1, 2, 3) AND TIDB_ROW_CHECKSUM() IS NOT NULL",
        "SELECT TIDB_ROW_CHECKSUM() FROM t",
        "SELECT TIDB_ROW_CHECKSUM() FROM t WHERE id > 0",
    ] {
        assert!(tk.Exec(sql, Vec::new()).is_err(), "sql={sql}");
    }
}

#[test]
fn TestTiDBRowChecksumBuiltinAfterDropColumn() {
    let mut tk = new_testkit();
    tk.MustExec("SET GLOBAL tidb_enable_row_level_checksum = 1", Vec::new());
    tk.MustExec(
        "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT)",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO t VALUES(1, 1, 1)", Vec::new());
    let old = tk
        .MustQuery("SELECT TIDB_ROW_CHECKSUM() FROM t WHERE a = 1", Vec::new())
        .Rows()[0][0]
        .clone();
    tk.MustExec("ALTER TABLE t DROP COLUMN b", Vec::new());
    let new = tk
        .MustQuery("SELECT TIDB_ROW_CHECKSUM() FROM t WHERE a = 1", Vec::new())
        .Rows()[0][0]
        .clone();
    assert_ne!(old, new);
}

#[test]
fn TestTiDBRowChecksumBuiltinAfterAddColumn() {
    let mut tk = new_testkit();
    tk.MustExec("SET GLOBAL tidb_enable_row_level_checksum = 1", Vec::new());
    tk.MustExec("CREATE TABLE t(a INT PRIMARY KEY, b INT)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES(1, 1)", Vec::new());
    let old = calculateChecksum(&[ChecksumValue::Int(1), ChecksumValue::Int(1)]).to_string();
    tk.MustQuery("SELECT TIDB_ROW_CHECKSUM() FROM t WHERE a = 1", Vec::new())
        .Check(Rows(&[&old]));
    tk.MustExec("ALTER TABLE t ADD COLUMN c INT DEFAULT 1", Vec::new());
    let new = calculateChecksum(&[
        ChecksumValue::Int(1),
        ChecksumValue::Int(1),
        ChecksumValue::Int(1),
    ])
    .to_string();
    tk.MustQuery("SELECT TIDB_ROW_CHECKSUM() FROM t WHERE a = 1", Vec::new())
        .Check(Rows(&[&new]));
    assert_ne!(old, new);
}

#[test]
fn TestSetVariables() {
    let mut tk = new_testkit();
    for sql in [
        "SET sql_mode='adfasdfadsfdasd'",
        "SET @@sql_mode='adfasdfadsfdasd'",
        "SET @@global.sql_mode='adfasdfadsfdasd'",
        "SET @@session.sql_mode='adfasdfadsfdasd'",
        "SET sql_mode=' ,'",
    ] {
        assert!(tk.Exec(sql, Vec::new()).is_err(), "sql={sql}");
    }
    tk.MustExec(
        "SET @@session.sql_mode=',NO_ZERO_DATE,ANSI,ANSI_QUOTES'",
        Vec::new(),
    );
    let sql_mode = "NO_ZERO_DATE,REAL_AS_FLOAT,PIPES_AS_CONCAT,ANSI_QUOTES,IGNORE_SPACE,ONLY_FULL_GROUP_BY,ANSI";
    tk.MustQuery("SELECT @@session.sql_mode", Vec::new())
        .Check(Rows(&[sql_mode]));
    tk.MustQuery("SHOW VARIABLES LIKE 'sql_mode'", Vec::new())
        .Check(Rows(&[&format!("sql_mode {sql_mode}")]));

    tk.MustExec("DROP TABLE IF EXISTS tab0", Vec::new());
    tk.MustExec("CREATE TABLE tab0(col1 TIME)", Vec::new());
    tk.MustExec("SET sql_mode='STRICT_TRANS_TABLES'", Vec::new());
    assert!(
        tk.Exec(
            "INSERT INTO tab0 SELECT CAST('999:44:33' AS TIME)",
            Vec::new(),
        )
        .is_err()
    );
    assert!(tk.Exec("SET sql_mode=' ,'", Vec::new()).is_err());
    assert!(
        tk.Exec(
            "INSERT INTO tab0 SELECT CAST('999:44:33' AS TIME)",
            Vec::new(),
        )
        .is_err()
    );

    tk.MustExec("SET SESSION TRANSACTION READ WRITE", Vec::new());
    tk.MustExec("SET GLOBAL TRANSACTION READ WRITE", Vec::new());
    tk.MustQuery(
        "SELECT @@session.tx_read_only, @@global.tx_read_only, \
         @@session.transaction_read_only, @@global.transaction_read_only",
        Vec::new(),
    )
    .Check(Rows(&["0 0 0 0"]));
    assert!(
        tk.Exec("SET SESSION TRANSACTION READ ONLY", Vec::new())
            .is_err()
    );
    assert!(tk.Exec("START TRANSACTION READ ONLY", Vec::new()).is_err());
    tk.MustExec("SET tidb_enable_noop_functions=1", Vec::new());
    tk.MustExec("SET SESSION TRANSACTION READ ONLY", Vec::new());
    tk.MustExec("START TRANSACTION READ ONLY", Vec::new());
    tk.MustQuery(
        "SELECT @@session.tx_read_only, @@global.tx_read_only, \
         @@session.transaction_read_only, @@global.transaction_read_only",
        Vec::new(),
    )
    .Check(Rows(&["1 0 1 0"]));
    assert!(
        tk.Exec("SET GLOBAL TRANSACTION READ ONLY", Vec::new())
            .is_err()
    );
    tk.MustExec("SET GLOBAL tidb_enable_noop_functions=1", Vec::new());
    tk.MustExec("SET GLOBAL TRANSACTION READ ONLY", Vec::new());
    tk.MustQuery(
        "SELECT @@session.tx_read_only, @@global.tx_read_only, \
         @@session.transaction_read_only, @@global.transaction_read_only",
        Vec::new(),
    )
    .Check(Rows(&["1 1 1 1"]));
    tk.MustExec("SET SESSION TRANSACTION READ WRITE", Vec::new());
    tk.MustExec("SET GLOBAL TRANSACTION READ WRITE", Vec::new());
    tk.MustQuery(
        "SELECT @@session.tx_read_only, @@global.tx_read_only, \
         @@session.transaction_read_only, @@global.transaction_read_only",
        Vec::new(),
    )
    .Check(Rows(&["0 0 0 0"]));
    tk.MustExec("SET tidb_enable_noop_functions=0", Vec::new());
    tk.MustExec("SET GLOBAL tidb_enable_noop_functions=1", Vec::new());

    assert!(
        tk.Exec("SET @@global.max_user_connections=''", Vec::new())
            .is_err()
    );
    assert!(
        tk.Exec("SET @@global.max_prepared_stmt_count=''", Vec::new())
            .is_err()
    );
    tk.MustQuery("SHOW VARIABLES LIKE 'max_connections'", Vec::new())
        .Check(Rows(&["max_connections 0"]));
    tk.MustExec("SET GLOBAL max_connections=1234", Vec::new());
    tk.MustQuery("SHOW VARIABLES LIKE 'max_connections'", Vec::new())
        .Check(Rows(&["max_connections 1234"]));
    tk.MustExec("SET GLOBAL max_connections=0", Vec::new());
}

#[test]
fn TestPreparePlanCacheOnCachedTable() {
    let mut tk = new_testkit();
    tk.MustExec("SET tidb_enable_prepared_plan_cache=ON", Vec::new());
    tk.MustExec("CREATE TABLE t(a INT)", Vec::new());
    tk.MustExec("ALTER TABLE t CACHE", Vec::new());
    for _ in 0..50 {
        tk.MustQuery("SELECT * FROM t WHERE a = 1", Vec::new())
            .Check(Vec::<Vec<&str>>::new());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    tk.MustExec(
        "PREPARE stmt FROM 'SELECT * FROM t WHERE a = ?'",
        Vec::new(),
    );
    tk.MustExec("SET @a = 1", Vec::new());
    tk.MustExec("EXECUTE stmt USING @a", Vec::new());
    tk.MustQuery("SELECT @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("EXECUTE stmt USING @a", Vec::new());
    tk.MustQuery("SELECT @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
}

#[test]
/// 非确定 RANDOM_BYTES 的 prepared plan 可缓存，但每次执行必须重新求值。
fn TestIssue16205() {
    let mut tk = new_testkit();
    tk.MustExec("SET tidb_enable_prepared_plan_cache=ON", Vec::new());
    tk.MustExec("PREPARE stmt FROM 'SELECT RANDOM_BYTES(3)'", Vec::new());
    let first = tk.MustQuery("EXECUTE stmt", Vec::new()).Rows();
    let second = tk.MustQuery("EXECUTE stmt", Vec::new()).Rows();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0][0], second[0][0]);
}

#[test]
/// EXPLAIN ANALYZE 的 Projection 内存单位保持 KB，不错误升级为 MB/GB。
fn TestIssue16697() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(v VARCHAR(1024))", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (SPACE(1024))", Vec::new());
    for _ in 0..5 {
        tk.MustExec("INSERT INTO t SELECT * FROM t", Vec::new());
    }
    for row in tk
        .MustQuery("EXPLAIN ANALYZE SELECT * FROM t", Vec::new())
        .Rows()
    {
        let line = row.join(" ");
        if line.contains("Projection") {
            assert!(line.contains("KB"), "{line}");
            assert!(!line.contains("MB"), "{line}");
            assert!(!line.contains("GB"), "{line}");
        }
    }
}

#[test]
fn TestIssue66661() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE table1 (active BIT)", Vec::new());
    tk.MustExec("INSERT INTO table1 VALUES (1)", Vec::new());
    for vectorized in ["ON", "OFF"] {
        tk.MustExec(
            &format!("SET @@tidb_enable_vectorized_expression={vectorized}"),
            Vec::new(),
        );
        tk.MustQuery(
            "SELECT HEX(CAST(ANY_VALUE(active) AS CHAR)) FROM table1",
            Vec::new(),
        )
        .Check(Rows(&["01"]));
        tk.MustQuery(
            "SELECT STR_TO_DATE(ANY_VALUE(active), '%h:%i:%s') FROM table1",
            Vec::new(),
        )
        .Check(Rows(&["<nil>"]));
        tk.MustQuery("SHOW WARNINGS", Vec::new())
            .CheckContain("Incorrect datetime value: '0000-00-00 00:00:00");
        tk.MustQuery(
            "SELECT MAX(active) AS c0 FROM table1 \
             WHERE STR_TO_DATE(ANY_VALUE(active), '%h:%i:%s')",
            Vec::new(),
        )
        .Check(Rows(&["<nil>"]));
        tk.MustQuery("SHOW WARNINGS", Vec::new())
            .CheckContain("Incorrect datetime value: '0000-00-00 00:00:00");
    }
}

#[test]
fn TestIssue43527() {
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE t (a DATETIME, b BIGINT, c DECIMAL(10, 2), d FLOAT)",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO t VALUES('2010-10-10 10:10:10', 100, 100.01, 100)",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT @total := @total + c FROM (SELECT c FROM t) AS temp, \
         (SELECT @total := 200) AS T1",
        Vec::new(),
    )
    .Check(Rows(&["300.01"]));
    tk.MustQuery(
        "SELECT @total := @total + d FROM (SELECT d FROM t) AS temp, \
         (SELECT @total := 200) AS T1",
        Vec::new(),
    )
    .Check(Rows(&["300"]));
    tk.MustExec(
        "INSERT INTO t VALUES('2010-10-10 10:10:10', 100, 100.01, 100)",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT @total := @total + d FROM (SELECT d FROM t) AS temp, \
         (SELECT @total := b FROM t) AS T1 WHERE @total >= 100",
        Vec::new(),
    )
    .Check(Rows(&["200", "300", "400", "500"]));
}

#[test]
/// 空聚合参与 ANY 子查询时保持三值逻辑，不错误返回源行。
fn TestIssue44706() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t0(c2 BIGINT)", Vec::new());
    tk.MustExec("INSERT INTO t0 VALUES (1)", Vec::new());
    tk.MustQuery("SELECT MIN(c2) FROM t0 WHERE FALSE", Vec::new())
        .Check(astersql_testkit::Rows(&["<nil>"]));
    tk.MustQuery(
        "SELECT c2 FROM t0 WHERE ((-1)<=(~('n')=ANY(SELECT NULL)))",
        Vec::new(),
    )
    .Check(Vec::<Vec<&str>>::new());
    tk.MustQuery(
        "SELECT c2 FROM t0 WHERE \
         ((-1)<=(~('n')=ANY(SELECT MIN(c2) FROM t0 WHERE FALSE)))",
        Vec::new(),
    )
    .Check(Vec::<Vec<&str>>::new());
}

#[test]
/// NULLIF 结果与各 CAST 类型做 NULL-safe 比较时不产生误匹配（Issue 51842/56744）。
fn TestIssue51842() {
    let mut tk = new_testkit();
    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
    tk.MustExec("CREATE TABLE t0(c0 DOUBLE)", Vec::new());
    tk.MustExec(
        "REPLACE INTO t0(c0) VALUES (0.40194983109852933)",
        Vec::new(),
    );
    tk.MustExec(
        "CREATE VIEW v0(c0) AS \
         SELECT CAST(')' AS TIME) FROM t0 WHERE '0.030417148673465677'",
        Vec::new(),
    );
    for right in [
        "1292367147",
        "CAST(123988.42132 AS REAL)",
        "CAST(123988.42132 AS DECIMAL)",
        "CAST('fdasge' AS CHAR)",
        "CAST('10:10:10' AS TIME)",
        "CAST(2024 AS YEAR)",
        "CAST('2024-1-1 10:10:10' AS DATETIME)",
    ] {
        tk.MustQuery(
            &format!(
                "SELECT f1 FROM (
                    SELECT NULLIF(v0.c0,1371581446) AS f1 FROM v0,t0
                 ) AS t WHERE f1<=>{right}"
            ),
            Vec::new(),
        )
        .Check(Vec::<Vec<&str>>::new());
    }

    tk.MustExec("DROP TABLE IF EXISTS lrr", Vec::new());
    tk.MustExec(
        "CREATE TABLE lrr(col1 TIME DEFAULT NULL,col2 TIME DEFAULT NULL)",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO lrr(col2) VALUES ('-229:53:34')", Vec::new());
    tk.MustQuery("SELECT * FROM lrr WHERE col1<=>NULL", Vec::new())
        .Check(Rows(&["<nil> -229:53:34"]));
    tk.MustQuery("SELECT * FROM lrr WHERE NULL<=>col1", Vec::new())
        .Check(Rows(&["<nil> -229:53:34"]));
}

#[test]
/// VAR_POP 窗口在唯一分区键、NULL/极值 DOUBLE 与 LIMIT 下可执行（Issue 55885）。
fn TestIssue55885() {
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE t_jg8o(
            c_s INT NOT NULL UNIQUE,
            c__qy DOUBLE,
            c_z INT NOT NULL,
            c_a90ol TEXT NOT NULL
        )",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO t_jg8o(c_s,c__qy,c_z,c_a90ol) VALUES
        (-975033779,85.65,-355481284,'gnip'),
        (-2018599732,85.86,1617093413,'m'),
        (-960107027,4.6,-2042358076,'y1q'),
        (-3,38.1,-1528586343,'ex_2'),
        (69386953,32768.0,-62220810,'tfkxjj5c'),
        (587181689,-9223372036854775806.3,-1666156943,'queemvgj'),
        (-218561796,85.2,-670390288,'nf990nol'),
        (858419954,2147483646.0,-1649362344,'won_9'),
        (-1120115215,22.100,1509989939,'w'),
        (-1388119356,94.32,-1694148464,'gu4i4knyhm'),
        (-1016230734,-4294967295.8,1430313391,'s'),
        (-1861825796,36.52,-1457928755,'j'),
        (1963621165,88.87,18928603,'gxbsloff'),
        (1492879828,CAST(NULL AS DOUBLE),759883041,'zwue'),
        (-1607192175,12.36,1669523024,'qt5zch71a'),
        (1534068569,46.79,-392085130,'bc'),
        (155707446,9223372036854775809.4,1727199557,'qyghenu9t6'),
        (-1524976778,75.99,335492222,'sdgde0z'),
        (175403335,CAST(NULL AS DOUBLE),-69711503,'ja'),
        (-272715456,48.62,753928713,'ur'),
        (-2035825967,257.3,-1598426762,'lmqmn'),
        (-1178957955,2147483648.100000,1432554380,'dqpb210'),
        (-2056628646,254.5,-1476177588,'k41ajpt7x'),
        (-914210874,126.7,-421919910,'x57ud7oy1'),
        (-88586773,1.2,1568247510,'drmxi8'),
        (-834563269,-4294967296.7,1163133933,'wp'),
        (-84490060,54.13,-630289437,'_3_twecg5h'),
        (267700893,54.75,370343042,'n72'),
        (552106333,32766.2,2365745,'s7tt'),
        (643440707,65536.8,-850412592,'wmluxa9a'),
        (1709853766,-4294967296.5,-21041749,'obqj0uu5v'),
        (-7,80.88,528792379,'n5qr9m26i'),
        (-456431629,28.43,1958788149,'b'),
        (-28841240,11.86,-1089765168,'pqg'),
        (-807839288,25.89,504535500,'cs3tkhs'),
        (-52910064,85.16,354032882,'_ffjo67yxe'),
        (1919869830,81.81,-272247558,'aj'),
        (165434725,-2147483648.0,11,'xxnsf5'),
        (3,-2147483648.7,1616632952,'g7t8tqyi'),
        (1851859144,70.73,-1105664209,'qjfhjr')",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT subq_0.c3 AS c1
         FROM (
             SELECT c_a90ol AS c3,
                    c_a90ol AS c4,
                    VAR_POP(CAST(c__qy AS DOUBLE))
                        OVER(PARTITION BY c_a90ol,c_s ORDER BY c_z) AS c5
             FROM t_jg8o LIMIT 65
         ) AS subq_0
         LIMIT 37",
        Vec::new(),
    );
}

#[test]
/// 空表上的嵌套外连接 CTE 与 EXISTS 不产生行（Issue 55886）。
fn TestIssue55886() {
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE t1(c_foveoe TEXT,c_jbb TEXT,c_cz TEXT NOT NULL)",
        Vec::new(),
    );
    tk.MustExec("CREATE TABLE t2(c_g7eofzlxn INT)", Vec::new());
    tk.MustExec("SET collation_connection='latin1_bin'", Vec::new());
    tk.MustQuery(
        "WITH cte_0 AS (
            SELECT 1 AS c1,
                   CASE WHEN ref_0.c_jbb
                        THEN INET6_ATON(ref_0.c_foveoe)
                        ELSE ref_4.c_cz END AS c5
            FROM t1 AS ref_0
            JOIN (t1 AS ref_4 RIGHT OUTER JOIN t2 AS ref_5
                  ON ref_5.c_g7eofzlxn!=1)
         ),
         cte_4 AS (SELECT 1 AS c1 FROM t2)
         SELECT ref_34.c1 AS c5
         FROM cte_0 AS ref_34
         WHERE EXISTS (
             SELECT 1 FROM cte_4 AS ref_35
             WHERE ref_34.c1<=CASE WHEN ref_34.c5
                                   THEN CAST(1 AS CHAR)
                                   ELSE ref_34.c5 END
         )",
        Vec::new(),
    )
    .Check(Vec::<Vec<&str>>::new());
}

#[test]
/// 视图自连接中的除零、TRUNCATE 与 BETWEEN NULL 返回类型可重复执行（Issue 57608）。
fn TestIssue57608() {
    let mut tk = new_testkit();
    tk.MustExec("DROP TABLE IF EXISTS t1", Vec::new());
    tk.MustExec("CREATE TABLE t1(c1 INT PRIMARY KEY)", Vec::new());
    tk.MustExec(
        "INSERT INTO t1(c1) VALUES
         (1),(2),(3),(4),(5),(6),(7),(11),(12),(13),(14),(15),(16),(17),
         (21),(22),(23),(24),(25),(26),(27),(116),(127),(121),(122),(113),
         (214),(251),(261),(217),(91),(92),(39),(94),(95),(69),(79),(191),(129)",
        Vec::new(),
    );
    tk.MustExec("CREATE VIEW v2 AS SELECT 0 AS q2 FROM t1", Vec::new());
    for _ in 0..10 {
        tk.MustQuery(
            "SELECT DISTINCT
                    1 BETWEEN NULL AND 1 AS w0,
                    TRUNCATE(1,(CAST(ref_1.q2 AS UNSIGNED)%0)) AS w1,
                    1 BETWEEN TRUNCATE(1,(CAST(ref_1.q2 AS UNSIGNED)%0)) AND 1 AS w2
             FROM v2 AS ref_0 INNER JOIN v2 AS ref_1 ON (1=1)",
            Vec::new(),
        )
        .Check(Rows(&["<nil> <nil> <nil>"]));
    }
}

#[test]
/// 视图 DECIMAL 返回类型参与 LIKE/CAST/ATAN2 时可独立修改（Issue 57608）。
fn TestDeepCopyRetType() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t0(c0 INT)", Vec::new());
    tk.MustExec("CREATE TABLE t1(c0 DECIMAL)", Vec::new());
    tk.MustExec("INSERT INTO t1(c0) VALUES (1687)", Vec::new());
    tk.MustExec("INSERT INTO t0(c0) VALUES (0)", Vec::new());
    tk.MustExec(
        "CREATE VIEW v0(c0) AS \
         SELECT CAST((t1.c0 DIV t1.c0) AS DECIMAL) FROM t1",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT * FROM v0 INNER JOIN t0
         ON (v0.c0 LIKE CAST(v0.c0 AS CHAR)<=t0.c0)
            AND (NOT ATAN2(t0.c0,v0.c0))",
        Vec::new(),
    )
    .Check(Vec::<Vec<&str>>::new());
}
