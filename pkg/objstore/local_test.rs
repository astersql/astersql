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

// `LocalStorage` 单元测试：删除、遍历、URI、范围读与软链接边界。

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;

use anyhow::bail;
use objstore::local::NewLocalStorage;
use objstore::storage::{Context, ReaderOption, Storage, WalkOption};

#[test]
fn local_storage_storeapi_bridge_streams_and_observes_cancellation() {
    use storeapi::{
        Context as ApiContext, ReaderOption as ApiReaderOption, Storage as ApiStorage,
        WalkOption as ApiWalkOption,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = NewLocalStorage(dir.path()).unwrap();
    let context = ApiContext::default();
    ApiStorage::WriteFile(&store, &context, "data.csv", b"1,2\n3,4\n").unwrap();
    let mut reader = ApiStorage::Open(
        &store,
        &context,
        "data.csv",
        Some(&ApiReaderOption {
            StartOffset: Some(4),
            EndOffset: Some(8),
            ..Default::default()
        }),
    )
    .unwrap();
    let mut data = Vec::new();
    reader.read_to_end(&mut data).unwrap();
    reader.close().unwrap();
    assert_eq!(data, b"3,4\n");
    let mut names = Vec::new();
    ApiStorage::WalkDir(
        &store,
        &context,
        Some(&ApiWalkOption::default()),
        &mut |name, size| {
            names.push((name.to_owned(), size));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(names, [("data.csv".into(), 8)]);
    context.cancel();
    assert!(ApiStorage::ReadFile(&store, &context, "data.csv").is_err());
}

#[test]
/// 创建后删除：存在性与 DeleteFile 行为。
fn test_delete_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = NewLocalStorage(dir.path()).unwrap();
    let ctx = Context::background();
    let name = "test_delete";
    assert!(!store.FileExists(&ctx, name).unwrap());
    let writer = store.Create(&ctx, name, None).unwrap();
    drop(writer);
    assert!(store.FileExists(&ctx, name).unwrap());
    store.DeleteFile(&ctx, name).unwrap();
    assert!(!store.FileExists(&ctx, name).unwrap());
}

#[cfg(unix)]
#[test]
/// Go filepath.Join 保留存储 base；绝对对象名不能让 Rust 丢弃 base。
fn test_absolute_object_name_stays_under_base() {
    let base = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let absolute_name = outside.path().join("object");
    let expected = base.path().join(absolute_name.strip_prefix("/").unwrap());
    let store = NewLocalStorage(base.path()).unwrap();

    store
        .WriteFile(
            &Context::background(),
            absolute_name.to_str().unwrap(),
            b"inside",
        )
        .unwrap();

    assert_eq!(fs::read(expected).unwrap(), b"inside");
    assert!(!absolute_name.exists());
}

#[test]
/// WalkDir：软链接文件大小、前缀过滤、空子目录与写后遍历。
fn test_walk_dir_with_soft_link_file() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let name1 = "test.warehouse.0.sql";
        let name2 = "test.warehouse.1.sql";
        let data = "/* whatever pragmas */;\
INSERT INTO `namespaced`.`table` (columns, more, columns) VALUES (1,-2, 3),\n(4,5., 6);\
INSERT `namespaced`.`table` (x,y,z) VALUES (7,8,9);\
insert another_table values (10,11e1,12, '(13)', '(', 14, ')');";
        fs::write(dir1.path().join(name1), data).unwrap();
        fs::write(dir2.path().join(name2), data).unwrap();
        // 将 dir1 中文件软链到 dir2，WalkDir 应能看到两份。
        symlink(dir1.path().join(name1), dir2.path().join(name1)).unwrap();

        let store = NewLocalStorage(dir2.path()).unwrap();
        let ctx = Context::background();
        let mut files = Vec::new();
        store
            .WalkDir(&ctx, None, &mut |name, size| {
                files.push((name.to_owned(), size));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            files,
            vec![
                (name1.into(), data.len() as i64),
                (name2.into(), data.len() as i64)
            ]
        );

        files.clear();
        store
            .WalkDir(
                &ctx,
                Some(&WalkOption {
                    obj_prefix: "test.warehouse.1".into(),
                    ..Default::default()
                }),
                &mut |name, size| {
                    files.push((name.to_owned(), size));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(files, vec![(name2.into(), data.len() as i64)]);

        assert!(!store.FileExists(&ctx, "123/456").unwrap());
        store
            .WalkDir(
                &ctx,
                Some(&WalkOption {
                    sub_dir: "123/456".into(),
                    ..Default::default()
                }),
                &mut |_name, _size| bail!("callback must not run"),
            )
            .unwrap();
        store
            .WriteFile(&ctx, "123/456/789.txt", data.as_bytes())
            .unwrap();
        assert!(store.FileExists(&ctx, "123/456").unwrap());
        store
            .WalkDir(
                &ctx,
                Some(&WalkOption {
                    sub_dir: "123/456".into(),
                    ..Default::default()
                }),
                &mut |name, _size| {
                    if name == "123/456/789.txt" {
                        Ok(())
                    } else {
                        bail!("unexpected file {name}")
                    }
                },
            )
            .unwrap();
    }
}

#[test]
/// skip_sub_dir=true 时不进入子目录。
fn test_walk_dir_skip_sub_dir() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("test1.txt"), b"test1").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/test2.txt"), b"test2").unwrap();
    let store = NewLocalStorage(dir.path()).unwrap();
    let ctx = Context::background();
    assert_eq!(
        walk_names(&store, &ctx, None),
        vec!["sub/test2.txt", "test1.txt"]
    );
    assert_eq!(
        walk_names(
            &store,
            &ctx,
            Some(&WalkOption {
                skip_sub_dir: true,
                ..Default::default()
            })
        ),
        vec!["test1.txt"]
    );
}

#[test]
/// start_after 游标：跳过字典序 <= 游标的对象。
fn test_walk_dir_start_after() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("meta")).unwrap();
    for (name, data) in [
        ("0001.meta", b"1"),
        ("0002.meta", b"2"),
        ("0003.meta", b"3"),
    ] {
        fs::write(dir.path().join("meta").join(name), data).unwrap();
    }
    let store = NewLocalStorage(dir.path()).unwrap();
    let ctx = Context::background();
    assert_eq!(
        walk_names(
            &store,
            &ctx,
            Some(&WalkOption {
                sub_dir: "meta".into(),
                start_after: "meta/0001.meta".into(),
                ..Default::default()
            })
        ),
        vec!["meta/0002.meta", "meta/0003.meta"]
    );
}

