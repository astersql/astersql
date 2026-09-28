// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Mock 对象存储的迁移单元测试。
//
// 验证 `MockStorage`：按 EXPECT 注册的结果顺序转发 Write/Read/Exists/Rename 等；
// WalkDir/Presign 保留 Go 风格回调与 Duration；Create/Open/Delete 各消费一条期望。

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::time::Duration;

use crate::mockobjstore::{Call, NewMockStorage};
use crate::storeapi::{ReaderOption, Storage, WriterOption};
use crate::{Context, Reader, Writer};

/// 基于内存 Cursor 的测试用 Reader。
struct TestReader(Cursor<Vec<u8>>);

impl Read for TestReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Seek for TestReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}

impl Reader for TestReader {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn file_size(&self) -> io::Result<i64> {
        Ok(self.0.get_ref().len() as i64)
    }
}

/// 将写入数据追加到内存缓冲的测试用 Writer。
#[derive(Default)]
struct TestWriter(Vec<u8>);

impl Writer for TestWriter {
    fn write(&mut self, _ctx: &Context, data: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(data);
        Ok(data.len())
    }

    fn close(&mut self, _ctx: &Context) -> io::Result<()> {
        Ok(())
    }
}

/// 按注册顺序转发 Storage 调用，并校验 Call 记录与 verify 清空期望。
#[test]
fn forwards_expected_storage_calls_and_results_in_order() {
    let mut storage = NewMockStorage();
    let expect = storage.EXPECT();
    expect.WriteFile(Ok(()));
    expect.ReadFile(Ok(b"payload".to_vec()));
    expect.FileExists(Ok(true));
    expect.Rename(Err("rename denied".to_owned()));
    expect.URI("s3://bucket/prefix".to_owned());
    expect.Close();

    let ctx = Context::default();
    Storage::WriteFile(&storage, &ctx, "old", b"payload").unwrap();
    assert_eq!(
        Storage::ReadFile(&storage, &ctx, "old").unwrap(),
        b"payload"
    );
    assert!(Storage::FileExists(&storage, &ctx, "old").unwrap());
    assert_eq!(
        Storage::Rename(&storage, &ctx, "old", "new")
            .unwrap_err()
            .to_string(),
        "rename denied"
    );
    assert_eq!(Storage::URI(&storage), "s3://bucket/prefix");
    Storage::Close(&mut storage);

    assert_eq!(
        storage.calls(),
        vec![
            Call::WriteFile("old".to_owned(), b"payload".to_vec()),
            Call::ReadFile("old".to_owned()),
            Call::FileExists("old".to_owned()),
            Call::Rename("old".to_owned(), "new".to_owned()),
            Call::URI,
            Call::Close,
        ]
    );
    storage.verify();
}

/// WalkDir 回调收到条目列表；PresignFile 保留 expire Duration 参数。
#[test]
fn walk_and_presign_preserve_go_callback_and_duration_behavior() {
    let storage = NewMockStorage();
    let expect = storage.EXPECT();
    expect.WalkDir(
        vec![("a.sst".to_owned(), 7), ("nested/b.sst".to_owned(), 11)],
        Ok(()),
    );
    expect.PresignFile(Ok("https://signed.example/object".to_owned()));

    let ctx = Context::default();
    let mut visited = Vec::new();
    Storage::WalkDir(&storage, &ctx, None, &mut |name, size| {
        visited.push((name.to_owned(), size));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        visited,
        vec![("a.sst".to_owned(), 7), ("nested/b.sst".to_owned(), 11)]
    );
    assert_eq!(
        Storage::PresignFile(&storage, &ctx, "a.sst", Duration::from_secs(45)).unwrap(),
        "https://signed.example/object"
    );
    assert_eq!(
        storage.calls(),
        vec![
            Call::WalkDir(None),
            Call::PresignFile("a.sst".to_owned(), Duration::from_secs(45)),
        ]
    );
    storage.verify();
}

/// Create/Open/DeleteFile/DeleteFiles 各消费一条 EXPECT，错误路径可返回 Err。
#[test]
fn create_open_and_delete_methods_consume_one_expectation_each() {
    let storage = NewMockStorage();
    let expect = storage.EXPECT();
    expect.Create(Ok(Box::new(TestWriter::default())));
    expect.Open(Ok(Box::new(TestReader(Cursor::new(b"reader".to_vec())))));
    expect.DeleteFile(Ok(()));
    expect.DeleteFiles(Err("batch delete failed".to_owned()));

    let ctx = Context::default();
    let mut writer = Storage::Create(&storage, &ctx, "created", None).unwrap();
    assert_eq!(writer.write(&ctx, b"written").unwrap(), 7);
    let mut reader = Storage::Open(&storage, &ctx, "opened", None).unwrap();
    let mut contents = String::new();
    reader.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "reader");
    Storage::DeleteFile(&storage, &ctx, "single").unwrap();
    assert_eq!(
        Storage::DeleteFiles(&storage, &ctx, &["one".to_owned(), "two".to_owned()])
            .unwrap_err()
            .to_string(),
        "batch delete failed"
    );

    assert_eq!(
        storage.calls(),
        vec![
            Call::Create("created".to_owned(), None),
            Call::Open("opened".to_owned(), None),
            Call::DeleteFile("single".to_owned()),
            Call::DeleteFiles(vec!["one".to_owned(), "two".to_owned()]),
        ]
    );
    storage.verify();
}

/// GoMock 会把 Create/Open 的 option 参数转交控制器，调用记录不能丢失它们。
#[test]
fn create_and_open_calls_preserve_distinct_options() {
    let storage = NewMockStorage();
    let expect = storage.EXPECT();
    expect.Create(Ok(Box::new(TestWriter::default())));
    expect.Create(Ok(Box::new(TestWriter::default())));
    expect.Open(Ok(Box::new(TestReader(Cursor::new(Vec::new())))));
    expect.Open(Ok(Box::new(TestReader(Cursor::new(Vec::new())))));

    let ctx = Context::default();
    Storage::Create(
        &storage,
        &ctx,
        "same",
        Some(&WriterOption {
            Concurrency: 2,
            PartSize: 8,
        }),
    )
    .unwrap();
    Storage::Create(
        &storage,
        &ctx,
        "same",
        Some(&WriterOption {
            Concurrency: 4,
            PartSize: 16,
        }),
    )
    .unwrap();
    Storage::Open(
        &storage,
        &ctx,
        "same",
        Some(&ReaderOption {
            StartOffset: Some(1),
            EndOffset: Some(9),
            PrefetchSize: 32,
        }),
    )
    .unwrap();
    Storage::Open(&storage, &ctx, "same", None).unwrap();

    let calls = storage.calls();
    assert_ne!(calls[0], calls[1], "Create options must remain observable");
    assert_ne!(calls[2], calls[3], "Open options must remain observable");
    storage.verify();
}

/// GoMock Controller 可被并发调用，MockStorage 也必须能安全跨线程共享。
#[test]
fn mock_storage_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<crate::mockobjstore::MockStorage>();
}
