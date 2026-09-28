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

// AES-CTR 加解密层单元测试：与 checksum 管线组合的读写正确性。
//
// 对应 Go `aes_layer_test.go`。用内存文件模拟底层存储，覆盖纯加密、
// checksum→加密、加密→checksum 以及双层加密管线；并保留 Go Benchmark 名称占位。

use checksum;
use std::sync::{Arc, Mutex};
use util_encrypt::aes_layer;

/// 线程安全内存文件，实现加解密层与 checksum 两侧的读写 trait。
#[derive(Clone, Default)]
struct MemoryFile(Arc<Mutex<Vec<u8>>>);

impl aes_layer::WriteCloser for MemoryFile {
    fn write(&mut self, p: &[u8]) -> (usize, Option<String>) {
        self.0.lock().unwrap().extend_from_slice(p);
        (p.len(), None)
    }

    fn close(&mut self) -> Option<String> {
        None
    }
}

impl aes_layer::ReaderAt for MemoryFile {
    fn read_at(&self, p: &mut [u8], off: u64) -> (usize, Option<String>) {
        let data = self.0.lock().unwrap();
        let start = usize::try_from(off).unwrap();
        if start >= data.len() {
            return (0, Some("EOF".to_owned()));
        }
        let n = p.len().min(data.len() - start);
        p[..n].copy_from_slice(&data[start..start + n]);
        (n, (n < p.len()).then(|| "EOF".to_owned()))
    }
}

impl checksum::WriteCloserDraft for MemoryFile {
    fn Write(&mut self, p: &[u8]) -> (usize, Option<String>) {
        aes_layer::WriteCloser::write(self, p)
    }

    fn Close(&mut self) -> Option<String> {
        None
    }
}

impl checksum::ReaderAtDraft for MemoryFile {
    fn ReadAt(&self, p: &mut [u8], off: i64) -> (usize, Option<String>) {
        aes_layer::ReaderAt::read_at(self, p, off as u64)
    }
}

/// 将 `aes_layer::Writer` 适配为 `WriteCloser`。
struct AesWriter<W: aes_layer::WriteCloser>(aes_layer::Writer<W>);

impl<W: aes_layer::WriteCloser> aes_layer::WriteCloser for AesWriter<W> {
    fn write(&mut self, p: &[u8]) -> (usize, Option<String>) {
        self.0.Write(p)
    }

    fn close(&mut self) -> Option<String> {
        self.0.Close()
    }
}

/// 将 AES Writer 适配为 checksum 的 `WriteCloserDraft`。
struct AesAsChecksumWriter<W: aes_layer::WriteCloser>(aes_layer::Writer<W>);

impl<W: aes_layer::WriteCloser> checksum::WriteCloserDraft for AesAsChecksumWriter<W> {
    fn Write(&mut self, p: &[u8]) -> (usize, Option<String>) {
        self.0.Write(p)
    }

    fn Close(&mut self) -> Option<String> {
        self.0.Close()
    }
}

/// 将 checksum Writer 适配为 AES 层的 `WriteCloser`。
struct ChecksumAsAesWriter<W: checksum::WriteCloserDraft>(checksum::Writer<W>);

impl<W: checksum::WriteCloserDraft> aes_layer::WriteCloser for ChecksumAsAesWriter<W> {
    fn write(&mut self, p: &[u8]) -> (usize, Option<String>) {
        self.0.Write(p)
    }

    fn close(&mut self) -> Option<String> {
        self.0.Close()
    }
}

/// 将 `aes_layer::Reader` 适配为 `ReaderAt`。
struct AesReader<R: aes_layer::ReaderAt>(aes_layer::Reader<R>);

impl<R: aes_layer::ReaderAt> aes_layer::ReaderAt for AesReader<R> {
    fn read_at(&self, p: &mut [u8], off: u64) -> (usize, Option<String>) {
        self.0.ReadAt(p, off as i64)
    }
}

/// 将 AES Reader 适配为 checksum 的 `ReaderAtDraft`。
struct AesAsChecksumReader<R: aes_layer::ReaderAt>(aes_layer::Reader<R>);

impl<R: aes_layer::ReaderAt> checksum::ReaderAtDraft for AesAsChecksumReader<R> {
    fn ReadAt(&self, p: &mut [u8], off: i64) -> (usize, Option<String>) {
        self.0.ReadAt(p, off)
    }
}

/// 将 checksum Reader 适配为 AES 层的 `ReaderAt`。
struct ChecksumAsAesReader<R: checksum::ReaderAtDraft>(checksum::Reader<R>);

