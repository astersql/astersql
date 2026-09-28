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

// checksum 迁移补充回归：块编码、跨块读、损坏检测与 short write。
//
// 使用内存假文件验证 Writer/Reader 与 Go 语义对齐，含 CRC 小端布局、
// EOF 部分读以及 Flush 失败后锁存错误。

use std::cell::RefCell;
use std::rc::Rc;

use crate::{
    NewReader, NewWriter, ReaderAtDraft, WriteCloserDraft, checksumPayloadSize, errChecksumFail,
};

/// 内存假文件：可同时作为 WriteCloser 与 ReaderAt。
#[derive(Clone, Default)]
struct MemoryFile {
    data: Rc<RefCell<Vec<u8>>>,
    close_error: Option<String>,
}

impl WriteCloserDraft for MemoryFile {
    fn Write(&mut self, p: &[u8]) -> (usize, Option<String>) {
        self.data.borrow_mut().extend_from_slice(p);
        (p.len(), None)
    }

    fn Close(&mut self) -> Option<String> {
        self.close_error.clone()
    }
}

impl ReaderAtDraft for MemoryFile {
    fn ReadAt(&self, p: &mut [u8], off: i64) -> (usize, Option<String>) {
        let data = self.data.borrow();
        let off = off as usize;
        if off >= data.len() {
            return (0, Some("EOF".to_owned()));
        }
        let n = p.len().min(data.len() - off);
        p[..n].copy_from_slice(&data[off..off + n]);
        let err = (off + n == data.len()).then(|| "EOF".to_owned());
        (n, err)
    }
}

/// 写入跨两块的载荷，校验小端 CRC 与缓存偏移。
#[test]
fn writes_little_endian_crc_blocks_and_tracks_cache_offset() {
    let file = MemoryFile::default();
    let view = file.clone();
    let payload = vec![0x5a; checksumPayloadSize + 7];
    let mut writer = NewWriter(file);

    assert_eq!(writer.Write(&payload), (payload.len(), None));
    assert_eq!(writer.GetCache(), &[0x5a; 7]);
    assert_eq!(writer.GetCacheDataOffset(), checksumPayloadSize as i64);
    assert_eq!(writer.Close(), None);

    let encoded = view.data.borrow();
    assert_eq!(encoded.len(), payload.len() + 8);
    let first_crc = u32::from_le_bytes(encoded[..4].try_into().unwrap());
    assert_eq!(first_crc, crc32fast::hash(&payload[..checksumPayloadSize]));
    let second = 4 + checksumPayloadSize;
    let second_crc = u32::from_le_bytes(encoded[second..second + 4].try_into().unwrap());
    assert_eq!(second_crc, crc32fast::hash(&payload[checksumPayloadSize..]));
}

/// 跨块中间读与尾部 EOF 语义应与 Go 一致。
#[test]
fn reads_across_blocks_and_preserves_go_eof_semantics() {
    let file = MemoryFile::default();
    let view = file.clone();
    let input = b"0123456789".repeat(250);
    let mut writer = NewWriter(file);
    assert_eq!(writer.Write(&input), (input.len(), None));
    assert_eq!(writer.Close(), None);

    let reader = NewReader(view);
    let mut middle = vec![0; 1500];
    assert_eq!(reader.ReadAt(&mut middle, 505), (1500, None));
    assert_eq!(middle, input[505..2005]);

    let mut tail = vec![0; 600];
    assert_eq!(
        reader.ReadAt(&mut tail, 2200),
        (300, Some("EOF".to_owned()))
    );
    assert_eq!(&tail[..300], &input[2200..]);
    assert!(tail[300..].iter().all(|byte| *byte == 0));
}

/// 篡改编码字节后 ReadAt 应返回 `errChecksumFail`。
#[test]
fn detects_payload_corruption() {
    let file = MemoryFile::default();
    let view = file.clone();
    let mut writer = NewWriter(file);
    assert_eq!(writer.Write(&vec![7; 1500]), (1500, None));
    assert_eq!(writer.Close(), None);
    view.data.borrow_mut()[100] ^= 1;

    let mut output = vec![0; 10];
    assert_eq!(
        NewReader(view).ReadAt(&mut output, 96),
        (0, Some(errChecksumFail.to_owned()))
    );
}

/// 故意少写若干字节以触发 short write。
struct ShortWriter {
    calls: usize,
}

impl WriteCloserDraft for ShortWriter {
    fn Write(&mut self, p: &[u8]) -> (usize, Option<String>) {
        self.calls += 1;
        (p.len() - 5, None)
    }

    fn Close(&mut self) -> Option<String> {
        panic!("Close must not run after Flush fails")
    }
}

/// Flush 遇到 short write 后应锁存错误，且 Close 不再触达底层。
#[test]
fn latches_short_write_and_does_not_close_underlying_writer() {
    let mut writer = NewWriter(ShortWriter { calls: 0 });
    assert_eq!(writer.Write(&[1; 20]), (20, None));
    assert_eq!(writer.Flush(), Some("short write".to_owned()));
    assert_eq!(writer.Flush(), Some("short write".to_owned()));
    assert_eq!(writer.Close(), Some("short write".to_owned()));
}

/// 空切片读取不应访问底层源。
#[test]
fn empty_reads_do_not_touch_the_source() {
    let reader = NewReader(MemoryFile::default());
    assert_eq!(reader.ReadAt(&mut [], 42), (0, None));
}
