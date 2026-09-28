// Copyright 2026 AsterSQL.

use crate::{Encoder, types};

#[test]
fn encode_ignores_values_without_column_ids_like_go() {
    let encoded_with_extra_value = Encoder::new(true)
        .Encode(
            None,
            vec![1],
            vec![types::NewIntDatum(7), types::NewIntDatum(99)],
            None,
            Vec::new(),
        )
        .expect("Go ignores values that have no corresponding column ID");
    let encoded_without_extra_value = Encoder::new(true)
        .Encode(None, vec![1], vec![types::NewIntDatum(7)], None, Vec::new())
        .expect("single-column row should encode");

    assert_eq!(encoded_without_extra_value, encoded_with_extra_value);
}
