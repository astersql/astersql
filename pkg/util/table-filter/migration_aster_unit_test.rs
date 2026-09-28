// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// table-filter 迁移对照单测：与 Go 语义对齐的规则/解析/兼容路径。
//
// 覆盖通配与正则、列规则、引号标识符、`@file` 导入及 MySQL 复制兼容。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

#[cfg(test)]
mod tests {
    use crate::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 将 `&str` 切片转为拥有所有权的 `String` 列表。
    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    /// 校验规则顺序、glob、正则与大小写不敏感匹配。
    fn migration_table_rules_preserve_order_globs_regex_and_case() {
        let filter = Parse(strings(&["*.*", "!foo*.tmp?", "foo1.tmp1"])).unwrap();
        assert!(filter.MatchTable("else", "anything"));
        assert!(!filter.MatchTable("foo2", "tmp9"));
        assert!(filter.MatchTable("foo1", "tmp1"));

        let filter = Parse(strings(&["/^prod[0-9]+$/.*"])).unwrap();
        assert!(filter.MatchTable("prod42", "t"));
        assert!(!filter.MatchTable("xprod42", "t"));

        let filter = CaseInsensitive(Parse(strings(&["BAR.`SomeTable`"])).unwrap());
        assert!(filter.MatchTable("bar", "sometable"));
        assert!(filter.MatchSchema("BaR"));
    }

    #[test]
    /// 否定单表不否决 schema；否定 `schema.*` 则否决整个库。
    fn migration_schema_matching_matches_go_negative_table_semantics() {
        let filter = Parse(strings(&["*.*", "!foo.bar"])).unwrap();
        assert!(!filter.MatchTable("foo", "bar"));
        assert!(filter.MatchSchema("foo"));

        let filter = Parse(strings(&["*.*", "!foo.*"])).unwrap();
        assert!(!filter.MatchSchema("foo"));
    }

    #[test]
    /// 列规则大小写不敏感且后写规则优先。
    fn migration_column_rules_are_case_insensitive_and_last_rule_wins() {
        let filter = ParseColumnFilter(strings(&["*", "!Secret*", "secret_ok"])).unwrap();
        assert!(!filter.MatchColumn("SECRET_VALUE"));
        assert!(filter.MatchColumn("Secret_OK"));
        assert!(filter.MatchColumn("ordinary"));
    }

    #[test]
    /// 引号标识符与字符类语义对齐 Go。
    fn migration_quoted_identifiers_and_character_classes_match_go() {
        let filter = Parse(strings(&[
            r#""some ""quoted""".`identifiers?`"#,
            "[!a-z].[^a-z]",
        ]))
        .unwrap();
        assert!(filter.MatchTable(r#"some "quoted""#, "identifiers?"));
        assert!(filter.MatchTable("!", "^"));
        assert!(!filter.MatchTable("a", "^"));
    }

    #[test]
    /// 错误消息含源位置，并拒绝非法模式/前瞻正则。
    fn migration_parser_reports_source_location_and_rejects_bad_patterns() {
        let error = Parse(strings(&["db"])).unwrap_err().to_string();
        assert!(
            error.contains("at <cmdline>:1: wrong table pattern"),
            "{error}"
        );

        let error = ParseColumnFilter(strings(&[r"a\tb"]))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot escape a letter or number"),
            "{error}"
        );

        let error = Parse(strings(&["/^t(?=copy)$/.*"]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid pattern"), "{error}");
    }

    #[test]
    /// `@file` 导入成功路径，并拒绝递归导入。
    fn migration_imports_rules_and_rejects_recursive_imports() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("table-filter-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        let rules = dir.join("rules.txt");
        fs::write(&rules, "db?.tbl?\n!db4.tbl4\n").unwrap();

        let filter = Parse(vec![format!("@{}", rules.display())]).unwrap();
        assert!(filter.MatchTable("db1", "tbl1"));
        assert!(!filter.MatchTable("db4", "tbl4"));

        let nested = dir.join("nested.txt");
        fs::write(&nested, format!("@{}", rules.display())).unwrap();
        let error = Parse(vec![format!("@{}", nested.display())])
            .unwrap_err()
            .to_string();
        assert!(error.contains("importing filter files recursively is not allowed"));
        assert!(error.contains("nested.txt:1"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    /// IgnoreDB/IgnoreTable 黑名单优先级与兼容解析。
    fn migration_compat_filters_match_mysql_replication_precedence() {
        let rules = MySQLReplicationRules {
            DoTables: vec![],
            DoDBs: vec![],
            IgnoreTables: vec![Box::new(Table::new("db", "tmp*"))],
            IgnoreDBs: vec!["ignored*".to_owned()],
        };
        let filter = ParseMySQLReplicationRules(Some(&rules)).unwrap();
        assert!(!filter.MatchTable("ignored1", "t"));
        assert!(!filter.MatchTable("db", "tmp1"));
        assert!(filter.MatchTable("db", "kept"));
    }

    #[test]
    /// Table 显示/Clone、SchemasFilter 与 All 的基本行为。
    fn migration_compat_constructors_table_display_clone_and_all() {
        let table = Table::new("Db", "Tbl");
        assert_eq!(table.to_string(), "`Db`.`Tbl`");
        assert_eq!(table.Clone().to_string(), table.to_string());

        let schemas = NewSchemasFilter(strings(&["Db"]));
        assert!(schemas.MatchTable("Db", "anything"));
        assert!(!schemas.MatchSchema("db"));
        assert!(CaseInsensitive(schemas).MatchSchema("DB"));

        let all = CaseInsensitive(All());
        assert!(all.MatchTable("any", "table"));
        assert!(all.MatchSchema("any"));
    }
}
