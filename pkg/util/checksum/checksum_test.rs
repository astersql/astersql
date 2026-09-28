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

// 对应 Go `checksum_test.go`：校验和读写、损坏模式与 CTR 加密叠加场景。
//
// 通过 FakeFile / MockWriter 注入增删改字节，并可选叠加 AES-CTR，
// 验证 Reader 在明文与加密路径下均能正确检出损坏块。

#![allow(non_snake_case)]

use crate::{NewReader, NewWriter, ReaderAtDraft, WriteCloserDraft, errChecksumFail};
use aes::Aes256;
use ctr::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use std::cell::RefCell;
use std::rc::Rc;

/// 对齐 Go `io.EOF` 的错误文案。
const EOF: &str = "EOF";
type Aes256Ctr = ctr::Ctr128BE<Aes256>;

/// 多层嵌套 Writer/Reader 的基本偏移读写。
#[test]
fn test_checksum_read_at() {
    let f = FakeFile::default();
    let data = repeated("0123456789", 510);
    let mut writer = NewWriter(NewWriter(NewWriter(NewWriter(f.clone()))));
    let (n1, err) = writer.Write(&data);
    assert_eq!(None, err);
    let (n2, err) = writer.Write(&data);
    assert_eq!(None, err);
    assert_eq!(None, writer.Close());

    let assert_read = |off, expected_error: Option<&str>, expected_n, expected: &[u8]| {
        let reader = NewReader(NewReader(NewReader(NewReader(f.clone()))));
        let mut result = [0; 10];
        let (n, err) = reader.ReadAt(&mut result, off);
        assert_error(err, expected_error);
        assert_eq!(expected_n, n);
        assert_eq!(expected, result);
    };
    assert_read(0, None, 10, b"0123456789");
    assert_read(5, None, 10, b"5678901234");
    assert_read((n1 + n2) as i64 - 5, Some(EOF), 5, b"56789\0\0\0\0\0");
}

/// 在编码流中插入一字节，校验后续块均报 checksum 失败。
#[test]
fn test_add_one_byte() {
    for encrypted in [false, true] {
        let file = write_corrupted(encrypted, |mut data, offset| {
            let insert_pos = 5000;
            if offset < insert_pos && offset + data.len() >= insert_pos {
                data.insert(insert_pos - offset, 0);
            }
            data
        });
        assert_corruption_pattern(&file, encrypted, |i| i >= 5);
    }
}

/// 删除一字节后，损坏块及之后应失败。
#[test]
fn test_delete_one_byte() {
    for encrypted in [false, true] {
        let file = write_corrupted(encrypted, |mut data, offset| {
            let delete_pos = 5000;
            if offset < delete_pos && offset + data.len() >= delete_pos {
                data.remove(delete_pos - offset - 1);
            }
            data
        });
        assert_corruption_pattern(&file, encrypted, |i| i >= 5);
    }
}

/// 修改单字节仅使对应块失败。
#[test]
fn test_modify_one_byte() {
    for encrypted in [false, true] {
        let file = write_corrupted(encrypted, |mut data, offset| {
            let modify_pos = 5000;
            if offset < modify_pos && offset + data.len() >= modify_pos {
                let pos = modify_pos - offset - 1;
                data[pos] = data[pos].wrapping_sub(1);
            }
            data
        });
        assert_corruption_pattern(&file, encrypted, |i| i == 5);
    }
}

/// 空文件任意偏移读均返回 EOF。
#[test]
fn test_read_empty_file() {
    for encrypted in [false, true] {
        let file = FakeFile::default();
        for i in 0..11 {
            let mut result = [0; 10];
            let (_, err) = read_checksum(&file, encrypted, &mut result, (i * 1020) as i64);
            assert_error(err, Some(EOF));
        }
    }
}

/// 同一块内改三字节，仍只影响该块。
#[test]
fn test_modify_three_bytes() {
    for encrypted in [false, true] {
        let file = write_corrupted(encrypted, |mut data, offset| {
            let modify_pos = 5000;
            if offset < modify_pos && offset + data.len() >= modify_pos && data.len() == 1024 {
                for pos in [200, 300, 400] {
                    data[pos] = data[pos].wrapping_sub(1);
                }
            }
            data
        });
        assert_corruption_pattern(&file, encrypted, |i| i == 5);
    }
}