impl<R: checksum::ReaderAtDraft> aes_layer::ReaderAt for ChecksumAsAesReader<R> {
    fn read_at(&self, p: &mut [u8], off: u64) -> (usize, Option<String>) {
        self.0.ReadAt(p, off as i64)
    }
}

/// 连续写两遍相同数据并关闭，返回总写入字节数。
fn write_twice<W: aes_layer::WriteCloser>(mut writer: W, data: &[u8]) -> usize {
    let (n1, err1) = writer.write(data);
    assert!(err1.is_none());
    let (n2, err2) = writer.write(data);
    assert!(err2.is_none());
    assert!(writer.close().is_none());
    n1 + n2
}

/// checksum 路径下连续写两遍并关闭，返回总写入字节数。
fn write_twice_checksum<W: checksum::WriteCloserDraft>(mut writer: W, data: &[u8]) -> usize {
    let (n1, err1) = writer.Write(data);
    assert!(err1.is_none());
    let (n2, err2) = writer.Write(data);
    assert!(err2.is_none());
    assert!(writer.Close().is_none());
    n1 + n2
}

/// 在若干偏移处断言解密读出的明文与期望切片一致。
fn assert_reads<R: aes_layer::ReaderAt>(reader: R, total: usize, pipeline: &str) {
    for (off, expected_error, expected_n, expected) in [
        (0_u64, None, 10, b"0123456789".as_slice()),
        (5, None, 10, b"5678901234".as_slice()),
        (
            (total - 5) as u64,
            Some("EOF".to_owned()),
            5,
            b"56789".as_slice(),
        ),
    ] {
        let mut buf = [0_u8; 10];
        let (n, error) = reader.read_at(&mut buf, off);
        assert_eq!(expected_error, error, "{pipeline} at offset {off}");
        assert_eq!(expected_n, n, "{pipeline} at offset {off}");
        assert_eq!(expected, &buf[..n], "{pipeline} at offset {off}");
        assert!(buf[n..].iter().all(|byte| *byte == 0));
    }
}

/// 覆盖四条管线：纯加密、checksum→加密、加密→checksum、双层加密的 ReadAt 正确性。
#[test]
fn test_read_at() {
    let data = "0123456789".repeat(510).into_bytes();

    let file = MemoryFile::default();
    let cipher = aes_layer::NewCtrCipher().unwrap();
    let total = write_twice(
        AesWriter(aes_layer::NewWriter(file.clone(), &cipher)),
        &data,
    );
    assert_reads(
        AesReader(aes_layer::NewReader(file, cipher)),
        total,
        "encrypt",
    );

    let file = MemoryFile::default();
    let cipher = aes_layer::NewCtrCipher().unwrap();
    let writer = checksum::NewWriter(AesAsChecksumWriter(aes_layer::NewWriter(
        file.clone(),
        &cipher,
    )));
    let total = write_twice_checksum(writer, &data);
    let reader = checksum::NewReader(AesAsChecksumReader(aes_layer::NewReader(file, cipher)));
    assert_reads(ChecksumAsAesReader(reader), total, "checksum-encrypt");

    let file = MemoryFile::default();
    let cipher = aes_layer::NewCtrCipher().unwrap();
    let writer = AesWriter(aes_layer::NewWriter(
        ChecksumAsAesWriter(checksum::NewWriter(file.clone())),
        &cipher,
    ));
    let total = write_twice(writer, &data);
    let reader = AesReader(aes_layer::NewReader(
        ChecksumAsAesReader(checksum::NewReader(file)),
        cipher,
    ));
    assert_reads(reader, total, "encrypt-checksum");

    let file = MemoryFile::default();
    let cipher1 = aes_layer::NewCtrCipher().unwrap();
    let cipher2 = aes_layer::NewCtrCipher().unwrap();
    let writer1 = AesWriter(aes_layer::NewWriter(file.clone(), &cipher1));
    let writer2 = AesWriter(aes_layer::NewWriter(writer1, &cipher2));
    let total = write_twice(writer2, &data);
    let reader1 = AesReader(aes_layer::NewReader(file, cipher1));
    let reader2 = AesReader(aes_layer::NewReader(reader1, cipher2));
    assert_reads(reader2, total, "double-encrypt");
}

/// 保留 Go BenchmarkReadAt 三条管线名称，避免伪装成普通测试。
// Go 的 BenchmarkReadAt 不属于默认单元测试；这里保留其三条管线名称，避免把 benchmark
// 伪装成普通测试或在稳定 Rust 上引入 nightly benchmark harness。
#[allow(dead_code)]
fn benchmark_read_at() -> [&'static str; 3] {
    [
        "data->file",
        "data->checksum->file",
        "data->checksum->encrypt->file",
    ]
}
