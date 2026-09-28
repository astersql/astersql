// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 预取 Reader 单元测试：覆盖跨缓冲读、UnexpectedEOF 原样透传、提前 Close、
// 以及源端分片读时 ReadFull 仍能填满预取缓冲的行为。

use std::io::{self, Cursor, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::reader::{NewReader, ReadCloser};

/// 基于内存 Cursor 的简易 ReadCloser，供正常路径测试。
struct TestReader {
    /// 内存数据游标。
    inner: Cursor<Vec<u8>>,
}

impl TestReader {
    /// 用给定字节构造 TestReader。
    fn new(data: &[u8]) -> Self {
        Self {
            inner: Cursor::new(data.to_vec()),
        }
    }
}

impl Read for TestReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl ReadCloser for TestReader {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// TestBasic: read across ping-pong buffers, then verify full and short reads.
/// 跨双缓冲分段读、整段读，以及预取缓冲大于数据源时的短读。
#[test]
fn test_basic() {
    let mut reader = NewReader(Box::new(TestReader::new(b"01234567890")), 11, 3);

    for (size, expected) in [(1, b"0".as_slice()), (2, b"12"), (3, b"345"), (4, b"6789")] {
        let mut buf = vec![0; size];
        assert_eq!(reader.read(&mut buf).unwrap(), size);
        assert_eq!(&buf, expected);
    }

    let mut buf = [0; 4];
    assert_eq!(reader.read(&mut buf).unwrap(), 1);
    assert_eq!(&buf[..1], b"0");
    assert_eq!(reader.read(&mut buf).unwrap(), 0);

    let mut reader = NewReader(Box::new(TestReader::new(b"01234567890")), 11, 3);
    let mut buf = [0; 11];
    assert_eq!(reader.read(&mut buf).unwrap(), 11);
    assert_eq!(&buf, b"01234567890");
    assert_eq!(reader.read(&mut buf).unwrap(), 0);

    let mut reader = NewReader(Box::new(TestReader::new(b"01234")), 5, 100);
    let mut buf = [0; 11];
    assert_eq!(reader.read(&mut buf).unwrap(), 5);
    assert_eq!(&buf[..5], b"01234");
    assert_eq!(reader.read(&mut buf).unwrap(), 0);
}

/// 每次 Read 都返回 UnexpectedEof 的桩，用于验证源端错误不被误转为 EOF。
struct UnexpectedEofReader;

impl Read for UnexpectedEofReader {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "underlying unexpected EOF",
        ))
    }
}

impl ReadCloser for UnexpectedEofReader {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// TestConvertUnexpectedEOF: do not convert an error returned by the source.
/// 源端直接返回 UnexpectedEof 时应原样透传，不按预取偏大场景转换。
#[test]
fn test_convert_unexpected_eof() {
    let mut reader = NewReader(Box::new(UnexpectedEofReader), 10, 10);
    let mut buf = [0; 10];
    assert_eq!(
        reader.read(&mut buf).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

// TestCloseBeforeDrainRead: closing before consuming prefetched data succeeds.
/// 尚未消费完预取数据时调用 Close 也应成功并收尾后台线程。
#[test]
fn test_close_before_drain_read() {
    let mut reader = NewReader(Box::new(TestReader::new(&vec![0; 1024])), 1024, 2);
    reader.close().unwrap();
}

/// 每次最多返回 `fragment_size` 字节的分片源，并用原子计数记录已消费偏移。
struct FragmentReader {
    /// 源数据。
    data: Vec<u8>,
    /// 当前读偏移。
    offset: usize,
    /// 单次 Read 上限。
    fragment_size: usize,
    /// 已消费字节数，供测试等待预取进度。
    consumed: Arc<AtomicUsize>,
}

impl Read for FragmentReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.offset == self.data.len() {
            return Ok(0);
        }
        let n = buf
            .len()
            .min(self.fragment_size)
            .min(self.data.len() - self.offset);
        buf[..n].copy_from_slice(&self.data[self.offset..self.offset + n]);
        self.offset += n;
        self.consumed.store(self.offset, Ordering::SeqCst);
        Ok(n)
    }
}

impl ReadCloser for FragmentReader {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 构造固定数据、每片最多 3 字节的 FragmentReader。
fn fragment_reader(consumed: Arc<AtomicUsize>) -> FragmentReader {
    FragmentReader {
        data: b"0123456789".to_vec(),
        offset: 0,
        fragment_size: 3,
        consumed,
    }
}

/// 轮询等待谓词成立，超时则失败；用于同步后台预取进度。
fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition was not met in time");
        thread::sleep(Duration::from_millis(10));
    }
}

// TestFillPrefetchBuffer: ReadFull fills each 5-byte prefetch buffer even when
// the source returns at most three bytes per read.
/// 源端分片返回时，预取侧 ReadFull 仍应凑满半缓冲后再交付前台。
#[test]
fn test_fill_prefetch_buffer() {
    let consumed = Arc::new(AtomicUsize::new(0));
    let mut source = fragment_reader(Arc::clone(&consumed));
    let mut buf = [0; 5];
    assert_eq!(source.read(&mut buf).unwrap(), 3);
    assert_eq!(&buf[..3], b"012");

    let mut buf = [0; 1];
    assert_eq!(source.read(&mut buf).unwrap(), 1);
    assert_eq!(&buf, b"3");

    let consumed = Arc::new(AtomicUsize::new(0));
    let source = fragment_reader(Arc::clone(&consumed));
    let mut reader = NewReader(Box::new(source), 10, 10);

    wait_until(|| consumed.load(Ordering::SeqCst) == 5);
    let mut buf = [0; 2];
    assert_eq!(reader.read(&mut buf).unwrap(), 2);
    assert_eq!(&buf, b"01");
    wait_until(|| consumed.load(Ordering::SeqCst) == 10);
}