/// 不同请求长度的跨块读取与 EOF 填充语义。
#[test]
fn test_read_different_block_size() {
    for encrypted in [false, true] {
        let file = FakeFile::default();
        write_checksum(&file, encrypted, &repeated("0123456789", 1020));

        assert_read(
            &file,
            encrypted,
            2000,
            1000,
            1000,
            None,
            repeated("0123456789", 100),
        );
        assert_read(
            &file,
            encrypted,
            3005,
            3000,
            3000,
            None,
            repeated("5678901234", 300),
        );
        assert_read(
            &file,
            encrypted,
            10000,
            200,
            200,
            None,
            repeated("0123456789", 20),
        );
        let mut expected = repeated("0123456789", 20);
        expected.push(0);
        assert_read(&file, encrypted, 10000, 201, 200, Some(EOF), expected);
        assert_read(
            &file,
            encrypted,
            5000,
            5200,
            5200,
            None,
            repeated("0123456789", 520),
        );
        let mut expected = repeated("0123456789", 520);
        expected.extend([0; 800]);
        assert_read(&file, encrypted, 5000, 6000, 5200, Some(EOF), expected);
        assert_read(
            &file,
            encrypted,
            0,
            10200,
            10200,
            None,
            repeated("0123456789", 1020),
        );
        let mut expected = repeated("0123456789", 1020);
        expected.extend([0; 800]);
        assert_read(&file, encrypted, 0, 11000, 10200, Some(EOF), expected);
    }
}

/// 整段写入与分块写入应得到相同编码结果。
#[test]
fn test_write_different_block_size() {
    for encrypted in [false, true] {
        let file1 = FakeFile::default();
        let file2 = FakeFile::default();
        let data = repeated("0123456789", 1020);
        write_checksum(&file1, encrypted, &data);
        write_checksum_in_chunks(&file2, encrypted, &data, 100);
        assert_eq!(file1.bytes(), file2.bytes());
        assert_read(&file1, encrypted, 0, 10200, 10200, None, data.clone());
        assert_read(&file2, encrypted, 0, 10200, 10200, None, data.clone());
    }
}

/// Flush 后可读回，并更新缓存偏移。
#[test]
fn test_checksum_writer() {
    let file = FakeFile::default();
    let data = repeated("0123456789", 100);
    let mut writer = NewWriter(file.clone());
    assert_eq!((1000, None), writer.Write(&data));
    assert_eq!(None, writer.Flush());
    assert_read(&file, false, 0, 1000, 1000, None, data);
    assert_eq!(1000, writer.GetCacheDataOffset());
}

/// 缓冲满时 Write 自动 Flush 完整块。
#[test]
fn test_checksum_writer_auto_flush() {
    let file = FakeFile::default();
    let data = repeated("0123456789", 102);
    let mut writer = NewWriter(file.clone());
    assert_eq!((data.len(), None), writer.Write(&data));
    assert_eq!((1, None), writer.Write(b"0"));
    assert_read(&file, false, 0, 1020, 1020, None, data.clone());
    assert_eq!(data.len() as i64, writer.GetCacheDataOffset());
}

/// 重复字符串以构造测试载荷。
fn repeated(value: &str, count: usize) -> Vec<u8> {
    value.repeat(count).into_bytes()
}

/// 比较 Option 错误文案。
fn assert_error(actual: Option<String>, expected: Option<&str>) {
    assert_eq!(expected, actual.as_deref());
}

/// 按偏移读取并断言长度、错误与内容。
fn assert_read(
    file: &FakeFile,
    encrypted: bool,
    off: i64,
    len: usize,
    expected_n: usize,
    expected_error: Option<&str>,
    expected: Vec<u8>,
) {
    let mut result = vec![0; len];
    let (n, err) = read_checksum(file, encrypted, &mut result, off);
    assert_error(err, expected_error);
    assert_eq!(expected_n, n);
    assert_eq!(expected, result);
}

/// 可选经 CTR 解密路径读取校验和流。
fn read_checksum(
    file: &FakeFile,
    encrypted: bool,
    result: &mut [u8],
    off: i64,
) -> (usize, Option<String>) {
    if encrypted {
        NewReader(CtrReader(file.clone())).ReadAt(result, off)
    } else {
        NewReader(file.clone()).ReadAt(result, off)
    }
}

/// 一次性写入完整载荷（可选加密）。
fn write_checksum(file: &FakeFile, encrypted: bool, data: &[u8]) {
    if encrypted {
        write_all(NewWriter(CtrWriter::new(file.clone())), data);
    } else {
        write_all(NewWriter(file.clone()), data);
    }
}

/// 按固定 chunk 大小分次写入（可选加密）。
fn write_checksum_in_chunks(file: &FakeFile, encrypted: bool, data: &[u8], chunk_size: usize) {
    if encrypted {
        write_chunks(NewWriter(CtrWriter::new(file.clone())), data, chunk_size);
    } else {
        write_chunks(NewWriter(file.clone()), data, chunk_size);
    }
}

fn write_all<W: WriteCloserDraft>(mut writer: W, data: &[u8]) {
    assert_eq!((data.len(), None), writer.Write(data));
    assert_eq!(None, writer.Close());
}

fn write_chunks<W: WriteCloserDraft>(mut writer: W, data: &[u8], chunk_size: usize) {
    for chunk in data.chunks(chunk_size) {
        assert_eq!((chunk.len(), None), writer.Write(chunk));
    }
    assert_eq!(None, writer.Close());
}

