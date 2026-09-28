// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// SST 数据文件的 KV 编码与内存写入器。
//
// 线格式为两个大端 uint64 长度头后接 key/value 正文；`KeyValueStore` 在写入时
// 同步累计 RangeProperty（范围属性）。调用方须保证 key 严格递增。

// 当前不会写对象存储、关闭真实文件或执行业务动作；Context、ObjectWriter 和 collector 等待跨文件接线。
//
/// 统计文件和重复键文件使用与 Go 相同的文件名后缀。
// const statSuffix: &str = "_stat";
// const dupSuffix: &str = "_dup";
//
/// LengthBytes 表示 key/value 长度头的宽度；线格式使用大端 uint64。
// pub const LengthBytes: usize = 8;
//
/// KeyValueStore 对应 Go 的同名结构，把有序 KV 写入数据文件并同步累计范围属性。
/// 调用方必须保证传入 key 严格递增，collector 才能正确记录 FirstKey 与 LastKey。
// pub struct KeyValueStore {
//     dataWriter: ObjectWriter,
//     rc: Option<RangePropertiesCollector>,
//     ctx: Context,
//     offset: u64,
// }
// */
use crate::writer::RangePropertiesCollector;
use crate::{Error, Result};

/// 统计文件名后缀，对应 Go 的 `_stat`。
pub const STAT_SUFFIX: &str = "_stat";
/// 重复键冲突文件名后缀，对应 Go 的 `_dup`。
pub const DUP_SUFFIX: &str = "_dup";
/// key/value 长度头的字节宽度；线格式使用大端 uint64。
pub const LengthBytes: usize = 8;

/// 将原始 key/value 编码为 `<keyLen><valueLen><key><value>` 并返回新缓冲。
pub fn encode_kv(key: &[u8], value: &[u8]) -> Result<Vec<u8>> {
    let length = (LengthBytes * 2)
        .checked_add(key.len())
        .and_then(|n| n.checked_add(value.len()))
        .ok_or_else(|| Error::InvalidData("encoded key/value length overflow".into()))?;
    let mut output = vec![0; length];
    encode_to_buf(&mut output, key, value)?;
    Ok(output)
}

/// 写入两个大端 uint64 长度头及 key/value 正文；目标切片长度必须精确匹配。
pub fn encode_to_buf(buf: &mut [u8], key: &[u8], value: &[u8]) -> Result<()> {
    if buf.len() != LengthBytes * 2 + key.len() + value.len() {
        return Err(Error::InvalidData(
            "incorrect destination size for key/value".into(),
        ));
    }
    buf[..LengthBytes].copy_from_slice(&(key.len() as u64).to_be_bytes());
    buf[LengthBytes..LengthBytes * 2].copy_from_slice(&(value.len() as u64).to_be_bytes());
    buf[LengthBytes * 2..LengthBytes * 2 + key.len()].copy_from_slice(key);
    buf[LengthBytes * 2 + key.len()..].copy_from_slice(value);
    Ok(())
}

/// 把有序 KV 写入内存缓冲并同步累计范围属性（对应 Go KeyValueStore）。
#[derive(Clone, Debug)]
pub struct KeyValueStore {
    /// 已编码的数据文件内容。
    data: Vec<u8>,
    /// 可选的范围属性收集器。
    collector: Option<RangePropertiesCollector>,
    /// 已成功写入的编码字节总数。
    offset: u64,
}

impl KeyValueStore {
    /// 构造空写入器；初始偏移为 0。
    pub fn new(collector: Option<RangePropertiesCollector>) -> Self {
        Self {
            data: Vec::new(),
            collector,
            offset: 0,
        }
    }

    /// 校验并追加一条已编码记录，成功后更新偏移并通知 collector。
    pub fn add_encoded_data(&mut self, data: &[u8]) -> Result<()> {
        if data.len() < LengthBytes * 2 {
            return Err(Error::InvalidData("truncated encoded key/value".into()));
        }
        let key_len = u64::from_be_bytes(data[..8].try_into().unwrap()) as usize;
        let value_len = u64::from_be_bytes(data[8..16].try_into().unwrap()) as usize;
        let expected = 16usize
            .checked_add(key_len)
            .and_then(|n| n.checked_add(value_len))
            .ok_or_else(|| Error::InvalidData("encoded key/value length overflow".into()))?;
        if expected != data.len() {
            return Err(Error::InvalidData(
                "encoded key/value length mismatch".into(),
            ));
        }
        self.data.extend_from_slice(data);
        // offset 是本条写完后的文件尾；collector 用它作为下一范围起点。
        self.offset = self
            .offset
            .checked_add(data.len() as u64)
            .ok_or_else(|| Error::InvalidData("file offset overflow".into()))?;
        if let Some(collector) = self.collector.as_mut() {
            collector.on_next_encoded_data(data, self.offset)?;
        }
        Ok(())
    }

