// Copyright 2026 AsterSQL.

use super::{NewChunkWithCapacity, mysql, types};

/// Go iterates over every Chunk column and indexes the matching field type, so
/// a short field-type slice is a caller contract violation rather than a
/// request to format a prefix of the row.
#[test]
fn to_string_rejects_field_types_shorter_than_the_row() {
    let fields = vec![
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeVarString),
    ];
    let mut chunk = NewChunkWithCapacity(fields, 1);
    chunk.AppendInt64(0, 7);
    chunk.AppendString(1, "aster");

    let short_fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let result = std::panic::catch_unwind(|| chunk.GetRow(0).ToString(&short_fields));

    assert!(
        result.is_err(),
        "short field metadata must not truncate a row"
    );
}
