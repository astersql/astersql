// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// digester API 的 Aster 迁移单元测试。
//
// 对照 Go digester 测试用例，验证 Digest 构造、Normalize / NormalizeForBinding /
// NormalizeKeepHint / MARKER 模式，以及特殊注释展开与关键字分区元数据。

use parser::{
    DigestHash, DigestNormalized, Keywords, NewDigest, Normalize, NormalizeDigest,
    NormalizeDigestForBinding, NormalizeForBinding, NormalizeKeepHint,
};

/// 校验 Digest 十六进制/字节视图与 DigestNormalized 固定哈希。
#[test]
fn digest_bytes_and_sha256_match_go() {
    let digest = NewDigest(vec![0x00, 0xab, 0xff]);
    assert_eq!(digest.String(), "00abff");
    assert_eq!(digest.Bytes(), &[0x00, 0xab, 0xff]);

    let empty = NewDigest(Vec::new());
    assert_eq!(empty.String(), "");
    assert!(empty.Bytes().is_empty());

    assert_eq!(
        DigestNormalized("select ?").String(),
        "e1c71d1661ae46e09b7aaec1c390957f0d6260410df4e4bc71b9c8d681021471"
    );
}

/// 抽样校验 Normalize 与 NormalizeDigest / DigestHash 的一致性。
#[test]
fn normalize_and_digest_follow_go_cases() {
    let cases = [
        ("SELECT 1", "select ?"),
        (
            "select * from b where id = 1",
            "select * from `b` where `id` = ?",
        ),
        (
            "select 1 from b where id in (1, 3, '3')",
            "select ? from `b` where `id` in ( ... )",
        ),
        ("select * from t Force Index(kk)", "select * from `t`"),
    ];

    for (sql, expected) in cases {
        let normalized = Normalize(sql, "ON");
        assert_eq!(normalized, expected, "{sql}");
        let (combined, digest) = NormalizeDigest(sql);
        assert_eq!(combined, normalized);
        assert_eq!(digest, DigestNormalized(&normalized));
        assert_eq!(DigestHash(sql), digest);
    }

    assert_eq!(
        NormalizeForBinding("select * from t where a in (1)", false),
        "select * from `t` where `a` in ( ... )"
    );
}

/// 内置函数、窗口函数标识与残缺 hint 的规范化形状。
#[test]
fn normalization_matches_go_for_builtin_and_window_identifiers() {
    assert_eq!(
        Normalize("select truncate(1, 2)", "ON"),
        "select truncate ( ... )"
    );
    assert_eq!(
        Normalize("select count(a) from t", "ON"),
        "select count ( `a` ) from `t`"
    );
    assert_eq!(
        NormalizeKeepHint("select * from t1, lateral (select t1.a) as dt"),
        "select * from `t1` , lateral ( select `t1` . `a` ) as `dt`",
    );
    assert_eq!(Normalize("select /*+ ", "ON"), "select ");
    assert_eq!(NormalizeKeepHint("select /*+ "), "select ");
    assert_eq!(
        Normalize(
            "select first_value(v) over (partition by p order by o range between 3 preceding and 0 following)",
            "MARKER",
        ),
        "select `first_value` ( `v` ) `over` ( partition by `p` order by `o` range between ‹3› preceding and ‹0› following )",
    );
}

/// Keywords 表保留 reserved/unreserved/tidb 分区与 Reserved 标志。
#[test]
fn generated_keywords_keep_go_sections_and_reserved_flags() {
    let select = Keywords
        .iter()
        .find(|keyword| keyword.Word == "SELECT")
        .unwrap();
    assert!(select.Reserved);
    assert_eq!(select.Section, "reserved");

    let account = Keywords
        .iter()
        .find(|keyword| keyword.Word == "ACCOUNT")
        .unwrap();
    assert!(!account.Reserved);
    assert_eq!(account.Section, "unreserved");

    let tidb = Keywords
        .iter()
        .find(|keyword| keyword.Word == "TIDB")
        .unwrap();
    assert!(!tidb.Reserved);
    assert_eq!(tidb.Section, "tidb");
}

