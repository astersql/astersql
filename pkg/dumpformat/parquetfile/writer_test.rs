// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::ColumnInfo;
use crate::writer::ParquetWriter;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct PartialThenErrorSink {
    successful_writes: usize,
    written: Arc<AtomicUsize>,
}

impl Write for PartialThenErrorSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.successful_writes == 0 {
            self.successful_writes += 1;
            let count = 3.min(bytes.len());
            self.written.fetch_add(count, Ordering::Relaxed);
            return Ok(count);
        }
        Err(io::Error::other("forced error after partial write"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn counting_writer_counts_partial_bytes_before_an_error_like_go() {
    let column = ColumnInfo {
        name: "name".into(),
        database_type_name: "VARCHAR".into(),
        nullable: false,
        precision: 0,
        scale: 0,
    };
    let written = Arc::new(AtomicUsize::new(0));
    let mut writer = ParquetWriter::new(
        PartialThenErrorSink {
            successful_writes: 0,
            written: written.clone(),
        },
        &[column],
        &[],
    )
    .unwrap();
    writer.write(&[Some(b"value".to_vec())]).unwrap();

    let error = writer.flush_rows().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("forced error after partial write")
    );
    assert_eq!(
        writer.total_written_bytes(),
        written.load(Ordering::Relaxed) as i64
    );
    assert_eq!(writer.total_written_bytes(), 3);
}
