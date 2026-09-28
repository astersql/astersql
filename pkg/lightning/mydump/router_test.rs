// Copyright 2023 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// 文件路径路由器单元测试。
//
// 覆盖规则编译校验、默认 mydumper 命名、单/多规则优先级、模板展开、
// 常量 path 转义，以及压缩 parquet 拒绝路径等场景。
use crate::*;

/// 构造仅含 pattern/schema/table/type 的测试规则。
fn rule(pattern: &str, schema: &str, table: &str, type_name: &str) -> FileRouteRule {
    FileRouteRule {
        pattern: pattern.to_owned(),
        schema: schema.to_owned(),
        table: table.to_owned(),
        type_name: type_name.to_owned(),
        ..Default::default()
    }
}

/// 断言路径必须匹配并返回 RouteResult。
fn routed(router: &dyn FileRouter, path: &str) -> RouteResult {
    router.Route(path).unwrap().expect("path must be routed")
}

#[test]
/// 合法捕获模板可编译；非法正则或越界捕获应失败。
fn TestRouteParser() {
    let valid = [
        rule(r"^([^/.]+)\.([^./]+)\.(csv|sql)$", "$1", "$2", "$3"),
        rule(r"^.+\.(csv|sql)$", "test", "t", "$1"),
        rule(
            r"^(?P<schema>[^/.]+)\.(?P<table>[^./]+)\.(?P<type>csv|sql)$",
            "$schema",
            "$table",
            "$type",
        ),
    ];
    for route in valid {
        NewFileRouter(&[route], Logger::default()).unwrap();
    }

    let invalid_regex = rule("(", "x", "y", TYPE_CSV);
    assert!(NewFileRouter(&[invalid_regex], Logger::default()).is_err());
    let invalid_capture = rule(r"^([^.]+)\.csv$", "$schema", "t", TYPE_CSV);
    assert!(NewFileRouter(&[invalid_capture], Logger::default()).is_err());
    let invalid_index = rule(r"^([^.]+)\.csv$", "$1", "$2", TYPE_CSV);
    assert!(NewFileRouter(&[invalid_index], Logger::default()).is_err());
}

#[test]
/// 默认规则识别 schema/table/view/sql/csv/parquet 与 .bak 忽略。
fn TestDefaultRouter() {
    let router = NewDefaultFileRouter(Logger::default()).unwrap();
    let cases = [
        (
            "a/test-schema-create.sql.bak",
            "",
            "",
            "",
            Compression::None,
            SourceType::Ignore,
        ),
        (
            "my_schema.my_table.0001.sql.snappy.BAK",
            "",
            "",
            "",
            Compression::None,
            SourceType::Ignore,
        ),
        (
            "test-schema-create.sql",
            "test",
            "",
            "",
            Compression::None,
            SourceType::SchemaSchema,
        ),
        (
            "test-schema-create.sql.gz",
            "test",
            "",
            "",
            Compression::Gz,
            SourceType::SchemaSchema,
        ),
        (
            "c/d/test.t-schema.sql",
            "test",
            "t",
            "",
            Compression::None,
            SourceType::TableSchema,
        ),
        (
            "test.t-schema.sql.lzo",
            "test",
            "t",
            "",
            Compression::Lzo,
            SourceType::TableSchema,
        ),
        (
            "/bc/dc/test.v1-schema-view.sql",
            "test",
            "v1",
            "",
            Compression::None,
            SourceType::ViewSchema,
        ),
        (
            "test.v1-schema-view.sql.snappy",
            "test",
            "v1",
            "",
            Compression::Snappy,
            SourceType::ViewSchema,
        ),
        (
            "my_schema.my_table.sql",
            "my_schema",
            "my_table",
            "",
            Compression::None,
            SourceType::Sql,
        ),
        (
            "/test/123/my_schema.my_table.sql.gz",
            "my_schema",
            "my_table",
            "",
            Compression::Gz,
            SourceType::Sql,
        ),
        (
            "my_dir/my_schema.my_table.csv.lzo",
            "my_schema",
            "my_table",
            "",
            Compression::Lzo,
            SourceType::Csv,
        ),
        (
            "my_schema.my_table.0001.sql.snappy",
            "my_schema",
            "my_table",
            "0001",
            Compression::Snappy,
            SourceType::Sql,
        ),
        (
            "my_schema.my_table.0001.gz.parquet",
            "my_schema",
            "my_table",
            "0001",
            Compression::None,
            SourceType::Parquet,
        ),
        (
            "my_schema.my_table.0001.snappy.parquet",
            "my_schema",
            "my_table",
            "0001",
            Compression::None,
            SourceType::Parquet,
        ),
    ];
    for (path, schema, table, key, compression, source_type) in cases {
        let result = routed(&router, path);
        assert_eq!(
            (
                result.schema.as_str(),
                result.name.as_str(),
                result.key.as_str()
            ),
            (schema, table, key)
        );
        assert_eq!(result.compression, compression);
        assert_eq!(result.source_type, source_type);
    }
}

