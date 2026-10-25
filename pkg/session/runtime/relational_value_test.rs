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
    assert_eq!(
        pg.execute("SELECT f::int4 FROM pg_core_int4_numeric")
            .unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        vec!["1"]
    );
    assert!(
        pg.execute("SELECT f::int4 FROM pg_core_int4_numeric WHERE id = 0")
            .unwrap()[0]
            .next_row()
            .unwrap()
            .is_none()
    );
    for sql in [
        "SELECT n::int4 FROM pg_core_int4_numeric",
        "SELECT n::int4 FROM pg_core_int4_numeric WHERE id = 0",
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

#[test]
fn pg_core_int8_explicit_cast() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for (input, expected) in [
        ("(-9223372036854775808)", "-9223372036854775808"),
        ("9223372036854775807", "9223372036854775807"),
        ("9007199254740993", "9007199254740993"),
        ("' +09007199254740993 '", "9007199254740993"),
        ("2147483647::int4", "2147483647"),
    ] {
        let sql = format!("SELECT CAST({input} AS bigint)");
        let mut results = pg.execute(&sql).unwrap();
        assert_eq!(
            results[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .GetType(),
            astersql_parser_mysql::r#type::TypeLonglong
        );
        assert_eq!(
            results[0].next_row().unwrap().unwrap(),
            vec![expected],
            "{sql}"
        );
    }
    for sql in [
        "SELECT 9223372036854775808::int8",
        "SELECT (-9223372036854775809)::int8",
        "SELECT '18446744073709551615'::int8",
        "SELECT '12x'::int8",
        "SELECT 2147483648::int8::int4",
        "SELECT 2147483648::int4::int8",
    ] {
        let error = match pg.execute(sql) {
            Ok(_) => panic!("must reject {sql}"),
            Err(error) => error,
        };
        let expected = if sql.contains("12x") {
            astersql_expression::PgInt4Error::InvalidSyntax
        } else {
            astersql_expression::PgInt4Error::OutOfRange
        };
        assert_eq!(error.pg_int4_error(), Some(expected), "{sql}: {error}");
        assert_eq!(
            pg.execute("SELECT 7").unwrap()[0]
                .next_row()
                .unwrap()
                .unwrap(),
            vec!["7"]
        );
    }
}

#[test]
fn pg_scalar_01_boolean_type_and_null() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for sql in [
        "SELECT TRUE::boolean,FALSE::bool,NULL::boolean",
        "SELECT CAST(TRUE AS BOOL),CAST(FALSE AS BOOLEAN),CAST(NULL AS BOOLEAN)",
    ] {
        let mut records = pg.execute(sql).unwrap();
        assert_eq!(
            records[0].next_row().unwrap().unwrap(),
            vec!["1", "0", "<nil>"]
        );
    }
}

#[test]
fn pg_scalar_03_relational_boolean_conversions() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for (sql, expected) in [
        ("SELECT (-1)::int4::boolean", "1"),
        ("SELECT 2::int4::boolean", "1"),
        ("SELECT FALSE::int4", "0"),
        ("SELECT TRUE::text", "true"),
        ("SELECT 'of'::text::boolean", "0"),
    ] {
        assert_eq!(
            pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap(),
            vec![expected],
            "{sql}"
        );
    }
    for sql in [
        "SELECT 1 WHERE 1",
        "SELECT '2'::text::boolean",
        "SELECT 2::int8::boolean",
        "SELECT NULL::int8::boolean LIMIT 0",
    ] {
        assert!(pg.execute(sql).is_err(), "{sql}");
    }
}

#[test]
fn pg_scalar_05_relational_int2_cast() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for sql in [
        "SELECT (-32768)::smallint,32767::int2,NULL::int2",
        "SELECT CAST(-32768 AS int2),CAST(32767 AS smallint),NULL::int8::int2",
    ] {
        let mut results = pg.execute(sql).unwrap();
        assert_eq!(
            results[0].next_row().unwrap().unwrap(),
            vec!["-32768", "32767", "<nil>"]
        );
    }
    for sql in [
        "SELECT (-32769)::int4::int2",
        "SELECT 32768::int8::smallint",
    ] {
        let error = match pg.execute(sql) {
            Ok(_) => panic!("must reject {sql}"),
            Err(e) => e,
        };
        assert_eq!(
            error.pg_int4_error(),
            Some(astersql_expression::PgInt4Error::OutOfRange)
        );
    }
}

