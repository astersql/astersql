// Copyright 2026 AsterSQL.

use astersql_dumpling_export::{self as export, CompressType};

use crate::config_flags::{DefineFlags, ParseFromFlags};
use crate::stubs::FlagSet;

fn parse_compress(value: &str) -> Result<CompressType, String> {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags.Parse(&["--compress".into(), value.into()])?;
    let mut conf = export::DefaultConfig();
    ParseFromFlags(&mut conf, &flags)?;
    Ok(conf.CompressType)
}

#[test]
fn compress_flag_matches_go_accepted_values() {
    assert_eq!(parse_compress("zst").unwrap(), CompressType::Zstd);
    assert!(parse_compress("none").is_err());
    assert!(parse_compress("uncompressed").is_err());
    assert!(parse_compress("GZIP").is_err());
}

#[test]
fn column_filters_cli_preserves_inline_toml_and_repeated_priority() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags
        .Parse(&[
            "--column-filter".into(),
            r#"{matcher=["db.*"],columns=["*","!c*"]}"#.into(),
            "--column-filter".into(),
            r#"{matcher=["db.t"],columns=["c2"]}"#.into(),
        ])
        .unwrap();
    let mut conf = export::DefaultConfig();
    ParseFromFlags(&mut conf, &flags).unwrap();
    assert_eq!(conf.columnFilter.Filters.len(), 2);
    assert_eq!(
        conf.columnFilter
            .applyToColumns("DB", "T", &["c1".into(), "C2".into(), "d".into()])
            .unwrap(),
        (vec!["C2".into(), "d".into()], vec![1, 2])
    );
}

#[test]
fn column_filters_cli_file_conflict_and_sql_validation() {
    let path = std::env::temp_dir().join(format!("column-filter-cli-{}.toml", std::process::id()));
    std::fs::write(&path, "[[filters]]\nmatcher=['db.t']\ncolumns=['name']").unwrap();
    for (args, expected) in [
        (
            vec!["--column-filter-file".into(), path.to_str().unwrap().into()],
            "",
        ),
        (
            vec![
                "--column-filter-file".into(),
                path.to_str().unwrap().into(),
                "--column-filter".into(),
                r#"{matcher=["*.*"],columns=["*"]}"#.into(),
            ],
            "both --column-filter and --column-filter-file",
        ),
        (
            vec![
                "--column-filter-file".into(),
                path.to_str().unwrap().into(),
                "--sql".into(),
                "select * from t".into(),
            ],
            "both --sql and --column-filter-file",
        ),
    ] {
        let mut flags = FlagSet::new();
        DefineFlags(&mut flags);
        flags.Parse(&args).unwrap();
        let mut conf = export::DefaultConfig();
        let result = ParseFromFlags(&mut conf, &flags);
        if expected.is_empty() {
            result.unwrap();
            assert_eq!(
                conf.columnFilter
                    .applyToColumns("db", "t", &["id".into(), "name".into()])
                    .unwrap()
                    .0,
                vec!["name"]
            );
        } else {
            assert!(result.unwrap_err().contains(expected));
        }
    }
    std::fs::remove_file(path).unwrap();
}
