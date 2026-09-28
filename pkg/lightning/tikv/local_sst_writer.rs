// Copyright 2024 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// 本地 SST（Sorted String Table）写入器：按 TiKV write CF 编码键值并落盘。
//
// write CF 存的是短值写记录（Put + 版本时间戳）；键经 memcomparable 编码并附
// MVCC 时间戳后缀。关闭时附带 Mvcc/Range 属性，便于后续 Ingest。

use crate::{InternalKey, MvccPropCollector, RangePropertiesCollector, TikvError};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

const BLOCK_TRAILER_LEN: usize = 5;
const DATA_BLOCK_SIZE: usize = 32 * 1024;
const ROCKSDB_FOOTER_LEN: usize = 53;
const ROCKSDB_SST_MAGIC: &[u8; 8] = b"\xf7\xcf\xf4\x85\xb7\x41\xe2\x88";
const ROCKSDB_COMPRESSION_OPTIONS: &str = "window_bits=-14; level=32767; strategy=0; max_dict_bytes=0; zstd_max_train_bytes=0; enabled=0; ";

/// write CF 的有序 KV 缓冲写入器；`close` 时落盘为本地 SST。
pub struct WriteCFWriter {
    /// 目标 SST 文件路径。
    path: PathBuf,
    /// 写入所用的统一版本时间戳（commit ts）。
    ts: u64,
    /// 已编码的 (key, value) 记录，必须严格递增。
    records: Vec<(Vec<u8>, Vec<u8>)>,
}

/// 按路径与时间戳创建 `WriteCFWriter`（Go 风格命名别名）。
pub fn newWriteCFWriter(path: impl AsRef<Path>, ts: u64) -> Result<WriteCFWriter, TikvError> {
    WriteCFWriter::new(path, ts)
}

impl WriteCFWriter {
    /// 创建空文件并初始化写入器。
    pub fn new(path: impl AsRef<Path>, ts: u64) -> Result<Self, TikvError> {
        let path = path.as_ref().to_path_buf();
        File::create(&path)?;
        Ok(Self {
            path,
            ts,
            records: Vec::new(),
        })
    }

    /// 追加一条用户键值：校验短值、编码 MVCC 键与 write 值，并保证键严格递增。
    pub fn set(&mut self, key: &[u8], value: &[u8]) -> Result<(), TikvError> {
        assert!(
            isShortValue(value),
            "not implemented, need to write to default CF"
        );
        let actual_key = encode_mvcc_key(key, self.ts);
        let actual_value = encode_write_value(value, self.ts);
        // SST 要求键有序；乱序会破坏后续 Ingest 与属性索引。
        if self
            .records
            .last()
            .is_some_and(|(last, _)| last.as_slice() >= actual_key.as_slice())
        {
            return Err(TikvError::InvalidArgument(
                "SST keys must be strictly increasing".into(),
            ));
        }
        self.records.push((actual_key, actual_value));
        Ok(())
    }

    /// Go 风格别名：`set`。
    pub fn Set(&mut self, key: &[u8], value: &[u8]) -> Result<(), TikvError> {
        self.set(key, value)
    }

    /// 收集属性并写出完整本地 SST 文件。
    pub fn close(self) -> Result<(), TikvError> {
        write_sst(&self.path, self.ts, &self.records)
    }

    /// Go 风格别名：`close`。
    pub fn Close(self) -> Result<(), TikvError> {
        self.close()
    }
}

/// 判断 value 是否可放入 write CF 短值（长度 ≤ 255）。
pub fn isShortValue(value: &[u8]) -> bool {
    value.len() <= u8::MAX as usize
}

/// `isShortValue` 的 snake_case 别名。
pub fn is_short_value(value: &[u8]) -> bool {
    isShortValue(value)
}

