// Copyright 2026 AsterSQL.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::mem_reader::{
    ColumnInfo, Datum, FieldType, Handle, KeyRange, KvPair, MemReaderBackend, MemReaderError,
    TableInfo, UnionScanSpec, buildMemTableReader, compareExec,
};

#[derive(Default)]
struct CommonHandleBackend;

impl MemReaderBackend for CommonHandleBackend {
    fn txn_snapshot(&self, _: &KeyRange, _: bool) -> Result<Vec<KvPair>, MemReaderError> {
        Ok(Vec::new())
    }

    fn temporary_snapshot(
        &self,
        _: &KeyRange,
        _: bool,
    ) -> Result<Option<Vec<KvPair>>, MemReaderError> {
        Ok(None)
    }

    fn cache_snapshot(&self, _: &KeyRange, _: bool) -> Result<Option<Vec<KvPair>>, MemReaderError> {
        Ok(None)
    }

    fn decode_index_values(
        &self,
        _: &[u8],
        _: &[u8],
        _: usize,
        _: &[FieldType],
    ) -> Result<Vec<Datum>, MemReaderError> {
        unreachable!()
    }

    fn decode_row_handle(&self, _: &[u8]) -> Result<Handle, MemReaderError> {
        unreachable!()
    }

    fn decode_common_handle_column(
        &self,
        handle: &Handle,
        index: usize,
    ) -> Result<Datum, MemReaderError> {
        let encoded = match handle {
            Handle::Common(encoded) => encoded,
            Handle::Partition { inner, .. } => {
                return self.decode_common_handle_column(inner, index);
            }
            Handle::Int(_) => {
                return Err(MemReaderError::Decode(
                    "integer handle cannot provide a common-handle column".into(),
                ));
            }
        };
        let component = encoded
            .split(|byte| *byte == b'|')
            .nth(index)
            .ok_or_else(|| MemReaderError::Decode(format!("missing handle component {index}")))?;
        Ok(Datum::Text(String::from_utf8(component.to_vec()).map_err(
            |error| MemReaderError::Decode(error.to_string()),
        )?))
    }

    fn decode_index_handle(&self, _: &[u8], _: &[u8], _: usize) -> Result<Handle, MemReaderError> {
        unreachable!()
    }

    fn decode_partition_id(&self, _: &[u8], _: &[u8]) -> Result<i64, MemReaderError> {
        unreachable!()
    }

    fn decode_row(
        &self,
        _: &TableInfo,
        _: &[ColumnInfo],
        _: &Handle,
        _: &[u8],
    ) -> Result<BTreeMap<i64, Datum>, MemReaderError> {
        Ok(BTreeMap::new())
    }

    fn default_column_value(&self, _: &ColumnInfo) -> Result<Datum, MemReaderError> {
        Ok(Datum::Null)
    }

    fn evaluate_conditions(&self, _: &[String], _: &[Datum]) -> Result<bool, MemReaderError> {
        Ok(true)
    }

    fn compare_rows(&self, _: &[Datum], _: &[Datum]) -> Result<Ordering, MemReaderError> {
        Ok(Ordering::Equal)
    }

    fn table_handles_to_ranges(
        &self,
        _: i64,
        _: &[Handle],
    ) -> Result<Vec<KeyRange>, MemReaderError> {
        unreachable!()
    }

    fn int_handle_to_common(&self, _: i64) -> Result<Vec<u8>, MemReaderError> {
        unreachable!()
    }
}

#[test]
fn common_handle_fills_each_primary_key_column_from_its_encoded_component() {
    let columns = vec![
        ColumnInfo {
            id: 11,
            offset: 0,
            primary_key: true,
            unsigned: false,
            needs_restored_data: false,
        },
        ColumnInfo {
            id: 12,
            offset: 1,
            primary_key: true,
            unsigned: false,
            needs_restored_data: false,
        },
    ];
    let table = TableInfo {
        id: 7,
        columns: columns.clone(),
        pk_is_handle: false,
        common_handle: true,
        common_pk_column_ids: vec![11, 12],
    };
    let spec = UnionScanSpec {
        backend: Arc::new(CommonHandleBackend),
        table,
        columns,
        conditions: Vec::new(),
        desc: false,
        keep_order: false,
        compare: compareExec::default(),
        physical_table_id_index: None,
        partition_ids: BTreeSet::new(),
    };
    let reader = buildMemTableReader(&spec, Vec::new());

    let row = reader
        .getRowData(&Handle::Common(b"first|second".to_vec()), b"row")
        .unwrap();

    assert_eq!(row.get(&11), Some(&Datum::Text("first".into())));
    assert_eq!(row.get(&12), Some(&Datum::Text("second".into())));

    let partitioned = reader
        .getRowData(
            &Handle::Partition {
                partition_id: 99,
                inner: Box::new(Handle::Common(b"left|right".to_vec())),
            },
            b"row",
        )
        .unwrap();
    assert_eq!(partitioned.get(&11), Some(&Datum::Text("left".into())));
    assert_eq!(partitioned.get(&12), Some(&Datum::Text("right".into())));
}
