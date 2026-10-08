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

// 表达式集成测试 Part1：VECTOR（向量浮点）类型与距离函数。
//
// 对应 Go `integration_test` 中向量相关用例：维度校验、算术、比较、
// 序列化、聚合与 L1/L2/余弦距离等。VECTOR 为 TiDB 扩展类型，用于近似最近邻检索。

#![allow(non_snake_case)]

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};
use astersql_types::vector::{CheckVectorDimValid, ParseVectorFloat32};

/// 解析向量字面量文本（失败则 panic，供测试断言）。
fn vector(text: &str) -> astersql_types::vector::VectorFloat32 {
    ParseVectorFloat32(text).unwrap()
}

/// 创建绑定真实 ConcreteSession/KV 的 TestKit，并选择 Go 用例使用的 test 库。
fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk
}

#[test]
/// 16383 维 VECTOR 在无索引、建表索引、ALTER 索引三条路径完成 DML/搜索。
fn TestVectorLong() {
    fn gen_vec(dimensions: usize, start: i32) -> String {
        let values = (0..dimensions)
            .map(|offset| (start + offset as i32 * 100).to_string())
            .collect::<Vec<_>>();
        format!("[{}]", values.join(","))
    }
    fn run_workload(tk: &mut TestKit) {
        let vec100 = gen_vec(16_383, 100);
        let vec200 = gen_vec(16_383, 200);
        let vec300 = gen_vec(16_383, 300);
        let vec500 = gen_vec(16_383, 500);
        tk.MustExec(&format!("INSERT INTO t1 VALUES (1,'{vec100}')"), Vec::new());
        tk.MustQuery("SELECT * FROM t1 ORDER BY id", Vec::new())
            .Check(Rows(&[&format!("1 {vec100}")]));
        tk.MustExec(
            &format!("DELETE FROM t1 WHERE vec>'{}'", gen_vec(16_383, 200)),
            Vec::new(),
        );
        tk.MustQuery("SELECT * FROM t1 ORDER BY id", Vec::new())
            .Check(Rows(&[&format!("1 {vec100}")]));
        tk.MustExec(
            &format!("DELETE FROM t1 WHERE vec>'{}'", gen_vec(16_383, 50)),
            Vec::new(),
        );
        tk.MustQuery("SELECT * FROM t1 ORDER BY id", Vec::new())
            .Check(Vec::<Vec<&str>>::new());
        for (id, value) in [(1, &vec100), (2, &vec200), (3, &vec300)] {
            tk.MustExec(
                &format!("INSERT INTO t1 VALUES ({id},'{value}')"),
                Vec::new(),
            );
        }
        tk.MustQuery(
            &format!(
                "SELECT id FROM t1 ORDER BY VEC_L2_DISTANCE(vec,'{}') LIMIT 2",
                gen_vec(16_383, 180)
            ),
            Vec::new(),
        )
        .Check(Rows(&["2", "1"]));
        tk.MustExec(
            &format!("UPDATE t1 SET vec='{vec500}' WHERE id=1"),
            Vec::new(),
        );
        tk.MustQuery("SELECT * FROM t1 ORDER BY id", Vec::new())
            .Check(Rows(&[
                &format!("1 {vec500}"),
                &format!("2 {vec200}"),
                &format!("3 {vec300}"),
            ]));
        tk.MustQuery(
            &format!(
                "SELECT id FROM t1 ORDER BY VEC_L2_DISTANCE(vec,'{}') LIMIT 2",
                gen_vec(16_383, 180)
            ),
            Vec::new(),
        )
        .Check(Rows(&["2", "3"]));
    }

    assert!(CheckVectorDimValid(16_383).is_ok());
    assert!(CheckVectorDimValid(16_384).is_err());
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE t1(id INT PRIMARY KEY,vec VECTOR(16383))",
        Vec::new(),
    );
    run_workload(&mut tk);
    tk.MustExec("DROP TABLE t1", Vec::new());

    tk.MustExec(
        "CREATE TABLE t1(id INT PRIMARY KEY,vec VECTOR(16383),\
         VECTOR INDEX((VEC_COSINE_DISTANCE(vec))))",
        Vec::new(),
    );
    run_workload(&mut tk);
    tk.MustExec("DROP TABLE t1", Vec::new());

    tk.MustExec(
        "CREATE TABLE t1(id INT PRIMARY KEY,vec VECTOR(16383))",
        Vec::new(),
    );
    tk.MustExec("ALTER TABLE t1 SET TIFLASH REPLICA 1", Vec::new());
    tk.MustExec(
        "ALTER TABLE t1 ADD VECTOR INDEX((VEC_COSINE_DISTANCE(vec)))",
        Vec::new(),
    );
    run_workload(&mut tk);
    tk.MustExec("DROP TABLE t1", Vec::new());
}

