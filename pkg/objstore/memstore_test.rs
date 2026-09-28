// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `MemStorage` 单元测试：基本 IO、范围读、WalkDir 与并发修改隔离。

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use objstore::memstore::NewMemStorage;
use objstore::storage::{Context, ReaderOption, Storage, WalkOption};

#[test]
/// 基本读写删、Create 延迟可见、Open 快照隔离、Rename 覆盖。
fn test_mem_store_basic() {
    let store = NewMemStorage();
    let ctx = Context::background();
    store.WriteFile(&ctx, "/hello.txt", b"hello world").unwrap();
    assert_eq!(store.ReadFile(&ctx, "/hello.txt").unwrap(), b"hello world");
    store
        .WriteFile(&ctx, "/hello.txt", b"hello world 2")
        .unwrap();
    assert_eq!(
        store.ReadFile(&ctx, "/hello.txt").unwrap(),
        b"hello world 2"
    );
    store.DeleteFile(&ctx, "/hello.txt").unwrap();
    assert!(store.DeleteFile(&ctx, "/hello.txt").is_err());
    assert!(store.ReadFile(&ctx, "/hello.txt").is_err());

    // close 前 ReadFile 应为空；close 后可见写入内容。
    let mut writer = store.Create(&ctx, "/hello.txt", None).unwrap();
    writer.write(&ctx, b"hello world 3").unwrap();
    assert_eq!(store.ReadFile(&ctx, "/hello.txt").unwrap(), b"");
    writer.close(&ctx).unwrap();
    assert_eq!(
        store.ReadFile(&ctx, "/hello.txt").unwrap(),
        b"hello world 3"
    );

    // 打开快照后删除原文件，已打开 reader 仍可读。
    let mut reader = store.Open(&ctx, "/hello.txt", None).unwrap();
    let mut snapshot = store.Open(&ctx, "/hello.txt", None).unwrap();
    let mut value = Vec::new();
    reader.read_to_end(&mut value).unwrap();
    assert_eq!(value, b"hello world 3");
    reader.close().unwrap();
    assert_eq!(reader.read(&mut [0; 3]).unwrap(), 0);
    store.DeleteFile(&ctx, "/hello.txt").unwrap();
    snapshot.seek(SeekFrom::Start(5)).unwrap();
    value.clear();
    snapshot.read_to_end(&mut value).unwrap();
    assert_eq!(value, b" world 3");

    store
        .WriteFile(&ctx, "/hello.txt", b"hello world 3")
        .unwrap();
    store
        .WriteFile(&ctx, "/hello2.txt", b"hello world 2")
        .unwrap();
    assert!(
        store
            .Rename(&ctx, "/NOT_EXIST.txt", "/NEW_FILE.txt")
            .is_err()
    );
    store.Rename(&ctx, "/hello2.txt", "/hello3.txt").unwrap();
    store.Rename(&ctx, "/hello.txt", "/hello3.txt").unwrap();
    assert!(!store.FileExists(&ctx, "/hello.txt").unwrap());
    assert!(!store.FileExists(&ctx, "/hello2.txt").unwrap());
    assert!(store.FileExists(&ctx, "/hello3.txt").unwrap());
}

#[test]
/// 范围 Open 后 seek/read 边界：End 定位到完整长度，越 end 读 0。
fn test_mem_store_open_range_seek() {
    let store = NewMemStorage();
    let ctx = Context::background();
    store
        .WriteFile(&ctx, "/seek-range.txt", b"0123456789")
        .unwrap();
    let mut reader = store
        .Open(
            &ctx,
            "/seek-range.txt",
            Some(&ReaderOption {
                start_offset: Some(2),
                end_offset: Some(5),
            }),
        )
        .unwrap();
    assert_eq!(reader.seek(SeekFrom::Current(0)).unwrap(), 2);
    assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 10);
    assert_eq!(reader.read(&mut [0; 2]).unwrap(), 0);
    assert_eq!(reader.seek(SeekFrom::Start(1)).unwrap(), 1);
    let mut bytes = [0; 10];
    assert_eq!(reader.read(&mut bytes).unwrap(), 4);
    assert_eq!(&bytes[..4], b"1234");
    assert_eq!(reader.read(&mut bytes).unwrap(), 0);
}

#[derive(Debug, Eq, PartialEq)]
/// WalkDir 回调收集的文件信息。
struct IterFileInfo {
    name: String,
    size: i64,
    content: Vec<u8>,
}

#[test]
/// WalkDir：全量、子目录与文件名前缀过滤。
fn test_mem_store_walk_dir() {
    let store = NewMemStorage();
    let ctx = Context::background();
    let all = BTreeMap::from([
        ("/hello.txt", b"hello world".to_vec()),
        ("/hello2.txt", b"hello world 2".to_vec()),
        ("/aaa/hello.txt", b"aaa: hello world".to_vec()),
        ("/aaa/world.txt", b"aaa: world".to_vec()),
        ("/dummy.txt", b"dummy".to_vec()),
    ]);
    for (name, content) in &all {
        store.WriteFile(&ctx, name, content).unwrap();
    }
    assert_walk_infos(&store, &ctx, None, &all);
    assert_walk_infos(
        &store,
        &ctx,
        Some(&WalkOption {
            sub_dir: "/aaa".into(),
            ..Default::default()
        }),
        &pick_files(&all, &["/aaa/hello.txt", "/aaa/world.txt"]),
    );
    assert_walk_infos(
        &store,
        &ctx,
        Some(&WalkOption {
            obj_prefix: "hello".into(),
            ..Default::default()
        }),
        &pick_files(&all, &["/hello.txt", "/hello2.txt", "/aaa/hello.txt"]),
    );
}

