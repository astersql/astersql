// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `config_test.go`.
//!
//! 这些测试把配置层最容易回归的几类行为钉死下来：
//! 默认值、表名解析、Parquet 参数解析、CSV 方言大小写兼容，
//! 以及分片输出模板必须包含 `{{.Index}}` 的安全约束。

use crate::main_test::default_config_for_test;
use crate::*;

#[test]
fn test_create_external_storage() {
    // 默认配置未显式指定外部存储时，应回落到本地 file storage。
    let mut conf = default_config_for_test();
    let loc = conf.createExternalStorage().unwrap();
    // 这里只检查 URI 前缀，避免和具体临时目录实现细节耦合。
    // 一旦这里不再是 file:，通常意味着默认存储选择逻辑被改动了。
    assert!(loc.URI().starts_with("file:"), "uri={}", loc.URI());
}

#[test]
fn test_match_mysql_bug_version() {
    // 覆盖 bug 版本区间的左右边界，防止比较条件出现 off-by-one。
    // TiDB 版本也放在这里，确保不会被误判成 MySQL bug 版本。
    let cases = [
        ("5.7.25-TiDB-3.0.6", false),
        ("8.0.2", false),
        ("8.0.3", true),
        ("8.0.22", true),
        ("8.0.23", false),
    ];
    // 每个 case 都先走 ParseServerInfo，确保测试比较的是完整解析后的版本结构。
    for (src, expected) in cases {
        // 断言信息里保留原始版本串，便于回归时直接定位是哪一个边界失效。
        let info = ParseServerInfo(src);
        assert_eq!(expected, matchMysqlBugversion(&info), "server info: {src}");
    }
}

#[test]
fn test_get_conf_tables() {
    // 第一组：第一个表名缺少 `db.table` 中的点，应直接报错。
    let tables_list = vec!["db1t1".into(), "db2.t1".into()];
    let err = GetConfTables(&tables_list).err().expect("expected error");
    assert_eq!(
        err.msg,
        format!(
            "--tables-list only accepts qualified table names, but `{}` lacks a dot",
            tables_list[0]
        )
    );

    // 第二组：第二个表名缺少点，验证错误定位会指向真正出错的元素。
    let tables_list = vec!["db1.t1".into(), "db2t1".into()];
    let err = GetConfTables(&tables_list).err().expect("expected error");
    assert_eq!(
        err.msg,
        format!(
            "--tables-list only accepts qualified table names, but `{}` lacks a dot",
            tables_list[1]
        )
    );

    // 第三组：全部合法时，应按数据库名聚合成 `DatabaseTables` 结构。
    let tables_list = vec!["db1.t1".into(), "db2.t1".into()];
    let mut expected = NewDatabaseTables();
    // 期望结构按库名聚合，和后续导出器真正消费的数据形态一致。
    expected
        .AppendTables("db1", &["t1".into()], &[0])
        .AppendTables("db2", &["t1".into()], &[0]);
    let actual = GetConfTables(&tables_list).unwrap();
    assert_eq!(expected, actual);
}

#[test]
fn test_parse_parquet_default_flags() {
    // 同时验证静态默认值和 CLI 空参数解析后的默认值保持一致。
    // 这能防止 DefaultConfig 与 DefineFlags 的默认值悄悄分叉。
    let default_conf = DefaultConfig();
    assert_eq!(DefaultCompressionType, default_conf.ParquetCompressType);
    assert_eq!(MiB, default_conf.ParquetPageSize);
    assert_eq!(120 * MiB, default_conf.ParquetRowGroupSize);

    let conf = parse_config_from_args_for_test(&[]);
    assert_eq!(MiB, conf.ParquetPageSize);
    assert_eq!(120 * MiB, conf.ParquetRowGroupSize);
    assert_eq!(DefaultCompressionType, conf.ParquetCompressType);

    // 压缩类型解析既支持空串走默认，也支持显式指定具体算法。
    // 这里顺手验证了解析器对空值和小写算法名的兼容性。
    let tp = parseParquetCompressType("").unwrap();
    assert_eq!(DefaultCompressionType, tp);
    let tp = parseParquetCompressType("zstd").unwrap();
    assert_eq!(CompressType::Zstd, tp);
}

