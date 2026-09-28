// Copyright 2026 AsterSQL.

use crate::util::{
    Begin, COLUMN_FLAG, COLUMN_ID, COLUMN_LEN, COLUMN_TP, CONTENT_TYPE_JSON, ColumnFlag, ColumnID,
    ColumnLen, ColumnTp, DUMP_PARTITION_STATS, DumpPartitionStats, END, End, Error,
    ExtractTableAndPartitionName, FILE_NAME, FileName, HEADER_CONTENT_TYPE, HeaderContentType,
    ID_NAME_ONLY, IDNameOnly, IS_DUMP, IS_HISTORY_VIEW, IS_SKIP_STATS, IsDump, IsHistoryView,
    IsSkipStats, JOB_ID, JobID, LIMIT, Limit, ROW_BIN, ResponseWriter, RowBin, SECONDS, SNAPSHOT,
    Seconds, Snapshot, TABLE_ID_QUERY, TABLE_IDS_QUERY, TYPE, TableIDQuery, TableIDsQuery, Type,
    WriteData, WriteError,
};

#[test]
fn go_exported_constant_spellings_are_preserved() {
    let pairs = [
        (ColumnID, COLUMN_ID),
        (ColumnTp, COLUMN_TP),
        (ColumnFlag, COLUMN_FLAG),
        (ColumnLen, COLUMN_LEN),
        (RowBin, ROW_BIN),
        (Snapshot, SNAPSHOT),
        (FileName, FILE_NAME),
        (DumpPartitionStats, DUMP_PARTITION_STATS),
        (Begin, crate::util::BEGIN),
        (End, END),
        (Type, TYPE),
        (IsDump, IS_DUMP),
        (IsSkipStats, IS_SKIP_STATS),
        (IsHistoryView, IS_HISTORY_VIEW),
        (TableIDQuery, TABLE_ID_QUERY),
        (TableIDsQuery, TABLE_IDS_QUERY),
        (IDNameOnly, ID_NAME_ONLY),
        (Limit, LIMIT),
        (JobID, JOB_ID),
        (Seconds, SECONDS),
        (HeaderContentType, HEADER_CONTENT_TYPE),
        (crate::util::ContentTypeJSON, CONTENT_TYPE_JSON),
    ];

    assert!(
        pairs
            .iter()
            .all(|(go_name, rust_name)| go_name == rust_name)
    );
}

#[test]
fn response_helpers_match_go_status_body_and_json_contract() {
    let mut error_writer = ResponseWriter::default();
    WriteError(&mut error_writer, Error::new("bad input"));
    assert_eq!(error_writer.status_code(), Some(400));
    assert_eq!(error_writer.body_bytes(), b"bad input");

    let mut data_writer = ResponseWriter::default();
    WriteData(&mut data_writer, "quote: \" newline:\n");
    assert_eq!(data_writer.status_code(), Some(200));
    assert_eq!(data_writer.body_bytes(), br#""quote: \" newline:\n""#);
}

#[test]
fn table_and_partition_extraction_matches_go_branches() {
    assert_eq!(
        ExtractTableAndPartitionName("orders(p0)"),
        ("orders".to_owned(), "p0".to_owned())
    );
    assert_eq!(
        ExtractTableAndPartitionName("orders"),
        ("orders".to_owned(), String::new())
    );
    assert_eq!(
        ExtractTableAndPartitionName("orders(p0"),
        ("orders(p0".to_owned(), String::new())
    );
    assert_eq!(
        ExtractTableAndPartitionName("orders(p0)ignored"),
        ("orders".to_owned(), "p0".to_owned())
    );
}
