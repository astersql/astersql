// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// digester 的 Go 同路径单元测试与基准形状。
//
// 覆盖 SQL normalize、digest hash、digest bytes，以及 hex 编码基准循环形状；
// 通过本地 require/testing/fmt 适配层对齐 Go testify 断言风格。

// 覆盖 SQL normalize、digest hash、digest bytes 和基准测试形状。
use parser::digester_impl as parser;
use sha2::{Digest as _, Sha256};

/// 对齐 Go require 包的断言辅助。
#[allow(non_snake_case)]
mod require {
    use std::fmt::Debug;
    /// 相等断言。
    pub fn Equal<T: Debug + PartialEq<U>, U: Debug>(expected: T, actual: U) {
        assert_eq!(expected, actual);
    }
    /// 带格式参数的相等断言（格式参数忽略，仅保留形状）。
    pub fn Equalf<T: Debug + PartialEq<U>, U: Debug, F, A>(expected: T, actual: U, _: F, _: A) {
        assert_eq!(expected, actual);
    }
    /// 不等断言。
    pub fn NotEqual<T: Debug + PartialEq<U>, U: Debug>(unexpected: T, actual: U) {
        assert_ne!(unexpected, actual);
    }
}

/// 对齐 Go testing.B 的最小基准桩。
mod testing {
    #[allow(non_snake_case)]
    /// 基准状态：`N` 为迭代次数。
    pub struct B {
        /// 迭代次数。
        pub N: usize,
    }
    #[allow(non_snake_case)]
    impl B {
        /// 重置计时器（桩实现为空）。
        pub fn ResetTimer(&mut self) {}
    }
}

/// 对齐 Go fmt.Sprintf("%x", ...) 的十六进制编码辅助。
#[allow(non_snake_case)]
mod fmt {
    /// 将字节编码为十六进制字符串（忽略格式串）。
    pub fn Sprintf(_: &str, bytes: Vec<u8>) -> String {
        hex::encode(bytes)
    }
}

/// Normalize 用例：输入 SQL 与期望规范文本。
struct NormalizeCase {
    /// 输入 SQL。
    input: &'static str,
    /// 期望规范化结果。
    expect: &'static str,
}

/// NormalizeDigest 用例：SQL、规范文本与固定 digest。
struct DigestCase {
    /// 输入 SQL。
    sql: &'static str,
    /// 期望规范化结果。
    normalized: &'static str,
    /// 期望 digest 十六进制。
    digest: &'static str,
}