/// 将用户键编码为 TiKV MVCC 键：`z` + memcomparable(key) + !ts（大端）。
///
/// 取反时间戳使同一用户键下较新版本在字典序上更靠前。
pub fn encode_mvcc_key(key: &[u8], ts: u64) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(1 + key.len() + key.len() / 8 + 18);
    encoded.push(b'z');
    encode_memcomparable(&mut encoded, key);
    encoded.extend_from_slice(&(!ts).to_be_bytes());
    encoded
}

/// memcomparable 编码：每 8 字节一组后跟 0xff，末组补零并写剩余标记。
fn encode_memcomparable(output: &mut Vec<u8>, input: &[u8]) {
    let mut chunks = input.chunks_exact(8);
    for chunk in &mut chunks {
        output.extend_from_slice(chunk);
        output.push(0xff);
    }
    let remainder = chunks.remainder();
    output.extend_from_slice(remainder);
    let padding = 8 - remainder.len();
    output.resize(output.len() + padding, 0);
    output.push(0xff - padding as u8);
}

/// 编码 write CF 短值：`P` + uvarint(ts) + `v` + len + payload。
pub fn encode_write_value(value: &[u8], ts: u64) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(value.len() + 13);
    encoded.push(b'P');
    append_uvarint(&mut encoded, ts);
    encoded.push(b'v');
    encoded.push(value.len() as u8);
    encoded.extend_from_slice(value);
    encoded
}

/// 追加无符号变长整数（LEB128 风格，高位 continuation）。
fn append_uvarint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push(value as u8 | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

#[derive(Clone, Copy, Debug)]
struct BlockHandle {
    offset: u64,
    length: u64,
}

/// RocksDB block：前缀压缩的 KV 条目加 restart 数组。
struct BlockBuilder {
    data: Vec<u8>,
    previous_key: Vec<u8>,
    restarts: Vec<u32>,
    entries: usize,
    restart_interval: usize,
}

impl BlockBuilder {
    fn new(restart_interval: usize) -> Self {
        Self {
            data: Vec::new(),
            previous_key: Vec::new(),
            restarts: Vec::new(),
            entries: 0,
            restart_interval,
        }
    }

    fn is_empty(&self) -> bool {
        self.entries == 0
    }

    fn estimated_size_with(&self, key: &[u8], value: &[u8]) -> usize {
        self.data.len() + 3 * 10 + key.len() + value.len() + (self.restarts.len() + 2) * 4
    }

    fn add(&mut self, key: &[u8], value: &[u8]) {
        let shared = if self.entries % self.restart_interval == 0 {
            self.restarts.push(self.data.len() as u32);
            0
        } else {
            shared_prefix_len(&self.previous_key, key)
        };
        append_uvarint(&mut self.data, shared as u64);
        append_uvarint(&mut self.data, (key.len() - shared) as u64);
        append_uvarint(&mut self.data, value.len() as u64);
        self.data.extend_from_slice(&key[shared..]);
        self.data.extend_from_slice(value);
        self.previous_key.clear();
        self.previous_key.extend_from_slice(key);
        self.entries += 1;
    }

    fn finish(mut self) -> Vec<u8> {
        if self.restarts.is_empty() {
            self.restarts.push(0);
        }
        for restart in &self.restarts {
            self.data.extend_from_slice(&restart.to_le_bytes());
        }
        self.data
            .extend_from_slice(&(self.restarts.len() as u32).to_le_bytes());
        self.data
    }
}

fn shared_prefix_len(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
}

fn encode_internal_key(user_key: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(user_key.len() + 8);
    key.extend_from_slice(user_key);
    // Pebble Writer.Set 使用 sequence number 0、InternalKeyKindSet(1)。
    key.extend_from_slice(&1_u64.to_le_bytes());
    key
}

fn encode_block_handle(handle: BlockHandle) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(20);
    append_uvarint(&mut encoded, handle.offset);
    append_uvarint(&mut encoded, handle.length);
    encoded
}