#[test]
fn test_parse_parquet_size_flags() {
    // 人类可读容量字符串应被正确解析成内部字节数。
    // page size 和 row-group size 分别校验，避免两者共用解析器时互相串值。
    let conf = parse_config_from_args_for_test(&[
        "--filetype",
        "parquet",
        "--parquet-page-size",
        "2MiB",
        "--parquet-row-group-size",
        "128MiB",
    ]);
    assert_eq!(2 * MiB, conf.ParquetPageSize);
    assert_eq!(128 * MiB, conf.ParquetRowGroupSize);
}

#[test]
fn test_parse_csv_output_dialect_accepts_uppercase() {
    // 文件类型和方言名字都允许大写输入，保持与 Go CLI 一样的宽松解析。
    // 这类兼容性对脚本迁移尤其重要，因为历史参数常常不是统一大小写。
    let conf = parse_config_from_args_for_test(&[
        "--filetype",
        "CSV",
        "--csv-output-dialect",
        "SNOWFLAKE",
    ]);
    assert_eq!(CSVDialect::CSVDialectSnowflake, conf.CsvOutputDialect);
}

#[test]
fn test_go_config_parity_edge_cases() {
    assert_eq!(2 * MiB as u64, ParseFileSize("2").unwrap());
    assert!(ParseFileSize("bogus").is_err());

    assert_eq!(
        CSVDialect::CSVDialectDefault,
        ParseOutputDialect("default").unwrap()
    );
    assert!(ParseOutputDialect("mysql").is_err());

    let tables = vec!["db.table".to_string()];
    let swapped_defaults = vec![DefaultTableFilter.to_string(), "*.*".to_string()];
    assert!(ParseTableFilter(&tables, &swapped_defaults).is_err());
    let case_changed_defaults = vec!["*.*".to_string(), DefaultTableFilter.to_ascii_lowercase()];
    assert!(ParseTableFilter(&tables, &case_changed_defaults).is_err());
}

#[test]
fn test_parse_from_flags_preserves_go_fields_and_errors() {
    let conf = parse_config_from_args_for_test(&[
        "--allow-cleartext-passwords",
        "--loglevel",
        "debug",
        "--logfile",
        "dumpling.log",
        "--logfmt",
        "json",
        "--status-addr",
        ":9090",
        "--no-header",
        "--no-schemas",
        "--no-data",
        "--csv-null-value",
        "NULL",
        "--escape-backslash=false",
        "--ca",
        "ca.pem",
        "--cert",
        "cert.pem",
        "--key",
        "key.pem",
        "--transactional-consistency=false",
        "--tidb-mem-quota-query",
        "4096",
        "--compress",
        "zstd",
    ]);
    assert!(conf.AllowCleartextPasswords);
    assert_eq!("debug", conf.LogLevel);
    assert_eq!("dumpling.log", conf.LogFile);
    assert_eq!("json", conf.LogFormat);
    assert_eq!(":9090", conf.StatusAddr);
    assert!(conf.NoHeader);
    assert!(conf.NoSchemas);
    assert!(conf.NoData);
    assert_eq!("NULL", conf.CsvNullValue);
    assert!(!conf.EscapeBackslash);
    assert_eq!("ca.pem", conf.Security.CAPath);
    assert_eq!("cert.pem", conf.Security.CertPath);
    assert_eq!("key.pem", conf.Security.KeyPath);
    assert!(!conf.TransactionalConsistency);
    assert_eq!(4096, conf.TiDBMemQuotaQuery);
    assert_eq!(CompressType::Zstd, conf.CompressType);

    assert!(parse_config_from_args_for_test_with_err(&["--rows", "invalid"]).is_err());
    assert!(parse_config_from_args_for_test_with_err(&["--parquet-page-size", "invalid"]).is_err());
    assert!(parse_config_from_args_for_test_with_err(&["--csv-separator", ""]).is_err());
    assert!(
        parse_config_from_args_for_test_with_err(&["--csv-output-dialect", "snowflake"]).is_err()
    );
    assert!(parse_config_from_args_for_test_with_err(&["--compress", "zip"]).is_err());
}