// test_normalize 对应 Go 的 TestNormalize。
// 第一组保留通用归一化规则；第二组保留 binding 专用规则，并分别校验 normalize 与 digest 的一致性。
/// 校验通用规范化与 binding 专用规则，并交叉验证 NormalizeDigest。
#[test]
fn test_normalize() {
    let tests_for_generic_normalization_rules = vec![
        NormalizeCase {
            input: "select _utf8mb4'123'",
            expect: "select (_charset) ?",
        },
        NormalizeCase {
            input: "select * from b where id in (_utf8mb4'123')",
            expect: "select * from `b` where `id` in ( (_charset) ? )",
        },
        NormalizeCase {
            input: "select * from b where id in (_utf8mb4'123', _binary'34')",
            expect: "select * from `b` where `id` in ( ... )",
        },
        NormalizeCase {
            input: "select * from b where id in (_utf8mb4'123', _binary'34', _binary'56')",
            expect: "select * from `b` where `id` in ( ... )",
        },
        NormalizeCase {
            input: "SELECT 1",
            expect: "select ?",
        },
        NormalizeCase {
            input: "select null",
            expect: "select ?",
        },
        NormalizeCase {
            input: r"select \N",
            expect: "select ?",
        },
        NormalizeCase {
            input: "SELECT `null`",
            expect: "select `null`",
        },
        NormalizeCase {
            input: "select * from b where id = 1",
            expect: "select * from `b` where `id` = ?",
        },
        NormalizeCase {
            input: "select 1 from b where id in (1, 3, '3', 1, 2, 3, 4)",
            expect: "select ? from `b` where `id` in ( ... )",
        },
        NormalizeCase {
            input: "select 1 from b where id in (1, a, 4)",
            expect: "select ? from `b` where `id` in ( ? , `a` , ? )",
        },
        NormalizeCase {
            input: "select 1 from b order by 2",
            expect: "select ? from `b` order by 2",
        },
        NormalizeCase {
            input: "select /*+ a hint */ 1",
            expect: "select ?",
        },
        NormalizeCase {
            input: "select /* a hint */ 1",
            expect: "select ?",
        },
        NormalizeCase {
            input: "select truncate(1, 2)",
            expect: "select truncate ( ... )",
        },
        NormalizeCase {
            input: "select -1 + - 2 + b - c + 0.2 + (-2) from c where d in (1, -2, +3)",
            expect: "select ? + ? + `b` - `c` + ? + ( ? ) from `c` where `d` in ( ... )",
        },
        NormalizeCase {
            input: "select * from t where a <= -1 and b < -2 and c = -3 and c > -4 and c >= -5 and e is 1",
            expect: "select * from `t` where `a` <= ? and `b` < ? and `c` = ? and `c` > ? and `c` >= ? and `e` is ?",
        },
        NormalizeCase {
            input: "select count(a), b from t group by 2",
            expect: "select count ( `a` ) , `b` from `t` group by 2",
        },
        NormalizeCase {
            input: "select count(a), b, c from t group by 2, 3",
            expect: "select count ( `a` ) , `b` , `c` from `t` group by 2 , 3",
        },
        NormalizeCase {
            input: "select count(a), b, c from t group by (2, 3)",
            expect: "select count ( `a` ) , `b` , `c` from `t` group by ( 2 , 3 )",
        },
        NormalizeCase {
            input: "select a, b from t order by 1, 2",
            expect: "select `a` , `b` from `t` order by 1 , 2",
        },
        NormalizeCase {
            input: "select count(*) from t",
            expect: "select count ( ? ) from `t`",
        },
        NormalizeCase {
            input: "select * from t Force Index(kk)",
            expect: "select * from `t`",
        },
        NormalizeCase {
            input: "select * from t USE Index(kk)",
            expect: "select * from `t`",
        },
        NormalizeCase {
            input: "select * from t Ignore Index(kk)",
            expect: "select * from `t`",
        },
        NormalizeCase {
            input: "select * from t1 straight_join t2 on t1.id=t2.id",
            expect: "select * from `t1` join `t2` on `t1` . `id` = `t2` . `id`",
        },
        NormalizeCase {
            input: "select * from `table`",
            expect: "select * from `table`",
        },
        NormalizeCase {
            input: "select * from `30`",
            expect: "select * from `30`",
        },
        NormalizeCase {
            input: "select * from `select`",
            expect: "select * from `select`",
        },
        NormalizeCase {
            input: "select * from 🥳",
            expect: "select * from `🥳`",
        },
        // Go 注释：语法错误由 parser 检查，但 normalize 不应进入死循环。
        NormalizeCase {
            input: "select * from t ignore index(",
            expect: "select * from `t` ignore index",
        },
        NormalizeCase {
            input: "select /*+ ",
            expect: "select ",
        },
        NormalizeCase {
            input: "select 1 / 2",
            expect: "select ? / ?",
        },
        NormalizeCase {
            input: "select * from t where a = 40 limit ?, ?",
            expect: "select * from `t` where `a` = ? limit ...",
        },
        NormalizeCase {
            input: "select * from t where a > ?",
            expect: "select * from `t` where `a` > ?",
        },
        NormalizeCase {
            input: "select @a=b from t",
            expect: "select @a = `b` from `t`",
        },
        NormalizeCase {
            input: "select * from `table",
            expect: "select * from",
        },
        NormalizeCase {
            input: "Select * from t where (i, j) in ((1,1), (2,2))",
            expect: "select * from `t` where ( `i` , `j` ) in ( ( ... ) )",
        },
        NormalizeCase {
            input: "insert into t values (1,1), (2,2)",
            expect: "insert into `t` values ( ... )",
        },
        NormalizeCase {
            input: "insert into t values (1), (2)",
            expect: "insert into `t` values ( ... )",
        },
        NormalizeCase {
            input: "insert into t values (1)",
            expect: "insert into `t` values ( ? )",
        },
    ];
    for test in tests_for_generic_normalization_rules {
        let normalized = parser::Normalize(test.input, "ON");
        let digest = parser::DigestNormalized(&normalized);
        require::Equal(test.expect, normalized.as_str());

        // Go 同时走 NormalizeDigest，确保返回的 normalized 与单独 Normalize 一致，digest 字符串也一致。
        let (normalized2, digest2) = parser::NormalizeDigest(test.input);
        require::Equal(normalized, normalized2);
        require::Equalf(digest.String(), digest2.String(), "%+v", test);
    }

    let tests_for_binding_specific_rules = vec![
        // Binding specific rules: IN (Lit) => IN ( ... ) #44298
        NormalizeCase {
            input: "select * from t where a in (1)",
            expect: "select * from `t` where `a` in ( ... )",
        },
        NormalizeCase {
            input: "select * from t where (a, b) in ((1, 1))",
            expect: "select * from `t` where ( `a` , `b` ) in ( ( ... ) )",
        },
        NormalizeCase {
            input: "select * from t where (a, b) in ((1, 1), (2, 2))",
            expect: "select * from `t` where ( `a` , `b` ) in ( ( ... ) )",
        },
        NormalizeCase {
            input: "select * from t where a in(1, 2)",
            expect: "select * from `t` where `a` in ( ... )",
        },
        NormalizeCase {
            input: "select * from t where a in(1, 2, 3)",
            expect: "select * from `t` where `a` in ( ... )",
        },
    ];
    for test in tests_for_binding_specific_rules {
        let normalized = parser::NormalizeForBinding(test.input, false);
        let digest = parser::DigestNormalized(&normalized);
        require::Equal(test.expect, normalized.as_str());

        let (normalized2, digest2) = parser::NormalizeDigestForBinding(test.input);
        require::Equal(normalized, normalized2);
        require::Equalf(digest.String(), digest2.String(), "%+v", test);
    }
}

