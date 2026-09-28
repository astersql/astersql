// Copyright 2026 AsterSQL.

use super::key_set_shard::keySetShard;

#[test]
#[should_panic]
fn negative_key_panics_like_go_array_indexing() {
    let shards = keySetShard::newKeySetShard();
    let _ = shards.Get(-1);
}