#[test]
/// path/pattern 皆空、缺 type、同时设置 path+pattern 应报错。
fn TestInvalidRouteRule() {
    let error = NewFileRouter(&[FileRouteRule::default()], Logger::default())
        .err()
        .unwrap();
    assert!(error.to_string().contains("must not be both empty"));

    let missing_type = FileRouteRule {
        pattern: r"^([^.]+)\.([^.]+)\.csv$".into(),
        schema: "$1".into(),
        table: "$2".into(),
        ..Default::default()
    };
    assert!(
        NewFileRouter(&[missing_type], Logger::default())
            .err()
            .unwrap()
            .to_string()
            .contains("field 'type'")
    );

    let both = FileRouteRule {
        path: "x.csv".into(),
        pattern: "x".into(),
        schema: "s".into(),
        table: "t".into(),
        type_name: TYPE_CSV.into(),
        ..Default::default()
    };
    assert!(
        NewFileRouter(&[both], Logger::default())
            .err()
            .unwrap()
            .to_string()
            .contains("can't set both")
    );

    let invalid_named_capture = FileRouteRule {
        pattern: r"^(?P<table>[^.]+)\.csv$".into(),
        schema: "$schema".into(),
        table: "$table".into(),
        type_name: TYPE_CSV.into(),
        ..Default::default()
    };
    assert!(
        NewFileRouter(&[invalid_named_capture], Logger::default())
            .err()
            .unwrap()
            .to_string()
            .contains("invalid named capture '$schema'")
    );
}

#[test]
/// 命名捕获提取 schema/table/key/type/compression；不匹配返回 None。
fn TestSingleRouteRule() {
    let router = NewFileRouter(
        &[FileRouteRule {
            pattern: r"^(?P<schema>[^.]+)\.(?P<table>[^.]+)\.(?P<key>[0-9]+)\.(?P<type>csv|sql)(?:\.(?P<cp>\w+))?$".into(),
            schema: "$schema".into(),
            table: "$table".into(),
            key: "$key".into(),
            type_name: "$type".into(),
            compression: "$cp".into(),
            ..Default::default()
        }],
        Logger::default(),
    )
    .unwrap();
    let result = routed(&router, "db.tbl.001.csv.gz");
    assert_eq!(result.schema, "db");
    assert_eq!(result.name, "tbl");
    assert_eq!(result.key, "001");
    assert_eq!(result.source_type, SourceType::Csv);
    assert_eq!(result.compression, Compression::Gz);
    for path in [
        "my_table.sql",
        "/schema/table.sql",
        "my_schema.my_table.txt",
        "my_schema.my_table.001.txt",
        "my_schema.my_table.0001-002.sql",
    ] {
        assert!(router.Route(path).unwrap().is_none(), "{path}");
    }

    let permissive = NewFileRouter(
        &[FileRouteRule {
            pattern: r"^(?P<schema>[^.]+)\.(?P<table>[^.]+)(?:\.(?P<key>[0-9]+))?\.(?P<type>\w+)(?:\.(?P<cp>\w+))?$".into(),
            schema: "$schema".into(),
            table: "$table".into(),
            key: "$key".into(),
            type_name: "$type".into(),
            compression: "$cp".into(),
            ..Default::default()
        }],
        Logger::default(),
    )
    .unwrap();
    for path in ["my_schema.my_table.sql.rar", "my_schema.my_table.txt"] {
        assert!(permissive.Route(path).is_err(), "{path}");
    }
}

#[test]
/// 多规则按顺序优先匹配 special- 前缀再回退通用规则。
fn TestMultiRouteRule() {
    let router = NewFileRouter(
        &[
            rule(r"^special-([^.]+)\.csv$", "special", "$1", TYPE_CSV),
            rule(r"^([^.]+)\.([^.]+)\.csv$", "$1", "$2", TYPE_CSV),
        ],
        Logger::default(),
    )
    .unwrap();
    let first = routed(&router, "special-table.csv");
    assert_eq!(
        (first.schema.as_str(), first.name.as_str()),
        ("special", "table")
    );
    let second = routed(&router, "db.table.csv");
    assert_eq!(
        (second.schema.as_str(), second.name.as_str()),
        ("db", "table")
    );
}