/// 完整复跑 Go 通用规范化与 binding 专用用例表。
#[test]
fn all_go_normalize_cases_match() {
    let generic = [
        ("select _utf8mb4'123'", "select (_charset) ?"),
        (
            "select * from b where id in (_utf8mb4'123')",
            "select * from `b` where `id` in ( (_charset) ? )",
        ),
        (
            "select * from b where id in (_utf8mb4'123', _binary'34')",
            "select * from `b` where `id` in ( ... )",
        ),
        (
            "select * from b where id in (_utf8mb4'123', _binary'34', _binary'56')",
            "select * from `b` where `id` in ( ... )",
        ),
        ("SELECT 1", "select ?"),
        ("select null", "select ?"),
        (r"select \N", "select ?"),
        ("SELECT `null`", "select `null`"),
        (
            "select * from b where id = 1",
            "select * from `b` where `id` = ?",
        ),
        (
            "select 1 from b where id in (1, 3, '3', 1, 2, 3, 4)",
            "select ? from `b` where `id` in ( ... )",
        ),
        (
            "select 1 from b where id in (1, a, 4)",
            "select ? from `b` where `id` in ( ? , `a` , ? )",
        ),
        ("select 1 from b order by 2", "select ? from `b` order by 2"),
        ("select /*+ a hint */ 1", "select ?"),
        ("select /* a hint */ 1", "select ?"),
        ("select truncate(1, 2)", "select truncate ( ... )"),
        (
            "select -1 + - 2 + b - c + 0.2 + (-2) from c where d in (1, -2, +3)",
            "select ? + ? + `b` - `c` + ? + ( ? ) from `c` where `d` in ( ... )",
        ),
        (
            "select * from t where a <= -1 and b < -2 and c = -3 and c > -4 and c >= -5 and e is 1",
            "select * from `t` where `a` <= ? and `b` < ? and `c` = ? and `c` > ? and `c` >= ? and `e` is ?",
        ),
        (
            "select count(a), b from t group by 2",
            "select count ( `a` ) , `b` from `t` group by 2",
        ),
        (
            "select count(a), b, c from t group by 2, 3",
            "select count ( `a` ) , `b` , `c` from `t` group by 2 , 3",
        ),
        (
            "select count(a), b, c from t group by (2, 3)",
            "select count ( `a` ) , `b` , `c` from `t` group by ( 2 , 3 )",
        ),
        (
            "select a, b from t order by 1, 2",
            "select `a` , `b` from `t` order by 1 , 2",
        ),
        ("select count(*) from t", "select count ( ? ) from `t`"),
        ("select * from t Force Index(kk)", "select * from `t`"),
        ("select * from t USE Index(kk)", "select * from `t`"),
        ("select * from t Ignore Index(kk)", "select * from `t`"),
        (
            "select * from t1 straight_join t2 on t1.id=t2.id",
            "select * from `t1` join `t2` on `t1` . `id` = `t2` . `id`",
        ),
        ("select * from `table`", "select * from `table`"),
        ("select * from `30`", "select * from `30`"),
        ("select * from `select`", "select * from `select`"),
        ("select * from 🥳", "select * from `🥳`"),
        (
            "select * from t ignore index(",
            "select * from `t` ignore index",
        ),
        ("select /*+ ", "select "),
        ("select 1 / 2", "select ? / ?"),
        (
            "select * from t where a = 40 limit ?, ?",
            "select * from `t` where `a` = ? limit ...",
        ),
        (
            "select * from t where a > ?",
            "select * from `t` where `a` > ?",
        ),
        ("select @a=b from t", "select @a = `b` from `t`"),
        ("select * from `table", "select * from"),
        (
            "Select * from t where (i, j) in ((1,1), (2,2))",
            "select * from `t` where ( `i` , `j` ) in ( ( ... ) )",
        ),
        (
            "insert into t values (1,1), (2,2)",
            "insert into `t` values ( ... )",
        ),
        (
            "insert into t values (1), (2)",
            "insert into `t` values ( ... )",
        ),
        ("insert into t values (1)", "insert into `t` values ( ? )"),
    ];
    for (input, expected) in generic {
        assert_eq!(Normalize(input, "ON"), expected, "{input}");
    }

    let binding = [
        (
            "select * from t where a in (1)",
            "select * from `t` where `a` in ( ... )",
        ),
        (
            "select * from t where (a, b) in ((1, 1))",
            "select * from `t` where ( `a` , `b` ) in ( ( ... ) )",
        ),
        (
            "select * from t where (a, b) in ((1, 1), (2, 2))",
            "select * from `t` where ( `a` , `b` ) in ( ( ... ) )",
        ),
        (
            "select * from t where a in(1, 2)",
            "select * from `t` where `a` in ( ... )",
        ),
        (
            "select * from t where a in(1, 2, 3)",
            "select * from `t` where `a` in ( ... )",
        ),
    ];
    for (input, expected) in binding {
        let normalized = NormalizeForBinding(input, false);
        assert_eq!(normalized, expected, "{input}");
        let (normalized_with_digest, digest) = NormalizeDigestForBinding(input);
        assert_eq!(normalized_with_digest, normalized, "{input}");
        assert_eq!(digest, DigestNormalized(&normalized), "{input}");
    }
}