#[test]
/// VECTOR 在严格/非严格模式下的 NULL、DEFAULT、NOT NULL 与维度约束矩阵。
fn TestVectorDefaultValue() {
    let mut tk = new_testkit();

    for mode in ["", "STRICT_ALL_TABLES"] {
        tk.MustExec(&format!("SET @@SESSION.sql_mode='{mode}'"), Vec::new());
        tk.MustExec("CREATE TABLE t(embedding VECTOR)", Vec::new());
        for value in ["'[1,2,3]'", "'[4]'", "DEFAULT", "NULL"] {
            tk.MustExec(&format!("INSERT INTO t VALUES ({value})"), Vec::new());
        }
        tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
            .Check(Rows(&["[1,2,3] 3", "[4] 1", "<nil> <nil>", "<nil> <nil>"]));
        tk.MustExec("DROP TABLE t", Vec::new());

        tk.MustExec("CREATE TABLE t(embedding VECTOR(3))", Vec::new());
        tk.MustExec("INSERT INTO t VALUES ('[1,2,3]')", Vec::new());
        let _ = tk.ExecToErr("INSERT INTO t VALUES ('[4]')");
        tk.MustExec("INSERT INTO t VALUES (DEFAULT)", Vec::new());
        tk.MustExec("INSERT INTO t VALUES (NULL)", Vec::new());
        tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
            .Check(Rows(&["[1,2,3] 3", "<nil> <nil>", "<nil> <nil>"]));
        tk.MustExec("DROP TABLE t", Vec::new());
    }

    tk.MustContainErrMsg(
        "CREATE TABLE t(embedding VECTOR DEFAULT '[1,2,3]')",
        "VECTOR column 'embedding' can't have a literal default. \
         Use expression default instead: ((VEC_FROM_TEXT('...')))",
    );
    tk.MustExec(
        "CREATE TABLE t(embedding VECTOR DEFAULT (VEC_FROM_TEXT('[1,2,3]')))",
        Vec::new(),
    );
    for values in ["", "'[4]'", "DEFAULT", "NULL", "DEFAULT(embedding)"] {
        tk.MustExec(&format!("INSERT INTO t VALUES ({values})"), Vec::new());
    }
    tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
        .Check(Rows(&[
            "[1,2,3] 3",
            "[4] 1",
            "[1,2,3] 3",
            "<nil> <nil>",
            "[1,2,3] 3",
        ]));
    tk.MustExec("DROP TABLE t", Vec::new());

    tk.MustExec(
        "CREATE TABLE t(embedding VECTOR(5) DEFAULT (VEC_FROM_TEXT('[1,2,3]')))",
        Vec::new(),
    );
    for sql in ["INSERT INTO t VALUES ()", "INSERT INTO t VALUES (DEFAULT)"] {
        assert_eq!(
            tk.ExecToErr(sql).message(),
            "vector has 3 dimensions, does not fit VECTOR(5)"
        );
    }
    tk.MustExec("INSERT INTO t VALUES ('[1,2,3,4,5]')", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (NULL)", Vec::new());
    tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
        .Check(Rows(&["[1,2,3,4,5] 5", "<nil> <nil>"]));
    tk.MustExec("DROP TABLE t", Vec::new());

    tk.MustExec(
        "CREATE TABLE t(embedding VECTOR(5) DEFAULT (UUID()))",
        Vec::new(),
    );
    tk.MustContainErrMsg("INSERT INTO t VALUES ()", "Invalid vector text:");
    tk.MustExec("INSERT INTO t VALUES ('[1,2,3,4,5]')", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (NULL)", Vec::new());
    tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
        .Check(Rows(&["[1,2,3,4,5] 5", "<nil> <nil>"]));
    tk.MustExec("DROP TABLE t", Vec::new());

    tk.MustExec("SET @@SESSION.sql_mode=''", Vec::new());
    tk.MustExec("CREATE TABLE t(embedding VECTOR NOT NULL)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES ('[1,2,3]')", Vec::new());
    tk.MustExec("INSERT INTO t VALUES ('[4]')", Vec::new());
    tk.MustExec("INSERT INTO t VALUES (DEFAULT)", Vec::new());
    let _ = tk.ExecToErr("INSERT INTO t VALUES (NULL)");
    tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
        .Check(Rows(&["[1,2,3] 3", "[4] 1", "[] 0"]));
    tk.MustExec("DROP TABLE t", Vec::new());

    for mode in ["", "STRICT_ALL_TABLES"] {
        tk.MustExec(&format!("SET @@SESSION.sql_mode='{mode}'"), Vec::new());
        tk.MustExec("CREATE TABLE t(embedding VECTOR(3) NOT NULL)", Vec::new());
        tk.MustExec("INSERT INTO t VALUES ('[1,2,3]')", Vec::new());
        for value in ["'[4]'", "DEFAULT", "NULL"] {
            let _ = tk.ExecToErr(&format!("INSERT INTO t VALUES ({value})"));
        }
        tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
            .Check(Rows(&["[1,2,3] 3"]));
        tk.MustExec("DROP TABLE t", Vec::new());
    }

    tk.MustExec("CREATE TABLE t(embedding VECTOR NOT NULL)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES ('[1,2,3]')", Vec::new());
    tk.MustExec("INSERT INTO t VALUES ('[4]')", Vec::new());
    let _ = tk.ExecToErr("INSERT INTO t VALUES (DEFAULT)");
    let _ = tk.ExecToErr("INSERT INTO t VALUES (NULL)");
    tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
        .Check(Rows(&["[1,2,3] 3", "[4] 1"]));
    tk.MustExec("DROP TABLE t", Vec::new());

    for mode in ["", "STRICT_ALL_TABLES"] {
        tk.MustExec(&format!("SET @@SESSION.sql_mode='{mode}'"), Vec::new());
        tk.MustExec(
            "CREATE TABLE t(embedding VECTOR NOT NULL \
             DEFAULT (VEC_FROM_TEXT('[1,2,3]')))",
            Vec::new(),
        );
        for values in ["", "'[4]'", "DEFAULT"] {
            tk.MustExec(&format!("INSERT INTO t VALUES ({values})"), Vec::new());
        }
        let _ = tk.ExecToErr("INSERT INTO t VALUES (NULL)");
        tk.MustExec("INSERT INTO t VALUES (DEFAULT(embedding))", Vec::new());
        tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
            .Check(Rows(&["[1,2,3] 3", "[4] 1", "[1,2,3] 3", "[1,2,3] 3"]));
        tk.MustExec("DROP TABLE t", Vec::new());

        tk.MustExec(
            "CREATE TABLE t(embedding VECTOR(1) NOT NULL \
             DEFAULT (VEC_FROM_TEXT('[1,2,3]')))",
            Vec::new(),
        );
        let _ = tk.ExecToErr("INSERT INTO t VALUES ()");
        tk.MustExec("INSERT INTO t VALUES ('[4]')", Vec::new());
        for value in ["DEFAULT", "NULL", "DEFAULT(embedding)"] {
            let _ = tk.ExecToErr(&format!("INSERT INTO t VALUES ({value})"));
        }
        tk.MustQuery("SELECT *,VEC_DIMS(embedding) FROM t", Vec::new())
            .Check(Rows(&["[4] 1"]));
        tk.MustExec("DROP TABLE t", Vec::new());
    }
}

#[test]
/// VECTOR 列在 SHOW CREATE/COLUMNS/INFORMATION_SCHEMA 中保留类型与维度。
fn TestVectorColumnInfo() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(embedding VECTOR)", Vec::new());
    tk.MustQuery("SHOW CREATE TABLE t", Vec::new())
        .Check(vec![vec![
            "t",
            "CREATE TABLE `t` (\n  `embedding` vector DEFAULT NULL\n) \
         ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
        ]]);
    tk.MustQuery("SHOW COLUMNS FROM t", Vec::new())
        .Check(Rows(&["embedding vector YES  <nil> "]));

    for dimension in [3, 3, 0] {
        tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
        tk.MustExec(
            &format!("CREATE TABLE t(embedding VECTOR({dimension}))"),
            Vec::new(),
        );
    }
    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
    tk.MustExec("CREATE TABLE t(embedding VECTOR(3))", Vec::new());
    tk.MustQuery("SHOW CREATE TABLE t", Vec::new())
        .Check(vec![vec![
            "t",
            "CREATE TABLE `t` (\n  `embedding` vector(3) DEFAULT NULL\n) \
         ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
        ]]);
    tk.MustQuery("SHOW COLUMNS FROM t", Vec::new())
        .Check(Rows(&["embedding vector(3) YES  <nil> "]));
    tk.MustQuery(
        "SELECT data_type,column_type FROM INFORMATION_SCHEMA.COLUMNS \
         WHERE table_name='t'",
        Vec::new(),
    )
    .Check(Rows(&["vector vector(3)"]));
    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
    assert_eq!(
        tk.ExecToErr("CREATE TABLE t(embedding VECTOR(16384))")
            .message(),
        "vector cannot have more than 16383 dimensions"
    );
}

#[test]
/// EXPLAIN brief 对向量常量使用科学计数并限制展示长度。
fn TestVectorExplainTruncate() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(c VECTOR)", Vec::new());
    tk.MustQuery(
        "EXPLAIN FORMAT='brief' SELECT
         VEC_COSINE_DISTANCE(c,'[3,100,12345,10000]'),
         VEC_COSINE_DISTANCE(c,'[11111111111,11111111111.23456789,3.1,5.12456]'),
         VEC_COSINE_DISTANCE(c,'[-11111111111,-11111111111.23456789,-3.1,-5.12456]')
         FROM t",
        Vec::new(),
    )
    .Check(vec![
        vec![
            "Projection",
            "10000.00",
            "root",
            "",
            "vec_cosine_distance(test.t.c, [3,1e+02,1.2e+04,1e+04])->Column#4, \
             vec_cosine_distance(test.t.c, [1.1e+10,1.1e+10,3.1,5.1])->Column#5, \
             vec_cosine_distance(test.t.c, [-1.1e+10,-1.1e+10,-3.1,-5.1])->Column#6",
        ],
        vec![
            "└─TableReader",
            "10000.00",
            "root",
            "",
            "data:TableFullScan",
        ],
        vec![
            "  └─TableFullScan",
            "10000.00",
            "cop[tikv]",
            "table:t",
            "keep order:false, stats:pseudo",
        ],
    ]);
}

