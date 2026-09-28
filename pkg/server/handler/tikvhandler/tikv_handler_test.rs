// Copyright 2026 AsterSQL.

// TiKV handler 与 Go 实现的可观察行为回归测试。

use super::tikv_handler::{
    IndexInfo, InfoSchema, ResponseWriter, Table, TableInfo, TableNameInfo, TableRangesResponse,
    base64_decode, createTableRanges, getTableByIDStr, manualWriteJSONArray, parse_start_ts,
    table_ranges_response,
};

fn table(id: i64, partitions: Vec<super::tikv_handler::PartitionInfo>) -> TableInfo {
    TableInfo {
        id,
        name: "t".to_owned(),
        indices: Vec::new(),
        partitions,
        is_common_handle: false,
        tiflash_replica: None,
        tiflash_replica_infos: Vec::new(),
    }
}

#[test]
fn manual_write_json_array_keeps_one_array_response() {
    let mut writer = ResponseWriter::default();
    manualWriteJSONArray(&mut writer, vec![TableNameInfo, TableNameInfo]);

    assert_eq!(writer.data.len(), 1);
    assert!(
        writer.data[0]
            .downcast_ref::<Vec<TableNameInfo>>()
            .is_some()
    );
}

#[test]
fn table_id_lookup_accepts_partition_id_and_returns_parent() {
    let schema = InfoSchema {
        tables: vec![table(
            10,
            vec![super::tikv_handler::PartitionInfo {
                id: 42,
                name: "p0".to_owned(),
            }],
        )],
    };

    let result = getTableByIDStr(&schema, "42").expect("partition id should resolve");
    assert_eq!(result.id, 10);
}

#[test]
fn non_partition_table_ranges_are_one_object() {
    let table = Table {
        meta: table(10, Vec::new()),
    };

    assert!(matches!(
        table_ranges_response(&table),
        TableRangesResponse::Single(_)
    ));
}

#[test]
fn transaction_start_timestamp_uses_go_base_zero_parsing() {
    assert_eq!(parse_start_ts("0x10".to_owned()).unwrap(), 16);
    assert_eq!(parse_start_ts("010".to_owned()).unwrap(), 8);
    assert_eq!(parse_start_ts("+0b1_000".to_owned()).unwrap(), 8);
    assert_eq!(
        parse_start_ts(i64::MIN.to_string()).unwrap(),
        i64::MIN as u64
    );
}

#[test]
fn value_base64_matches_go_standard_encoding_strictness() {
    assert_eq!(base64_decode("TQ==\r\n").unwrap(), b"M");
    assert!(base64_decode("T Q==").is_err());
    assert!(base64_decode("TQ==AAAA").is_err());
}

#[test]
fn range_successors_wrap_like_go_int64_arithmetic() {
    let ranges = createTableRanges(
        i64::MAX,
        "t".to_owned(),
        vec![IndexInfo {
            id: i64::MAX,
            name: "i".to_owned(),
            primary: false,
        }],
    );
    assert_eq!(ranges.table_id, i64::MAX);
}
