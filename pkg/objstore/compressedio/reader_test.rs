// Copyright 2026 AsterSQL.

use std::io::Cursor;

use super::{CompressType, DecompressConfig, new_reader};

#[test]
fn invalid_gzip_header_is_rejected_when_reader_is_created() {
    let result = new_reader(
        CompressType::Gzip,
        DecompressConfig::default(),
        Box::new(Cursor::new(b"not a gzip stream".to_vec())),
    );

    assert!(
        result.is_err(),
        "Go gzip.NewReader rejects the invalid header"
    );
}

#[test]
fn gzip_reader_consumes_all_members_like_go() {
    use std::io::{Read, Write};
    let mut stream = Vec::new();
    for payload in [b"first".as_slice(), b"second".as_slice()] {
        let mut writer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        writer.write_all(payload).unwrap();
        stream.extend(writer.finish().unwrap());
    }
    let mut reader = new_reader(
        CompressType::Gzip,
        DecompressConfig::default(),
        Box::new(Cursor::new(stream)),
    )
    .unwrap()
    .unwrap();
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"firstsecond");
}