#[test]
/// 数字扫描只允许指数符号，不能吞掉后续算术运算符。
fn numeric_literals_do_not_consume_following_arithmetic_like_go() {
    for (input, expected) in [
        ("select 1-2", "select ? - ?"),
        ("select 1+2", "select ? + ?"),
        ("select 1e-2+3", "select ? + ?"),
        ("select 1e+2-3", "select ? - ?"),
        ("select .1+2", "select ? + ?"),
        ("select @@global.autocommit", "select @@global.autocommit"),
        ("select 12abc", "select `12abc`"),
        ("select 1e+", "select `1e` +"),
        (
            "select @@SESSION.`autocommit`",
            "select @@session.autocommit",
        ),
        ("select @'a'", "select @a"),
        ("select @`a`", "select @a"),
        ("select 1--2", "select ? - ?"),
        ("select 1-- 2", "select ?"),
        ("select 1---2", "select ? - - ?"),
        ("select 0xg", "select `0xg`"),
        ("select 0x1g", "select `0x1g`"),
        ("select 0x", "select `0x`"),
        ("select 0b", "select `0b`"),
        ("select 0b2", "select `0b2`"),
        ("select 1.2abc", "select ? `abc`"),
    ] {
        assert_eq!(Normalize(input, "ON"), expected, "{input}");
    }
}

/// MARKER 脱敏与 KeepHint 路径的 Go 用例对照。
#[test]
fn all_go_marker_and_keep_hint_cases_match() {
    let marker = [
        (
            "select * from t where a in (1)",
            "select * from `t` where `a` in ( ‹1› )",
        ),
        (
            "select * from t where a in (1, 3)",
            "select * from `t` where `a` in ( ‹1› , ‹3› )",
        ),
        (
            "select ? from b order by 2",
            "select ? from `b` order by ‹2›",
        ),
        (
            "select ? from b order by 2 limit 10 offset 10",
            "select ? from `b` order by ‹2› limit ‹10› offset ‹10›",
        ),
        (
            "with recursive cte1(c1) as (select c1 from t1 union select c1 + 1 c1 from cte1 limit 100 offset 100) select * from cte1;",
            "with recursive `cte1` ( `c1` ) as ( select `c1` from `t1` union select `c1` + ‹1› `c1` from `cte1` limit ‹100› offset ‹100› ) select * from `cte1`",
        ),
        (
            "select *, first_value(v) over (partition by p order by o range between 3 preceding and 0 following) as a from test.first_range",
            "select * , `first_value` ( `v` ) `over` ( partition by `p` order by `o` range between ‹3› preceding and ‹0› following ) as `a` from `test` . `first_range`",
        ),
    ];
    for (input, expected) in marker {
        assert_eq!(Normalize(input, "MARKER"), expected, "{input}");
    }

    let keep_hint = [
        ("select _utf8mb4'123'", "select (_charset) ?"),
        ("SELECT 1", "select ?"),
        ("select null", "select ?"),
        (r"select \N", "select ?"),
        ("SELECT `null`", "select `null`"),
        (
            "select * from b where id = 1",
            "select * from `b` where `id` = ?",
        ),
        (
            "select 1 from b where id in (1, 3, '3', 1, 2, 3, 4)",
            "select ? from `b` where `id` in ( ... )",
        ),
        (
            "select 1 from b where id in (1, a, 4)",
            "select ? from `b` where `id` in ( ? , `a` , ? )",
        ),
        ("select 1 from b order by 2", "select ? from `b` order by 2"),
        ("select /*+ a hint */ 1", "select /*+ a hint */ ?"),
        ("select /* a hint */ 1", "select ?"),
        ("select truncate(1, 2)", "select truncate ( ... )"),
        (
            "select -1 + - 2 + b - c + 0.2 + (-2) from c where d in (1, -2, +3)",
            "select ? + ? + `b` - `c` + ? + ( ? ) from `c` where `d` in ( ... )",
        ),
        (
            "select * from t where a <= -1 and b < -2 and c = -3 and c > -4 and c >= -5 and e is 1",
            "select * from `t` where `a` <= ? and `b` < ? and `c` = ? and `c` > ? and `c` >= ? and `e` is ?",
        ),
        (
            "select count(a), b from t group by 2",
            "select count ( `a` ) , `b` from `t` group by 2",
        ),
        (
            "select count(a), b, c from t group by 2, 3",
            "select count ( `a` ) , `b` , `c` from `t` group by 2 , 3",
        ),
        (
            "select count(a), b, c from t group by (2, 3)",
            "select count ( `a` ) , `b` , `c` from `t` group by ( 2 , 3 )",
        ),
        (
            "select a, b from t order by 1, 2",
            "select `a` , `b` from `t` order by 1 , 2",
        ),
        ("select count(*) from t", "select count ( ? ) from `t`"),
        (
            "select * from t Force Index(kk)",
            "select * from `t` force index ( `kk` )",
        ),
        (
            "select * from t USE Index(kk)",
            "select * from `t` use index ( `kk` )",
        ),
        (
            "select * from t Ignore Index(kk)",
            "select * from `t` ignore index ( `kk` )",
        ),
        (
            "select * from t1 straight_join t2 on t1.id=t2.id",
            "select * from `t1` straight_join `t2` on `t1` . `id` = `t2` . `id`",
        ),
        ("select * from `table`", "select * from `table`"),
        ("select * from `30`", "select * from `30`"),
        ("select * from `select`", "select * from `select`"),
        ("select * from 🥳", "select * from `🥳`"),
        (
            "select * from t ignore index(",
            "select * from `t` ignore index (",
        ),
        ("select /*+ ", "select "),
        ("select 1 / 2", "select ? / ?"),
        (
            "select * from t where a = 40 limit ?, ?",
            "select * from `t` where `a` = ? limit ...",
        ),
        (
            "select * from t where a > ?",
            "select * from `t` where `a` > ?",
        ),
        ("select @a=b from t", "select @a = `b` from `t`"),
        ("select * from `table", "select * from"),
    ];
    for (input, expected) in keep_hint {
        assert_eq!(NormalizeKeepHint(input), expected, "{input}");
    }
}

