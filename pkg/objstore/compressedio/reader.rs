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

// 按压缩类型构造解压 Reader：Gzip / Snappy 同步，Zstd 可同步或异步解码。

use std::io::{self, Read};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use flate2::read::MultiGzDecoder;
use snap::read::FrameDecoder;
use zstd::stream::read::Decoder as ZstdDecoder;

use super::{CompressType, DecompressConfig};

/// 可跨线程传递的动态 Reader（Zstd 异步解码会把源迁到工作线程）。
type SendReader = Box<dyn Read + Send>;

/// 惰性 Zstd 解码器：首次 `read` 时才创建底层 Decoder，避免空流开销。
struct LazyZstdReader {
    source: Option<SendReader>,
    decoder: Option<SendReader>,
}

impl LazyZstdReader {
    /// 用尚未消费的压缩源构造惰性包装。
    fn new(source: SendReader) -> Self {
        Self {
            source: Some(source),
            decoder: None,
        }
    }
}

impl Read for LazyZstdReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        // 首次读取时消费 source 并初始化解码器。
        if self.decoder.is_none() {
            let source = self.source.take().ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "zstd source is unavailable")
            })?;
            self.decoder = Some(Box::new(ZstdDecoder::new(source)?));
        }
        self.decoder
            .as_mut()
            .expect("zstd decoder initialized")
            .read(output)
    }
}

/// 异步解码线程经通道回传的消息：数据块、错误或结束。
enum DecodeMessage {
    Data(Vec<u8>),
    Error(io::Error),
    Eof,
}

/// 后台线程解码 Zstd，主线程经有界通道拉取已解压块（并发度 > 1 时使用）。
struct AsyncZstdReader {
    receiver: Receiver<DecodeMessage>,
    current: io::Cursor<Vec<u8>>,
    finished: bool,
}

impl AsyncZstdReader {
    /// 启动解码线程并返回包装 Reader。
    fn new(source: SendReader) -> Self {
        let (sender, receiver) = sync_channel(1);
        std::thread::spawn(move || decode_zstd(source, sender));
        Self {
            receiver,
            current: io::Cursor::new(Vec::new()),
            finished: false,
        }
    }
}

impl Read for AsyncZstdReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        loop {
            // 先从当前缓冲块读；耗尽后再向通道要下一块。
            let read = self.current.read(output)?;
            if read != 0 || output.is_empty() {
                return Ok(read);
            }
            if self.finished {
                return Ok(0);
            }
            match self.receiver.recv() {
                Ok(DecodeMessage::Data(data)) => self.current = io::Cursor::new(data),
                Ok(DecodeMessage::Error(error)) => {
                    self.finished = true;
                    return Err(error);
                }
                Ok(DecodeMessage::Eof) | Err(_) => {
                    self.finished = true;
                    return Ok(0);
                }
            }
        }
    }
}

/// 工作线程：循环读 Zstd 解码器并向发送端推送数据 / 错误 / EOF。
fn decode_zstd(source: SendReader, sender: SyncSender<DecodeMessage>) {
    let mut decoder = match ZstdDecoder::new(source) {
        Ok(decoder) => decoder,
        Err(error) => {
            let _ = sender.send(DecodeMessage::Error(error));
            return;
        }
    };
    let mut buffer = vec![0; 32 * 1024];
    loop {
        match decoder.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(DecodeMessage::Eof);
                return;
            }
            Ok(read) => {
                // 对端已关闭则停止，避免无意义地继续解码。
                if sender
                    .send(DecodeMessage::Data(buffer[..read].to_vec()))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(DecodeMessage::Error(error));
                return;
            }
        }
    }
}

/// 按压缩类型包装底层 Reader；无压缩返回 `Ok(None)`（对齐 Go 的 nil）。
pub fn new_reader(
    compress_type: CompressType,
    cfg: DecompressConfig,
    reader: SendReader,
) -> io::Result<Option<SendReader>> {
    match compress_type {
        CompressType::Gzip => {
            // Go gzip.Reader enables multistream mode by default.
            let decoder = MultiGzDecoder::new(reader);
            if decoder.header().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid gzip header",
                ));
            }
            Ok(Some(Box::new(decoder)))
        }
        CompressType::Snappy => Ok(Some(Box::new(FrameDecoder::new(reader)))),
        CompressType::Zstd => {
            // 并发度为 1 时同步惰性解码；否则异步后台解码。
            if cfg.zstd_decode_concurrency == 1 {
                Ok(Some(Box::new(LazyZstdReader::new(reader))))
            } else {
                Ok(Some(Box::new(AsyncZstdReader::new(reader))))
            }
        }
        CompressType::NoCompression => Ok(None),
    }
}
