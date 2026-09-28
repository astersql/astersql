// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Aster 单元测试：helper/local/locking/memstore/noop/parse 行为对齐 Go。
//
// 覆盖 Backend 解析、本地与内存存储、noop 范围读、远程锁与目录反序列化。

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use helper_test_support::*;

#[test]
/// 校验 ParseBackend：查询参数解析、密钥保留，以及 FormatBackendURL 脱敏与错误路径。
fn parse_backend_matches_go_query_and_secret_cleanup() {
    // 含 access_key/secret 与 force_path_style 的 S3 URI。
    let backend = ParseBackend(
        "s3://bucket/a/b?access_key=a+b&secret_access_key=secret&force_path_style=false",
        None,
    )
    .unwrap();
    let StorageBackend::S3(s3) = backend else {
        panic!("expected s3 backend");
    };
    assert_eq!(s3.bucket, "bucket");
    assert_eq!(s3.prefix, "a/b");
    assert_eq!(s3.access_key, "a+b");
    assert_eq!(s3.secret_access_key, "secret");
    assert!(!s3.force_path_style);
    // Format 应去掉密钥查询参数，仅保留桶与前缀。
    assert_eq!(FormatBackendURL(&StorageBackend::S3(s3)), "s3://bucket/a/b");

    assert!(
        ParseBackend("", None)
            .unwrap_err()
            .to_string()
            .contains("empty store")
    );
    assert!(
        ParseBackend("s3:///prefix", None)
            .unwrap_err()
            .to_string()
            .contains("specify the bucket")
    );
    assert!(
        ParseBackend("s3://bucket?secret-access-key=x", None)
            .unwrap_err()
            .to_string()
            .contains("access_key not found")
    );
    assert_eq!(
        NormalizeQueryParameterKey("Force_Path_STYLE"),
        "force-path-style"
    );
}

#[test]
/// 本地存储：原子写、WalkDir 游标、范围读、CopyFrom 硬链接与 Presign。
fn local_storage_preserves_atomic_write_walk_range_and_copy_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let source = Arc::new(NewLocalStorage(temp.path().join("source")).unwrap());
    let target = Arc::new(NewLocalStorage(temp.path().join("target")).unwrap());
    let ctx = Context::background();

    source.WriteFile(&ctx, "a/1.txt", b"abcdef").unwrap();
    source.WriteFile(&ctx, "a/2.txt", b"xy").unwrap();
    source.WriteFile(&ctx, "b/3.txt", b"z").unwrap();
    assert_eq!(source.ReadFile(&ctx, "a/1.txt").unwrap(), b"abcdef");

    let mut names = Vec::new();
    // start_after 之后仅应看到 a/2.txt。
    source
        .WalkDir(
            &ctx,
            Some(&WalkOption {
                sub_dir: "a".into(),
                obj_prefix: String::new(),
                skip_sub_dir: false,
                include_tombstone: false,
                start_after: "a/1.txt".into(),
            }),
            &mut |name, size| {
                names.push((name.to_owned(), size));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(names, vec![("a/2.txt".into(), 2)]);

    // 范围读 [1,4) 得到 bcd，文件大小仍为完整 6。
    let mut reader = source
        .Open(
            &ctx,
            "a/1.txt",
            Some(&ReaderOption {
                start_offset: Some(1),
                end_offset: Some(4),
            }),
        )
        .unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"bcd");
    assert_eq!(reader.get_file_size().unwrap(), 6);
    reader.seek(SeekFrom::Start(0)).unwrap();

    // CopyFrom 在本地后端用硬链接复制对象。
    target
        .CopyFrom(
            &ctx,
            source.clone(),
            &CopySpec {
                from: "a/1.txt".into(),
                to: "copied/1.txt".into(),
            },
        )
        .unwrap();
    assert_eq!(target.ReadFile(&ctx, "copied/1.txt").unwrap(), b"abcdef");
    assert_eq!(
        source
            .PresignFile(&ctx, "a/1.txt", std::time::Duration::ZERO)
            .unwrap(),
        "1.txt"
    );
}