#[test]
fn pg_finite_numeric_relational_consumers() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_num_rows (v TEXT)").unwrap();
    for value in [
        "12.340".to_owned(),
        "9007199254740993".into(),
        "9".repeat(82),
    ] {
        pg.execute("DELETE FROM pg_num_rows").unwrap();
        pg.execute(&format!("INSERT INTO pg_num_rows VALUES ('{value}')"))
            .unwrap();
        for sql in [
            "SELECT v::numeric FROM pg_num_rows",
            "SELECT CAST(v AS numeric) FROM pg_num_rows",
        ] {
            assert_eq!(
                pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap(),
                vec![value.clone()]
            );
        }
    }
}

#[test]
fn pg_numeric_modifier_relational_consumers() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_numeric_modifier_rows (v TEXT)")
        .unwrap();
    pg.execute("INSERT INTO pg_numeric_modifier_rows VALUES ('1250')")
        .unwrap();
    for sql in [
        "SELECT v::numeric(2,-2) FROM pg_numeric_modifier_rows",
        "SELECT CAST(v AS numeric(2,-2)) FROM pg_numeric_modifier_rows",
    ] {
        assert_eq!(
            pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap(),
            vec!["1300"]
        );
    }
    pg.execute("DROP TABLE pg_numeric_modifier_rows").unwrap();
}

#[test]
fn pg_numeric_special_relational_consumers() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_numeric_special_rows (v TEXT)")
        .unwrap();
    for (input, expected) in [("nan", "NaN"), ("+inf", "Infinity"), ("-inf", "-Infinity")] {
        pg.execute(&format!(
            "INSERT INTO pg_numeric_special_rows VALUES ('{input}')"
        ))
        .unwrap();
        for sql in [
            "SELECT v::numeric FROM pg_numeric_special_rows",
            "SELECT CAST(v AS numeric) FROM pg_numeric_special_rows",
        ] {
            assert_eq!(
                pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap(),
                vec![expected]
            );
        }
        let constrained = pg.execute("SELECT v::numeric(3,1) FROM pg_numeric_special_rows");
        if expected == "NaN" {
            assert_eq!(
                constrained.unwrap()[0].next_row().unwrap().unwrap(),
                vec!["NaN"]
            );
        } else {
            assert!(constrained.is_err());
        }
        pg.execute("DELETE FROM pg_numeric_special_rows").unwrap();
    }
    pg.execute("DROP TABLE pg_numeric_special_rows").unwrap();
}

#[test]
fn pg_scalar_11_relational_numeric_integer() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_scalar_11_rows (v TEXT)")
        .unwrap();
    pg.execute("INSERT INTO pg_scalar_11_rows VALUES ('-2.5')")
        .unwrap();
    for target in ["int2", "int4", "int8"] {
        let sql = format!("SELECT (v::numeric)::{target} FROM pg_scalar_11_rows");
        assert_eq!(
            pg.execute(&sql).unwrap()[0].next_row().unwrap().unwrap(),
            vec!["-3"]
        );
        pg.execute(&format!("INSERT INTO pg_scalar_11_rows SELECT ((v::numeric)::{target})::text FROM pg_scalar_11_rows WHERE v='-2.5'")).unwrap();
    }
    pg.execute("DROP TABLE pg_scalar_11_rows").unwrap();
}

