// Copyright 2026 AsterSQL.

use std::fs;

use super::MemStore;

/// Go 的命名返回错误会被延迟执行的 Close 结果覆盖；正常关闭时，读取错误最终返回 nil。
#[test]
fn load_read_errors_are_overwritten_by_successful_close_like_go() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.bin");
    let truncated_value = dir.path().join("truncated-value.bin");
    fs::write(&empty, []).unwrap();
    fs::write(&truncated_value, [1, 0, 0, 0, b'm', 1, 0, 0, 0, b'k']).unwrap();

    for path in [&empty, &truncated_value] {
        let mut store = MemStore::NewMemStore(256);
        assert_eq!(store.LoadFromFile(path.to_str().unwrap()).unwrap(), None);
        assert_eq!(store.Len(), 0);
    }
}
