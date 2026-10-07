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

// 外部文件 Writer 集成测试（对齐 Go objectio 外部包测试）。
//
// 通过桥接 legacy `objstore::storage` 与 storeapi，在本地目录上验证：
// 明文分块写入、压缩读写往返、以及 zstd 解压读在不同并发度下的管道行为。

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::mpsc::{Receiver, sync_channel};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use objectio::compressedio::{self, CompressType, DecompressConfig};
use objstore::compress::CompressionStorage;
use objstore::storage as legacy;
use storeapi::Storage as _;

/// 将 anyhow 错误转为 `io::Error`。
fn io_error(error: anyhow::Error) -> io::Error {
    io::Error::other(error)
}

/// legacy ObjectReader → objectio::Reader 适配桥。
struct ReaderBridge(Box<dyn legacy::ObjectReader>);

impl Read for ReaderBridge {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.0.read(output)
    }
}

impl Seek for ReaderBridge {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}

impl objectio::Reader for ReaderBridge {
    fn close(&mut self) -> io::Result<()> {
        self.0.close().map_err(io_error)
    }

    fn file_size(&self) -> io::Result<i64> {
        self.0.get_file_size().map_err(io_error)
    }
}

/// legacy ObjectWriter → objectio::Writer 适配桥。
struct WriterBridge(Box<dyn legacy::ObjectWriter>);

impl objectio::Writer for WriterBridge {
    fn write(&mut self, ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        ctx.check()?;
        self.0
            .write(&legacy::Context::background(), data)
            .map_err(io_error)
    }

    fn close(&mut self, ctx: &objectio::Context) -> io::Result<()> {
        ctx.check()?;
        self.0
            .close(&legacy::Context::background())
            .map_err(io_error)
    }
}

/// legacy StorageRef → storeapi::Storage 适配桥。
struct StorageBridge(legacy::StorageRef);

/// 将 objectio Context 的取消检查映射为 legacy background Context。
fn legacy_context(ctx: &objectio::Context) -> Result<legacy::Context> {
    ctx.check()?;
    Ok(legacy::Context::background())
}

impl storeapi::Storage for StorageBridge {
    fn WriteFile(&self, ctx: &objectio::Context, name: &str, data: &[u8]) -> Result<()> {
        self.0.WriteFile(&legacy_context(ctx)?, name, data)
    }

    fn ReadFile(&self, ctx: &objectio::Context, name: &str) -> Result<Vec<u8>> {
        self.0.ReadFile(&legacy_context(ctx)?, name)
    }

    fn FileExists(&self, ctx: &objectio::Context, name: &str) -> Result<bool> {
        self.0.FileExists(&legacy_context(ctx)?, name)
    }

    fn DeleteFile(&self, ctx: &objectio::Context, name: &str) -> Result<()> {
        self.0.DeleteFile(&legacy_context(ctx)?, name)
    }

    fn Open(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        let option = option.map(|option| legacy::ReaderOption {
            start_offset: option.StartOffset,
            end_offset: option.EndOffset,
        });
        Ok(Box::new(ReaderBridge(self.0.Open(
            &legacy_context(ctx)?,
            path,
            option.as_ref(),
        )?)))
    }

    fn DeleteFiles(&self, ctx: &objectio::Context, names: &[String]) -> Result<()> {
        self.0.DeleteFiles(&legacy_context(ctx)?, names)
    }

    fn WalkDir(
        &self,
        ctx: &objectio::Context,
        option: Option<&storeapi::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        let option = option.map(|option| legacy::WalkOption {
            sub_dir: option.SubDir.clone(),
            obj_prefix: option.ObjPrefix.clone(),
            skip_sub_dir: option.SkipSubDir,
            include_tombstone: option.IncludeTombstone,
            start_after: option.StartAfter.clone(),
        });
        self.0
            .WalkDir(&legacy_context(ctx)?, option.as_ref(), callback)
    }

    fn URI(&self) -> String {
        self.0.URI()
    }

    fn Create(
        &self,
        ctx: &objectio::Context,
        path: &str,
        _option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        Ok(Box::new(WriterBridge(self.0.Create(
            &legacy_context(ctx)?,
            path,
            None,
        )?)))
    }

    fn Rename(&self, ctx: &objectio::Context, old_name: &str, new_name: &str) -> Result<()> {
        self.0.Rename(&legacy_context(ctx)?, old_name, new_name)
    }

    fn PresignFile(
        &self,
        ctx: &objectio::Context,
        name: &str,
        duration: Duration,
    ) -> Result<String> {
        self.0.PresignFile(&legacy_context(ctx)?, name, duration)
    }

    fn Close(&self) {
        self.0.Close();
    }
}

/// 按 URI 解析后端并包一层压缩存储。
fn get_store(uri: &str, compress_type: CompressType) -> CompressionStorage<StorageBridge> {
    let backend = objstore::parse::ParseBackend(uri, None).expect("ParseBackend");
    let storage =
        legacy::Create(&legacy::Context::background(), &backend, true).expect("Create storage");
    objstore::compress::with_compression(
        StorageBridge(storage),
        compress_type,
        DecompressConfig::default(),
    )
}

/// 用 Create/write/close 将多行内容写入指定对象。
fn write_file(storage: &dyn storeapi::Storage, file_name: &str, lines: &[&str]) {
    let ctx = objectio::Context::default();
    let mut writer = storage
        .Create(&ctx, file_name, None)
        .expect("Create writer");
    for line in lines {
        let data = line.as_bytes();
        let written = writer.write(&ctx, data).expect("writer.write");
        assert_eq!(written, data.len());
    }
    writer.close(&ctx).expect("writer.close");
}