// test_normalize_redact 对应 Go 的 TestNormalizeRedact，保留 MARKER 模式下的字面量脱敏文本。
/// 校验 MARKER 模式下字面量包在 ‹› 中的脱敏文本。
#[test]
fn test_normalize_redact() {
    let cases = vec![
        NormalizeCase {
            input: "select * from t where a in (1)",
            expect: "select * from `t` where `a` in ( ‹1› )",
        },
        NormalizeCase {
            input: "select * from t where a in (1, 3)",
            expect: "select * from `t` where `a` in ( ‹1› , ‹3› )",
        },
        NormalizeCase {
            input: "select ? from b order by 2",
            expect: "select ? from `b` order by ‹2›",
        },
        NormalizeCase {
            input: "select ? from b order by 2 limit 10 offset 10",
            expect: "select ? from `b` order by ‹2› limit ‹10› offset ‹10›",
        },
        NormalizeCase {
            input: "with recursive cte1(c1) as (select c1 from t1 union select c1 + 1 c1 from cte1 limit 100 offset 100) select * from cte1;",
            expect: "with recursive `cte1` ( `c1` ) as ( select `c1` from `t1` union select `c1` + ‹1› `c1` from `cte1` limit ‹100› offset ‹100› ) select * from `cte1`",
        },
        NormalizeCase {
            input: "select *, first_value(v) over (partition by p order by o range between 3 preceding and 0 following) as a from test.first_range",
            expect: "select * , `first_value` ( `v` ) `over` ( partition by `p` order by `o` range between ‹3› preceding and ‹0› following ) as `a` from `test` . `first_range`",
        },
    ];

    for c in cases {
        let normalized = parser::Normalize(c.input, "MARKER");
        require::Equal(c.expect, normalized);
    }
}