#[test]
/// 常量 VECTOR 在普通、构造函数、TopN 与预处理计划中均截断 EXPLAIN 文本。
fn TestVectorConstantExplain() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(c VECTOR)", Vec::new());
    let projection = "vec_cosine_distance(test.t.c, [1,2,3,4,5,(6 more)...])->Column#4";
    for sql in [
        "EXPLAIN FORMAT='brief' SELECT \
         VEC_COSINE_DISTANCE(c,'[1,2,3,4,5,6,7,8,9,10,11]') FROM t",
        "EXPLAIN FORMAT='brief' SELECT \
         VEC_COSINE_DISTANCE(c,VEC_FROM_TEXT('[1,2,3,4,5,6,7,8,9,10,11]')) FROM t",
    ] {
        tk.MustQuery(sql, Vec::new()).Check(vec![
            vec!["Projection", "10000.00", "root", "", projection],
            vec![
                "└─TableReader",
                "10000.00",
                "root",
                "",
                "data:TableFullScan",
            ],
            vec![
                "  └─TableFullScan",
                "10000.00",
                "cop[tikv]",
                "table:t",
                "keep order:false, stats:pseudo",
            ],
        ]);
    }
    tk.MustQuery(
        "EXPLAIN FORMAT='brief' SELECT \
         VEC_COSINE_DISTANCE(c,'[1,2,3,4,5,6,7,8,9,10,11]') AS d \
         FROM t ORDER BY d LIMIT 10",
        Vec::new(),
    )
    .Check(vec![
        vec!["Projection", "10.00", "root", "", projection],
        vec![
            "└─TopN",
            "10.00",
            "root",
            "",
            "Column#5, offset:0, count:10",
        ],
        vec!["  └─TableReader", "10.00", "root", "", "data:TopN"],
        vec![
            "    └─TopN",
            "10.00",
            "cop[tikv]",
            "",
            "Column#5, offset:0, count:10",
        ],
        vec![
            "      └─Projection",
            "10.00",
            "cop[tikv]",
            "",
            "test.t.c, vec_cosine_distance(test.t.c, \
             [1,2,3,4,5,(6 more)...])->Column#5",
        ],
        vec![
            "        └─TableFullScan",
            "10000.00",
            "cop[tikv]",
            "table:t",
            "keep order:false, stats:pseudo",
        ],
    ]);

    let large = format!("[{}]", vec!["100"; 100].join(","));
    tk.MustExec(
        "PREPARE vector_explain FROM \
         'EXPLAIN FORMAT=\"brief\" SELECT VEC_COSINE_DISTANCE(c,?) FROM t'",
        Vec::new(),
    );
    tk.MustExec(&format!("SET @vector='{large}'"), Vec::new());
    let rows = tk
        .MustQuery("EXECUTE vector_explain USING @vector", Vec::new())
        .Rows();
    assert!(
        rows.iter()
            .flatten()
            .any(|cell| cell.contains("[1e+02,1e+02,1e+02,1e+02,1e+02,(95 more)...]")),
        "{rows:?}"
    );
}

#[test]
/// TiFlash HNSW VECTOR INDEX 的 ANN TopN 计划保持距离、维度截断与 limit。
fn TestVectorIndexExplain() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("DROP TABLE IF EXISTS t1", Vec::new());
    tk.MustExec("CREATE TABLE t1(vec VECTOR(100))", Vec::new());
    tk.MustExec("ALTER TABLE t1 SET TIFLASH REPLICA 1", Vec::new());
    tk.MustExec(
        "ALTER TABLE t1 ADD VECTOR INDEX((VEC_COSINE_DISTANCE(vec))) USING HNSW",
        Vec::new(),
    );
    domain
        .set_tiflash_replica_for_test("test", "t1", 1, true)
        .expect("mark test.t1 TiFlash replica available");
    let vector = format!("[{}]", vec!["100"; 100].join(","));
    tk.MustQuery(
        &format!(
            "EXPLAIN FORMAT='brief' SELECT * FROM t1 \
             ORDER BY VEC_COSINE_DISTANCE(vec,'{vector}') LIMIT 1"
        ),
        Vec::new(),
    )
    .Check(vec![
        vec!["TopN", "1.00", "root", "", "Column#6, offset:0, count:1"],
        vec![
            "└─TableReader",
            "1.00",
            "root",
            "",
            "MppVersion: 3, data:ExchangeSender",
        ],
        vec![
            "  └─ExchangeSender",
            "1.00",
            "mpp[tiflash]",
            "",
            "ExchangeType: PassThrough",
        ],
        vec![
            "    └─TopN",
            "1.00",
            "mpp[tiflash]",
            "",
            "Column#6, offset:0, count:1",
        ],
        vec![
            "      └─Projection",
            "1.00",
            "mpp[tiflash]",
            "",
            "test.t1.vec, vec_cosine_distance(test.t1.vec, \
             [1e+02,1e+02,1e+02,1e+02,1e+02,(95 more)...])->Column#6",
        ],
        vec![
            "        └─TableFullScan",
            "1.00",
            "mpp[tiflash]",
            "table:t1, index:vector_index(vec)",
            "keep order:false, stats:pseudo, \
             annIndex:COSINE(vec..[1e+02,1e+02,1e+02,1e+02,1e+02,(95 more)...], limit:1)",
        ],
    ]);
}

#[test]
/// VECTOR 混合维度与固定维度的 ALTER/DML 约束走真实 SQL。
fn TestFixedVector() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(embedding VECTOR)", Vec::new());
    tk.MustExec("INSERT INTO t VALUES ('[1,2,3]'),('[1,2,3,4]')", Vec::new());
    assert!(
        tk.Exec(
            "ALTER TABLE t MODIFY COLUMN embedding VECTOR(3)",
            Vec::new()
        )
        .unwrap_err()
        .message()
        .contains("vector has 4 dimensions, does not fit VECTOR(3)")
    );
    tk.MustExec("DELETE FROM t WHERE VEC_DIMS(embedding) != 3", Vec::new());
    tk.MustExec(
        "ALTER TABLE t MODIFY COLUMN embedding VECTOR(3)",
        Vec::new(),
    );
    for (sql, message) in [
        (
            "INSERT INTO t VALUES ('[]')",
            "vector has 0 dimensions, does not fit VECTOR(3)",
        ),
        (
            "INSERT INTO t VALUES ('[1,2,3,4]')",
            "vector has 4 dimensions, does not fit VECTOR(3)",
        ),
        (
            "INSERT INTO t VALUES (VEC_FROM_TEXT('[]'))",
            "vector has 0 dimensions, does not fit VECTOR(3)",
        ),
        (
            "INSERT INTO t VALUES (VEC_FROM_TEXT('[1,2,3,4]'))",
            "vector has 4 dimensions, does not fit VECTOR(3)",
        ),
        (
            "UPDATE t SET embedding='[1,2,3,4]' WHERE embedding='[1,2,3]'",
            "vector has 4 dimensions, does not fit VECTOR(3)",
        ),
        (
            "UPDATE t SET embedding='[]' WHERE embedding='[1,2,3]'",
            "vector has 0 dimensions, does not fit VECTOR(3)",
        ),
    ] {
        assert_eq!(tk.ExecToErr(sql).message(), message, "sql={sql}");
    }
    tk.MustExec("ALTER TABLE t MODIFY COLUMN embedding VECTOR", Vec::new());
    tk.MustExec("INSERT INTO t VALUES ('[1,2,3,4]')", Vec::new());
    assert_eq!(
        tk.ExecToErr("ALTER TABLE t MODIFY COLUMN embedding VECTOR(16384)")
            .message(),
        "vector cannot have more than 16383 dimensions"
    );
}

