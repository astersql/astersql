// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

use crate::main_test::{app_logger, default_config_for_test};
use crate::util_for_test::{bytes_cell, mockTableIR};
use crate::*;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::RowAccessor;

struct BufferWriter(Vec<u8>);

impl ObjectWriter for BufferWriter {
    fn Write(&mut self, data: &[u8]) -> Result<usize> {
        self.0.extend_from_slice(data);
        Ok(data.len())
    }

    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn unknown_file_format_uses_go_fallback_strings() {
    assert_eq!(FileFormat::FileFormatUnknown.String(), "unknown");
    assert_eq!(FileFormat::FileFormatUnknown.Extension(), "unknown_format");
}

#[test]
fn escape_string_only_doubles_backticks_like_go() {
    assert_eq!(escapeString(r"a\b`c"), r"a\b``c");
}

#[test]
fn write_insert_uses_selected_field_independently_of_complete_insert() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let conf = default_config_for_test();
    assert!(!conf.CompleteInsert);

    let mut meta = mockTableIR::new("test", "employee", vec![], &[], &["INT"]);
    meta.selected_field = "`id`".into();
    let mut ir = mockTableIR::new(
        "test",
        "employee",
        vec![vec![bytes_cell("1")]],
        &[],
        &["INT"],
    );
    let mut writer = BufferWriter(vec![]);

    WriteInsertSQL(&tctx, &conf, &meta, &mut ir, &mut writer, None).unwrap();
    assert_eq!(
        String::from_utf8(writer.0).unwrap(),
        "INSERT INTO `employee` (`id`) VALUES\n(1);\n"
    );
}

#[test]
fn write_insert_emits_special_comments_before_sql() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let conf = default_config_for_test();
    let meta = mockTableIR::new(
        "test",
        "employee",
        vec![],
        &["/*!40101 SET NAMES binary */;"],
        &["INT"],
    );
    let mut ir = mockTableIR::new(
        "test",
        "employee",
        vec![vec![bytes_cell("1")]],
        &[],
        &["INT"],
    );
    let mut writer = BufferWriter(vec![]);

    WriteInsertSQL(&tctx, &conf, &meta, &mut ir, &mut writer, None).unwrap();
    assert_eq!(
        String::from_utf8(writer.0).unwrap(),
        "/*!40101 SET NAMES binary */;\nINSERT INTO `employee` VALUES\n(1);\n"
    );
}

#[test]
fn write_insert_preserves_go_empty_selected_field_behavior() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let conf = default_config_for_test();
    let mut meta = mockTableIR::new("test", "generated", vec![], &[], &["INT"]);
    meta.selected_field.clear();
    let mut ir = mockTableIR::new(
        "test",
        "generated",
        vec![vec![bytes_cell("ignored")]],
        &[],
        &["INT"],
    );
    let mut writer = BufferWriter(vec![]);

    WriteInsertSQL(&tctx, &conf, &meta, &mut ir, &mut writer, None).unwrap();
    assert_eq!(
        String::from_utf8(writer.0).unwrap(),
        "INSERT INTO `generated` VALUES\n();\n"
    );
}

#[test]
fn csv_header_and_empty_selected_field_match_go() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let mut conf = default_config_for_test();
    conf.NoHeader = false;
    conf.EscapeBackslash = false;
    conf.CsvDelimiter = "\"".into();
    conf.CsvSeparator = ",".into();
    conf.CsvLineTerminator = "\n".into();

    let mut meta = mockTableIR::new("test", "generated", vec![], &[], &["INT"]);
    meta.selected_field.clear();
    meta.col_names = vec!["a\"b".into()];
    let mut ir = mockTableIR::new(
        "test",
        "generated",
        vec![vec![bytes_cell("ignored")]],
        &[],
        &["INT"],
    );
    let mut writer = BufferWriter(vec![]);

    WriteInsertInCsv(&tctx, &conf, &meta, &mut ir, &mut writer, None).unwrap();
    assert_eq!(String::from_utf8(writer.0).unwrap(), "\n");

    meta.selected_field = "*".into();
    let mut empty_ir = mockTableIR::new("test", "generated", vec![], &[], &["INT"]);
    let mut empty_writer = BufferWriter(vec![]);
    WriteInsertInCsv(&tctx, &conf, &meta, &mut empty_ir, &mut empty_writer, None).unwrap();
    assert!(empty_writer.0.is_empty());

    let mut one_ir = mockTableIR::new(
        "test",
        "generated",
        vec![vec![bytes_cell("1")]],
        &[],
        &["INT"],
    );
    let mut one_writer = BufferWriter(vec![]);
    WriteInsertInCsv(&tctx, &conf, &meta, &mut one_ir, &mut one_writer, None).unwrap();
    assert_eq!(String::from_utf8(one_writer.0).unwrap(), "\"a\"\"b\"\n1\n");
}

#[test]
fn parquet_output_is_readable_and_preserves_rows() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let conf = default_config_for_test();
    let infos = vec![
        ColumnInfo {
            Name: "id".into(),
            DatabaseTypeName: "INT".into(),
            Nullable: false,
            Precision: 0,
            Scale: 0,
        },
        ColumnInfo {
            Name: "name".into(),
            DatabaseTypeName: "VARCHAR".into(),
            Nullable: true,
            Precision: 0,
            Scale: 0,
        },
    ];
    let meta = mockTableIR::with_column_info("test", "people", vec![], &[], infos.clone());
    let mut ir = mockTableIR::with_column_info(
        "test",
        "people",
        vec![
            vec![bytes_cell("1"), bytes_cell("alice")],
            vec![bytes_cell("2"), None],
        ],
        &[],
        infos,
    );
    let mut writer = BufferWriter(vec![]);

    WriteInsertInParquet(&tctx, &conf, &meta, &mut ir, &mut writer, None).unwrap();
    assert_eq!(&writer.0[..4], b"PAR1");
    assert_eq!(&writer.0[writer.0.len() - 4..], b"PAR1");

    let path = std::env::temp_dir().join(format!(
        "astersql-writer-util-{}-{}.parquet",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, &writer.0).unwrap();
    let reader = SerializedFileReader::new(std::fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 2);
    let rows = reader
        .get_row_iter(None)
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows[0].get_int(0).unwrap(), 1);
    assert_eq!(rows[0].get_string(1).unwrap(), "alice");
    assert_eq!(rows[1].get_int(0).unwrap(), 2);
    assert!(rows[1].get_string(1).is_err());
    std::fs::remove_file(path).unwrap();
}
