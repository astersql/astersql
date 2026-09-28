// Copyright 2019-present PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// lockstore 从 Go 迁移到 Rust 的行为对齐单测。
//
// 覆盖 arena 分配器对齐/扩容/延迟复用、MemStore 增删改与 Hint、
// 迭代器边界、Dump/Load 往返以及单写多读并发语义。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
use crate::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    /// 校验 arena 块对齐、扩容后新块索引，以及 free 后进入 pending 而非立即可写队列。
    #[test]
    fn migration_arena_matches_go_alignment_growth_and_delayed_reuse() {
        let mut arena = newArenaLocator(32);
        let first = arena.alloc(3);
        let second = arena.alloc(8);
        assert_eq!(first.blockIdx(), 0);
        assert_eq!(first.blockOffset(), 0);
        // 第二个分配按 8 字节对齐；再申请 24 字节时当前块放不下。
        assert_eq!(second.blockOffset(), 8);
        assert_eq!(arena.alloc(24), nullArenaAddr);

        let mut grown = arena.grow();
        assert_eq!(grown.blocks.len(), 2);
        assert_eq!(grown.alloc(24).blockIdx(), 1);

        // free 后块进入 pendingBlocks，延迟复用（与 Go 一致）。
        arena.free(first);
        arena.free(second);
        assert_eq!(arena.pendingBlocks.len(), 1);
        assert!(arena.writableQueue.is_empty());
    }

    /// Go `grow` copies block pointers, so existing blocks remain shared by old and new locators.
    #[test]
    fn migration_arena_growth_shares_existing_blocks_like_go() {
        let mut arena = newArenaLocator(32);
        let addr = arena.alloc(8);
        arena.get_mut(addr, 8).copy_from_slice(b"original");

        let mut grown = arena.grow();
        grown.get_mut(addr, 8).copy_from_slice(b"updated!");

        assert_eq!(arena.get(addr, 8), b"updated!");
    }

    /// 校验 Put/Get/Delete（含 Hint）与 Len、MaxEntrySize 行为对齐 Go。
    #[test]
    fn migration_memstore_crud_replace_and_hint_match_go() {
        let mut store = MemStore::NewMemStore(256);
        let mut hint = Hint::new();
        assert!(store.PutWithHint(b"b", b"old", Some(&mut hint)));
        assert!(store.PutWithHint(b"a", b"one", Some(&mut hint)));
        // 同键再次 Put 返回 false（已存在，仅替换 value）。
        assert!(!store.PutWithHint(b"b", b"new", Some(&mut hint)));
        assert_eq!(store.Len(), 2);

        let mut buf = Vec::new();
        assert_eq!(store.Get(b"b", &mut buf).as_deref(), Some(&b"new"[..]));
        assert_eq!(store.Get(b"missing", &mut buf), None);
        assert!(store.DeleteWithHint(b"a", Some(&mut hint)));
        assert!(!store.DeleteWithHint(b"a", Some(&mut hint)));
        assert_eq!(store.Len(), 1);
        assert!(store.MaxEntrySize() > 0);
    }

    /// 校验 Seek/Next/Prev 及各类边界定位与 Go 迭代器语义一致。
    #[test]
    fn migration_iterator_boundaries_match_go() {
        let mut store = MemStore::NewMemStore(256);
        for key in [b"a", b"c", b"e"] {
            assert!(store.Put(key, key));
        }
        let mut it = store.NewIterator();
        assert!(!it.Valid());
        it.Seek(b"b");
        assert_eq!(it.Key(), b"c");
        it.Next();
        assert_eq!(it.Key(), b"e");
        it.Next();
        assert!(!it.Valid());
        it.SeekForPrev(b"d");
        assert_eq!(it.Key(), b"c");
        // SeekForExclusivePrev：定位到严格小于目标键的最大键。
        it.SeekForExclusivePrev(b"c");
        assert_eq!(it.Key(), b"a");
        it.SeekToLast();
        assert_eq!(it.Key(), b"e");
        it.Prev();
        assert_eq!(it.Value(), b"c");
        it.SeekToFirst();
        assert_eq!(it.Key(), b"a");
    }

    /// 校验 DumpToFile/LoadFromFile 往返与缺失文件返回 None。
    #[test]
    fn migration_dump_load_round_trip_and_missing_file_match_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locks.bin");
        let missing = dir.path().join("missing.bin");
        let mut store = MemStore::NewMemStore(256);
        store.Put(b"b", b"two");
        store.Put(b"a", b"one");
        store
            .DumpToFile(path.to_str().unwrap(), b"metadata")
            .unwrap();
        // 临时文件应在 dump 完成后被清理。
        assert!(!path.with_extension("bin.tmp").exists());

        let mut loaded = MemStore::NewMemStore(256);
        assert_eq!(
            loaded.LoadFromFile(path.to_str().unwrap()).unwrap(),
            Some(b"metadata".to_vec())
        );
        assert_eq!(loaded.Len(), 2);
        let mut buf = Vec::new();
        assert_eq!(loaded.Get(b"a", &mut buf).as_deref(), Some(&b"one"[..]));
        assert_eq!(
            loaded.LoadFromFile(missing.to_str().unwrap()).unwrap(),
            None
        );
        fs::remove_file(path).unwrap();
    }

    /// 校验截断文件的读取错误被成功 Close 覆盖，与 Go 的命名返回值语义一致。
    #[test]
    fn migration_load_ignores_truncated_items_like_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.bin");
        fs::write(&path, [3, 0, 0, 0, b'm', b'e', b't', 2, 0]).unwrap();
        let mut store = MemStore::NewMemStore(256);
        assert_eq!(store.LoadFromFile(path.to_str().unwrap()).unwrap(), None);
    }

    /// 校验单写多读：多线程并发 Get 可正确读到已写入键值。
    #[test]
    fn migration_memstore_supports_go_single_writer_multiple_readers() {
        let mut store = MemStore::NewMemStore(256);
        for i in 0..32 {
            let key = format!("key-{i:02}");
            store.Put(key.as_bytes(), key.as_bytes());
        }
        let store: Arc<MemStore> = Arc::from(store);
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    for i in 0..32 {
                        let key = format!("key-{i:02}");
                        let mut buf = Vec::new();
                        assert_eq!(
                            store.Get(key.as_bytes(), &mut buf).as_deref(),
                            Some(key.as_bytes())
                        );
                    }
                })
            })
            .collect();
        for reader in readers {
            reader.join().unwrap();
        }
    }
}