#[test]
/// VECTOR DDL/DML、维度函数、排序和余弦距离均走真实 SQL 会话。
fn TestVector() {
    let mut tk = new_testkit();

    tk.MustExec("CREATE TABLE t1 (v VECTOR)", Vec::new());
    for invalid in ["abc", "[1,2.1,null]", "[1,2.1,inf]", "[1,2.1,nan]"] {
        assert!(
            tk.Exec(&format!("INSERT INTO t1 VALUES ('{invalid}')"), Vec::new())
                .is_err(),
            "invalid vector unexpectedly inserted: {invalid}"
        );
    }
    tk.MustExec("INSERT INTO t1 VALUES ('[1,2.1,3.3]')", Vec::new());
    tk.MustExec("INSERT INTO t1 VALUES ('[]')", Vec::new());
    tk.MustExec("INSERT INTO t1 VALUES (NULL)", Vec::new());
    tk.MustQuery("SELECT * FROM t1", Vec::new())
        .Check(Rows(&["[1,2.1,3.3]", "[]", "<nil>"]));
    tk.MustQuery("SELECT VEC_DIMS(v) FROM t1", Vec::new())
        .Check(Rows(&["3", "0", "<nil>"]));
    tk.MustQuery("SELECT VEC_DIMS(NULL)", Vec::new())
        .Check(Rows(&["<nil>"]));
    tk.MustQuery("SELECT VEC_DIMS('[]')", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("SELECT VEC_DIMS('[5, 3, 2]')", Vec::new())
        .Check(Rows(&["3"]));
    tk.MustQuery("SELECT VEC_FROM_TEXT('[]')", Vec::new())
        .Check(Rows(&["[]"]));

    tk.MustExec("CREATE TABLE t(val VECTOR)", Vec::new());
    tk.MustExec(
        "INSERT INTO t VALUES \
         ('[8.7, 5.7, 7.7, 9.8, 1.5]'),\
         ('[3.6, 9.7, 2.4, 6.6, 4.9]'),\
         ('[4.7, 4.9, 2.6, 5.2, 7.4]'),\
         ('[7.7, 6.7, 8.3, 7.8, 5.7]'),\
         ('[1.4, 4.5, 8.5, 7.7, 6.2]')",
        Vec::new(),
    );
    tk.MustQuery("SELECT * FROM t ORDER BY val DESC", Vec::new())
        .Check(Rows(&[
            "[8.7,5.7,7.7,9.8,1.5]",
            "[7.7,6.7,8.3,7.8,5.7]",
            "[4.7,4.9,2.6,5.2,7.4]",
            "[3.6,9.7,2.4,6.6,4.9]",
            "[1.4,4.5,8.5,7.7,6.2]",
        ]));
    tk.MustQuery(
        "SELECT val, ROUND(VEC_COSINE_DISTANCE(val, '[1,2,3,4,5]'), 5) AS d \
         FROM t ORDER BY d DESC",
        Vec::new(),
    )
    .Check(Rows(&[
        "[8.7,5.7,7.7,9.8,1.5] 0.25641",
        "[3.6,9.7,2.4,6.6,4.9] 0.18577",
        "[7.7,6.7,8.3,7.8,5.7] 0.12677",
        "[4.7,4.9,2.6,5.2,7.4] 0.06925",
        "[1.4,4.5,8.5,7.7,6.2] 0.04973",
    ]));
}

#[test]
/// VECTOR 的布尔/NULL 谓词及 BETWEEN/IN 比较走真实 SQL。
fn TestVectorOperators() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(embedding VECTOR)", Vec::new());
    tk.MustExec(
        "INSERT INTO t VALUES ('[1,2,3]'),('[4,5,6]'),('[7,8,9]')",
        Vec::new(),
    );
    for (sql, expected) in [
        ("SELECT VEC_FROM_TEXT('[]') IS TRUE", "0"),
        ("SELECT VEC_FROM_TEXT('[]') IS FALSE", "1"),
        ("SELECT VEC_FROM_TEXT('[]') IS UNKNOWN", "0"),
        ("SELECT VEC_FROM_TEXT('[]') IS NOT NULL", "1"),
        ("SELECT VEC_FROM_TEXT('[]') IS NULL", "0"),
    ] {
        tk.MustQuery(sql, Vec::new()).Check(Rows(&[expected]));
    }
    tk.MustQuery(
        "SELECT * FROM t WHERE embedding = VEC_FROM_TEXT('[1,2,3]')",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]"]));
    tk.MustQuery(
        "SELECT * FROM t WHERE embedding BETWEEN '[1,2,3]' AND '[4,5,6]'",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]", "[4,5,6]"]));
    tk.MustQuery(
        "SELECT * FROM t WHERE embedding IN ('[1,2,3]','[4,5,6]')",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]", "[4,5,6]"]));
    tk.MustQuery(
        "SELECT * FROM t WHERE embedding NOT IN ('[1,2,3]','[4,5,6]')",
        Vec::new(),
    )
    .Check(Rows(&["[7,8,9]"]));
}

