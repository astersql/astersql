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

#[test]
fn csv_generated_columns_count_line_terminators_and_rows() {
    let ctx = tcontext::Background();
    let mut conf = default_config_for_test();
    conf.CsvLineTerminator = "\r\n".into();
    let mut meta = mockTableIR::new("db", "generated", vec![], &[], &["INT"]);
    meta.selected_field.clear();
    let mut ir = mockTableIR::new(
        "db",
        "generated",
        vec![
            vec![bytes_cell("1")],
            vec![bytes_cell("2")],
            vec![bytes_cell("3")],
        ],
        &[],
        &["INT"],
    );
    let mut sink = BufferWriter(vec![]);
    let metrics = newMetrics(conf.PromFactory.as_ref(), &conf.Labels);
    WriteInsertInCsv(&ctx, &conf, &meta, &mut ir, &mut sink, Some(&metrics)).unwrap();
    assert_eq!(sink.0, b"\r\n\r\n\r\n");
    assert_eq!(ReadGauge(Some(&metrics.finishedRowsGauge)), 3.0);
    assert_eq!(ReadGauge(Some(&metrics.finishedSizeGauge)), 6.0);
}

#[test]
fn csv_failure_rolls_back_periodic_metrics() {
    struct FailAfter {
        calls: usize,
    }
    impl ObjectWriter for FailAfter {
        fn Write(&mut self, data: &[u8]) -> Result<usize> {
            self.calls += 1;
            if self.calls == 1001 {
                Err(errors_new("write failed"))
            } else {
                Ok(data.len())
            }
        }
        fn Close(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let mut conf = default_config_for_test();
    conf.NoHeader = true;
    let metrics = newMetrics(conf.PromFactory.as_ref(), &conf.Labels);
    let meta = mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    let mut ir = mockTableIR::new("db", "t", vec![vec![bytes_cell("1")]; 1002], &[], &["INT"]);
    let err = WriteInsertInCsv(
        &tcontext::Background(),
        &conf,
        &meta,
        &mut ir,
        &mut FailAfter { calls: 0 },
        Some(&metrics),
    )
    .unwrap_err();
    assert!(err.to_string().contains("write failed"));
    assert_eq!(ReadGauge(Some(&metrics.finishedRowsGauge)), 0.0);
    assert_eq!(ReadGauge(Some(&metrics.finishedSizeGauge)), 0.0);
}

#[test]
fn multipart_limit_annotation_preserves_sentinel_only() {
    let annotated = annotatePartLimit(astersql_objstore_storeapi::ErrExceedMaxUploadParts.into());
    assert!(annotated.exceed_upload_parts);
    assert!(annotated.to_string().contains("specify --filesize (-F)"));
    let ordinary = errors_new(astersql_objstore_storeapi::ErrExceedMaxUploadParts.to_string());
    assert!(
        !annotatePartLimit(ordinary)
            .to_string()
            .contains("specify --filesize")
    );
}

#[test]
fn csv_upload_limit_error_retains_identity_through_io_adapter() {
    struct LimitWriter;
    impl ObjectWriter for LimitWriter {
        fn Write(&mut self, _: &[u8]) -> Result<usize> {
            Err(astersql_objstore_storeapi::ErrExceedMaxUploadParts.into())
        }
        fn Close(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let mut conf = default_config_for_test();
    conf.NoHeader = true;
    let meta = mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    let mut ir = mockTableIR::new("db", "t", vec![vec![bytes_cell("1")]], &[], &["INT"]);
    let error = WriteInsertInCsv(
        &tcontext::Background(),
        &conf,
        &meta,
        &mut ir,
        &mut LimitWriter,
        None,
    )
    .unwrap_err();
    assert!(error.exceed_upload_parts);
    assert!(error.to_string().contains("specify --filesize (-F)"));
}

#[test]
fn sql_statement_limit_splits_before_next_row() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let mut conf = default_config_for_test();
    conf.StatementSize = 1;
    let meta = mockTableIR::new("test", "t", vec![], &[], &["INT"]);
    let mut ir = mockTableIR::new(
        "test",
        "t",
        vec![vec![bytes_cell("1")], vec![bytes_cell("2")]],
        &[],
        &["INT"],
    );
    let mut writer = BufferWriter(vec![]);
    WriteInsertSQL(&tctx, &conf, &meta, &mut ir, &mut writer, None).unwrap();
    assert_eq!(
        writer.0,
        b"INSERT INTO `t` VALUES\n(1);\nINSERT INTO `t` VALUES\n(2);\n"
    );
}

#[test]
fn sql_failure_rolls_back_periodic_metrics() {
    struct FailAfter {
        calls: usize,
    }
    impl ObjectWriter for FailAfter {
        fn Write(&mut self, data: &[u8]) -> Result<usize> {
            self.calls += 1;
            if self.calls == 1001 {
                Err(errors_new("write failed"))
            } else {
                Ok(data.len())
            }
        }
        fn Close(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let mut conf = default_config_for_test();
    conf.NoHeader = true;
    let metrics = newMetrics(conf.PromFactory.as_ref(), &conf.Labels);
    let meta = mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    let mut ir = mockTableIR::new("db", "t", vec![vec![bytes_cell("1")]; 1002], &[], &["INT"]);
    let err = WriteInsertSQL(
        &tcontext::Background(),
        &conf,
        &meta,
        &mut ir,
        &mut FailAfter { calls: 0 },
        Some(&metrics),
    )
    .unwrap_err();
    assert!(err.to_string().contains("write failed"));
    assert_eq!(ReadGauge(Some(&metrics.finishedRowsGauge)), 0.0);
    assert_eq!(ReadGauge(Some(&metrics.finishedSizeGauge)), 0.0);
}