/// 写入过程中通过 `corrupt` 回调篡改落盘字节，得到损坏文件。
fn write_corrupted<F>(encrypted: bool, corrupt: F) -> FakeFile
where
    F: Fn(Vec<u8>, usize) -> Vec<u8> + 'static,
{
    let file = FakeFile::default();
    let mock = MockWriter {
        inner: file.clone(),
        corrupt: Box::new(corrupt),
        offset: 0,
    };
    let data = repeated("0123456789", 510);
    if encrypted {
        let mut writer = NewWriter(CtrWriter::new(mock));
        assert_eq!((data.len(), None), writer.Write(&data));
        assert_eq!((data.len(), None), writer.Write(&data));
        assert_eq!(None, writer.Close());
    } else {
        let mut writer = NewWriter(mock);
        assert_eq!((data.len(), None), writer.Write(&data));
        assert_eq!((data.len(), None), writer.Write(&data));
        assert_eq!(None, writer.Close());
    }
    file
}

/// 按块步进读取，断言哪些偏移应返回 checksum 失败。
fn assert_corruption_pattern<F>(file: &FakeFile, encrypted: bool, is_corrupt: F)
where
    F: Fn(usize) -> bool,
{
    for i in 0.. {
        let mut result = [0; 10];
        let (_, err) = read_checksum(file, encrypted, &mut result, (i * 1000) as i64);
        if err.as_deref() == Some(EOF) {
            break;
        }
        if is_corrupt(i) {
            assert_error(err, Some(errChecksumFail));
        } else {
            assert_error(err, None);
        }
    }
}

/// 内存假文件，实现 WriteCloser / ReaderAt。
#[derive(Clone, Default)]
struct FakeFile(Rc<RefCell<Vec<u8>>>);

impl FakeFile {
    fn bytes(&self) -> Vec<u8> {
        self.0.borrow().clone()
    }
}

impl WriteCloserDraft for FakeFile {
    fn Write(&mut self, data: &[u8]) -> (usize, Option<String>) {
        self.0.borrow_mut().extend_from_slice(data);
        (data.len(), None)
    }

    fn Close(&mut self) -> Option<String> {
        None
    }
}

impl ReaderAtDraft for FakeFile {
    fn ReadAt(&self, result: &mut [u8], off: i64) -> (usize, Option<String>) {
        let data = self.0.borrow();
        let off = off as usize;
        if off > data.len() {
            return (0, Some(EOF.to_owned()));
        }
        let n = result.len().min(data.len() - off);
        result[..n].copy_from_slice(&data[off..off + n]);
        let err = (off + n == data.len()).then(|| EOF.to_owned());
        (n, err)
    }
}

/// 写入时按偏移回调篡改数据，用于注入损坏。
struct MockWriter {
    inner: FakeFile,
    corrupt: Box<dyn Fn(Vec<u8>, usize) -> Vec<u8>>,
    offset: usize,
}

impl WriteCloserDraft for MockWriter {
    fn Write(&mut self, data: &[u8]) -> (usize, Option<String>) {
        let reported = data.len();
        let data = (self.corrupt)(data.to_vec(), self.offset);
        let (written, err) = self.inner.Write(&data);
        self.offset += written;
        (reported, err)
    }

    fn Close(&mut self) -> Option<String> {
        self.inner.Close()
    }
}

/// 在校验和之下叠加 AES-CTR 加密的 Writer 包装。
struct CtrWriter<W> {
    inner: W,
    offset: u64,
}

impl<W> CtrWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, offset: 0 }
    }
}

impl<W: WriteCloserDraft> WriteCloserDraft for CtrWriter<W> {
    fn Write(&mut self, data: &[u8]) -> (usize, Option<String>) {
        let mut encrypted = data.to_vec();
        apply_ctr(self.offset, &mut encrypted);
        let (n, err) = self.inner.Write(&encrypted);
        self.offset += n as u64;
        (n, err)
    }

    fn Close(&mut self) -> Option<String> {
        self.inner.Close()
    }
}

/// 读取后按偏移做 CTR 解密的 Reader 包装。
struct CtrReader(FakeFile);

impl ReaderAtDraft for CtrReader {
    fn ReadAt(&self, result: &mut [u8], off: i64) -> (usize, Option<String>) {
        let (n, err) = self.0.ReadAt(result, off);
        apply_ctr(off as u64, &mut result[..n]);
        (n, err)
    }
}

/// 用固定 key/iv 对 `[offset, offset+len)` 做 AES-256-CTR 异或。
fn apply_ctr(offset: u64, data: &mut [u8]) {
    let key = [7_u8; 32];
    let iv = [9_u8; 16];
    let mut cipher = Aes256Ctr::new(&key.into(), &iv.into());
    cipher.seek(offset);
    cipher.apply_keystream(data);
}