#[test]
/// `${name}_suffix` 模板展开；checkSubPatterns 校验越界捕获。
fn TestRouteExpanding() {
    let pattern = r"^(?P<schema>[^.]+)\.(?P<table_name>[^.]+)(?:\.(?P<key>[0-9]+))?\.(?P<type>csv|sql)(?:\.(?P<cp>\w+))?$";
    let cases = [
        ("$schema", "db"),
        ("$table_name", "table"),
        ("$schema.$table_name", "db.table"),
        ("${1}", "db"),
        ("${1}_$table_name", "db_table"),
        ("${2}.schema", "table.schema"),
        ("$${2}", "${2}"),
        ("$$table_name", "$table_name"),
        ("$table_name-123", "table-123"),
        ("$$12$1$schema", "$12dbdb"),
        ("${table_name}$$2", "table$2"),
        ("${table_name}$$", "table$"),
        ("{1}$$", "{1}$"),
        ("my_table", "my_table"),
    ];
    for (template, expected) in cases {
        let router = NewFileRouter(
            &[FileRouteRule {
                pattern: pattern.into(),
                schema: "$schema".into(),
                table: template.into(),
                type_name: "$type".into(),
                key: "$key".into(),
                compression: "$cp".into(),
                ..Default::default()
            }],
            Logger::default(),
        )
        .unwrap();
        assert_eq!(routed(&router, "db.table.001.sql").name, expected);
    }
    for invalid in ["$1_$schema", "$schema_$table_name", "$6"] {
        assert!(checkSubPatterns(pattern, invalid).is_err(), "{invalid}");
    }
}

#[test]
fn parsing_contract_matches_go() {
    for (value, expected) in [
        (" SQL ", SourceType::Sql),
        ("Csv", SourceType::Csv),
        ("PARQUET", SourceType::Parquet),
        ("ignore", SourceType::Ignore),
    ] {
        assert_eq!(parseSourceType(value).unwrap(), expected);
    }
    assert!(parseSourceType("txt").is_err());

    for (value, expected) in [
        (" GZIP ", Compression::Gz),
        ("zst", Compression::Zstd),
        ("snappy", Compression::Snappy),
        ("", Compression::None),
    ] {
        assert_eq!(parseCompressionType(value).unwrap(), expected);
    }
    assert!(parseCompressionType("rar").is_err());
    assert_eq!(
        ParseCompressionOnFileExtension("dump.SQL.GZ"),
        Compression::Gz
    );
    assert_eq!(
        ParseCompressionOnFileExtension("dump.sql.rar"),
        Compression::None
    );
    assert_eq!(ParseCompressionOnFileExtension("dump"), Compression::None);

    assert_eq!(
        ToStorageCompressType(Compression::Gz).unwrap(),
        CompressType::Gzip
    );
    assert_eq!(
        ToStorageCompressType(Compression::Snappy).unwrap(),
        CompressType::Snappy
    );
    assert_eq!(
        ToStorageCompressType(Compression::Zstd).unwrap(),
        CompressType::Zstd
    );
    assert_eq!(
        ToStorageCompressType(Compression::None).unwrap(),
        CompressType::NoCompression
    );
    assert!(ToStorageCompressType(Compression::Lz4).is_err());
}

#[test]
/// 常量 path 字面匹配，模板中的 `$` 保持字面量。
fn TestRouteWithPath() {
    let router = NewFileRouter(
        &[FileRouteRule {
            path: "db.$table.csv".into(),
            schema: "db$1".into(),
            table: "$table".into(),
            type_name: TYPE_CSV.into(),
            ..Default::default()
        }],
        Logger::default(),
    )
    .unwrap();
    let result = routed(&router, "db.$table.csv");
    assert_eq!(result.schema, "db$1");
    assert_eq!(result.name, "$table");
    assert!(router.Route("db.other.csv").unwrap().is_none());
}

#[test]
/// 整体压缩的 parquet 路径应返回错误。
fn TestRouteWithCompressedParquet() {
    let router = NewFileRouter(
        &[FileRouteRule {
            pattern: r"^([^.]+)\.([^.]+)\.parquet\.(\w+)$".into(),
            schema: "$1".into(),
            table: "$2".into(),
            type_name: TYPE_PARQUET.into(),
            compression: "$3".into(),
            ..Default::default()
        }],
        Logger::default(),
    )
    .unwrap();
    let error = router.Route("db.t.parquet.gz").unwrap_err();
    assert!(error.to_string().contains("compressed parquet"));
}
