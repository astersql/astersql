// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! 流备份 KV 事件缓冲的编解码与顺序迭代。
//! 对应 Go `br/pkg/stream/decode_kv.go`：条目布局为
//! `u32le(key_len) | key | u32le(value_len) | value`，与 TiKV 日志落盘格式对齐。
//! `EventIterator` 在缓冲上前进；解码失败时把错误挂到迭代器，后续 `Valid` 为假。

/// 顺序读取 KV 事件的迭代接口，语义对齐 Go `stream.Iterator`。
pub trait Iterator {
    fn Next(&mut self);
    fn Valid(&mut self) -> bool;
    fn Key(&self) -> &[u8];
    fn Value(&self) -> &[u8];
    fn GetError(&self) -> Option<&str>;
}

/// 基于整块 buffer 的事件迭代器；`pos` 用 u32 与 Go 一致，避免与长度比较时截断。
pub struct EventIterator {
    buff: Vec<u8>,
    pos: u32,
    k: Vec<u8>,
    v: Vec<u8>,
    err: Option<String>,
}

/// 构造迭代器；首条记录需调用方先 `Next` 再读 `Key`/`Value`（与 Go 相同）。
pub fn NewEventIterator(buff: Vec<u8>) -> EventIterator {
    EventIterator {
        buff,
        pos: 0,
        k: Vec::new(),
        v: Vec::new(),
        err: None,
    }
}

impl Iterator for EventIterator {
    fn Next(&mut self) {
        // 已无效时不再推进，保留既有错误或结束位置。
        if !self.Valid() {
            return;
        }
        match DecodeKVEntry(&self.buff[self.pos as usize..]) {
            Ok((k, v, pos)) => {
                self.k = k;
                self.v = v;
                self.pos += pos;
            }
            Err(e) => {
                // 解码失败后 `Valid` 恒为 false，供调用方 `GetError` 取因。
                self.err = Some(e);
            }
        }
    }

    fn Valid(&mut self) -> bool {
        if self.err.is_some() {
            return false;
        }
        let buffLen = self.buff.len();
        // 缓冲超过 u32 上限时拒绝，防止与 `pos` 比较时静默截断（对齐 Go MaxUint32 检查）。
        if buffLen > u32::MAX as usize {
            self.err = Some(format!(
                "buffer too large: {buffLen} bytes exceeds uint32 limit ({} bytes)",
                u32::MAX
            ));
            return false;
        }
        self.pos < buffLen as u32
    }

    /// 最近一次成功 `Next` 解码出的 key；失败或未推进前可能为空切片。
    fn Key(&self) -> &[u8] {
        &self.k
    }

    /// 与 `Key` 成对的 value 视图，生命周期绑定迭代器内部缓冲副本。
    fn Value(&self) -> &[u8] {
        &self.v
    }

    /// 迭代期间累积的解码/长度错误；无错误时为 `None`。
    fn GetError(&self) -> Option<&str> {
        self.err.as_deref()
    }
}

/// 按小端长度前缀编码单条 KV；容量预分配 8+len(k)+len(v)。
pub fn EncodeKVEntry(k: &[u8], v: &[u8]) -> Vec<u8> {
    let mut entry = Vec::with_capacity(4 + k.len() + 4 + v.len());
    entry.extend_from_slice(&(k.len() as u32).to_le_bytes());
    entry.extend_from_slice(k);
    entry.extend_from_slice(&(v.len() as u32).to_le_bytes());
    entry.extend_from_slice(v);
    entry
}

/// 解码一条 KV；成功时第三元为该条目占用字节数，供迭代器累加 `pos`。
/// 长度不足或声明长度越界时返回 `"invalid buff"`，与 Go 错误文案一致。
pub fn DecodeKVEntry(buff: &[u8]) -> Result<(Vec<u8>, Vec<u8>, u32), String> {
    // 至少需要两个 u32 长度头。
    if buff.len() < 8 {
        return Err("invalid buff".into());
    }
    let mut pos: u32 = 0;
    let kLen = u32::from_le_bytes(buff[0..4].try_into().unwrap());
    pos += 4;
    // key 体 + value 长度头共需 8+kLen 字节。
    if (buff.len() as u32) < 8 + kLen {
        return Err("invalid buff".into());
    }
    let k = buff[pos as usize..(pos + kLen) as usize].to_vec();
    pos += kLen;
    let vLen = u32::from_le_bytes(buff[pos as usize..pos as usize + 4].try_into().unwrap());
    pos += 4;
    // value 体不得越过缓冲末尾。
    if (buff.len() as u32) < 8 + kLen + vLen {
        return Err("invalid buff".into());
    }
    let v = buff[pos as usize..(pos + vLen) as usize].to_vec();
    pos += vLen;
    Ok((k, v, pos))
}
