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
fn config() -> Config {
    Config {
        fields_terminated_by: b",".to_vec(),
        fields_enclosed_by: b"\"".to_vec(),
        lines_terminated_by: b"\n".to_vec(),
        null_value: b"\\N".to_vec(),
        ..Config::default()
    }
}
fn encode(kinds: Vec<FieldKind>, row: Vec<Option<Vec<u8>>>, cfg: Config) -> Vec<u8> {
    let mut out = Vec::new();
    Writer::new(&mut out, kinds, cfg).write(&row).unwrap();
    out
}
fn value(s: &[u8]) -> Option<Vec<u8>> {
    Some(s.to_vec())
}
#[test]
fn backslash_escape() {
    let mut c = config();
    c.fields_escaped_by = b"\\".to_vec();
    assert_eq!(
        encode(vec![FieldKind::String], vec![value(b"a\0b\rc\nd\\e\"f")], c),
        b"\"a\\0b\\rc\\nd\\\\e\\\"f\"\n"
    );
}
#[test]
fn quote_doubling() {
    assert_eq!(
        encode(vec![FieldKind::String], vec![value(b"a\"b\"c")], config()),
        b"\"a\"\"b\"\"c\"\n"
    );
}
#[test]
fn null_and_kinds() {
    assert_eq!(
        encode(
            vec![FieldKind::Number, FieldKind::String, FieldKind::Bytes],
            vec![value(b"1"), None, value(b"ab")],
            config()
        ),
        b"1,\\N,\"ab\"\n"
    );
}
#[test]
fn bytes_hex() {
    let mut c = config();
    c.binary_format = BinaryFormat::HEX;
    assert_eq!(
        encode(vec![FieldKind::Bytes], vec![value(b"ab")], c),
        b"\"6162\"\n"
    );
}
#[test]
fn empty_row() {
    let mut out = Vec::new();
    let mut w = Writer::new(&mut out, vec![], config());
    w.write(&[]).unwrap();
    w.write(&[]).unwrap();
    assert_eq!(out, b"\n\n");
}
#[test]
fn unquoted_backslash() {
    let mut c = config();
    c.fields_enclosed_by.clear();
    c.fields_escaped_by = b"\\".to_vec();
    assert_eq!(
        encode(vec![FieldKind::String], vec![value(b"a,b\nc")], c),
        b"a\\,b\\nc\n"
    );
}
#[test]
fn unquoted_raw() {
    let mut c = config();
    c.fields_enclosed_by.clear();
    assert_eq!(
        encode(vec![FieldKind::String], vec![value(b"a,b")], c),
        b"a,b\n"
    );
}
#[test]
fn bytes_base64() {
    let mut c = config();
    c.binary_format = BinaryFormat::Base64;
    assert_eq!(
        encode(vec![FieldKind::Bytes], vec![value(b"ab")], c),
        b"\"YWI=\"\n"
    );
}
#[test]
fn header() {
    let mut out = Vec::new();
    Writer::new(
        &mut out,
        vec![FieldKind::Number, FieldKind::String],
        config(),
    )
    .write_header(&[b"id".to_vec(), b"name".to_vec()])
    .unwrap();
    assert_eq!(out, b"\"id\",\"name\"\n");
}
#[test]
fn estimate_file_size() {
    let mut out = Vec::new();
    let mut w = Writer::new(&mut out, vec![FieldKind::Number], config());
    w.write_header(&[b"n".to_vec()]).unwrap();
    w.write(&[value(b"1")]).unwrap();
    w.write(&[value(b"22")]).unwrap();
    assert_eq!(w.estimate_file_size(), 9);
    w.close().unwrap();
    assert_eq!(out.len(), 9);
}
#[test]
fn row_width_mismatch() {
    let mut out = Vec::new();
    let err = Writer::new(
        &mut out,
        vec![FieldKind::String, FieldKind::String],
        config(),
    )
    .write(&[value(b"only-one")])
    .unwrap_err();
    assert!(err.to_string().contains("row has 1 fields, want 2"));
    assert!(out.is_empty());
}
#[test]
fn custom_escape_and_multibyte_enclosure() {
    let mut c = config();
    c.fields_escaped_by = b"!".to_vec();
    assert_eq!(
        encode(vec![FieldKind::String], vec![value(b"a!b\n")], c),
        b"\"a!!b!n\"\n"
    );
    let mut c = config();
    c.fields_enclosed_by = b"<>".to_vec();
    assert_eq!(
        encode(vec![FieldKind::String], vec![value(b"a<>b")], c),
        b"<>a<><>b<>\n"
    );
}
#[test]
fn short_write_counts_actual_bytes() {
    struct Short;
    impl std::io::Write for Short {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Ok(1)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut w = Writer::new(Short, vec![FieldKind::Number], config());
    w.write(&[value(b"123")]).unwrap();
    assert_eq!(w.estimate_file_size(), 1);
}