/// 写出标准 RocksDB v2 BlockBasedTable。
fn write_sst(path: &Path, ts: u64, records: &[(Vec<u8>, Vec<u8>)]) -> Result<(), TikvError> {
    let mut mvcc = MvccPropCollector::new(ts);
    let mut ranges = RangePropertiesCollector::new();
    for (key, value) in records {
        let key = InternalKey::new(key.clone());
        mvcc.Add(&key, value)?;
        ranges.Add(&key, value)?;
    }
    let mut properties = HashMap::new();
    mvcc.Finish(&mut properties)?;
    ranges.Finish(&mut properties)?;

    let mut file = File::create(path)?;
    let mut data_blocks = Vec::new();
    let mut data_block = BlockBuilder::new(16);
    let mut last_key = Vec::new();
    for (key, value) in records {
        let internal_key = encode_internal_key(key);
        if !data_block.is_empty()
            && data_block.estimated_size_with(&internal_key, value) > DATA_BLOCK_SIZE
        {
            let handle = write_block(&mut file, &data_block.finish())?;
            data_blocks.push((last_key, handle));
            data_block = BlockBuilder::new(16);
        }
        data_block.add(&internal_key, value);
        last_key = internal_key;
    }
    if !data_block.is_empty() || data_blocks.is_empty() {
        let handle = write_block(&mut file, &data_block.finish())?;
        if last_key.is_empty() {
            // Pebble 的空表索引使用空 user key 加 8 字节 internal trailer。
            last_key.resize(8, 0);
        }
        data_blocks.push((last_key, handle));
    }
    let data_size = file.stream_position()?;

    let filter = build_bloom_filter(records.iter().map(|(key, _)| key.as_slice()));
    let filter_handle = write_block(&mut file, &filter)?;

    let mut index = BlockBuilder::new(1);
    for (last_key, handle) in &data_blocks {
        index.add(last_key, &encode_block_handle(*handle));
    }
    let index_bytes = index.finish();
    let index_size = index_bytes.len() as u64 + BLOCK_TRAILER_LEN as u64;
    let index_handle = write_block(&mut file, &index_bytes)?;

    insert_standard_properties(
        &mut properties,
        records,
        data_blocks.len() as u64,
        data_size,
        filter_handle.length,
        index_size,
    );
    let mut property_block = BlockBuilder::new(usize::MAX);
    let mut sorted_properties = properties.iter().collect::<Vec<_>>();
    sorted_properties.sort_by(|left, right| left.0.cmp(right.0));
    for (name, value) in sorted_properties {
        property_block.add(name.as_bytes(), value);
    }
    let property_handle = write_block(&mut file, &property_block.finish())?;

    let mut metaindex = BlockBuilder::new(1);
    metaindex.add(
        b"fullfilter.rocksdb.BuiltinBloomFilter",
        &encode_block_handle(filter_handle),
    );
    metaindex.add(b"rocksdb.properties", &encode_block_handle(property_handle));
    let metaindex_handle = write_block(&mut file, &metaindex.finish())?;
    write_footer(&mut file, metaindex_handle, index_handle)?;
    file.sync_all()?;
    Ok(())
}