#[test]
/// VECTOR 比较、GREATEST/LEAST 和 COALESCE 走真实 SQL。
fn TestVectorCompare() {
    let tk = new_testkit();
    for (operator, expected) in [
        ("=", "1"),
        ("!=", "0"),
        (">", "0"),
        (">=", "1"),
        ("<", "0"),
        ("<=", "1"),
    ] {
        tk.MustQuery(
            &format!("SELECT VEC_FROM_TEXT('[]') {operator} VEC_FROM_TEXT('[]')"),
            Vec::new(),
        )
        .Check(Rows(&[expected]));
    }
    for (operator, expected) in [("=", "1"), ("!=", "0")] {
        tk.MustQuery(
            &format!("SELECT VEC_FROM_TEXT('[1,2,3]') {operator} VEC_FROM_TEXT('[1,2,3]')"),
            Vec::new(),
        )
        .Check(Rows(&[expected]));
    }
    for (operator, expected) in [(">", "1"), (">=", "1"), ("<", "0"), ("<=", "0")] {
        tk.MustQuery(
            &format!("SELECT VEC_FROM_TEXT('[1,2,3]') {operator} VEC_FROM_TEXT('[1]')"),
            Vec::new(),
        )
        .Check(Rows(&[expected]));
    }
    for (operator, expected) in [(">", "1"), (">=", "1"), ("<", "0"), ("<=", "0")] {
        tk.MustQuery(
            &format!("SELECT VEC_FROM_TEXT('[1,2,3]') {operator} '[1]'"),
            Vec::new(),
        )
        .Check(Rows(&[expected]));
    }
    tk.MustQuery(
        "SELECT GREATEST(VEC_FROM_TEXT('[1,2,3]'),VEC_FROM_TEXT('[4,5,6]'),\
         VEC_FROM_TEXT('[7,8,9]'))",
        Vec::new(),
    )
    .Check(Rows(&["[7,8,9]"]));
    tk.MustQuery(
        "SELECT LEAST(VEC_FROM_TEXT('[1,2,3]'),VEC_FROM_TEXT('[4,5,6]'),\
         VEC_FROM_TEXT('[7,8,9]'))",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]"]));
    for (sql, expected) in [
        (
            "SELECT COALESCE(VEC_FROM_TEXT('[1,2,3]'),VEC_FROM_TEXT('[4,5,6]'))",
            "[1,2,3]",
        ),
        ("SELECT COALESCE(NULL,VEC_FROM_TEXT('[1,2,3]'))", "[1,2,3]"),
        ("SELECT COALESCE(VEC_FROM_TEXT('[1,2,3]'),1)", "[1,2,3]"),
        ("SELECT COALESCE(VEC_FROM_TEXT('[1,2,3]'),'1')", "[1,2,3]"),
        ("SELECT COALESCE(1,VEC_FROM_TEXT('[1,2,3]'),1)", "1"),
        ("SELECT COALESCE('1',VEC_FROM_TEXT('[1,2,3]'),'1')", "1"),
    ] {
        tk.MustQuery(sql, Vec::new()).Check(Rows(&[expected]));
    }
}

#[test]
/// VECTOR 与字符串的 CAST/CONVERT，以及不支持目标类型和固定维度错误。
fn TestVectorConversion() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t1(val VECTOR)", Vec::new());

    for sql in [
        "SELECT CAST(VEC_FROM_TEXT('[1,2,3]') AS BINARY)",
        "SELECT CAST(VEC_FROM_TEXT('[1,2,3]') AS CHAR)",
        "SELECT CAST('[1,2,3]' AS VECTOR)",
        "SELECT CAST('[1,2,3]' AS VECTOR(3))",
        "SELECT CAST(VEC_FROM_TEXT('[1,2,3]') AS VECTOR(3))",
        "SELECT CONVERT(VEC_FROM_TEXT('[1,2,3]'),BINARY)",
        "SELECT CONVERT(VEC_FROM_TEXT('[1,2,3]'),CHAR)",
        "SELECT CONVERT('[1,2,3]',VECTOR)",
        "SELECT CONVERT('[1,2,3]',VECTOR(3))",
        "SELECT CONVERT(VEC_FROM_TEXT('[1,2,3]'),VECTOR(3))",
    ] {
        tk.MustQuery(sql, Vec::new()).Check(Rows(&["[1,2,3]"]));
    }
    for sql in ["SELECT CAST('[]' AS VECTOR)", "SELECT CONVERT('[]',VECTOR)"] {
        tk.MustQuery(sql, Vec::new()).Check(Rows(&["[]"]));
    }
    for target in [
        "JSON",
        "DECIMAL(2)",
        "DOUBLE",
        "FLOAT",
        "REAL",
        "SIGNED",
        "UNSIGNED",
        "YEAR",
        "DATETIME",
        "DATE",
        "TIME",
    ] {
        let _ = tk.QueryToErr(&format!(
            "SELECT CAST(VEC_FROM_TEXT('[1,2,3]') AS {target})"
        ));
    }
    for target in [
        "JSON", "DECIMAL", "DOUBLE", "FLOAT", "REAL", "SIGNED", "UNSIGNED", "YEAR", "DATETIME",
        "DATE", "TIME",
    ] {
        let _ = tk.QueryToErr(&format!(
            "SELECT CONVERT(VEC_FROM_TEXT('[1,2,3]'),{target})"
        ));
    }
    tk.MustContainErrMsg(
        "SELECT CAST('[1,2,3]' AS VECTOR<DOUBLE>)",
        "Only VECTOR is supported for now",
    );
    tk.MustContainErrMsg(
        "SELECT CONVERT('[1,2,3]',VECTOR<DOUBLE>)",
        "Only VECTOR is supported for now",
    );
    for sql in [
        "SELECT CAST('[1,2,3]' AS VECTOR(2))",
        "SELECT CAST(VEC_FROM_TEXT('[1,2,3]') AS VECTOR(2))",
        "SELECT CONVERT('[1,2,3]',VECTOR(2))",
        "SELECT CONVERT(VEC_FROM_TEXT('[1,2,3]'),VECTOR(2))",
    ] {
        assert_eq!(
            tk.QueryToErr(sql).message(),
            "vector has 3 dimensions, does not fit VECTOR(2)"
        );
    }
}

#[test]
/// VECTOR 可写入用户变量并保持规范文本值。
fn TestVectorAssignVariable() {
    let mut tk = new_testkit();
    tk.MustExec("SET @a = VEC_FROM_TEXT('[1,2,3]')", Vec::new());
    tk.MustQuery("SELECT @a", Vec::new())
        .Check(Rows(&["[1,2,3]"]));
}

#[test]
/// 控制流函数须通过真实 SQL 会话保留 VECTOR 值语义。
fn TestVectorControlFlow() {
    let tk = new_testkit();

    tk.MustQuery("SELECT IF(VEC_FROM_TEXT('[1, 2, 3]'), 1, 0)", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery(
        "SELECT IF(TRUE, VEC_FROM_TEXT('[1, 2, 3]'), VEC_FROM_TEXT('[4, 5, 6]'))",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]"]));
    tk.MustQuery("SELECT IFNULL(VEC_FROM_TEXT('[1, 2, 3]'), 1)", Vec::new())
        .Check(Rows(&["[1,2,3]"]));
    tk.MustQuery(
        "SELECT IFNULL(NULL, VEC_FROM_TEXT('[1, 2, 3]'))",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]"]));
    tk.MustQuery(
        "SELECT NULLIF(VEC_FROM_TEXT('[1, 2, 3]'), VEC_FROM_TEXT('[1, 2, 3]'))",
        Vec::new(),
    )
    .Check(Rows(&["<nil>"]));
    tk.MustQuery(
        "SELECT NULLIF(VEC_FROM_TEXT('[1, 2, 3]'), VEC_FROM_TEXT('[4, 5, 6]'))",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]"]));
    tk.MustQuery(
        "SELECT CASE WHEN TRUE THEN VEC_FROM_TEXT('[1, 2, 3]') \
         ELSE VEC_FROM_TEXT('[4, 5, 6]') END",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]"]));
}

#[test]
/// LIKE/ILIKE/STRCMP 对 VECTOR 使用规范文本表示。
fn TestVectorStringCompare() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t1(val VECTOR)", Vec::new());
    tk.MustExec("INSERT INTO t1 VALUES ('[1,2,3]'),('[4,5,6]')", Vec::new());
    tk.MustQuery("SELECT * FROM t1 WHERE val LIKE '%2%'", Vec::new())
        .Check(Rows(&["[1,2,3]"]));
    tk.MustQuery("SELECT * FROM t1 WHERE val ILIKE '%2%'", Vec::new())
        .Check(Rows(&["[1,2,3]"]));
    tk.MustQuery(
        "SELECT STRCMP('[1,2,3]',VEC_FROM_TEXT('[1,2,3]'))",
        Vec::new(),
    )
    .Check(Rows(&["0"]));
    tk.MustQuery(
        "SELECT STRCMP('[4,5,6]',VEC_FROM_TEXT('[1,2,3]'))",
        Vec::new(),
    )
    .Check(Rows(&["1"]));
}

