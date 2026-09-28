// Copyright 2026 AsterSQL.

// Domain 中 ANALYZE TABLE SQL 识别逻辑的单元测试。
//
// `ANALYZE TABLE` 用于收集表的统计信息（statistics），供优化器估算代价。
// 本测试核对 `is_analyze_table_sql` 对语句前缀的判定是否与 TiDB 一致：
// 仅匹配以 analyze table 开头的语句，排除普通查询与 explain analyze 等变体。

/// 验证 analyze table 前缀识别与 TiDB 语句前缀规则一致。
#[test]
fn canonical_analyze_sql_detection_matches_tidb_statement_prefixes() {
    use crate::domain::is_analyze_table_sql;
    // 正例：大小写、空白与库名修饰均应识别为 ANALYZE TABLE。
    for sql in [
        "analyze table t",
        " ANALYZE TABLE db.t",
        "\nAnalyze Table `t`",
        "/* axxxx */ analyze table test.t",
        "/*\n\t\t/*> this is a\n\t\t/*> multiple-line comment\n\t\t/*> */ analyze table test.t",
        "/*+ hint */ analyze table test.t",
        "/*+ hint */analyze table test.t",
    ] {
        assert!(is_analyze_table_sql(sql), "{sql}");
    }
    // 反例：普通 SELECT、analyze index、explain analyze 均不应匹配。
    for sql in ["select 1", "analyze index t", "explain analyze table t"] {
        assert!(!is_analyze_table_sql(sql), "{sql}");
    }
}