fn insert_standard_properties(
    properties: &mut HashMap<String, Vec<u8>>,
    records: &[(Vec<u8>, Vec<u8>)],
    data_blocks: u64,
    data_size: u64,
    filter_size: u64,
    index_size: u64,
) {
    let raw_key_size = records.iter().map(|(key, _)| key.len() as u64 + 8).sum();
    let raw_value_size = records.iter().map(|(_, value)| value.len() as u64).sum();
    let mut uvarint = |name: &str, value: u64| {
        let mut encoded = Vec::new();
        append_uvarint(&mut encoded, value);
        properties.insert(name.into(), encoded);
    };
    uvarint("rocksdb.data.size", data_size);
    uvarint("rocksdb.filter.size", filter_size);
    uvarint("rocksdb.index.size", index_size);
    uvarint("rocksdb.num.data.blocks", data_blocks);
    uvarint("rocksdb.num.entries", records.len() as u64);
    uvarint("rocksdb.deleted.keys", 0);
    uvarint("rocksdb.merge.operands", 0);
    uvarint("rocksdb.num.range-deletions", 0);
    uvarint("rocksdb.raw.key.size", raw_key_size);
    uvarint("rocksdb.raw.value.size", raw_value_size);
    properties.insert(
        "rocksdb.block.based.table.index.type".into(),
        0_u32.to_le_bytes().to_vec(),
    );
    properties.insert(
        "rocksdb.comparator".into(),
        b"leveldb.BytewiseComparator".to_vec(),
    );
    properties.insert("rocksdb.compression".into(), b"ZSTD".to_vec());
    properties.insert(
        "rocksdb.compression_options".into(),
        ROCKSDB_COMPRESSION_OPTIONS.as_bytes().to_vec(),
    );
    properties.insert(
        "rocksdb.external_sst_file.version".into(),
        2_u32.to_le_bytes().to_vec(),
    );
    properties.insert(
        "rocksdb.external_sst_file.global_seqno".into(),
        0_u64.to_le_bytes().to_vec(),
    );
    properties.insert(
        "rocksdb.filter.policy".into(),
        b"rocksdb.BuiltinBloomFilter".to_vec(),
    );
    properties.insert("rocksdb.merge.operator".into(), b"nullptr".to_vec());
    properties.insert("rocksdb.prefix.extractor.name".into(), b"nullptr".to_vec());
    properties.insert(
        "rocksdb.block.based.table.prefix.filtering".into(),
        b"0".to_vec(),
    );
    properties.insert(
        "rocksdb.block.based.table.whole.key.filtering".into(),
        b"1".to_vec(),
    );
    properties.insert(
        "rocksdb.property.collectors".into(),
        b"[tikv.mvcc-properties-collector,tikv.range-properties-collector,BlobFileSizeCollector]"
            .to_vec(),
    );
}

fn write_block(file: &mut File, block: &[u8]) -> Result<BlockHandle, TikvError> {
    let handle = BlockHandle {
        offset: file.stream_position()?,
        length: block.len() as u64,
    };
    file.write_all(block)?;
    let block_type = 0_u8;
    file.write_all(&[block_type])?;
    let checksum = masked_crc32c(block.iter().copied().chain([block_type]));
    file.write_all(&checksum.to_le_bytes())?;
    Ok(handle)
}

fn write_footer(
    file: &mut File,
    metaindex: BlockHandle,
    index: BlockHandle,
) -> Result<(), TikvError> {
    let mut footer = vec![0; ROCKSDB_FOOTER_LEN];
    footer[0] = 1; // CRC32C
    let mut handles = encode_block_handle(metaindex);
    handles.extend_from_slice(&encode_block_handle(index));
    footer[1..1 + handles.len()].copy_from_slice(&handles);
    footer[41..45].copy_from_slice(&2_u32.to_le_bytes());
    footer[45..].copy_from_slice(ROCKSDB_SST_MAGIC);
    file.write_all(&footer)?;
    Ok(())
}

fn masked_crc32c(bytes: impl IntoIterator<Item = u8>) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f63b78 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    crc = !crc;
    crc.rotate_right(15).wrapping_add(0xa282ead8)
}

fn bloom_hash(mut key: &[u8]) -> u32 {
    const SEED: u32 = 0xbc9f1d34;
    const M: u32 = 0xc6a4a793;
    let mut hash = SEED ^ (key.len() as u32).wrapping_mul(M);
    while key.len() >= 4 {
        hash = hash.wrapping_add(u32::from_le_bytes(key[..4].try_into().unwrap()));
        hash = hash.wrapping_mul(M);
        hash ^= hash >> 16;
        key = &key[4..];
    }
    match key.len() {
        3 => hash = hash.wrapping_add((key[2] as i8 as i32 as u32) << 16),
        _ => {}
    }
    if key.len() >= 2 {
        hash = hash.wrapping_add((key[1] as i8 as i32 as u32) << 8);
    }
    if !key.is_empty() {
        hash = hash.wrapping_add(key[0] as i8 as i32 as u32);
        hash = hash.wrapping_mul(M);
        hash ^= hash >> 24;
    }
    hash
}