#[test]
/// VECTOR 的 GROUP BY、DISTINCT、MIN/MAX、HAVING 走真实 SQL。
fn TestVectorAggregations() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(val VECTOR)", Vec::new());
    tk.MustExec(
        "INSERT INTO t VALUES \
         ('[8.7,5.7,7.7,9.8,1.5]'),('[3.6,9.7,2.4,6.6,4.9]'),\
         ('[4.7,4.9,2.6,5.2,7.4]'),('[4.7,4.9,2.6,5.2,7.4]'),\
         ('[7.7,6.7,8.3,7.8,5.7]'),('[1.4,4.5,8.5,7.7,6.2]')",
        Vec::new(),
    );
    tk.MustExec("ANALYZE TABLE t", Vec::new());
    tk.MustQuery(
        "SELECT COUNT(*),val FROM t GROUP BY val ORDER BY val",
        Vec::new(),
    )
    .Check(Rows(&[
        "1 [1.4,4.5,8.5,7.7,6.2]",
        "1 [3.6,9.7,2.4,6.6,4.9]",
        "2 [4.7,4.9,2.6,5.2,7.4]",
        "1 [7.7,6.7,8.3,7.8,5.7]",
        "1 [8.7,5.7,7.7,9.8,1.5]",
    ]));
    tk.MustQuery("SELECT COUNT(val) FROM t", Vec::new())
        .Check(Rows(&["6"]));
    tk.MustQuery("SELECT COUNT(DISTINCT val) FROM t", Vec::new())
        .Check(Rows(&["5"]));
    tk.MustQuery("SELECT MIN(val) FROM t", Vec::new())
        .Check(Rows(&["[1.4,4.5,8.5,7.7,6.2]"]));
    tk.MustQuery("SELECT MAX(val) FROM t", Vec::new())
        .Check(Rows(&["[8.7,5.7,7.7,9.8,1.5]"]));
    let _ = tk.QueryToErr("SELECT SUM(val) FROM t");
    let _ = tk.QueryToErr("SELECT AVG(val) FROM t");
    tk.MustQuery(
        "SELECT val FROM t GROUP BY val \
         HAVING val > VEC_FROM_TEXT('[4.7,4.9,2.6,5.2,7.4]') ORDER BY val",
        Vec::new(),
    )
    .Check(Rows(&["[7.7,6.7,8.3,7.8,5.7]", "[8.7,5.7,7.7,9.8,1.5]"]));
}

#[test]
/// VECTOR 的窗口值、排名与前后行函数走真实 SQL。
fn TestVectorWindow() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(embedding VECTOR)", Vec::new());
    tk.MustExec(
        "INSERT INTO t VALUES ('[1,2,3]'),('[4,5,601]'),('[4,5,61]')",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT embedding,FIRST_VALUE(embedding) OVER w AS first,\
         NTH_VALUE(embedding,2) OVER w AS second,LAST_VALUE(embedding) OVER w AS last \
         FROM t WINDOW w AS (ORDER BY embedding) ORDER BY embedding",
        Vec::new(),
    )
    .Check(Rows(&[
        "[1,2,3] [1,2,3] <nil> [1,2,3]",
        "[4,5,61] [1,2,3] [4,5,61] [4,5,61]",
        "[4,5,601] [1,2,3] [4,5,61] [4,5,601]",
    ]));
    tk.MustExec("DELETE FROM t WHERE 1=1", Vec::new());
    tk.MustExec(
        "INSERT INTO t VALUES ('[1,2,3]'),('[4,5,6]'),('[4,5,6]'),('[7,8,9]')",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT embedding,ROW_NUMBER() OVER w AS `row_num`,RANK() OVER w AS `rank`,\
         DENSE_RANK() OVER w AS `dense_rank` FROM t \
         WINDOW w AS (ORDER BY embedding) ORDER BY embedding",
        Vec::new(),
    )
    .Check(Rows(&[
        "[1,2,3] 1 1 1",
        "[4,5,6] 2 2 2",
        "[4,5,6] 3 2 2",
        "[7,8,9] 4 4 3",
    ]));
    tk.MustQuery(
        "SELECT embedding,LAG(embedding) OVER w AS `lag`,LEAD(embedding) OVER w AS `lead` \
         FROM t WINDOW w AS (ORDER BY embedding) ORDER BY embedding",
        Vec::new(),
    )
    .Check(Rows(&[
        "[1,2,3] <nil> [4,5,6]",
        "[4,5,6] [1,2,3] [4,5,6]",
        "[4,5,6] [4,5,6] [7,8,9]",
        "[7,8,9] [4,5,6] <nil>",
    ]));
    tk.MustQuery(
        "SELECT embedding,ROW_NUMBER() OVER \
         (PARTITION BY embedding ORDER BY embedding) AS `row_num` FROM t ORDER BY embedding",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3] 1", "[4,5,6] 1", "[4,5,6] 2", "[7,8,9] 1"]));
}

#[test]
/// VECTOR 列只能创建 VECTOR INDEX，普通列不能承载向量索引。
fn TestVectorIndexSyntax() {
    let mut tk = new_testkit();
    tk.MustContainErrMsg(
        "CREATE TABLE t1(embedding VECTOR UNIQUE)",
        "only VECTOR INDEX can be added to vector column",
    );
    tk.MustContainErrMsg(
        "CREATE TABLE t1(embedding VECTOR,INDEX idx(embedding))",
        "only VECTOR INDEX can be added to vector column",
    );
    tk.MustContainErrMsg(
        "CREATE TABLE t1(embedding BLOB,VECTOR INDEX idx((VEC_COSINE_DISTANCE(embedding))))",
        "Unsupported add vector index: only support vector type",
    );
}

#[test]
/// 向量距离 ORDER BY 的预处理参数与 LIMIT 参数保持真实执行顺序。
fn TestVectorSearchPreparedStatement() {
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE t1(pk INT PRIMARY KEY,vec VECTOR(3),\
         VECTOR INDEX idx_embedding((VEC_COSINE_DISTANCE(vec))))",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO t1 VALUES (1,'[1,2,3]'),(2,'[4,5,6]'),(3,'[7,8,9]')",
        Vec::new(),
    );
    tk.MustExec("ANALYZE TABLE t1", Vec::new());
    tk.MustExec(
        "PREPARE stmt FROM \
         'SELECT pk FROM t1 ORDER BY VEC_COSINE_DISTANCE(vec,?) LIMIT ?'",
        Vec::new(),
    );
    tk.MustExec("SET @pvec='[7,8,9]'", Vec::new());
    tk.MustExec("SET @plimit=10", Vec::new());
    tk.MustQuery("EXECUTE stmt USING @pvec,@plimit", Vec::new())
        .Check(Rows(&["3", "2", "1"]));
}

