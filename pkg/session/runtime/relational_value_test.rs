// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use super::relational_like;

#[test]
fn like_underscore_matches_one_unicode_character() {
    assert!(relational_like("_", "中"));
    assert!(relational_like("a_c", "a中c"));
    assert!(!relational_like("__", "中"));
}

#[test]
fn like_backslash_escapes_wildcards() {
    assert!(relational_like(r"a\_c", "a_c"));
    assert!(relational_like(r"a\%c", "a%c"));
    assert!(!relational_like(r"a\_c", "a中c"));
}

#[test]
fn pg_core_int4_explicit_cast() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for sql in [
        "SELECT 2147483648::int4",
        "SELECT 9223372036854775808::int4",
        "SELECT 7 WHERE '12x'::int4 = 12",
        "SELECT CAST(-2147483649 AS integer)",
        "SELECT '12x'::int4",
        "SELECT ''::int4",
    ] {
        assert!(pg.execute(sql).is_err(), "must reject {sql}");
    }
    for (sql, expected) in [
        ("SELECT '-2147483648'::int4", "-2147483648"),
        ("SELECT CAST(' +00042 ' AS int)", "42"),
        ("SELECT 2147483647::int4", "2147483647"),
    ] {
        assert_eq!(
            pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap(),
            vec![expected]
        );
    }
}

#[test]
fn pg_core_int4_explicit_cast_numeric_columns_unsupported() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_core_int4_numeric (id INT PRIMARY KEY, n DECIMAL(8,2), f DOUBLE)")
        .unwrap();
    struct Cleanup<'a>(&'a ConcreteSession);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.execute("DROP TABLE pg_core_int4_numeric");
        }
    }
    let _cleanup = Cleanup(&pg);
    pg.execute("INSERT INTO pg_core_int4_numeric VALUES (1, 1.00, 1.0)")
        .unwrap();
    for sql in [
        "SELECT n::int4 FROM pg_core_int4_numeric",
        "SELECT f::int4 FROM pg_core_int4_numeric",
        "SELECT n::int4 FROM pg_core_int4_numeric WHERE id = 0",
        "SELECT f::int4 FROM pg_core_int4_numeric WHERE id = 0",
    ] {
        let error = match pg.execute(sql) {
            Ok(_) => panic!("numeric input must be unsupported: {sql}"),
            Err(error) => error,
        };
        assert_eq!(
            error.pg_int4_error(),
            Some(astersql_expression::PgInt4Error::Unsupported),
            "{sql}: {error}"
        );
    }
}
