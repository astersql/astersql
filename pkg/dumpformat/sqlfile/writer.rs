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

use crate::append_value;
use astersql_dumpformat::FieldKind;
use std::io::{self, Write};

#[derive(Clone, Copy, Default)]
pub struct Config {
    pub statement_size: u64,
    pub escape_backslash: bool,
}

/// Caller owns buffering and file rotation. Size includes the pending row separator.
pub struct Writer<W> {
    writer: W,
    cfg: Config,
    kinds: Vec<FieldKind>,
    prefix: Vec<u8>,
    buf: Vec<u8>,
    statement_size: u64,
    file_size: u64,
    in_statement: bool,
}
impl<W: Write> Writer<W> {
    pub fn new(writer: W, prefix: Vec<u8>, kinds: Vec<FieldKind>, cfg: Config) -> Self {
        Self {
            writer,
            prefix,
            kinds,
            cfg,
            buf: vec![],
            statement_size: 0,
            file_size: 0,
            in_statement: false,
        }
    }
    pub fn write(&mut self, row: &[Option<Vec<u8>>]) -> io::Result<()> {
        if row.len() != self.kinds.len() {
            return Err(io::Error::other(format!(
                "sqlfile: row has {} fields, want {}",
                row.len(),
                self.kinds.len()
            )));
        }
        self.buf.clear();
        if self.in_statement
            && self.cfg.statement_size > 0
            && self.statement_size >= self.cfg.statement_size
        {
            self.buf.extend_from_slice(b";\n");
            self.in_statement = false;
        }
        if !self.in_statement {
            self.buf.extend_from_slice(&self.prefix);
            self.statement_size = self.prefix.len() as u64;
            self.file_size += self.prefix.len() as u64;
            self.in_statement = true;
        } else {
            self.buf.extend_from_slice(b",\n");
        }
        let start = self.buf.len();
        self.buf.push(b'(');
        for (i, val) in row.iter().enumerate() {
            if i > 0 {
                self.buf.push(b',');
            }
            append_value(
                &mut self.buf,
                val.as_deref().unwrap_or_default(),
                val.is_none(),
                self.kinds[i],
                self.cfg.escape_backslash,
            );
        }
        self.buf.push(b')');
        let size = (self.buf.len() - start + 2) as u64;
        self.statement_size += size;
        self.file_size += size;
        self.writer.write(&self.buf).map(|_| ())
    }
    pub fn estimate_file_size(&self) -> u64 {
        self.file_size
    }
    pub fn close(&mut self) -> io::Result<()> {
        if !self.in_statement {
            return Ok(());
        }
        self.in_statement = false;
        self.writer.write(b";\n").map(|_| ())
    }
}