fn build_bloom_filter<'a>(keys: impl Iterator<Item = &'a [u8]>) -> Vec<u8> {
    let hashes = keys.map(bloom_hash).collect::<Vec<_>>();
    let mut lines = (hashes.len() * 10).div_ceil(512);
    if lines > 0 && lines % 2 == 0 {
        lines += 1;
    }
    let byte_count = lines * 64;
    let mut filter = vec![0; byte_count + 5];
    if lines > 0 {
        let probes = 6_u8;
        for mut hash in hashes {
            let delta = hash.rotate_right(17);
            let base = (hash as usize % lines) * 512;
            for _ in 0..probes {
                let bit = base + hash as usize % 512;
                filter[bit / 8] |= 1 << (bit % 8);
                hash = hash.wrapping_add(delta);
            }
        }
        filter[byte_count] = probes;
        filter[byte_count + 1..].copy_from_slice(&(lines as u32).to_le_bytes());
    }
    filter
}

/// 从本地 SST 读回的记录与属性。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSst {
    /// 已编码的 KV 对列表。
    pub records: Vec<(Vec<u8>, Vec<u8>)>,
    /// 表属性名 → 序列化值。
    pub properties: HashMap<String, Vec<u8>>,
}

/// 解析本地 RocksDB BlockBasedTable，供等价测试读回 KV 与属性。
pub fn read_local_sst(path: impl AsRef<Path>) -> Result<LocalSst, TikvError> {
    let file = std::fs::read(path)?;
    if file.len() < ROCKSDB_FOOTER_LEN {
        return Err(invalid_sst("file is smaller than RocksDB footer"));
    }
    let footer = &file[file.len() - ROCKSDB_FOOTER_LEN..];
    if &footer[45..] != ROCKSDB_SST_MAGIC {
        return Err(invalid_sst("bad RocksDB magic"));
    }
    if footer[0] != 1 || u32::from_le_bytes(footer[41..45].try_into().unwrap()) != 2 {
        return Err(invalid_sst("unsupported checksum or footer version"));
    }
    let mut footer_cursor = 1;
    let metaindex_handle = decode_block_handle(footer, &mut footer_cursor)?;
    let index_handle = decode_block_handle(footer, &mut footer_cursor)?;

    let metaindex = decode_block_entries(read_block(&file, metaindex_handle)?)?;
    let property_handle = metaindex
        .iter()
        .find(|(name, _)| name == b"rocksdb.properties")
        .ok_or_else(|| invalid_sst("missing rocksdb.properties metaindex entry"))
        .and_then(|(_, value)| {
            let mut cursor = 0;
            decode_block_handle(value, &mut cursor)
        })?;
    let properties = decode_block_entries(read_block(&file, property_handle)?)?
        .into_iter()
        .map(|(name, value)| {
            String::from_utf8(name)
                .map(|name| (name, value))
                .map_err(|error| invalid_sst(&error.to_string()))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;

    let mut records = Vec::new();
    for (_, encoded_handle) in decode_block_entries(read_block(&file, index_handle)?)? {
        let mut cursor = 0;
        let data_handle = decode_block_handle(&encoded_handle, &mut cursor)?;
        for (internal_key, value) in decode_block_entries(read_block(&file, data_handle)?)? {
            if internal_key.len() < 8 {
                return Err(invalid_sst("data entry is missing internal-key trailer"));
            }
            let user_key_len = internal_key.len() - 8;
            let trailer = u64::from_le_bytes(internal_key[user_key_len..].try_into().unwrap());
            if trailer != 1 {
                return Err(invalid_sst("data entry is not a sequence-zero SET"));
            }
            records.push((internal_key[..user_key_len].to_vec(), value));
        }
    }
    Ok(LocalSst {
        records,
        properties,
    })
}

fn invalid_sst(message: &str) -> TikvError {
    TikvError::InvalidData(message.into())
}

fn read_block(file: &[u8], handle: BlockHandle) -> Result<&[u8], TikvError> {
    let offset =
        usize::try_from(handle.offset).map_err(|_| invalid_sst("block offset overflow"))?;
    let length =
        usize::try_from(handle.length).map_err(|_| invalid_sst("block length overflow"))?;
    let end = offset
        .checked_add(length)
        .and_then(|end| end.checked_add(BLOCK_TRAILER_LEN))
        .ok_or_else(|| invalid_sst("block extent overflow"))?;
    if end > file.len() {
        return Err(invalid_sst("block extends beyond file"));
    }
    let block = &file[offset..offset + length];
    let block_type = file[offset + length];
    if block_type != 0 {
        return Err(invalid_sst(
            "compressed blocks are unsupported by debug reader",
        ));
    }
    let expected = u32::from_le_bytes(file[offset + length + 1..end].try_into().unwrap());
    let actual = masked_crc32c(block.iter().copied().chain([block_type]));
    if actual != expected {
        return Err(invalid_sst("block checksum mismatch"));
    }
    Ok(block)
}

fn decode_block_handle(bytes: &[u8], cursor: &mut usize) -> Result<BlockHandle, TikvError> {
    Ok(BlockHandle {
        offset: decode_uvarint(bytes, cursor)?,
        length: decode_uvarint(bytes, cursor)?,
    })
}

fn decode_uvarint(bytes: &[u8], cursor: &mut usize) -> Result<u64, TikvError> {
    let mut value = 0_u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| invalid_sst("truncated varint"))?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            return Err(invalid_sst("varint overflow"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    Err(invalid_sst("varint overflow"))
}

fn decode_block_entries(block: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, TikvError> {
    if block.len() < 8 {
        return Err(invalid_sst("block is missing restart array"));
    }
    let restart_count = u32::from_le_bytes(block[block.len() - 4..].try_into().unwrap()) as usize;
    if restart_count == 0 {
        return Err(invalid_sst("block has no restart points"));
    }
    let restart_bytes = restart_count
        .checked_add(1)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| invalid_sst("restart array overflow"))?;
    let entries_end = block
        .len()
        .checked_sub(restart_bytes)
        .ok_or_else(|| invalid_sst("restart array exceeds block"))?;
    let mut cursor = 0;
    let mut previous_key = Vec::new();
    let mut entries = Vec::new();
    while cursor < entries_end {
        let shared = usize::try_from(decode_uvarint(block, &mut cursor)?)
            .map_err(|_| invalid_sst("shared prefix overflow"))?;
        let unshared = usize::try_from(decode_uvarint(block, &mut cursor)?)
            .map_err(|_| invalid_sst("key length overflow"))?;
        let value_len = usize::try_from(decode_uvarint(block, &mut cursor)?)
            .map_err(|_| invalid_sst("value length overflow"))?;
        if shared > previous_key.len() {
            return Err(invalid_sst("shared prefix exceeds previous key"));
        }
        let key_end = cursor
            .checked_add(unshared)
            .ok_or_else(|| invalid_sst("key extent overflow"))?;
        let value_end = key_end
            .checked_add(value_len)
            .ok_or_else(|| invalid_sst("value extent overflow"))?;
        if value_end > entries_end {
            return Err(invalid_sst("entry exceeds block"));
        }
        let mut key = previous_key[..shared].to_vec();
        key.extend_from_slice(&block[cursor..key_end]);
        let value = block[key_end..value_end].to_vec();
        cursor = value_end;
        previous_key = key.clone();
        entries.push((key, value));
    }
    if cursor != entries_end {
        return Err(invalid_sst("entry stream overlaps restart array"));
    }
    Ok(entries)
}
