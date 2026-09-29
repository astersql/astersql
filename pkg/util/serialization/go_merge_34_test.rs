// Copyright 2026 AsterSQL.

use crate::{DeserializeVectorFloat32, PosAndBuf, SerializeVectorFloat32, types};

#[test]
fn go_merge_34_vector_float32_round_trip_including_zero_value() {
    for vector in [
        types::VectorFloat32::default(),
        types::ParseVectorFloat32("[1.5,-2]").unwrap(),
    ] {
        let bytes = SerializeVectorFloat32(&vector, vec![0x7f]);
        assert_eq!(bytes[0], 0x7f);
        let mut cursor = PosAndBuf { Pos: 1, Buf: bytes };
        let decoded = DeserializeVectorFloat32(&mut cursor);
        if vector.SerializedSize() == 0 {
            assert!(decoded.Elements().is_empty());
        } else {
            assert_eq!(decoded.Elements(), vector.Elements());
        }
    }
}
