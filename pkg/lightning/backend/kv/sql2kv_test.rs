// Copyright 2019 PingCAP, Inc.
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

// SQL→KV 编码器（tableKVEncoder）单元测试与基准辅助。
//
// 覆盖编码/解码往返、索引 Handle 还原、行格式 v2、时间戳、自增缺省、
// 生成列、AUTO_RANDOM / 分片 RowID，以及 ClassifyAndAppend 校验和分类。

use std::sync::Arc;

use encode::{Column, ColumnType, Datum, EncodingConfig, Row, SessionOptions, Table};
use verification::{KvPair, NewKVChecksum};

use crate::*;

/// 构造仅含列名的默认 Column。
fn column(name: &str) -> Column {
    Column {
        name: name.into(),
        ..Default::default()
    }
}

/// 将 TableDefinition 包装为 EncodingConfig。
fn configForTable(table: TableDefinition) -> EncodingConfig {
    let table: Arc<dyn Table> = Arc::new(table);
    EncodingConfig {
        Table: Some(table),
        ..Default::default()
    }
}

/// 测试辅助：直接构造 BaseKVEncoder。
pub(crate) fn makeBaseEncoder(table: TableDefinition) -> BaseKVEncoder {
    NewBaseKVEncoder(&configForTable(table)).unwrap()
}

/// 带自增主键与二级索引的样例表。
fn mockTableInfo(name: &str) -> TableDefinition {
    TableDefinition {
        name: name.into(),
        id: 42,
        columns: vec![
            Column {
                name: "id".into(),
                auto_increment: true,
                primary_key: true,
                ..Default::default()
            },
            column("name"),
        ],
        indices: vec![IndexDefinition {
            id: 2,
            columns: vec![1],
            primary: false,
            unique: false,
        }],
        pk_is_handle: true,
        ..Default::default()
    }
}

/// 从 dyn Row 提取 Pairs（丢弃 RowID）。
fn fromRow(row: &dyn Row) -> Pairs {
    Pairs {
        Pairs: Row2KvPairs(row),
        RowID: Vec::new(),
    }
}

/// 从记录首列提取 Handle 的桩。
struct mockTable;

impl mockTable {
    fn AddRecord(record: &[Datum]) -> i64 {
        match record.first() {
            Some(Datum::Int(handle)) => *handle,
            _ => 0,
        }
    }
}

/// 日志数组编组：Null / Int 等类型名与展示值。
#[test]
fn TestMarshal() {
    let row = [Datum::Null, Datum::Int(7), Datum::String("hello".into())];
    let marshalled = RowArrayMarshaller(&row).MarshalLogArray();
    assert_eq!(marshalled[0], ("null".into(), "NULL".into()));
    assert_eq!(marshalled[1], ("int64".into(), "7".into()));
}