#[test]
fn test_output_filename_template_with_rows_validation() {
    // 这一组专门锁定 split 模式下的模板安全规则，避免 chunk 文件互相覆盖。
    match parse_config_from_args_for_test_with_err(&[
        "--rows",
        "10",
        "--output-filename-template",
        "{{.DB}}.{{.Table}}",
    ]) {
        Err(err) => assert!(err.msg.contains("standalone {{.Index}}")),
        Ok(_) => panic!("expected error"),
    }

    // 直接包含独立 `{{.Index}}` 的模板应通过校验。
    parse_config_from_args_for_test_with_err(&[
        "--rows",
        "10",
        "--output-filename-template",
        "{{.DB}}.{{.Table}}.{{.Index}}",
    ])
    .unwrap();

    // 仅在条件块里出现 Index 仍然不安全，因为不能保证每个 chunk 都带索引。
    match parse_config_from_args_for_test_with_err(&[
        "--rows",
        "10",
        "--output-filename-template",
        "{{if .Index}}{{end}}{{.DB}}.{{.Table}}",
    ]) {
        Err(err) => assert!(err.msg.contains("standalone {{.Index}}")),
        Ok(_) => panic!("expected error"),
    }

    // 即使条件块里输出了 Index，只要没有独立占位符，仍然应该拒绝。
    match parse_config_from_args_for_test_with_err(&[
        "--rows",
        "10",
        "--output-filename-template",
        "{{if lt .Index 2}}{{.Index}}{{end}}{{.DB}}.{{.Table}}",
    ]) {
        Err(err) => assert!(err.msg.contains("standalone {{.Index}}")),
        Ok(_) => panic!("expected error"),
    }

    // 条件前缀 + 独立 Index 是允许的，因为最终文件名仍然具备唯一性。
    parse_config_from_args_for_test_with_err(&[
        "--rows",
        "10",
        "--output-filename-template",
        "{{if lt .Index 2}}prefix.{{end}}{{.DB}}.{{.Table}}.{{.Index}}",
    ])
    .unwrap();

    // 非分片模式下模板可以不带 Index，因为不存在多个 chunk 互相覆盖。
    parse_config_from_args_for_test_with_err(&[
        "--rows",
        "0",
        "--output-filename-template",
        "{{.DB}}.{{.Table}}",
    ])
    .unwrap();

    // filesize 触发的 split 模式与 rows 一样，也必须遵守同一模板规则。
    match parse_config_from_args_for_test_with_err(&[
        "--filesize",
        "1MiB",
        "--output-filename-template",
        "{{.DB}}.{{.Table}}",
    ]) {
        Err(err) => assert!(err.msg.contains("standalone {{.Index}}")),
        Ok(_) => panic!("expected error"),
    }

    parse_config_from_args_for_test_with_err(&[
        "--filesize",
        "1MiB",
        "--output-filename-template",
        "{{.DB}}.{{.Table}}.{{.Index}}",
    ])
    .unwrap();

    parse_config_from_args_for_test_with_err(&["--output-filename-template", "{{.DB}}.{{.Table}}"])
        .unwrap();
    // 最后一条用例强调：只有真正进入 split 模式时，Index 才成为硬性要求。
}

fn parse_config_from_args_for_test(args: &[&str]) -> Config {
    // 成功路径 helper，供大多数断言直接拿到解析后的 Config。
    // 这样每个测试只关注自己关心的配置字段，不必重复样板代码。
    parse_config_from_args_for_test_with_err(args).expect("parse config")
}

fn parse_config_from_args_for_test_with_err(args: &[&str]) -> Result<Config> {
    // 保留错误对象的 helper，用于验证参数校验文案是否符合预期。
    // 所有测试都统一走 DefineFlags -> ParseFromFlags，确保覆盖真实 CLI 路径。
    let mut conf = DefaultConfig();
    let mut flags = FlagSet::new();
    conf.DefineFlags(&mut flags);
    flags.Parse(args)?;
    conf.ParseFromFlags(&flags)?;
    Ok(conf)
}