#[test]
fn pg_scalar_12_relational_literal_precision() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_scalar_12_rows (v TEXT)")
        .unwrap();
    pg.execute("INSERT INTO pg_scalar_12_rows VALUES ('row')")
        .unwrap();
    for (literal, expected) in [
        ("1.2300e0", "1.2300"),
        (
            "-123456789012345678901234567890.123456789",
            "-123456789012345678901234567890.123456789",
        ),
    ] {
        assert_eq!(
            pg.execute(&format!("SELECT {literal} FROM pg_scalar_12_rows"))
                .unwrap()[0]
                .next_row()
                .unwrap()
                .unwrap(),
            vec![expected]
        );
        pg.execute(&format!("INSERT INTO pg_scalar_12_rows SELECT ({literal})::text FROM pg_scalar_12_rows WHERE v='row'")).unwrap();
        assert_eq!(
            pg.execute(&format!(
                "SELECT v FROM pg_scalar_12_rows WHERE v='{expected}'"
            ))
            .unwrap()[0]
                .next_row()
                .unwrap()
                .unwrap(),
            vec![expected]
        );
    }
    pg.execute("DROP TABLE pg_scalar_12_rows").unwrap();
}

#[test]
fn pg_scalar_13_unsigned_target() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let mysql =
        ConcreteSession::new_with_dialect(domain.clone(), astersql_parser_ast::SqlDialect::MySQL);
    mysql
        .execute("CREATE TABLE scalar13_unsigned (v BIGINT UNSIGNED)")
        .unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    for input in ["0", "18446744073709551615", "NULL"] {
        pg.execute(&format!("INSERT INTO scalar13_unsigned VALUES ({input})"))
            .unwrap();
    }
    for input in ["-1", "18446744073709551616", "1.5"] {
        let error = pg
            .execute(&format!("INSERT INTO scalar13_unsigned VALUES ({input})"))
            .err()
            .unwrap();
        assert_eq!(error.pg_numeric_error().unwrap().sqlstate(), "22003");
    }
    assert_eq!(
        pg.execute("SELECT count(*) FROM scalar13_unsigned")
            .unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        vec!["3"]
    );
    pg.execute("DROP TABLE scalar13_unsigned").unwrap();
}

#[test]
fn pg_scalar_16_float4_relational_consumers() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_float4_16 (v TEXT)").unwrap();
    for (input, expected) in [
        ("7.038531e-26", f32::from_bits(0x15ae43fd)),
        ("-0", -0.0),
        ("1.401298464324817e-45", f32::from_bits(1)),
        ("Infinity", f32::INFINITY),
        ("NaN", f32::NAN),
    ] {
        pg.execute(&format!("INSERT INTO pg_float4_16 VALUES ('{input}')"))
            .unwrap();
        for sql in [
            "SELECT v::float4 FROM pg_float4_16",
            "SELECT CAST(v AS FLOAT4) FROM pg_float4_16",
        ] {
            let actual = pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap()[0]
                .parse::<f32>()
                .unwrap();
            if expected.is_nan() {
                assert!(actual.is_nan());
            } else {
                assert_eq!(actual.to_bits(), expected.to_bits(), "{input}");
            }
        }
        pg.execute("DELETE FROM pg_float4_16").unwrap();
    }
    pg.execute("INSERT INTO pg_float4_16 VALUES ('1e39')")
        .unwrap();
    assert_eq!(
        pg.execute("SELECT v::float4 FROM pg_float4_16")
            .err()
            .unwrap()
            .pg_float4_error()
            .unwrap()
            .sqlstate(),
        "22003"
    );
    pg.execute("DROP TABLE pg_float4_16").unwrap();
}

#[test]
fn pg_scalar_17_float8_prepared_identity() {
    use crate::runtime::{ConcretePreparedArgument, ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    assert_eq!(
        pg.protocol_parameter_types("SELECT ?", &[701]).unwrap(),
        [701]
    );
    let (id, count, fields) = pg
        .prepare_protocol_statement_with_parameter_types("SELECT ?", &[701])
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fields[0].column.FieldType.GetType(),
        astersql_parser_mysql::r#type::TypeDouble
    );
    for expected in [
        f64::MAX,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        -0.0,
        f64::INFINITY,
        f64::NAN,
    ] {
        let mut result = pg
            .execute_protocol_statement(id, &[ConcretePreparedArgument::Float(expected)])
            .unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .GetType(),
            astersql_parser_mysql::r#type::TypeDouble
        );
        let actual = result[0].next_row().unwrap().unwrap()[0]
            .parse::<f64>()
            .unwrap();
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }
}

