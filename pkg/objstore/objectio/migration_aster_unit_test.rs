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

// 缓冲写入器（BufferedWriter）单元测试。
//
// 用内存 Sink 校验：明文分块与 Go 一致、满容量等到 close 再上传、
// 上传失败不关闭底层、Gzip 路径真实压缩并在 close 刷尾。

use std::io::{self, Read};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::recording::AccessStats;
use crate::{CompressType, Context, Writer, new_buffered_writer};

/// Sink 内部状态：已上传块、是否已关闭、是否模拟写失败。
#[derive(Default)]
struct SinkState {
    chunks: Vec<Vec<u8>>,
    closed: bool,
    fail_write: bool,
    fail_close: bool,
    write_contexts_cancelled: Vec<bool>,
    close_contexts_cancelled: Vec<bool>,
}

/// 将每次 write 的数据原样记入 `SinkState` 的测试用 Writer。
struct Sink(Arc<Mutex<SinkState>>);

impl Writer for Sink {
    fn write(&mut self, ctx: &Context, data: &[u8]) -> io::Result<usize> {
        let mut state = self.0.lock().unwrap();
        state.write_contexts_cancelled.push(ctx.is_cancelled());
        if state.fail_write {
            return Err(io::Error::other("upload failed"));
        }
        state.chunks.push(data.to_vec());
        Ok(data.len())
    }

    fn close(&mut self, ctx: &Context) -> io::Result<()> {
        let mut state = self.0.lock().unwrap();
        state.close_contexts_cancelled.push(ctx.is_cancelled());
        if state.fail_close {
            return Err(io::Error::other("close failed"));
        }
        state.closed = true;
        Ok(())
    }
}

/// 拼接已上传块为完整字节序列。
fn joined(state: &Arc<Mutex<SinkState>>) -> Vec<u8> {
    state.lock().unwrap().chunks.concat()
}

/// 明文分块：超容量时先上传满块；close 后数据完整且统计 accepted 字节。
#[test]
fn plain_writer_matches_go_chunking_and_records_accepted_bytes() {
    let state = Arc::new(Mutex::new(SinkState::default()));
    let stats = Arc::new(AccessStats::default());
    let mut writer = new_buffered_writer(
        Box::new(Sink(state.clone())),
        4,
        CompressType::NoCompression,
        Some(stats.clone()),
    );
    let ctx = Context::default();

    assert_eq!(writer.write(&ctx, b"abc").unwrap(), 3);
    assert_eq!(writer.write(&ctx, b"defgh").unwrap(), 5);
    assert_eq!(state.lock().unwrap().chunks, vec![b"abcd".to_vec()]);
    writer.close(&ctx).unwrap();

    assert_eq!(joined(&state), b"abcdefgh");
    assert!(state.lock().unwrap().closed);
    assert_eq!(stats.traffic.write.load(Ordering::Relaxed), 8);
}

/// 恰好填满容量时不立即上传，与 Go 一样等到 close 再刷出。
#[test]
fn exact_capacity_waits_until_close_like_go() {
    let state = Arc::new(Mutex::new(SinkState::default()));
    let mut writer = new_buffered_writer(
        Box::new(Sink(state.clone())),
        4,
        CompressType::NoCompression,
        None,
    );
    let ctx = Context::default();

    assert_eq!(writer.write(&ctx, b"1234").unwrap(), 4);
    assert!(state.lock().unwrap().chunks.is_empty());
    writer.close(&ctx).unwrap();
    assert_eq!(state.lock().unwrap().chunks, vec![b"1234".to_vec()]);
}

/// 上传失败向上返回错误，且不调用底层 close。
#[test]
fn upload_failure_is_returned_and_does_not_close_sink() {
    let state = Arc::new(Mutex::new(SinkState {
        fail_write: true,
        ..SinkState::default()
    }));
    let mut writer = new_buffered_writer(
        Box::new(Sink(state.clone())),
        2,
        CompressType::NoCompression,
        None,
    );
    let ctx = Context::default();

    let error = writer.write(&ctx, b"abc").unwrap_err();
    assert_eq!(error.to_string(), "upload failed");
    assert!(!state.lock().unwrap().closed);
}

/// 尾块上传成功后，底层 close 错误必须原样返回。
#[test]
fn underlying_close_failure_is_returned_after_tail_upload() {
    let state = Arc::new(Mutex::new(SinkState {
        fail_close: true,
        ..SinkState::default()
    }));
    let mut writer = new_buffered_writer(
        Box::new(Sink(state.clone())),
        8,
        CompressType::NoCompression,
        None,
    );
    let ctx = Context::default();

    assert_eq!(writer.write(&ctx, b"tail").unwrap(), 4);
    let error = writer.close(&ctx).unwrap_err();

    assert_eq!(error.to_string(), "close failed");
    let state = state.lock().unwrap();
    assert_eq!(state.chunks, vec![b"tail".to_vec()]);
    assert!(!state.closed);
}

/// BufferedWriter 本身不消费 context；即使已取消，仅缓冲写也应成功。
#[test]
fn cancelled_context_is_forwarded_instead_of_rejected_while_buffering() {
    let state = Arc::new(Mutex::new(SinkState::default()));
    let mut writer = new_buffered_writer(
        Box::new(Sink(state.clone())),
        8,
        CompressType::NoCompression,
        None,
    );
    let ctx = Context::default();
    ctx.cancel();

    assert_eq!(writer.write(&ctx, b"data").unwrap(), 4);
    {
        let state = state.lock().unwrap();
        assert!(state.chunks.is_empty());
        assert!(state.write_contexts_cancelled.is_empty());
    }

    writer.close(&ctx).unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.write_contexts_cancelled, vec![true]);
    assert_eq!(state.close_contexts_cancelled, vec![true]);
}

/// 空缓冲 close 仍应把已取消的 context 交给底层 Writer，由底层决定如何处理。
#[test]
fn cancelled_context_does_not_skip_underlying_close_for_empty_buffer() {
    let state = Arc::new(Mutex::new(SinkState::default()));
    let mut writer = new_buffered_writer(
        Box::new(Sink(state.clone())),
        8,
        CompressType::NoCompression,
        None,
    );
    let ctx = Context::default();
    ctx.cancel();

    writer.close(&ctx).unwrap();
    let state = state.lock().unwrap();
    assert!(state.closed);
    assert!(state.write_contexts_cancelled.is_empty());
    assert_eq!(state.close_contexts_cancelled, vec![true]);
}

/// Gzip：写入后可用 GzDecoder 还原；close 会刷出压缩尾部。
#[test]
fn gzip_path_uses_real_compression_and_flushes_tail_on_close() {
    let state = Arc::new(Mutex::new(SinkState::default()));
    let mut writer =
        new_buffered_writer(Box::new(Sink(state.clone())), 8, CompressType::Gzip, None);
    let ctx = Context::default();
    let input = b"hello worldhello worldhello world";

    assert_eq!(writer.write(&ctx, input).unwrap(), input.len());
    writer.close(&ctx).unwrap();

    let compressed = joined(&state);
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, input);
}