// test_normalize_keep_hint 对应 Go 的 TestNormalizeKeepHint。
// 与普通 Normalize 不同，该路径保留 optimizer hint 和 index hint 的文本形状。
/// 校验 KeepHint 路径保留优化器 hint / index hint。
#[test]
fn test_normalize_keep_hint() {
    let tests = vec![
        NormalizeCase {
            input: "select _utf8mb4'123'",
            expect: "select (_charset) ?",
        },
        NormalizeCase {
            input: "SELECT 1",
            expect: "select ?",
        },
        NormalizeCase {
            input: "select null",
            expect: "select ?",
        },
        NormalizeCase {
            input: r"select \N",
            expect: "select ?",
        },
        NormalizeCase {
            input: "SELECT `null`",
            expect: "select `null`",
        },
        NormalizeCase {
            input: "select * from b where id = 1",
            expect: "select * from `b` where `id` = ?",
        },
        NormalizeCase {
            input: "select 1 from b where id in (1, 3, '3', 1, 2, 3, 4)",
            expect: "select ? from `b` where `id` in ( ... )",
        },
        NormalizeCase {
            input: "select 1 from b where id in (1, a, 4)",
            expect: "select ? from `b` where `id` in ( ? , `a` , ? )",
        },
        NormalizeCase {
            input: "select 1 from b order by 2",
            expect: "select ? from `b` order by 2",
        },
        NormalizeCase {
            input: "select /*+ a hint */ 1",
            expect: "select /*+ a hint */ ?",
        },
        NormalizeCase {
            input: "select /* a hint */ 1",
            expect: "select ?",
        },
        NormalizeCase {
            input: "select truncate(1, 2)",
            expect: "select truncate ( ... )",
        },
        NormalizeCase {
            input: "select -1 + - 2 + b - c + 0.2 + (-2) from c where d in (1, -2, +3)",
            expect: "select ? + ? + `b` - `c` + ? + ( ? ) from `c` where `d` in ( ... )",
        },
        NormalizeCase {
            input: "select * from t where a <= -1 and b < -2 and c = -3 and c > -4 and c >= -5 and e is 1",
            expect: "select * from `t` where `a` <= ? and `b` < ? and `c` = ? and `c` > ? and `c` >= ? and `e` is ?",
        },
        NormalizeCase {
            input: "select count(a), b from t group by 2",
            expect: "select count ( `a` ) , `b` from `t` group by 2",
        },
        NormalizeCase {
            input: "select count(a), b, c from t group by 2, 3",
            expect: "select count ( `a` ) , `b` , `c` from `t` group by 2 , 3",
        },
        NormalizeCase {
            input: "select count(a), b, c from t group by (2, 3)",
            expect: "select count ( `a` ) , `b` , `c` from `t` group by ( 2 , 3 )",
        },
        NormalizeCase {
            input: "select a, b from t order by 1, 2",
            expect: "select `a` , `b` from `t` order by 1 , 2",
        },
        NormalizeCase {
            input: "select count(*) from t",
            expect: "select count ( ? ) from `t`",
        },
        NormalizeCase {
            input: "select * from t Force Index(kk)",
            expect: "select * from `t` force index ( `kk` )",
        },
        NormalizeCase {
            input: "select * from t USE Index(kk)",
            expect: "select * from `t` use index ( `kk` )",
        },
        NormalizeCase {
            input: "select * from t Ignore Index(kk)",
            expect: "select * from `t` ignore index ( `kk` )",
        },
        NormalizeCase {
            input: "select * from t1 straight_join t2 on t1.id=t2.id",
            expect: "select * from `t1` straight_join `t2` on `t1` . `id` = `t2` . `id`",
        },
        NormalizeCase {
            input: "select * from `table`",
            expect: "select * from `table`",
        },
        NormalizeCase {
            input: "select * from `30`",
            expect: "select * from `30`",
        },
        NormalizeCase {
            input: "select * from `select`",
            expect: "select * from `select`",
        },
        NormalizeCase {
            input: "select * from 🥳",
            expect: "select * from `🥳`",
        },
        // Go 注释：语法错误由 parser 检查，但 keep-hint normalize 同样不应死循环。
        NormalizeCase {
            input: "select * from t ignore index(",
            expect: "select * from `t` ignore index (",
        },
        NormalizeCase {
            input: "select /*+ ",
            expect: "select ",
        },
        NormalizeCase {
            input: "select 1 / 2",
            expect: "select ? / ?",
        },
        NormalizeCase {
            input: "select * from t where a = 40 limit ?, ?",
            expect: "select * from `t` where `a` = ? limit ...",
        },
        NormalizeCase {
            input: "select * from t where a > ?",
            expect: "select * from `t` where `a` > ?",
        },
        NormalizeCase {
            input: "select @a=b from t",
            expect: "select @a = `b` from `t`",
        },
        NormalizeCase {
            input: "select * from `table",
            expect: "select * from",
        },
    ];
    for test in tests {
        let normalized = parser::NormalizeKeepHint(test.input);
        require::Equal(test.expect, normalized);
    }
}

// test_normalize_digest 对应 Go 的 TestNormalizeDigest：固定 SQL 应得到固定 digest。
/// 固定 SQL 应得到固定 digest，且与 Normalize+DigestNormalized 一致。
#[test]
fn test_normalize_digest() {
    let tests = vec![DigestCase {
        sql: "select 1 from b where id in (1, 3, '3', 1, 2, 3, 4)",
        normalized: "select ? from `b` where `id` in ( ... )",
        digest: "e1c8cc2738f596dc24f15ef8eb55e0d902910d7298983496362a7b46dbc0b310",
    }];
    for test in tests {
        let (normalized, digest) = parser::NormalizeDigest(test.sql);
        require::Equal(test.normalized, normalized);
        require::Equal(test.digest, digest.String());

        // Go 继续用 Normalize + DigestNormalized 重算一次，确保两条 API 路径输出一致。
        let normalized = parser::Normalize(test.sql, "ON");
        let digest = parser::DigestNormalized(&normalized);
        require::Equal(test.normalized, normalized);
        require::Equal(test.digest, digest.String());
    }
}