/// Go `TestNormalizeDigest` 的固定输出，可防止 Normalize 与哈希同时漂移而互相掩盖。
#[test]
fn normalized_digest_matches_go_fixed_value() {
    let sql = "select 1 from b where id in (1, 3, '3', 1, 2, 3, 4)";
    let expected_normalized = "select ? from `b` where `id` in ( ... )";
    let expected_digest = "e1c8cc2738f596dc24f15ef8eb55e0d902910d7298983496362a7b46dbc0b310";

    let (normalized, digest) = NormalizeDigest(sql);
    assert_eq!(normalized, expected_normalized);
    assert_eq!(digest.String(), expected_digest);
    assert_eq!(Normalize(sql, "ON"), expected_normalized);
    assert_eq!(
        DigestNormalized(expected_normalized).String(),
        expected_digest
    );
}

/// Go 的 DigestHash 等价类与非等价类全部复现。
#[test]
fn digest_hash_equivalence_groups_match_go() {
    let equivalent_groups: &[&[&str]] = &[
        &[
            "select * from b where id = 1",
            "select * from b where id = '1'",
            "select * from b where id =2",
        ],
        &[
            "select 2 from b, c where c.id > 1",
            "select 4 from b, c where c.id > 23",
        ],
        &["Select 3", "select 1"],
        &[
            "Select * from t where (i, j) in ((1,1), (2,2))",
            "select * from t where (i, j) in ((1,1), (2,2), (3,3))",
        ],
        &[
            "insert into t values (1,1)",
            "insert into t values (1,1), (2,2)",
        ],
    ];
    for group in equivalent_groups {
        let expected = DigestHash(group[0]);
        for sql in &group[1..] {
            assert_eq!(DigestHash(sql), expected, "{sql}");
        }
    }

    let distinct = [
        "select * from b where id = 1",
        "select a from b where id = 1",
        "select * from d where bid =1",
    ];
    let first = DigestHash(distinct[0]);
    for sql in &distinct[1..] {
        assert_ne!(DigestHash(sql), first, "{sql}");
    }
}

/// `/*T!` / `/*T![...]` 特殊注释展开与未知 feature 丢弃行为。
#[test]
fn special_comment_expansion_stops_at_scanner_eof() {
    assert_eq!(
        NormalizeKeepHint("/*T![auto_rand] auto_random(5) */"),
        "auto_random ( ? )"
    );
    assert_eq!(
        NormalizeKeepHint("/*T![auto_rand, clustered_index] auto_random(5) */"),
        "auto_random ( ? )"
    );
    assert_eq!(
        NormalizeKeepHint("/*T![unsupported_feature] unsupported(123) */"),
        ""
    );
    assert_eq!(
        NormalizeKeepHint("/*T! auto_random(5) */"),
        "auto_random ( ? )"
    );
}