#[test]
fn column_filter_flags_project_writable_columns() {
    let mut conf = DefaultConfig();
    let mut flags = FlagSet::new();
    conf.DefineFlags(&mut flags);
    flags
        .Parse(&[
            "--no-schemas",
            "--column-filter",
            r#"{ matcher = ["db.t"], columns = ["name"] }"#,
        ])
        .unwrap();
    conf.ParseFromFlags(&flags).unwrap();
    let conn = Conn::new();
    conn.seed_query(
        "SHOW COLUMNS FROM `db`.`t`",
        vec!["Field".into(), "Extra".into()],
        vec![
            vec![Some(b"id".to_vec()), Some(vec![])],
            vec![Some(b"name".to_vec()), Some(vec![])],
        ],
    );
    conn.seed_query(
        "SELECT `id`,`name` FROM `db`.`t` LIMIT 1",
        vec!["id".into(), "name".into()],
        vec![vec![Some(b"1".to_vec()), Some(b"alice".to_vec())]],
    );
    let mut db = newBaseConn(conn, false, None);
    conf.Tables.insert(
        "db".into(),
        vec![TableInfo {
            Name: "t".into(),
            Type: TableType::TableTypeBase,
            AvgRowLength: 0,
        }],
    );
    prepareColumnProjection(&tcontext::Background(), &mut conf, &mut db).unwrap();
    let meta = dumpTableMeta(
        &tcontext::Background(),
        &conf,
        &mut db,
        "db",
        &TableInfo {
            Name: "t".into(),
            Type: TableType::TableTypeBase,
            AvgRowLength: 0,
        },
    )
    .unwrap();
    assert_eq!(meta.SelectedField(), "`name`");
    assert_eq!(meta.ColumnNames(), vec!["name"]);
}

fn inline_column_filters(rules: &[&str], sensitive: bool) -> columnFilterConfig {
    parseColumnFilterArgs(
        &rules.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        sensitive,
    )
    .unwrap()
}