/// 扩展 Datum 经 canonical 类型适配和 TiDB 行格式可按列元数据往返。
#[test]
fn TestExtendedDatumRoundTrip() {
    assert_eq!(
        fromCanonicalDatum(&toCanonicalDatum(&Datum::MinNotNull).unwrap(), None).unwrap(),
        Datum::MinNotNull
    );
    assert_eq!(
        fromCanonicalDatum(&toCanonicalDatum(&Datum::MaxValue).unwrap(), None).unwrap(),
        Datum::MaxValue
    );
    let datums = vec![
        Datum::Json(r#"{"a": 1}"#.into()),
        Datum::BinaryLiteral(vec![0xab]),
        Datum::Bit(vec![0xff]),
        Datum::Enum {
            name: "enum-name".into(),
            value: 1,
        },
        Datum::Set {
            name: "a,c".into(),
            value: 5,
        },
        Datum::Duration("12:34:56".into()),
    ];
    let columns = vec![
        Column {
            column_type: ColumnType::Json,
            ..Default::default()
        },
        Column {
            column_type: ColumnType::BinaryLiteral,
            ..Default::default()
        },
        Column {
            column_type: ColumnType::Bit,
            ..Default::default()
        },
        Column {
            column_type: ColumnType::Enum,
            elements: vec!["enum-name".into()],
            ..Default::default()
        },
        Column {
            column_type: ColumnType::Set,
            elements: vec!["a".into(), "b".into(), "c".into()],
            ..Default::default()
        },
        Column {
            column_type: ColumnType::Duration,
            ..Default::default()
        },
    ];
    let encoded = encodeCanonicalRow(&datums, &columns, false).unwrap();
    assert_eq!(decodeCanonicalRow(&encoded, &columns).unwrap(), datums);
}

/// 基本 Encode：产出记录键与索引键，Session 时间戳已初始化。
#[test]
fn TestEncode() {
    let table = mockTableInfo("t");
    let mut encoder = NewTableKVEncoder(&configForTable(table)).unwrap();
    let row = encoder
        .Encode(
            &[Datum::Int(5), Datum::String("alice".into())],
            1,
            &[0, 1],
            0,
        )
        .unwrap();
    let pairs = fromRow(row.as_ref());
    assert_eq!(pairs.Pairs.len(), 2);
    assert_eq!(mockTable::AddRecord(&[Datum::Int(5)]), 5);
    assert!(pairs.Pairs.iter().any(|pair| isRecordKey(&pair.key)));
    assert_eq!(
        GetSession4test(encoder.as_ref())
            .GetExprCtx()
            .CurrentTimestamp
            > 0,
        true
    );
}

/// 编码后再用 TableKVDecoder 解码 Handle 与行数据。
#[test]
fn TestDecode() {
    let table = mockTableInfo("t");
    let mut encoder = NewTableKVEncoder(&configForTable(table.clone())).unwrap();
    let row = encoder
        .Encode(
            &[Datum::Int(5), Datum::String("alice".into())],
            1,
            &[0, 1],
            0,
        )
        .unwrap();
    let record = Row2KvPairs(row.as_ref())
        .into_iter()
        .find(|pair| isRecordKey(&pair.key))
        .unwrap();
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    assert!(decoder.Session().GetExprCtx().CurrentTimestamp > 0);
    let handle = decoder.DecodeHandleFromRowKey(&record.key).unwrap();
    assert_eq!(handle, Handle::Int(5));
    assert_eq!(
        decoder.DecodeRawRowData(&handle, &record.val).unwrap().0,
        vec![Datum::Int(5), Datum::String("alice".into())]
    );
}

/// 从二级索引键还原整数 Handle。
#[test]
fn TestDecodeIndex() {
    let table = mockTableInfo("t");
    let mut encoder = NewTableKVEncoder(&configForTable(table.clone())).unwrap();
    let row = encoder
        .Encode(&[Datum::Int(9), Datum::String("bob".into())], 1, &[0, 1], 0)
        .unwrap();
    let index = Row2KvPairs(row.as_ref())
        .into_iter()
        .find(|pair| !isRecordKey(&pair.key))
        .unwrap();
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    assert_eq!(
        decoder
            .DecodeHandleFromIndex(2, &index.key, &index.val)
            .unwrap(),
        Handle::Int(9)
    );
}

/// 唯一索引的 handle 位于 index value，仍由 canonical tablecodec 正确反解。
#[test]
fn TestDecodeUniqueIndex() {
    let mut table = mockTableInfo("t");
    table.indices[0].unique = true;
    let mut encoder = NewTableKVEncoder(&configForTable(table.clone())).unwrap();
    let row = encoder
        .Encode(
            &[Datum::Int(11), Datum::String("unique".into())],
            1,
            &[0, 1],
            0,
        )
        .unwrap();
    let index = Row2KvPairs(row.as_ref())
        .into_iter()
        .find(|pair| !isRecordKey(&pair.key))
        .unwrap();
    assert!(!index.val.is_empty());
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    assert_eq!(
        decoder
            .DecodeHandleFromIndex(2, &index.key, &index.val)
            .unwrap(),
        Handle::Int(11)
    );
}

/// tidb_row_format_version=2 时编码/解码 Bytes 与 Null。
#[test]
fn TestEncodeRowFormatV2() {
    let table = TableDefinition {
        columns: vec![
            Column {
                name: "a".into(),
                column_type: ColumnType::Bytes,
                charset: "binary".into(),
                ..Default::default()
            },
            Column {
                name: "b".into(),
                column_type: ColumnType::String,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut config = configForTable(table.clone());
    config
        .SessionOptions
        .SysVars
        .insert("tidb_row_format_version".into(), "2".into());
    let mut encoder = NewTableKVEncoder(&config).unwrap();
    let row = encoder
        .Encode(&[Datum::Bytes(vec![0, 1, 2]), Datum::Null], 3, &[0, 1], 0)
        .unwrap();
    let record = Row2KvPairs(row.as_ref())
        .into_iter()
        .find(|pair| isRecordKey(&pair.key))
        .unwrap();
    let decoder = NewTableKVDecoder(table, "t", &config.SessionOptions).unwrap();
    assert_eq!(
        decoder
            .DecodeRawRowData(&Handle::Int(3), &record.val)
            .unwrap()
            .0,
        vec![Datum::Bytes(vec![0, 1, 2]), Datum::Null]
    );
}

/// Timestamp Datum 经编码后可由 decodeDatumList 还原。
#[test]
fn TestEncodeTimestamp() {
    let table = TableDefinition {
        columns: vec![Column {
            name: "ts".into(),
            column_type: ColumnType::Timestamp,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut encoder = NewTableKVEncoder(&configForTable(table.clone())).unwrap();
    let expected = Datum::Timestamp("2026-07-19 12:34:56".into());
    let row = encoder
        .Encode(std::slice::from_ref(&expected), 1, &[0], 0)
        .unwrap();
    let record = Row2KvPairs(row.as_ref())
        .into_iter()
        .find(|pair| isRecordKey(&pair.key))
        .unwrap();
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    assert_eq!(
        decoder
            .DecodeRawRowData(&Handle::Int(1), &record.val)
            .unwrap()
            .0,
        vec![expected]
    );
}

/// 浮点自增列：GetAutoRecordID 按四舍五入取整。
#[test]
fn TestEncodeDoubleAutoIncrement() {
    assert_eq!(
        GetAutoRecordID(&Datum::Float(1.6), AutoIDFieldType::Float),
        2
    );
    assert_eq!(
        GetAutoRecordID(&Datum::Float(-1.6), AutoIDFieldType::Float),
        -2
    );
}

/// 缺省自增值：permutation 为 -1 时用 rowID 填充。
#[test]
fn TestEncodeMissingAutoValue() {
    let table = TableDefinition {
        columns: vec![Column {
            name: "id".into(),
            auto_increment: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut encoder = NewTableKVEncoder(&configForTable(table)).unwrap();
    let row = encoder.Encode(&[], 17, &[-1], 0).unwrap();
    let record = Row2KvPairs(row.as_ref())
        .into_iter()
        .find(|pair| isRecordKey(&pair.key))
        .unwrap();
    assert_eq!(decodeDatumList(&record.val).unwrap(), vec![Datum::Int(17)]);
    assert_eq!(
        GetEncoderSe(encoder.as_ref())
            .GetTableCtx()
            .RowEncodingEnabled,
        false
    );
}

/// 生成列：sum = a + b，编码结果含求值后的第三列。
#[test]
fn TestEncodeExpressionColumn() {
    let table = TableDefinition {
        columns: vec![
            column("a"),
            column("b"),
            Column {
                name: "sum".into(),
                generated: true,
                ..Default::default()
            },
        ],
        generated: [(2, GeneratedExpression::Add(0, 1))].into(),
        ..Default::default()
    };
    let mut encoder = NewTableKVEncoder(&configForTable(table)).unwrap();
    let row = encoder
        .Encode(&[Datum::Int(2), Datum::Int(5)], 1, &[0, 1, -1], 0)
        .unwrap();
    let record = Row2KvPairs(row.as_ref())
        .into_iter()
        .find(|pair| isRecordKey(&pair.key))
        .unwrap();
    assert_eq!(
        decodeDatumList(&record.val).unwrap(),
        vec![Datum::Int(2), Datum::Int(5), Datum::Int(7)]
    );
}

/// 相同 AutoRandomSeed 下增量 ID 函数确定且与原始 id 不同。
#[test]
fn TestDefaultAutoRandoms() {
    let table = TableDefinition {
        columns: vec![Column {
            name: "id".into(),
            auto_random: true,
            ..Default::default()
        }],
        auto_random_bits: 5,
        ..Default::default()
    };
    let mut config = configForTable(table);
    config.SessionOptions.AutoRandomSeed = 123;
    let encoder1 = NewTableKVEncoder(&config).unwrap();
    let encoder2 = NewTableKVEncoder(&config).unwrap();
    assert_eq!(
        GetEncoderIncrementalID(encoder1.as_ref(), 8),
        GetEncoderIncrementalID(encoder2.as_ref(), 8)
    );
    assert_ne!(GetEncoderIncrementalID(encoder1.as_ref(), 8), 8);
}

/// 分片 RowID：高位写入 shard 位，低 59 位保留原始 id。
#[test]
fn TestShardRowId() {
    let table = TableDefinition {
        columns: vec![column("v")],
        shard_row_id_bits: 4,
        ..Default::default()
    };
    let mut config = configForTable(table);
    config.SessionOptions.AutoRandomSeed = 7;
    let encoder = NewTableKVEncoder(&config).unwrap();
    let first = GetEncoderIncrementalID(encoder.as_ref(), 10);
    assert_ne!(first, 10);
    assert_eq!(first & ((1_i64 << 59) - 1), 10);
}

/// 记录键与索引键分别进入 data / indices，并更新校验和；ClearRow 清空。
#[test]
fn TestClassifyAndAppend() {
    let mut row = MakeRowFromKvPairs(vec![
        KvPair {
            key: b"t12345678_r1".to_vec(),
            val: b"record".to_vec(),
        },
        KvPair {
            key: b"t12345678_i2_x_h1".to_vec(),
            val: Vec::new(),
        },
    ]);
    let mut data = MakeRowsFromKvPairs(Vec::new());
    let mut indices = MakeRowsFromKvPairs(Vec::new());
    let mut data_checksum = NewKVChecksum();
    let mut index_checksum = NewKVChecksum();
    row.ClassifyAndAppend(
        &mut data,
        &mut data_checksum,
        &mut indices,
        &mut index_checksum,
    );
    assert_eq!(Rows2KvPairs(data.as_ref()).len(), 1);
    assert_eq!(Rows2KvPairs(indices.as_ref()).len(), 1);
    assert_eq!(data_checksum.SumKVS(), 1);
    assert_eq!(index_checksum.SumKVS(), 1);
    ClearRow(row.as_mut());
    assert!(Row2KvPairs(row.as_ref()).is_empty());
}

/// Go checks the tablecodec discriminator at its fixed offset. An `_r` occurring
/// later in an index key must not turn that index entry into record data.
#[test]
fn classify_does_not_treat_embedded_record_marker_as_a_record_key() {
    assert!(isRecordKey(b"t12345678_rpayload"));
    assert!(!isRecordKey(b"t12345678_i_rpayload"));
}

/// Go deliberately leaves GroupedPairs' Rows-only operations unsupported.
#[test]
#[should_panic(expected = "not implemented")]
fn grouped_pairs_split_into_chunks_remains_unsupported() {
    let groups = GroupedPairs::default();
    let _ = groups.SplitIntoChunks(1);
}

/// Go deliberately leaves GroupedPairs' Rows-only operations unsupported.
#[test]
#[should_panic(expected = "not implemented")]
fn grouped_pairs_clear_remains_unsupported() {
    let rows: Box<dyn encode::Rows> = Box::new(GroupedPairs::default());
    let _ = rows.Clear();
}

/// 基准套件：持有可复用的 tableKVEncoder。
pub(crate) struct benchSQL2KVSuite {
    encoder: Box<dyn encode::Encoder>,
}

/// 初始化基准编码器。
pub(crate) fn SetUpTest() -> benchSQL2KVSuite {
    benchSQL2KVSuite {
        encoder: NewTableKVEncoder(&configForTable(mockTableInfo("bench"))).unwrap(),
    }
}

/// 轻量吞吐检查：连续编码 100 行且 Size>0。
#[test]
fn BenchmarkSQL2KV() {
    let mut suite = SetUpTest();
    for id in 0..100 {
        let row = suite
            .encoder
            .Encode(
                &[Datum::Int(id), Datum::String("value".into())],
                id,
                &[0, 1],
                0,
            )
            .unwrap();
        assert_eq!(row.Size() > 0, true);
    }
}
