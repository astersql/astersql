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

use crate::*;
use astersql_dumpformat::FieldKind::{Bytes, Number, String as Text};
fn raw(s: &[u8]) -> Option<Vec<u8>> {
    Some(s.to_vec())
}
#[test]
fn framing_and_size() {
    let mut out = vec![];
    let mut w = Writer::new(
        &mut out,
        b"INSERT INTO `t` VALUES\n".to_vec(),
        vec![Number, Text, Bytes],
        Config::default(),
    );
    w.write(&[raw(b"1"), raw(b"ab"), raw(b"ab")]).unwrap();
    w.write(&[raw(b"2"), None, raw(b"")]).unwrap();
    w.close().unwrap();
    let size = w.estimate_file_size();
    drop(w);
    assert_eq!(
        out,
        b"INSERT INTO `t` VALUES\n(1,'ab',x'6162'),\n(2,NULL,x'');\n"
    );
    assert_eq!(size, out.len() as u64);
}
#[test]
fn escaping_and_binary_bytes() {
    for (escape, input, expected) in [
        (
            true,
            b"a'b\nc\rd\\e\0f\"g\x1ah".as_slice(),
            b"'a\\'b\\nc\\rd\\\\e\\0f\\\"g\\Zh'".as_slice(),
        ),
        (false, b"a'b", b"'a''b'"),
    ] {
        let mut out = vec![];
        append_value(&mut out, input, false, Text, escape);
        assert_eq!(out, expected);
    }
    let mut out = vec![];
    append_value(&mut out, &[0, 255], false, Bytes, false);
    assert_eq!(out, b"x'00ff'");
}
#[test]
fn statement_split_counts_pending_separator() {
    for limit in [6, 7] {
        let mut out = vec![];
        let mut w = Writer::new(
            &mut out,
            b"P\n".to_vec(),
            vec![Number],
            Config {
                statement_size: limit,
                ..Config::default()
            },
        );
        w.write(&[raw(b"1")]).unwrap();
        w.write(&[raw(b"2")]).unwrap();
        w.close().unwrap();
        drop(w);
        assert_eq!(out, b"P\n(1);\nP\n(2);\n");
    }
}
#[test]
fn empty_tuple_and_idempotent_close() {
    let mut out = vec![];
    let mut w = Writer::new(&mut out, b"P\n".to_vec(), vec![], Config::default());
    w.close().unwrap();
    assert_eq!(w.estimate_file_size(), 0);
    w.write(&[]).unwrap();
    w.write(&[]).unwrap();
    w.close().unwrap();
    w.close().unwrap();
    drop(w);
    assert_eq!(out, b"P\n(),\n();\n");
}
#[test]
fn width_mismatch_has_no_side_effects() {
    let mut out = vec![];
    let mut w = Writer::new(&mut out, b"P".to_vec(), vec![Number], Config::default());
    assert_eq!(
        w.write(&[]).unwrap_err().to_string(),
        "sqlfile: row has 0 fields, want 1"
    );
    assert_eq!(w.estimate_file_size(), 0);
    w.close().unwrap();
    drop(w);
    assert!(out.is_empty());
}
#[test]
fn write_errors_propagate_on_row_and_close() {
    struct Fail(usize);
    impl std::io::Write for Fail {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            self.0 += 1;
            Err(std::io::Error::other("sink failed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut w = Writer::new(Fail(0), b"P".to_vec(), vec![], Config::default());
    assert_eq!(w.write(&[]).unwrap_err().to_string(), "sink failed");
    assert_eq!(w.close().unwrap_err().to_string(), "sink failed");
    w.close().unwrap();
}

#[test]
fn limit_checks_previous_statement_size_not_next_row_projection() {
    let mut out = vec![];
    let mut w = Writer::new(
        &mut out,
        b"P\n".to_vec(),
        vec![Number],
        Config {
            statement_size: 8,
            ..Config::default()
        },
    );
    w.write(&[raw(b"1")]).unwrap();
    w.write(&[raw(b"2")]).unwrap();
    w.write(&[raw(b"3")]).unwrap();
    w.close().unwrap();
    drop(w);
    assert_eq!(out, b"P\n(1),\n(2);\nP\n(3);\n");
}