fn testVectorSearchInternal(tk: &mut TestKit) {
    tk.MustExec(
        "CREATE TABLE t1(
            id INT PRIMARY KEY,
            vec VECTOR(3),
            a INT,
            b INT,
            c VECTOR(3),
            d VECTOR,
            VECTOR INDEX idx_embedding((VEC_COSINE_DISTANCE(vec)))
        )",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO t1 VALUES
            (1,'[1,1,1]',11,111,'[1,1,1]','[1,1,1]'),
            (2,'[2,2,2]',22,222,'[2,2,2]','[2,2,2]'),
            (3,'[3,3,3]',33,333,'[3,3,3]','[3,3,3]')",
        Vec::new(),
    );
    tk.MustExec("ANALYZE TABLE t1", Vec::new());

    for (sql, expected) in [
        ("SELECT id FROM t1 ORDER BY id", Rows(&["1", "2", "3"])),
        (
            "SELECT id FROM t1 ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&["3", "2", "1"]),
        ),
        (
            "SELECT id FROM t1 ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 1",
            Rows(&["3"]),
        ),
        (
            "SELECT id FROM t1 ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]')",
            Rows(&["3", "2", "1"]),
        ),
        (
            "SELECT * FROM t1 ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&[
                "3 [3,3,3] 33 333 [3,3,3] [3,3,3]",
                "2 [2,2,2] 22 222 [2,2,2] [2,2,2]",
                "1 [1,1,1] 11 111 [1,1,1] [1,1,1]",
            ]),
        ),
        (
            "SELECT id,a,b FROM t1 ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&["3 33 333", "2 22 222", "1 11 111"]),
        ),
        (
            "SELECT a,id,b FROM t1 ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&["33 3 333", "22 2 222", "11 1 111"]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d FROM t1 ORDER BY d LIMIT 10",
            Rows(&["3 0", "2 1.7320508075688772", "1 3.4641016151377544"]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d FROM t1 ORDER BY d",
            Rows(&["3 0", "2 1.7320508075688772", "1 3.4641016151377544"]),
        ),
        (
            "SELECT *,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d FROM t1 ORDER BY d LIMIT 10",
            Rows(&[
                "3 [3,3,3] 33 333 [3,3,3] [3,3,3] 0",
                "2 [2,2,2] 22 222 [2,2,2] [2,2,2] 1.7320508075688772",
                "1 [1,1,1] 11 111 [1,1,1] [1,1,1] 3.4641016151377544",
            ]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d,a,b FROM t1 ORDER BY d LIMIT 10",
            Rows(&[
                "3 0 33 333",
                "2 1.7320508075688772 22 222",
                "1 3.4641016151377544 11 111",
            ]),
        ),
        (
            "SELECT id,a,b,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d FROM t1 ORDER BY d LIMIT 10",
            Rows(&[
                "3 33 333 0",
                "2 22 222 1.7320508075688772",
                "1 11 111 3.4641016151377544",
            ]),
        ),
    ] {
        tk.MustQuery(sql, Vec::new()).Check(expected);
    }

    tk.MustExec(
        "CREATE TABLE tp(
            id INT,
            vec VECTOR(3) COMMENT 'hnsw(distance=cosine)',
            a INT,
            b INT,
            store_id INT
        ) PARTITION BY RANGE COLUMNS(store_id)(
            PARTITION p0 VALUES LESS THAN (100),
            PARTITION p1 VALUES LESS THAN (200),
            PARTITION p2 VALUES LESS THAN (MAXVALUE)
        )",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO tp VALUES
            (1,'[1,1,1]',11,111,50),
            (2,'[2,2,2]',22,222,150),
            (3,'[3,3,3]',33,333,250)",
        Vec::new(),
    );
    tk.MustExec("ANALYZE TABLE tp", Vec::new());

    for (sql, expected) in [
        ("SELECT id FROM tp ORDER BY id", Rows(&["1", "2", "3"])),
        (
            "SELECT id FROM tp ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&["3", "2", "1"]),
        ),
        (
            "SELECT id FROM tp ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]')",
            Rows(&["3", "2", "1"]),
        ),
        (
            "SELECT * FROM tp ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&[
                "3 [3,3,3] 33 333 250",
                "2 [2,2,2] 22 222 150",
                "1 [1,1,1] 11 111 50",
            ]),
        ),
        (
            "SELECT id,a,b FROM tp ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&["3 33 333", "2 22 222", "1 11 111"]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d FROM tp ORDER BY d LIMIT 10",
            Rows(&["3 0", "2 1.7320508075688772", "1 3.4641016151377544"]),
        ),
        (
            "SELECT *,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d FROM tp ORDER BY d LIMIT 10",
            Rows(&[
                "3 [3,3,3] 33 333 250 0",
                "2 [2,2,2] 22 222 150 1.7320508075688772",
                "1 [1,1,1] 11 111 50 3.4641016151377544",
            ]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d,a,b FROM tp ORDER BY d LIMIT 10",
            Rows(&[
                "3 0 33 333",
                "2 1.7320508075688772 22 222",
                "1 3.4641016151377544 11 111",
            ]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d,a,b FROM tp ORDER BY d",
            Rows(&[
                "3 0 33 333",
                "2 1.7320508075688772 22 222",
                "1 3.4641016151377544 11 111",
            ]),
        ),
        (
            "SELECT id FROM tp PARTITION(p0) ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&["1"]),
        ),
        (
            "SELECT * FROM tp PARTITION(p0) ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&["1 [1,1,1] 11 111 50"]),
        ),
        (
            "SELECT id,a,b FROM tp PARTITION(p0) ORDER BY VEC_L2_DISTANCE(vec,'[3,3,3]') LIMIT 10",
            Rows(&["1 11 111"]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d FROM tp PARTITION(p0) ORDER BY d LIMIT 10",
            Rows(&["1 3.4641016151377544"]),
        ),
        (
            "SELECT *,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d FROM tp PARTITION(p0) ORDER BY d LIMIT 10",
            Rows(&["1 [1,1,1] 11 111 50 3.4641016151377544"]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d,a,b FROM tp PARTITION(p0) ORDER BY d LIMIT 10",
            Rows(&["1 3.4641016151377544 11 111"]),
        ),
        (
            "SELECT id,VEC_L2_DISTANCE(vec,'[3,3,3]') AS d,a,b FROM tp PARTITION(p0) ORDER BY d",
            Rows(&["1 3.4641016151377544 11 111"]),
        ),
    ] {
        tk.MustQuery(sql, Vec::new()).Check(expected);
    }
}

#[test]
/// 向量搜索投影提取覆盖普通表、分区表及 fix-control 两种会话。
fn TestVectorSearchExtractProj() {
    let mut tk = new_testkit();
    testVectorSearchInternal(&mut tk);

    let mut tk = new_testkit();
    tk.MustExec("SET SESSION tidb_opt_fix_control='56318:OFF'", Vec::new());
    testVectorSearchInternal(&mut tk);
}

#[test]
/// VECTOR 的 UNION/UNION ALL/INTERSECT/EXCEPT 走真实 SQL。
fn TestVectorSetOperation() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t1(embedding VECTOR)", Vec::new());
    tk.MustExec("INSERT INTO t1 VALUES ('[1,2,3]'),('[4,5,6]')", Vec::new());
    tk.MustExec("CREATE TABLE t2(embedding VECTOR)", Vec::new());
    tk.MustExec("INSERT INTO t2 VALUES ('[4,5,6]'),('[7,8,9]')", Vec::new());
    tk.MustQuery(
        "(SELECT embedding FROM t1 UNION SELECT embedding FROM t2) ORDER BY embedding",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]", "[4,5,6]", "[7,8,9]"]));
    tk.MustQuery(
        "(SELECT embedding FROM t1 UNION ALL SELECT embedding FROM t2) ORDER BY embedding",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]", "[4,5,6]", "[4,5,6]", "[7,8,9]"]));
    tk.MustQuery(
        "SELECT embedding FROM t1 INTERSECT SELECT embedding FROM t2",
        Vec::new(),
    )
    .Check(Rows(&["[4,5,6]"]));
    tk.MustQuery(
        "SELECT embedding FROM t1 EXCEPT SELECT embedding FROM t2",
        Vec::new(),
    )
    .Check(Rows(&["[1,2,3]"]));
}

#[test]
/// VECTOR 逐元素加减乘、维度和溢出错误走真实 SQL。
fn TestVectorArithmatic() {
    let mut tk = new_testkit();
    tk.MustExec("CREATE TABLE t(embedding VECTOR)", Vec::new());
    tk.MustExec(
        "INSERT INTO t VALUES ('[1,2,3]'),('[4,5,6]'),('[7,8,9]')",
        Vec::new(),
    );
    tk.MustQuery("SELECT embedding + '[1,2,3]' FROM t", Vec::new())
        .Check(Rows(&["[2,4,6]", "[5,7,9]", "[8,10,12]"]));
    tk.MustQuery("SELECT embedding + embedding FROM t", Vec::new())
        .Check(Rows(&["[2,4,6]", "[8,10,12]", "[14,16,18]"]));
    tk.MustQuery("SELECT embedding - '[1,2,3]' FROM t", Vec::new())
        .Check(Rows(&["[0,0,0]", "[3,3,3]", "[6,6,6]"]));
    tk.MustQuery("SELECT embedding - embedding FROM t", Vec::new())
        .Check(Rows(&["[0,0,0]", "[0,0,0]", "[0,0,0]"]));
    tk.MustQuery(
        "SELECT VEC_FROM_TEXT('[1,2]') + VEC_FROM_TEXT('[2,3]')",
        Vec::new(),
    )
    .Check(Rows(&["[3,5]"]));
    tk.MustQuery("SELECT VEC_FROM_TEXT('[1,2]') + '[2,3]'", Vec::new())
        .Check(Rows(&["[3,5]"]));
    tk.MustQuery("SELECT VEC_FROM_TEXT('[1,2,3]') * '[4,5,6]'", Vec::new())
        .Check(Rows(&["[4,10,18]"]));
    for sql in [
        "SELECT embedding + 1 FROM t",
        "SELECT embedding + '[]' FROM t",
        "SELECT embedding - '[1]' FROM t",
        "SELECT VEC_FROM_TEXT('[1,2]') + '[2,3,4]'",
        "SELECT VEC_FROM_TEXT('[1]') + 2",
        "SELECT VEC_FROM_TEXT('[1]') + '2'",
        "SELECT VEC_FROM_TEXT('[3e38]') + '[3e38]'",
        "SELECT VEC_FROM_TEXT('[1e37]') * '[1e37]'",
        "SELECT VEC_L2_NORM('[1e39]') + 1",
        "SELECT VEC_L2_NORM('[1e39]') * 0 + 1",
    ] {
        let _ = tk.QueryToErr(sql);
    }
}

#[test]
/// VECTOR 距离、负内积、余弦距离和范数走真实 SQL。
fn TestVectorFunctions() {
    let tk = new_testkit();
    for (sql, expected) in [
        ("SELECT VEC_L1_DISTANCE('[0,0]','[3,4]')", "7"),
        ("SELECT VEC_L1_DISTANCE('[0,0]','[0,1]')", "1"),
        ("SELECT VEC_L1_DISTANCE('[3e38]','[-3e38]')", "+Inf"),
        ("SELECT VEC_L2_DISTANCE('[0,0]','[3,4]')", "5"),
        ("SELECT VEC_L2_DISTANCE('[0,0]','[0,1]')", "1"),
        ("SELECT VEC_L2_DISTANCE('[3e38]','[-3e38]')", "+Inf"),
        ("SELECT VEC_NEGATIVE_INNER_PRODUCT('[1,2]','[3,4]')", "-11"),
        (
            "SELECT VEC_NEGATIVE_INNER_PRODUCT('[3e38]','[3e38]')",
            "-Inf",
        ),
        ("SELECT VEC_COSINE_DISTANCE('[1,2]','[2,4]')", "0"),
        ("SELECT VEC_COSINE_DISTANCE('[1,2]','[0,0]')", "<nil>"),
        ("SELECT VEC_COSINE_DISTANCE('[1,1]','[1,1]')", "0"),
        ("SELECT VEC_COSINE_DISTANCE('[1,0]','[0,2]')", "1"),
        ("SELECT VEC_COSINE_DISTANCE('[1,1]','[-1,-1]')", "2"),
        ("SELECT VEC_COSINE_DISTANCE('[1,1]','[1.1,1.1]')", "0"),
        ("SELECT VEC_COSINE_DISTANCE('[1,1]','[-1.1,-1.1]')", "2"),
        ("SELECT VEC_COSINE_DISTANCE('[3e38]','[3e38]')", "<nil>"),
        ("SELECT VEC_L2_NORM('[3,4]')", "5"),
        ("SELECT VEC_L2_NORM('[0,1]')", "1"),
    ] {
        tk.MustQuery(sql, Vec::new()).Check(Rows(&[expected]));
    }
    for sql in [
        "SELECT VEC_L1_DISTANCE('[1,2]','[3]')",
        "SELECT VEC_L2_DISTANCE('[1,2]','[3]')",
        "SELECT VEC_NEGATIVE_INNER_PRODUCT('[1,2]','[3]')",
        "SELECT VEC_COSINE_DISTANCE('[1,2]','[3]')",
    ] {
        let _ = tk.QueryToErr(sql);
    }
}

#[test]
/// ON DUPLICATE KEY UPDATE 中 VALUES/VEC_DIMS/VECTOR 算术走真实 SQL。
fn TestVectorMiscFunctions() {
    let mut tk = new_testkit();
    tk.MustExec(
        "CREATE TABLE a(pk INT PRIMARY KEY,c VECTOR(3),time INT)",
        Vec::new(),
    );
    tk.MustExec("INSERT INTO a VALUES (1,'[1,2,3]',5)", Vec::new());
    tk.MustQuery("SELECT * FROM a", Vec::new())
        .Check(Rows(&["1 [1,2,3] 5"]));
    tk.MustExec(
        "INSERT INTO a VALUES (1,'[1,1,1]',10) \
         ON DUPLICATE KEY UPDATE time=VALUES(time),c=VALUES(c)",
        Vec::new(),
    );
    tk.MustQuery("SELECT * FROM a", Vec::new())
        .Check(Rows(&["1 [1,1,1] 10"]));
    tk.MustExec(
        "INSERT INTO a VALUES (1,'[1,5,7]',15) \
         ON DUPLICATE KEY UPDATE time=VEC_DIMS(c),c=VALUES(c)+VALUES(c)",
        Vec::new(),
    );
    tk.MustQuery("SELECT * FROM a", Vec::new())
        .Check(Rows(&["1 [2,10,14] 3"]));
}
