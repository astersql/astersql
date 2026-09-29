// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn go_merge_32_vector_deserialization_advances_exactly_one_buffer() {
    let mut encoded = Vec::new();
    let vector = types::ParseVectorFloat32("[1.5, -2.25]").unwrap();
    let bytes = vector.SerializeTo(Vec::new());
    encoded.extend_from_slice(&(bytes.len() as isize).to_ne_bytes());
    encoded.extend_from_slice(&bytes);
    encoded.push(0x7f);
    let mut cursor = PosAndBuf {
        Buf: encoded,
        Pos: 0,
    };
    assert_eq!(
        DeserializeVectorFloat32(&mut cursor).SerializeTo(Vec::new()),
        bytes
    );
    assert_eq!(DeserializeByte(&mut cursor), 0x7f);
}

#[test]
#[should_panic]
fn go_merge_32_vector_deserialization_rejects_invalid_payload() {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(2_isize).to_ne_bytes());
    encoded.extend_from_slice(&[0xf1, 0xfc]);
    let mut cursor = PosAndBuf {
        Buf: encoded,
        Pos: 0,
    };
    let _ = DeserializeVectorFloat32(&mut cursor);
}