// test_digest_hash_eq_for_simple_sql 对应 Go 的 TestDigestHashEqForSimpleSQL。
// 每个分组内 SQL 字面量不同但归一化结构相同，digest hash 应保持一致。
/// 同结构不同字面量的 SQL 应共享同一 digest。
#[test]
fn test_digest_hash_eq_for_simple_sql() {
    let sql_groups = vec![
        vec![
            "select * from b where id = 1",
            "select * from b where id = '1'",
            "select * from b where id =2",
        ],
        vec![
            "select 2 from b, c where c.id > 1",
            "select 4 from b, c where c.id > 23",
        ],
        vec!["Select 3", "select 1"],
        vec![
            "Select * from t where (i, j) in ((1,1), (2,2))",
            "select * from t where (i, j) in ((1,1), (2,2), (3,3))",
        ],
        vec![
            "insert into t values (1,1)",
            "insert into t values (1,1), (2,2)",
        ],
    ];
    for sql_group in sql_groups {
        let mut d = String::new();
        for sql in sql_group {
            let dig = parser::DigestHash(sql);
            if d.is_empty() {
                d = dig.String().to_owned();
                continue;
            }
            require::Equal(dig.String(), d.as_str());
        }
    }
}

// test_digest_hash_not_eq_for_simple_sql 对应 Go 的 TestDigestHashNotEqForSimpleSQL。
// 分组内结构或标识符不同，后续 digest 不应等于首个 digest。
/// 结构或标识符不同的 SQL digest 应互不相同。
#[test]
fn test_digest_hash_not_eq_for_simple_sql() {
    let sql_groups = vec![vec![
        "select * from b where id = 1",
        "select a from b where id = 1",
        "select * from d where bid =1",
    ]];
    for sql_group in sql_groups {
        let mut d = String::new();
        for sql in sql_group {
            let dig = parser::DigestHash(sql);
            if d.is_empty() {
                d = dig.String().to_owned();
                continue;
            }
            require::NotEqual(dig.String(), d.as_str());
        }
    }
}

// test_gen_digest 对应 Go 的 TestGenDigest：从 sha256 bytes 构造 Digest，并校验字符串与 bytes 视图。
/// 从 sha256 bytes 构造 Digest，校验 String/Bytes 视图（含空摘要）。
#[test]
fn test_gen_digest() {
    let hash = gen_rand_digest("abc");
    let digest = parser::NewDigest(hash.clone());
    require::Equal(hex::encode(&hash), digest.String());
    require::Equal(hash, digest.Bytes());
    let digest = parser::NewDigest(Vec::new());
    require::Equal("", digest.String());
    require::Equal(Vec::<u8>::new(), digest.Bytes());
}

// gen_rand_digest 对应 Go 的 genRandDigest，返回输入字符串的 sha256 sum。
/// 返回输入字符串的 SHA-256 摘要字节。
fn gen_rand_digest(str_: &str) -> Vec<u8> {
    Sha256::digest(str_.as_bytes()).to_vec()
}

// benchmark_digest_hex_encode 对应 Go 的 BenchmarkDigestHexEncode。
// 仅保留 benchmark 循环形状；没有接入 Rust bench harness。
/// hex::encode 基准循环形状（未接入 Rust bench harness）。
fn benchmark_digest_hex_encode(b: &mut testing::B) {
    let digest1 = gen_rand_digest("abc");
    b.ResetTimer();
    for _ in 0..b.N {
        let _ = hex::encode(digest1.clone());
    }
}

// benchmark_digest_sprintf 对应 Go 的 BenchmarkDigestSprintf。
/// fmt.Sprintf("%x") 风格基准循环形状。
fn benchmark_digest_sprintf(b: &mut testing::B) {
    let digest1 = gen_rand_digest("abc");
    b.ResetTimer();
    for _ in 0..b.N {
        fmt::Sprintf("%x", digest1.clone());
    }
}
