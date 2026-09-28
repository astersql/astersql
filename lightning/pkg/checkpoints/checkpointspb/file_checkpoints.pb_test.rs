// Copyright 2026 AsterSQL.

use super::*;

fn length_delimited(field: u8, payload: &[u8]) -> Vec<u8> {
    assert!(field < 16);
    assert!(payload.len() < 128);

    let mut encoded = Vec::with_capacity(payload.len() + 2);
    encoded.push(field << 3 | 2);
    encoded.push(payload.len() as u8);
    encoded.extend_from_slice(payload);
    encoded
}

#[test]
fn repeated_map_value_keeps_only_the_last_message() {
    let mut table_entry = length_delimited(1, b"table");
    table_entry.extend(length_delimited(2, &[0x18, 7]));
    table_entry.extend(length_delimited(2, &[0x50, 9]));
    let encoded = length_delimited(1, &table_entry);

    let mut checkpoints = CheckpointsModel::default();
    checkpoints.Unmarshal(&encoded).unwrap();
    let table = &checkpoints.Checkpoints["table"];
    assert_eq!(table.Status, 0);
    assert_eq!(table.KvBytes, 9);

    let mut engine_entry = vec![0x08, 0x02];
    engine_entry.extend(length_delimited(2, &[0x08, 7]));
    engine_entry.extend(length_delimited(2, &[]));
    let encoded = length_delimited(8, &engine_entry);

    let mut table = TableCheckpointModel::default();
    table.Unmarshal(&encoded).unwrap();
    assert_eq!(table.Engines[&1], EngineCheckpointModel::default());

    let mut chunk_entry = length_delimited(1, b"chunk");
    chunk_entry.extend(length_delimited(2, &[0x10, 7]));
    chunk_entry.extend(length_delimited(2, &[0x30, 9]));
    let encoded = length_delimited(2, &chunk_entry);

    let mut engine = EngineCheckpointModel::default();
    engine.Unmarshal(&encoded).unwrap();
    let chunk = &engine.Chunks["chunk"];
    assert_eq!(chunk.Offset, 0);
    assert_eq!(chunk.Pos, 9);
}