#[cfg(unix)]
#[test]
/// start_after 已排除的子树不应再被打开；与 Go 的目录剪枝保持一致。
fn test_walk_dir_start_after_skips_unreadable_subtree() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let skipped = dir.path().join("old");
    fs::create_dir(&skipped).unwrap();
    fs::write(skipped.join("secret"), b"secret").unwrap();
    fs::write(dir.path().join("visible"), b"visible").unwrap();
    fs::set_permissions(&skipped, fs::Permissions::from_mode(0)).unwrap();

    let store = NewLocalStorage(dir.path()).unwrap();
    let ctx = Context::background();
    let result = store.WalkDir(
        &ctx,
        Some(&WalkOption {
            start_after: "old0".into(),
            ..Default::default()
        }),
        &mut |_name, _size| Ok(()),
    );

    fs::set_permissions(&skipped, fs::Permissions::from_mode(0o700)).unwrap();
    result.unwrap();
}

#[test]
/// 不存在的遍历根按 Go filepath.Rel 的 `.` 参与 tombstone 前缀匹配。
fn test_walk_dir_tombstone_prefix_matches_go() {
    let dir = tempfile::tempdir().unwrap();
    let store = NewLocalStorage(dir.path()).unwrap();
    let ctx = Context::background();
    let mut tombstones = Vec::new();
    store
        .WalkDir(
            &ctx,
            Some(&WalkOption {
                sub_dir: "missing".into(),
                obj_prefix: ".".into(),
                include_tombstone: true,
                ..Default::default()
            }),
            &mut |name, size| {
                tombstones.push((name.to_owned(), size));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(tombstones, vec![("missing".into(), -1)]);
}

#[test]
/// URI 形如 file:///path。
fn test_local_uri() {
    let store = NewLocalStorage("/tmp/folder").unwrap();
    assert_eq!(store.URI(), "file:///tmp/folder");
}

#[test]
/// Open 范围读：起止偏移组合与分次小缓冲读取。
fn test_local_file_read_range() {
    let dir = tempfile::tempdir().unwrap();
    let store = NewLocalStorage(dir.path()).unwrap();
    let ctx = Context::background();
    let mut writer = store.Create(&ctx, "test_read_range", None).unwrap();
    writer.write(&ctx, b"0123456789").unwrap();
    writer.close(&ctx).unwrap();

    for (option, expected) in [
        (
            Some(ReaderOption {
                start_offset: Some(2),
                end_offset: Some(6),
            }),
            "2345",
        ),
        (None, "0123456789"),
        (
            Some(ReaderOption {
                start_offset: Some(5),
                end_offset: None,
            }),
            "56789",
        ),
        (
            Some(ReaderOption {
                start_offset: None,
                end_offset: Some(5),
            }),
            "01234",
        ),
    ] {
        let mut reader = store
            .Open(&ctx, "test_read_range", option.as_ref())
            .unwrap();
        let mut value = String::new();
        reader.read_to_string(&mut value).unwrap();
        assert_eq!(value, expected);
        let mut extra = [0; 10];
        assert_eq!(reader.read(&mut extra).unwrap(), 0);
    }
    let mut reader = store
        .Open(
            &ctx,
            "test_read_range",
            Some(&ReaderOption {
                start_offset: Some(2),
                end_offset: Some(6),
            }),
        )
        .unwrap();
    let mut small = [0; 2];
    assert_eq!(reader.read(&mut small).unwrap(), 2);
    assert_eq!(&small, b"23");
}

#[test]
/// 断裂软链接：WalkDir 仍列出且 size=0。
fn test_walk_broken_sym_link() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        symlink(
            dir.path().join("non-existing-file"),
            dir.path().join("file-that-should-be-ignored"),
        )
        .unwrap();
        let store = NewLocalStorage(dir.path()).unwrap();
        let ctx = Context::background();
        let mut files = BTreeMap::new();
        store
            .WalkDir(&ctx, None, &mut |name, size| {
                files.insert(name.to_owned(), size);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            files,
            BTreeMap::from([("file-that-should-be-ignored".into(), 0)])
        );
    }
}

/// 收集 WalkDir 返回的对象名列表。
fn walk_names(store: &impl Storage, ctx: &Context, option: Option<&WalkOption>) -> Vec<String> {
    let mut names = Vec::new();
    store
        .WalkDir(ctx, option, &mut |name, _| {
            names.push(name.to_owned());
            Ok(())
        })
        .unwrap();
    names
}
