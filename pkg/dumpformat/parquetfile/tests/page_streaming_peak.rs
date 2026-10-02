// Copyright 2026 AsterSQL.
// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use parquet::basic::Compression;
use parquet::column::reader::ColumnReader;
use parquet::data_type::{ByteArray, ByteArrayType};
use parquet::file::properties::WriterProperties;
use parquet::file::reader::FileReader;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Allocator;
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            if ACTIVE.load(Ordering::Relaxed) {
                PEAK.fetch_max(now, Ordering::Relaxed);
            }
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn large_plain_page_reads_values_and_eof_with_bounded_peak() {
    let mut file = tempfile::tempfile().unwrap();
    let schema = Arc::new(
        parse_message_type("message schema { OPTIONAL BYTE_ARRAY data (UTF8); }").unwrap(),
    );
    let props = Arc::new(
        WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_compression(Compression::UNCOMPRESSED)
            .set_data_page_size_limit(64 << 20)
            .build(),
    );
    let mut writer = SerializedFileWriter::new(&mut file, schema, props).unwrap();
    let mut group = writer.next_row_group().unwrap();
    let mut column = group.next_column().unwrap().unwrap();
    let values: Vec<ByteArray> = (0..64)
        .map(|row| ByteArray::from((0..512 * 1024).map(|i| (row + i) as u8).collect::<Vec<_>>()))
        .collect();
    column
        .typed::<ByteArrayType>()
        .write_batch(&values, Some(&[1; 64]), None)
        .unwrap();
    column.close().unwrap();
    group.close().unwrap();
    writer.close().unwrap();
    drop(values);
    let baseline = CURRENT.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
    let reader = astersql_dumpformat_parquetfile::parser::open_file_reader(file).unwrap();
    assert!(reader.metadata().row_group(0).column(0).uncompressed_size() > 20 << 20);
    let group = reader.get_row_group(0).unwrap();
    let ColumnReader::ByteArrayColumnReader(mut column) = group.get_column_reader(0).unwrap()
    else {
        panic!("byte column")
    };
    let mut row = 0;
    while row < 64 {
        let mut values = vec![];
        let mut levels = vec![];
        let (records, read_values, read_levels) = column
            .read_records(
                astersql_dumpformat_parquetfile::parser::READ_BATCH_SIZE,
                Some(&mut levels),
                None,
                &mut values,
            )
            .unwrap();
        assert!(records > 0);
        assert_eq!(records, read_values);
        assert_eq!(records, read_levels);
        assert!(levels.iter().all(|level| *level == 1));
        for value in values {
            assert_eq!(value.len(), 512 * 1024);
            assert!(
                value
                    .data()
                    .iter()
                    .enumerate()
                    .all(|(i, b)| *b == (row + i) as u8)
            );
            row += 1;
        }
    }
    assert_eq!(row, 64);
    assert_eq!(
        column
            .read_records(1, Some(&mut vec![]), None, &mut vec![])
            .unwrap(),
        (0, 0, 0)
    );
    ACTIVE.store(false, Ordering::Relaxed);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    println!("streaming reader peak: {peak}");
    assert!(
        peak < 4 << 20,
        "reader peak {peak} exceeds streaming budget"
    );
}