#[test]
/// 内存存储：WalkDir 过滤排序、Create 延迟可见、关闭后不可写、取消上下文。
fn memstore_matches_go_snapshot_sort_cancel_and_writer_close_semantics() {
    let storage = Arc::new(NewMemStorage());
    let ctx = Context::background();
    storage.WriteFile(&ctx, "dir/b.meta", b"2").unwrap();
    storage.WriteFile(&ctx, "dir/a.meta", b"1").unwrap();
    storage.WriteFile(&ctx, "other/a.meta", b"3").unwrap();

    let mut visited = Vec::new();
    storage
        .WalkDir(
            &ctx,
            Some(&WalkOption {
                sub_dir: "dir/".into(),
                obj_prefix: "a".into(),
                ..WalkOption::default()
            }),
            &mut |name, size| {
                visited.push((name.to_owned(), size));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(visited, vec![("dir/a.meta".into(), 1)]);

    // Create 先占位空文件，close 后才落盘缓冲内容。
    let mut writer = storage.Create(&ctx, "created", None).unwrap();
    assert_eq!(writer.write(&ctx, b"hello").unwrap(), 5);
    assert_eq!(storage.ReadFile(&ctx, "created").unwrap(), b"");
    writer.close(&ctx).unwrap();
    assert_eq!(storage.ReadFile(&ctx, "created").unwrap(), b"hello");
    assert!(
        writer
            .write(&ctx, b"!")
            .unwrap_err()
            .to_string()
            .contains("writer closed")
    );

    // 已取消的 Context 应使后续 ReadFile 失败。
    let cancelled = Context::background();
    cancelled.cancel();
    assert!(storage.ReadFile(&cancelled, "created").is_err());
}

#[test]
/// noop 存储上的范围读：成功时不改缓冲；负偏移报错。
fn noop_and_range_read_keep_go_edge_cases() {
    let ctx = Context::background();
    let storage: StorageRef = Arc::new(newNoopStorage());
    let mut bytes = [7_u8; 4];
    assert_eq!(
        ReadDataInRange(&ctx, storage.clone(), "anything", 0, &mut bytes).unwrap(),
        4
    );
    assert_eq!(bytes, [7; 4]);
    assert!(
        ReadDataInRange(&ctx, storage, "anything", -1, &mut bytes)
            .unwrap_err()
            .to_string()
            .contains("negative start offset")
    );
}

#[test]
/// 远程锁：互斥 TryLockRemote，以及读写锁互斥兼容性。
fn remote_lock_enforces_mutex_and_read_write_compatibility() {
    let ctx = Context::background();
    let storage: StorageRef = Arc::new(NewMemStorage());
    let input = LockMetaInput {
        owner_id: "owner-a".into(),
        lock_type: "backup".into(),
        hint: "unit-test".into(),
    };

    // 同一路径二次加锁应失败并带上 owner 信息。
    let lock = TryLockRemote(&ctx, storage.clone(), "locks/main", input.clone()).unwrap();
    let err = TryLockRemote(&ctx, storage.clone(), "locks/main", input.clone()).unwrap_err();
    assert!(err.to_string().contains("locked"));
    assert!(err.to_string().contains("owner-a"));
    lock.Unlock(&ctx).unwrap();

    // 多读可共存，写与读互斥。
    let read1 = TryLockRemoteRead(&ctx, storage.clone(), "locks/rw", input.clone()).unwrap();
    let read2 = TryLockRemoteRead(&ctx, storage.clone(), "locks/rw", input.clone()).unwrap();
    assert!(TryLockRemoteWrite(&ctx, storage.clone(), "locks/rw", input.clone()).is_err());
    read1.Unlock(&ctx).unwrap();
    read2.Unlock(&ctx).unwrap();
    let write = TryLockRemoteWrite(&ctx, storage.clone(), "locks/rw", input).unwrap();
    assert!(TryLockRemoteRead(&ctx, storage, "locks/rw", LockMetaInput::default()).is_err());
    write.Unlock(&ctx).unwrap();
}

#[test]
/// UnmarshalDir：成功解析与坏 JSON 错误信息需带文件名。
fn unmarshal_dir_yields_values_and_annotates_the_file_name() {
    let ctx = Context::background();
    let storage: StorageRef = Arc::new(NewMemStorage());
    storage
        .WriteFile(&ctx, "meta/1.json", br#"{"id":1}"#)
        .unwrap();
    let iter = UnmarshalDir(
        ctx.clone(),
        WalkOption {
            sub_dir: "meta/".into(),
            ..WalkOption::default()
        },
        storage.clone(),
        |name, bytes| {
            serde_json::from_slice::<serde_json::Value>(bytes)
                .map_err(anyhow::Error::from)
                .map(|value| (name.to_owned(), value))
        },
    );
    let results = iter.collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .as_ref()
            .is_ok_and(|(name, value)| name == "meta/1.json" && value["id"] == 1)
    );

    storage.WriteFile(&ctx, "meta/2.json", b"broken").unwrap();
    let results = UnmarshalDir(
        ctx,
        WalkOption {
            sub_dir: "meta/2.json".into(),
            ..WalkOption::default()
        },
        storage,
        |name, bytes| {
            serde_json::from_slice::<serde_json::Value>(bytes)
                .map_err(anyhow::Error::from)
                .map(|value| (name.to_owned(), value))
        },
    )
    .collect::<Vec<_>>();
    assert!(
        results
            .iter()
            .any(|result| result.as_ref().is_err_and(|error| error
                .to_string()
                .contains("failed to unmarshal file meta/2.json")))
    );
}