    /// 编码原始 key/value 后复用 `add_encoded_data` 写入。
    pub fn add_raw_kv(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.add_encoded_data(&encode_kv(key, value)?)
    }
    /// 返回已成功写入的编码字节总数。
    pub fn offset(&self) -> u64 {
        self.offset
    }
    /// 收尾：刷新未满配额的最后一条范围属性。
    ///
    /// 与 Go 一样，这不会关闭底层写入器，也不会使 store 失效。
    pub fn finish(&mut self) {
        if let Some(c) = self.collector.as_mut() {
            c.on_file_end();
        }
    }
    /// 只读访问已编码数据缓冲。
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    /// 只读访问范围属性收集器。
    pub fn collector(&self) -> Option<&RangePropertiesCollector> {
        self.collector.as_ref()
    }
    /// 先 finish，再拆出数据缓冲与收集器。
    pub fn into_parts(mut self) -> (Vec<u8>, Option<RangePropertiesCollector>) {
        self.finish();
        (self.data, self.collector)
    }

    /// Go 风格别名：`add_raw_kv`。
    pub fn AddRawKV(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.add_raw_kv(key, value)
    }
    /// Go 风格别名：`offset`。
    pub fn Offset(&self) -> u64 {
        self.offset()
    }
    /// Go 风格别名：`finish`。
    pub fn Finish(&mut self) {
        self.finish()
    }
}

/// 对应 Go 构造函数，委托 `KeyValueStore::new`。
pub fn NewKeyValueStore(collector: Option<RangePropertiesCollector>) -> KeyValueStore {
    KeyValueStore::new(collector)
}
/*

/// NewKeyValueStore 对应 Go 构造函数；初始偏移为 0，writer 和 collector 的生命周期由上层管理。
pub fn NewKeyValueStore(
    ctx: &Context,
    dataWriter: ObjectWriter,
    rangePropertiesCollector: Option<RangePropertiesCollector>,
) -> KeyValueStore {
    KeyValueStore {
        dataWriter,
        rc: rangePropertiesCollector,
        ctx: ctx.clone(),
        offset: 0,
    }
}

impl KeyValueStore {
    /// addEncodedData 写入 `<keyLen><valueLen><key><value>`，成功后更新偏移与范围属性。
    fn addEncodedData(&mut self, data: &[u8]) -> Result<(), Error> {
        // Go 忽略 Write 返回的字节数，但错误时绝不推进 offset，也不通知 collector。
        self.dataWriter.Write(&self.ctx, data)?;
        self.offset += data.len() as u64;

        if let Some(collector) = self.rc.as_mut() {
            // offset 是本条数据写完后的文件尾位置，collector 用它作为下一范围的起点。
            collector.onNextEncodedData(data, self.offset);
        }
        Ok(())
    }

    /// Offset 对应 Go 的只读访问器，返回已经成功写入的编码字节总数。
    pub fn Offset(&self) -> u64 {
        self.offset
    }

    /// AddRawKV 把原始 key/value 编码为两个长度头和正文，再复用 addEncodedData 写入。
    pub fn AddRawKV(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        let length = key.len() + value.len() + LengthBytes * 2;
        let mut buf = vec![0; length];

        // encodeToBuf 写入两个大端 uint64；一次性分配确保传给 writer 的切片长度精确等于记录长度。
        encodeToBuf(&mut buf, key, value);
        self.addEncodedData(&buf)
    }

    /// Finish 对应 Go 的收尾钩子，把尚未达到阈值的最后范围属性追加到结果。
    /// 原实现并未在这里关闭 dataWriter；对象存储 writer 仍由创建者负责关闭。
    pub fn Finish(&mut self) {
        if let Some(collector) = self.rc.as_mut() {
            collector.onFileEnd();
        }
    }
}
*/