#[test]
fn column_filters_preserve_rule_priority_case_and_unmatched_tables() {
    let filter = inline_column_filters(
        &[
            r#"{ matcher = ["db1.*"], columns = ["*", "!c*"] }"#,
            r#"{ matcher = ["db1.t1"], columns = ["c2"] }"#,
            r#"{ matcher = ["db1.t2"], columns = ["*", "!c3"] }"#,
        ],
        false,
    );
    for (db, table, source, expected, indexes) in [
        (
            "DB1",
            "T1",
            vec!["c1", "C2", "c3", "d"],
            vec!["C2", "d"],
            vec![1, 3],
        ),
        ("db2", "t1", vec!["c1"], vec!["c1"], vec![0]),
        ("db1", "t2", vec!["c1", "c3"], vec!["c1"], vec![0]),
    ] {
        let source = source.iter().map(|c| c.to_string()).collect::<Vec<_>>();
        let (columns, actual_indexes) = filter.applyToColumns(db, table, &source).unwrap();
        assert_eq!(columns, expected);
        assert_eq!(actual_indexes, indexes);
    }
    let filter = inline_column_filters(&[r#"{ matcher = ["db1.t1"], columns = ["C2"] }"#], true);
    assert_eq!(
        filter
            .applyToColumns("DB1", "T1", &["c1".into()])
            .unwrap()
            .0,
        vec!["c1"]
    );
    assert_eq!(
        filter
            .applyToColumns("db1", "t1", &["C2".into()])
            .unwrap()
            .0,
        vec!["C2"]
    );
    assert!(
        filter
            .applyToColumns("db1", "t1", &["missing".into()])
            .unwrap_err()
            .msg
            .contains("selects no writable columns")
    );
}

#[test]
fn column_filter_inline_validation_preserves_errors_and_indices() {
    for (rule, expected) in [
        (
            r#"{ matcher = ["db.t"], columns = ["/unterminated"] }"#,
            "filter 0 columns",
        ),
        (
            r#"{ matcher = ["db.t"], colums = ["*"] }"#,
            "unknown TOML keys: filter.colums",
        ),
        (
            r#"{ matcher = ["db.t"] }"#,
            "requires at least one column rule",
        ),
        (
            r#"{ matcher = ["db.t"], columns = [] }"#,
            "requires at least one column rule",
        ),
        (r#"{ columns = ["*"] }"#, "requires at least one matcher"),
        (
            r#"{ matcher = ["/unterminated"], columns = ["*"] }"#,
            "filter 0 matcher",
        ),
        ("{", "failed to parse --column-filter 0"),
        (
            r#"{ matcher = "db.t", columns = ["*"] }"#,
            "matcher must be an array",
        ),
        (
            r#"{ matcher = ["db.t"], columns = [1] }"#,
            "columns must contain strings",
        ),
    ] {
        let error = parseColumnFilterArgs(&[rule.into()], false).unwrap_err();
        assert!(error.msg.contains(expected), "{}", error.msg);
    }
    let error = parseColumnFilterArgs(
        &[
            r#"{matcher=["*.*"],columns=["*"]}"#.into(),
            r#"{matcher=["*.*"]}"#.into(),
        ],
        false,
    )
    .unwrap_err();
    assert!(error.msg.contains("filter 1 requires"));
    assert!(
        parseColumnFilterArgs(&[], false)
            .unwrap_err()
            .msg
            .contains("at least one column filter")
    );
}

#[test]
fn column_filter_file_validation_and_flags_match_inline_rules() {
    let path = std::env::temp_dir().join(format!(
        "dumpling-column-filter-{}.toml",
        std::process::id()
    ));
    for (content, expected) in [
        ("", "requires at least one column filter"),
        (
            "[[filters]]\nmatcher=['db.t']\ncolums=['*']",
            "unknown TOML keys: filters.colums",
        ),
        (
            "[[filters]]\nmatcher=['db.t']",
            "requires at least one column rule",
        ),
        (
            "[[filters]]\nmatcher=['db.t']\ncolumns=[]",
            "requires at least one column rule",
        ),
        (
            "[[filters]]\nmatcher=['db.t']\ncolumns=['/unterminated']",
            "filter 0 columns",
        ),
        ("[[", "failed to parse --column-filter-file"),
    ] {
        std::fs::write(&path, content).unwrap();
        let error = parseColumnFilterConfig(path.to_str().unwrap(), false).unwrap_err();
        assert!(error.msg.contains(expected), "{}", error.msg);
    }
    std::fs::write(
        &path,
        "[[filters]]\nmatcher=['db1.t1','db2.t2']\ncolumns=['c1','C2']",
    )
    .unwrap();
    for sensitive in [false, true] {
        let filter = parseColumnFilterConfig(path.to_str().unwrap(), sensitive).unwrap();
        assert_eq!(
            filter
                .applyToColumns("DB1", "T1", &["c1".into(), "C2".into(), "c3".into()])
                .unwrap()
                .0,
            if sensitive {
                vec!["c1", "C2", "c3"]
            } else {
                vec!["c1", "C2"]
            }
        );
    }
    let mut conf = DefaultConfig();
    let mut flags = FlagSet::new();
    conf.DefineFlags(&mut flags);
    flags
        .Parse(&["--column-filter-file", path.to_str().unwrap()])
        .unwrap();
    conf.ParseFromFlags(&flags).unwrap();
    assert_eq!(conf.columnFilter.Filters.len(), 1);
    let args = vec![r#"{matcher=["*.*"],columns=["*"]}"#.into()];
    assert!(
        conf.parseColumnFilterOptions(&args, path.to_str().unwrap(), false)
            .unwrap_err()
            .msg
            .contains("can't specify both --column-filter and --column-filter-file")
    );
    conf.SQL = "select * from t".into();
    assert!(
        conf.parseColumnFilterOptions(&args, "", false)
            .unwrap_err()
            .msg
            .contains("both --sql and --column-filter")
    );
    assert!(
        conf.parseColumnFilterOptions(&[], path.to_str().unwrap(), false)
            .unwrap_err()
            .msg
            .contains("both --sql and --column-filter-file")
    );
    std::fs::remove_file(&path).unwrap();
    assert!(
        parseColumnFilterConfig(path.to_str().unwrap(), false)
            .unwrap_err()
            .msg
            .contains("failed to read --column-filter-file")
    );
}

#[test]
fn column_filter_decode_precedes_compile_and_unknown_keys_keep_input_order() {
    let error = parseColumnFilterArgs(
        &[
            r#"{matcher=["db.t"],columns=["/unterminated"]}"#.into(),
            "{".into(),
        ],
        false,
    )
    .unwrap_err();
    assert!(error.msg.contains("failed to parse --column-filter 1"));
    let error = parseColumnFilterArgs(
        &[r#"{matcher=["db.t"],columns=["*"],z=1,a=2}"#.into()],
        false,
    )
    .unwrap_err();
    assert!(
        error.msg.contains("unknown TOML keys: filter.z, filter.a"),
        "{}",
        error.msg
    );
}
