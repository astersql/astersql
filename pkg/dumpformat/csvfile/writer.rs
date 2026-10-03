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

use crate::{Config, FieldKind, csv::append_field};
use std::io::{self, Write};
/// Writes one framed row per sink write and counts actual accepted bytes.
pub struct Writer<W> {
    sink: W,
    cfg: Config,
    kinds: Vec<FieldKind>,
    buf: Vec<u8>,
    written: u64,
}
impl<W: Write> Writer<W> {
    pub fn new(sink: W, kinds: Vec<FieldKind>, cfg: Config) -> Self {
        Self {
            sink,
            cfg,
            kinds,
            buf: Vec::new(),
            written: 0,
        }
    }
    pub fn write(&mut self, row: &[Option<Vec<u8>>]) -> io::Result<()> {
        self.write_borrowed(row.iter().map(|val| val.as_deref()))
    }
    /// Borrowed raw bytes avoid allocating or copying scanned values per row.
    pub fn write_borrowed<'a>(
        &mut self,
        row: impl ExactSizeIterator<Item = Option<&'a [u8]>>,
    ) -> io::Result<()> {
        if row.len() != self.kinds.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "csvfile: row has {} fields, want {}",
                    row.len(),
                    self.kinds.len()
                ),
            ));
        }
        self.buf.clear();
        for (i, val) in row.enumerate() {
            if i > 0 {
                self.buf.extend_from_slice(&self.cfg.fields_terminated_by);
            }
            append_field(&mut self.buf, val, self.kinds[i], &self.cfg);
        }
        self.flush_row()
    }
    pub fn write_header(&mut self, names: &[Vec<u8>]) -> io::Result<()> {
        self.buf.clear();
        for (i, name) in names.iter().enumerate() {
            if i > 0 {
                self.buf.extend_from_slice(&self.cfg.fields_terminated_by);
            }
            append_field(&mut self.buf, Some(name), FieldKind::String, &self.cfg);
        }
        self.flush_row()
    }
    fn flush_row(&mut self) -> io::Result<()> {
        self.buf.extend_from_slice(&self.cfg.lines_terminated_by);
        let n = self.sink.write(&self.buf)?;
        self.written += n as u64;
        Ok(())
    }
    pub fn estimate_file_size(&self) -> u64 {
        self.written
    }
    pub fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}