#[test]
fn pg_scalar_18_precision_diagnostic() {
    use super::super::parse_in_dialect;
    use astersql_parser_ast::SqlDialect;
    for sql in ["SELECT NULL::float(0)", "SELECT CAST(NULL AS FLOAT(54))"] {
        let error = parse_in_dialect(sql, SqlDialect::PostgreSQL).err().unwrap();
        assert_eq!(
            error.to_string(),
            "precision for type float must be between 1 and 53 bits"
        );
    }
    assert!(parse_in_dialect("SELECT CAST(NULL AS FLOAT(0))", SqlDialect::MySQL).is_ok());
    let error = parse_in_dialect(
        "SELECT ) /* PostgreSQL float precision must be between 1 and 53 */",
        SqlDialect::PostgreSQL,
    )
    .err()
    .unwrap();
    assert!(error.to_string().starts_with("[parser:1064]"));
}

#[test]
fn pg_scalar_18_float_parameter_identity() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for (ty, oid) in [
        ("float(1)", 700),
        ("float(24)", 700),
        ("float(25)", 701),
        ("float(53)", 701),
        ("float", 701),
        ("real", 700),
        ("double precision", 701),
        ("float4", 700),
        ("float8", 701),
    ] {
        for sql in [format!("SELECT ?::{ty}"), format!("SELECT CAST(? AS {ty})")] {
            assert_eq!(
                pg.protocol_parameter_types(&sql, &[]).unwrap(),
                [oid],
                "{sql}"
            );
        }
    }
}

#[test]
fn pg_scalar_19_relational_float_integer() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_scalar_19_row (f DOUBLE)")
        .unwrap();
    pg.execute("INSERT INTO pg_scalar_19_row VALUES (2.5)")
        .unwrap();
    assert_eq!(
        pg.execute("SELECT f::int4 FROM pg_scalar_19_row").unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        vec!["2"]
    );
    pg.execute("DROP TABLE pg_scalar_19_row").unwrap();
}

#[test]
fn pg_scalar_20_relational_float8() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_scalar_20_row (id INT, d DOUBLE)")
        .unwrap();
    for (input, state) in [("bad", "22P02"), ("1e9999", "22003"), ("1e-9999", "22003")] {
        let error = pg
            .execute(&format!(
                "INSERT INTO pg_scalar_20_row VALUES (1,CAST('{input}' AS float8))"
            ))
            .err()
            .expect("float8 input must be rejected");
        assert_eq!(error.pg_float8_error().unwrap().sqlstate(), state);
    }
    assert!(
        pg.execute("SELECT d FROM pg_scalar_20_row").unwrap()[0]
            .next_row()
            .unwrap()
            .is_none()
    );
    pg.execute("DROP TABLE pg_scalar_20_row").unwrap();
}

#[test]
fn pg_scalar_21_relational_text_identity() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg_scalar_21_row (v VARCHAR(30))")
        .unwrap();
    pg.execute("INSERT INTO pg_scalar_21_row VALUES ('e\u{301}é😀')")
        .unwrap();
    for (name, oid) in [("text", 25), ("varchar", 1043), ("bpchar", 1042)] {
        let sql = format!("SELECT v::{name} FROM pg_scalar_21_row");
        let mut results = pg.execute(&sql).unwrap();
        assert_eq!(
            results[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLTextOid(),
            Some(oid),
            "{sql}"
        );
        assert_eq!(results[0].next_row().unwrap().unwrap(), vec!["e\u{301}é😀"]);
        let mut empty = pg.execute(&format!("{sql} WHERE FALSE")).unwrap();
        assert_eq!(
            empty[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLTextOid(),
            Some(oid)
        );
        assert!(empty[0].next_row().unwrap().is_none());
    }
    pg.execute("DROP TABLE pg_scalar_21_row").unwrap();
}
