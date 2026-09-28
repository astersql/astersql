// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 本文件对应 pkg/planner/core/tests/null/null_test.go。

#![allow(non_snake_case)]

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    TestKit::new(store)
}

#[test]
fn TestIssue54803() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        r#"
        CREATE TABLE t1db47fc1 (
            col_67 time NOT NULL DEFAULT '16:58:45',
            col_68 tinyint(3) unsigned DEFAULT NULL,
            col_69 bit(6) NOT NULL DEFAULT b'11110',
            col_72 double NOT NULL
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
        PARTITION BY HASH (col_68) PARTITIONS 5;
        "#,
        Vec::new(),
    );
    tk.MustQuery(
        r#"EXPLAIN format='plan_tree' SELECT col_68
        FROM t1db47fc1
        WHERE ISNULL(t1db47fc1.col_68)
        GROUP BY t1db47fc1.col_68
        HAVING ISNULL(t1db47fc1.col_68) OR t1db47fc1.col_68 IN (62, 200, 196, 99);"#,
        Vec::new(),
    )
    .Check(vec![
        vec!["HashAgg root  group by:test.t1db47fc1.col_68, funcs:firstrow(test.t1db47fc1.col_68)->test.t1db47fc1.col_68"],
        vec!["└─TableReader root partition:p0 data:Selection"],
        vec!["  └─Selection cop[tikv]  isnull(test.t1db47fc1.col_68)"],
        vec!["    └─TableFullScan cop[tikv] table:t1db47fc1 keep order:false, stats:pseudo"],
    ]);
    tk.MustQuery(
        r#"EXPLAIN format='plan_tree' SELECT TRIM(t1db47fc1.col_68) AS r0
        FROM t1db47fc1
        WHERE ISNULL(t1db47fc1.col_68)
        GROUP BY t1db47fc1.col_68
        HAVING ISNULL(t1db47fc1.col_68) OR t1db47fc1.col_68 IN (62, 200, 196, 99)
        LIMIT 106149535;"#,
        Vec::new(),
    )
    .Check(vec![
        vec!["Projection root  trim(cast(test.t1db47fc1.col_68, var_string(20)))->Column"],
        vec!["└─Limit root  offset:0, count:106149535"],
        vec!["  └─HashAgg root  group by:test.t1db47fc1.col_68, funcs:firstrow(test.t1db47fc1.col_68)->test.t1db47fc1.col_68"],
        vec!["    └─TableReader root partition:p0 data:Selection"],
        vec!["      └─Selection cop[tikv]  isnull(test.t1db47fc1.col_68)"],
        vec!["        └─TableFullScan cop[tikv] table:t1db47fc1 keep order:false, stats:pseudo"],
    ]);

    // Issue55299.
    tk.MustExec(
        r#"
        CREATE TABLE tcd8c2aac (
          col_21 char(87) COLLATE utf8mb4_general_ci DEFAULT NULL,
          KEY idx_12 (col_21(1))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;
        "#,
        Vec::new(),
    );
    tk.MustExec(
        r#"
        CREATE TABLE tle50fd846 (
          col_42 date DEFAULT '1989-10-30',
          col_43 varbinary(122) NOT NULL DEFAULT 'Vz!3_P0LOdG',
          col_44 json DEFAULT NULL,
          col_45 binary(129) DEFAULT NULL,
          col_46 double NOT NULL DEFAULT '4264.32300782421',
          col_47 char(251) NOT NULL DEFAULT 'g7uo-dlBEY22!fx3@&',
          col_48 char(229) NOT NULL,
          col_49 blob NOT NULL,
          col_50 blob DEFAULT NULL,
          col_51 json DEFAULT NULL,
          PRIMARY KEY (col_48) /*T![clustered_index] NONCLUSTERED */
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8 COLLATE=utf8_bin;
        "#,
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO `tcd8c2aac` VALUES(NULL),(NULL),('u!Vk+9B-3bn@'),('&PpQ*z!kQwj4g*ag#');",
        Vec::new(),
    );
    tk.MustExec(
        r#"INSERT INTO tle50fd846
        VALUES
        ('2029-05-09', x'757640736a42316c384162793124246b', '["YXt8UJAnVMWeMEZj1CzhNUzTMDJfzsmTWQkyOvVCsciA3eobvH8heH8gtr6ogxXa"]', x'577340000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000', 526.0218366710487, '%gMk', '58reJ%D&54', x'39254c48242556737474', x'6c66762b303567236f4068', '[2984188985038968170, 2580328438245089106, 4624130652422829118]');"#,
        Vec::new(),
    );
    tk.MustQuery(
        r#"EXPLAIN format='plan_tree' SELECT GROUP_CONCAT(tcd8c2aac.col_21 ORDER BY tcd8c2aac.col_21 SEPARATOR ',') AS r0
        FROM tcd8c2aac
        JOIN tle50fd846
        WHERE ISNULL(tcd8c2aac.col_21) OR tcd8c2aac.col_21='yJTkLeL5^yJ'
        GROUP BY tcd8c2aac.col_21
        HAVING ISNULL(tcd8c2aac.col_21)
        LIMIT 48579914;"#,
        Vec::new(),
    )
    .Check(vec![
        vec!["Limit root  offset:0, count:48579914"],
        vec!["└─HashAgg root  group by:test.tcd8c2aac.col_21, funcs:group_concat(test.tcd8c2aac.col_21 order by test.tcd8c2aac.col_21 separator \",\")->Column"],
        vec!["  └─HashJoin root  CARTESIAN inner join"],
        vec!["    ├─IndexLookUp(Build) root  "],
        vec!["    │ ├─Selection(Build) cop[tikv]  isnull(test.tcd8c2aac.col_21)"],
        vec!["    │ │ └─IndexRangeScan cop[tikv] table:tcd8c2aac, index:idx_12(col_21) range:[NULL,NULL], keep order:false, stats:pseudo"],
        vec!["    │ └─TableRowIDScan(Probe) cop[tikv] table:tcd8c2aac keep order:false, stats:pseudo"],
        vec!["    └─IndexReader(Probe) root  index:IndexFullScan"],
        vec!["      └─IndexFullScan cop[tikv] table:tle50fd846, index:PRIMARY(col_48) keep order:false, stats:pseudo"],
    ]);
    tk.MustQuery(
        r#"SELECT GROUP_CONCAT(tcd8c2aac.col_21 ORDER BY tcd8c2aac.col_21 SEPARATOR ',') AS r0
        FROM tcd8c2aac
        JOIN tle50fd846
        WHERE ISNULL(tcd8c2aac.col_21) OR tcd8c2aac.col_21='yJTkLeL5^yJ'
        GROUP BY tcd8c2aac.col_21
        HAVING ISNULL(tcd8c2aac.col_21)
        LIMIT 48579914;"#,
        Vec::new(),
    )
    .Check(vec![vec!["<nil>"]]);

    tk.MustExec(
        r#"CREATE TABLE ta31c32a7 (
          col_63 double DEFAULT '9963.92512636973',
          KEY idx_24 (col_63),
          KEY idx_25 (col_63),
          KEY idx_26 (col_63)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"INSERT INTO ta31c32a7 VALUES
        (5496.073863178138), (4027.8475888445246), (2995.154396178381), (3045.228783606007), (3618.0432407275603), (1156.6077897338241),
        (348.56448524702813), (2138.361831358777), (5904.959667345741), (2815.6976889801267), (6455.25717613724),
        (9721.34540217101), (6793.035010125108), (6080.120357332818), (NULL), (1780.7418079754723),
        (1222.1954607008702), (3576.2079432921923), (2187.4672702135276), (9129.689249510902),
        (1065.3222700463314), (7509.347382423184), (7413.331945779306), (986.9882817569359),
        (747.4145098692578), (4850.840161745998), (2607.5009231086797), (6499.136742855925),
        (2501.691252762187), (6138.096783185339);"#,
        Vec::new(),
    );
    tk.MustQuery(
        r#"explain format='plan_tree' SELECT BIT_XOR(ta31c32a7.col_63) AS r0
        FROM ta31c32a7
        WHERE ISNULL(ta31c32a7.col_63)
          OR ta31c32a7.col_63 IN (1780.7418079754723, 5904.959667345741, 1531.4023068774668)
        GROUP BY ta31c32a7.col_63
        HAVING ISNULL(ta31c32a7.col_63)
        LIMIT 65122436;"#,
        Vec::new(),
    )
    .Check(vec![
        vec!["Limit root  offset:0, count:65122436"],
        vec!["└─StreamAgg root  group by:test.ta31c32a7.col_63, funcs:bit_xor(Column)->Column"],
        vec!["  └─IndexReader root  index:StreamAgg"],
        vec!["    └─StreamAgg cop[tikv]  group by:test.ta31c32a7.col_63, funcs:bit_xor(cast(test.ta31c32a7.col_63, bigint(22) BINARY))->Column"],
        vec!["      └─IndexRangeScan cop[tikv] table:ta31c32a7, index:idx_24(col_63) range:[NULL,NULL], keep order:true, stats:pseudo"],
    ]);
    tk.MustQuery(
        r#"explain format='plan_tree' SELECT BIT_XOR(ta31c32a7.col_63) AS r0
        FROM ta31c32a7
        WHERE ISNULL(ta31c32a7.col_63)
          OR ta31c32a7.col_63 IN (1780.7418079754723, 5904.959667345741, 1531.4023068774668)
        GROUP BY ta31c32a7.col_63
        LIMIT 65122436;"#,
        Vec::new(),
    )
    .Check(vec![
        vec!["Limit root  offset:0, count:65122436"],
        vec!["└─StreamAgg root  group by:test.ta31c32a7.col_63, funcs:bit_xor(Column)->Column"],
        vec!["  └─IndexReader root  index:StreamAgg"],
        vec!["    └─StreamAgg cop[tikv]  group by:test.ta31c32a7.col_63, funcs:bit_xor(cast(test.ta31c32a7.col_63, bigint(22) BINARY))->Column"],
        vec!["      └─IndexRangeScan cop[tikv] table:ta31c32a7, index:idx_24(col_63) range:[NULL,NULL], [1531.4023068774668,1531.4023068774668], [1780.7418079754723,1780.7418079754723], [5904.959667345741,5904.959667345741], keep order:true, stats:pseudo"],
    ]);
    tk.MustExec(
        r#"CREATE TABLE tl75eff7ba (
        col_1 tinyint(1) DEFAULT '0',
        KEY idx_1 (col_1),
        UNIQUE KEY idx_2 (col_1),
        UNIQUE KEY idx_3 (col_1),
        KEY idx_4 (col_1) /*!80000 INVISIBLE */,
        UNIQUE KEY idx_5 (col_1)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8 COLLATE=utf8_general_ci;"#,
        Vec::new(),
    );
    tk.MustExec("INSERT INTO tl75eff7ba VALUES(1),(0);", Vec::new());
    tk.MustQuery(
        "SELECT tl75eff7ba.col_1 AS r0 FROM tl75eff7ba WHERE ISNULL(tl75eff7ba.col_1) OR tl75eff7ba.col_1 IN (0, 0, 1, 1) GROUP BY tl75eff7ba.col_1 HAVING ISNULL(tl75eff7ba.col_1) OR tl75eff7ba.col_1 IN (0, 1, 1, 0) LIMIT 58651509;",
        Vec::new(),
    )
    .Check(vec![vec!["0"], vec!["1"]]);
}

#[test]
fn TestIssue56745() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table lrr( `COL1` varchar(10) NOT NULL,`COL2` char(10) NOT NULL,PRIMARY KEY (`COL1`(5),`COL2`) /*T![clustered_index] CLUSTERED */);",
        Vec::new(),
    );
    tk.MustExec("insert into lrr values('','a');", Vec::new());
    tk.MustExec("insert into lrr values('test','b');", Vec::new());
    tk.MustExec(
        "prepare stmt from 'SELECT * FROM lrr t1 JOIN lrr t2 ON t1.col1 <=> t2.col1 WHERE t1.col1 <=> NULL AND t2.col1 = ?;';",
        Vec::new(),
    );
    tk.MustExec("set @a=NULL;", Vec::new());
    tk.MustExec("execute stmt using @a;", Vec::new());
}