/// 本地明文存储：多组短/长文本经 Writer 写入后与磁盘内容一致。
#[test]
fn test_external_file_writer() {
    let directory = tempfile::tempdir().unwrap();
    let cases = [
        ("short and sweet", vec!["hi"]),
        ("long text small chunks", vec!["hello world"; 6]),
        ("long text medium chunks", vec!["hello world"; 6]),
        ("long text large chunks", vec!["hello world"; 6]),
    ];

    for (name, content) in cases {
        let storage = get_store(
            &format!("local://{}", directory.path().display()),
            CompressType::NoCompression,
        );
        let file_name = format!("{}.txt", name.replace(' ', "-"));
        write_file(&storage, &file_name, &content);
        let actual = std::fs::read(directory.path().join(file_name)).unwrap();
        assert_eq!(actual, content.join("").as_bytes());
    }
}

/// 缺少 bucket 的错误不得泄露对象存储 URL 中的访问凭证。
#[test]
fn missing_bucket_error_redacts_credentials() {
    let raw = "s3:///prefix?access-key=secret-id&secret-access-key=secret-key&session-token=secret-token";
    let error = objstore::parse::ParseBackend(raw, None)
        .unwrap_err()
        .to_string();

    assert_eq!(
        error,
        "please specify the bucket for s3 in s3:///prefix?access-key=xxxxxx&secret-access-key=xxxxxx&session-token=xxxxxx"
    );
    for secret in ["secret-id", "secret-key", "secret-token"] {
        assert!(!error.contains(secret));
    }
}

/// Gzip/Snappy/Zstd：磁盘直接解压与经 Storage::Open 读取结果均与原文一致。
#[test]
fn test_compress_reader_writer() {
    let directory = tempfile::tempdir().unwrap();
    let cases = [
        ("long text medium chunks", vec!["hello world"; 6]),
        ("long text large chunks", vec!["hello world"; 6]),
    ];

    for (name, content) in cases {
        for compress_type in [CompressType::Gzip, CompressType::Snappy, CompressType::Zstd] {
            let file_name = format!("{}{}", name.replace(' ', "-"), compress_type.file_suffix());
            let storage = get_store(
                &format!("local://{}", directory.path().display()),
                compress_type,
            );
            write_file(&storage, &file_name, &content);

            let file = std::fs::File::open(directory.path().join(&file_name)).unwrap();
            let mut reader = compressedio::new_reader(
                compress_type,
                DecompressConfig::default(),
                Box::new(file),
            )
            .unwrap()
            .unwrap();
            let mut directly_decoded = Vec::new();
            reader.read_to_end(&mut directly_decoded).unwrap();
            assert_eq!(directly_decoded, content.join("").as_bytes());

            let mut reader = storage
                .Open(&objectio::Context::default(), &file_name, None)
                .unwrap();
            let mut storage_decoded = Vec::new();
            reader.read_to_end(&mut storage_decoded).unwrap();
            assert_eq!(storage_decoded, content.join("").as_bytes());
            reader.close().unwrap();
        }
    }
}

/// 从同步 channel 拉取分片的 Read 实现，模拟管道式压缩输入。
struct ChannelReader {
    receiver: Receiver<Option<Vec<u8>>>,
    current: io::Cursor<Vec<u8>>,
    eof: bool,
}

impl ChannelReader {
    fn new(receiver: Receiver<Option<Vec<u8>>>) -> Self {
        Self {
            receiver,
            current: io::Cursor::new(Vec::new()),
            eof: false,
        }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        loop {
            let read = self.current.read(output)?;
            if read != 0 || output.is_empty() {
                return Ok(read);
            }
            if self.eof {
                return Ok(0);
            }
            match self.receiver.recv() {
                Ok(Some(data)) => self.current = io::Cursor::new(data),
                Ok(None) | Err(_) => self.eof = true,
            }
        }
    }
}

/// zstd new_reader：默认并发下先发完管道再读；concurrency=1 需另线程写管道。
#[test]
fn test_new_compress_reader() {
    let compressed_data = zstd::stream::encode_all(&b"data"[..], 0).unwrap();

    // The default Go decoder consumes its pipe on a background worker, so both
    // rendezvous sends complete before the caller starts reading decoded bytes.
    // 默认 Go 解码器在后台消费管道，故两次 rendezvous send 可在开始读前完成。
    let (sender, receiver) = sync_channel(0);
    let mut reader = compressedio::new_reader(
        CompressType::Zstd,
        DecompressConfig::default(),
        Box::new(ChannelReader::new(receiver)),
    )
    .unwrap()
    .unwrap();
    sender.send(Some(compressed_data.clone())).unwrap();
    sender.send(None).unwrap();
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, b"data");

    // Concurrency 1 is synchronous. As in Go, a separate writer is required
    // for a rendezvous pipe while read_to_end drives decoding on this thread.
    // 并发度为 1 时同步解码；与 Go 一样需另线程写 rendezvous 管道。
    let (sender, receiver) = sync_channel(0);
    let mut reader = compressedio::new_reader(
        CompressType::Zstd,
        DecompressConfig {
            zstd_decode_concurrency: 1,
        },
        Box::new(ChannelReader::new(receiver)),
    )
    .unwrap()
    .unwrap();
    let writer = thread::spawn(move || {
        sender.send(Some(compressed_data)).unwrap();
        sender.send(None).unwrap();
    });
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual).unwrap();
    writer.join().unwrap();
    assert_eq!(actual, b"data");
}