#[test]
/// Go `path.Base` keeps the root slash when filtering object names.
fn test_mem_store_walk_dir_root_basename() {
    let store = NewMemStorage();
    let ctx = Context::background();
    store.WriteFile(&ctx, "/", b"root").unwrap();

    let mut names = Vec::new();
    store
        .WalkDir(
            &ctx,
            Some(&WalkOption {
                obj_prefix: "/".into(),
                ..Default::default()
            }),
            &mut |name, _| {
                names.push(name.to_owned());
                Ok(())
            },
        )
        .unwrap();

    assert_eq!(names, ["/"]);
}

#[test]
/// Go `filepath.Base` returns `.` for an empty path and `/` for the root path.
fn test_mem_store_presign_file_go_basename_edges() {
    let store = NewMemStorage();
    let ctx = Context::background();

    assert_eq!(
        store.PresignFile(&ctx, "", Duration::from_secs(1)).unwrap(),
        "."
    );
    assert_eq!(
        store
            .PresignFile(&ctx, "/", Duration::from_secs(1))
            .unwrap(),
        "/"
    );
}

/// 断言 WalkDir 结果与期望 map 内容一致。
fn assert_walk_infos(
    store: &impl Storage,
    ctx: &Context,
    option: Option<&WalkOption>,
    expected: &BTreeMap<&str, Vec<u8>>,
) {
    let mut infos = Vec::new();
    store
        .WalkDir(ctx, option, &mut |name, size| {
            infos.push(IterFileInfo {
                name: name.into(),
                size,
                content: store.ReadFile(ctx, name)?,
            });
            Ok(())
        })
        .unwrap();
    assert_eq!(infos.len(), expected.len());
    for info in infos {
        assert_eq!(info.size, info.content.len() as i64);
        assert_eq!(&info.content, expected.get(info.name.as_str()).unwrap());
    }
}

/// 从全集中按名挑选子集。
fn pick_files<'a>(
    all: &BTreeMap<&'a str, Vec<u8>>,
    names: &[&'a str],
) -> BTreeMap<&'a str, Vec<u8>> {
    names
        .iter()
        .map(|name| (*name, all[*name].clone()))
        .collect()
}

#[test]
/// 外部修改输入/输出缓冲不得影响已存对象（拷贝语义）。
fn test_mem_store_manipulate_bytes() {
    let store = NewMemStorage();
    let ctx = Context::background();
    let mut input = b"aaa1".to_vec();
    store.WriteFile(&ctx, "/aaa.txt", &input).unwrap();
    input[3] = b'2';
    assert_eq!(store.ReadFile(&ctx, "/aaa.txt").unwrap(), b"aaa1");
    let mut output = store.ReadFile(&ctx, "/aaa.txt").unwrap();
    output[3] = b'2';
    assert_eq!(store.ReadFile(&ctx, "/aaa.txt").unwrap(), b"aaa1");
}

#[test]
/// 遍历过程中并发写/删：Walk 使用开始时的名字快照。
fn test_mem_store_write_during_walk_dir() {
    let store = Arc::new(NewMemStorage());
    let ctx = Context::background();
    let files = BTreeMap::from([
        ("/hello1.txt", b"hello world 1".to_vec()),
        ("/hello2.txt", b"hello world 2".to_vec()),
        ("/hello3.txt", b"hello world 3".to_vec()),
    ]);
    let remaining = Arc::new(Mutex::new(files.keys().copied().collect::<BTreeSet<_>>()));
    for (name, content) in &files {
        store.WriteFile(&ctx, name, content).unwrap();
    }

    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let walk_store = store.clone();
    let walk_ctx = ctx.clone();
    let walk_remaining = remaining.clone();
    // 首个回调后阻塞，主线程插入新文件并删除未遍历项。
    let walker = thread::spawn(move || {
        let mut first = true;
        walk_store.WalkDir(&walk_ctx, None, &mut |name, _| {
            walk_remaining.lock().unwrap().remove(name);
            if first {
                first = false;
                started_tx.send(()).unwrap();
                thread::sleep(Duration::from_millis(200));
            }
            Ok(())
        })
    });

    started_rx.recv().unwrap();
    store
        .WriteFile(&ctx, "/hello4.txt", b"hello world4")
        .unwrap();
    let deleted = remaining
        .lock()
        .unwrap()
        .iter()
        .next()
        .copied()
        .unwrap()
        .to_owned();
    store.DeleteFile(&ctx, &deleted).unwrap();
    walker.join().unwrap().unwrap();
    assert!(remaining.lock().unwrap().contains(deleted.as_str()));
}
