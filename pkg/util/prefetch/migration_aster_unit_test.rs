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

// AsterSQL 迁移补充：prefetch Reader 跨缓冲读取与关闭语义测试。
//
// 覆盖跨缓冲顺序读、短源 EOF、底层 UnexpectedEof 透传、
// 分片填充预取缓冲，以及重复 close 的幂等性。

use std::io::{self, Cursor, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::reader::{NewReader, ReadCloser};

/// 可计数 close 次数的内存源，用于验证预取 Reader 生命周期。
struct TestReader {
    inner: Cursor<Vec<u8>>,
    close_count: Arc<AtomicUsize>,
}

impl TestReader {
    fn new(data: &[u8], close_count: Arc<AtomicUsize>) -> Self {
        Self {
            inner: Cursor::new(data.to_vec()),
            close_count,
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
        self.close_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
/// 校验跨多个预取缓冲的顺序读取并能正确到达 EOF。
fn reads_across_prefetch_buffers_and_reaches_eof() {
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut reader = NewReader(
        Box::new(TestReader::new(b"01234567890", close_count)),
        11,
        6,
    );

    for (size, expected) in [(1, "0"), (2, "12"), (3, "345"), (4, "6789")] {
        let mut buf = vec![0; size];
        assert_eq!(reader.read(&mut buf).unwrap(), size);
        assert_eq!(&buf, expected.as_bytes());
    }

    let mut tail = [0; 4];
    assert_eq!(reader.read(&mut tail).unwrap(), 1);
    assert_eq!(&tail[..1], b"0");
    assert_eq!(reader.read(&mut tail).unwrap(), 0);
}

#[test]
/// 校验源数据短于缓冲时先返回数据再返回 EOF。
fn short_source_returns_data_before_eof() {
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut reader = NewReader(Box::new(TestReader::new(b"01234", close_count)), 5, 100);
    let mut buf = [0; 11];

    assert_eq!(reader.read(&mut buf).unwrap(), 5);
    assert_eq!(&buf[..5], b"01234");
    assert_eq!(reader.read(&mut buf).unwrap(), 0);
}

/// 始终返回 UnexpectedEof 的底层源。
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

#[test]
/// 校验底层 UnexpectedEof 会原样向上透传。
fn preserves_underlying_unexpected_eof() {
    let mut reader = NewReader(Box::new(UnexpectedEofReader), 10, 10);
    let mut buf = [0; 10];
    assert_eq!(
        reader.read(&mut buf).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

/// 按固定分片大小返回数据的源，用于观察预取填充进度。
struct FragmentReader {
    data: Vec<u8>,
    offset: usize,
    fragment_size: usize,
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

/// 在超时前轮询等待谓词成立。
fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition was not met in time");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
/// 校验分片读场景下预取缓冲会被逐步填满。
fn fills_each_prefetch_buffer_across_fragmented_reads() {
    let consumed = Arc::new(AtomicUsize::new(0));
    let source = FragmentReader {
        data: b"0123456789".to_vec(),
        offset: 0,
        fragment_size: 3,
        consumed: Arc::clone(&consumed),
    };
    let mut reader = NewReader(Box::new(source), 10, 10);

    wait_until(|| consumed.load(Ordering::SeqCst) == 5);
    let mut buf = [0; 2];
    assert_eq!(reader.read(&mut buf).unwrap(), 2);
    assert_eq!(&buf, b"01");
    wait_until(|| consumed.load(Ordering::SeqCst) == 10);
}

#[test]
/// 校验未读完时 close 幂等，且底层只关闭一次。
fn close_before_drain_is_idempotent_and_closes_source_once() {
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut reader = NewReader(
        Box::new(TestReader::new(&vec![0; 1024], Arc::clone(&close_count))),
        1024,
        2,
    );

    reader.close().unwrap();
    reader.close().unwrap();
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
}
