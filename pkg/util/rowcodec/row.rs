// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 新格式 row 字节的内存视图：解析 header/列 ID/offset/data/checksum，并支持查列与重编码。
//
// Row Format：VER|FLAGS|非空/空列计数|列 ID|offsets|数据|可选 checksum（CRC32）。
// large 标志决定列 ID/offset 宽度；checksum 供 TiCDC 等端到端校验。

// rowcodec 包里“访问一行新格式 row bytes”的内部结构、查列逻辑和 checksum 计算入口。
// 或需要释放的资源，涉及 kv/types/crc32/time 的位置只保留 Go 语义形状，
// 方便后续批次在补齐 Rust 模块连线时继续对照原 Go 文件。
// 迁移提示：CodecVer、errInvalidCodecVer、errInvalidChecksumVer、bytesToU32Slice、
// bytes2U16Slice、u16SliceToBytes、u32SliceToBytes 来自同包 common.go；
// encodeValueDatum、checksumVersionRawKey 来自同包 encoder.go。本任务只迁移 row.go，
// 因此这些跨文件符号保留为尚未接通占位，不在这里额外复制实现。

/// FLAGS：列 ID>255 或 data 超 u16 时置 large。
const rowFlagLarge: u8 = 1 << 0;
/// FLAGS：行尾带 checksum。
const rowFlagChecksum: u8 = 1 << 1;

/// checksum header 低 3 位：版本号掩码。
const checksumMaskVersion: u8 = 0b0111;
/// checksum header 第 4 位：是否带 extra checksum。
const checksumFlagExtra: u8 = 0b1000;

// row is the struct type used to access a row and the row format is shown as the following.
// Row Format
// 0 1 2 3
//	+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
// | VER | FLAGS | NOT_NULL_COL_CNT |
//	+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
// | NULL_COL_CNT | ...NOT_NULL_COL_IDS... |
//	+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
// | ...NULL_COL_IDS... | ...NOT_NULL_COL_OFFSETS... |
//	+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
// | ...NOT_NULL_COL_DATA... |
//	+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
// | ...CHECKSUM... |
//	+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//	- FLAGS
//	  - 0x01: large (when max(col_ids) > 255 or len(col_data) > max_u16)
// - size of col_id = large ? 4 : 1
//	    - size of col_offset = large ? 4 : 2
//	  - 0x02: has checksum
// Checksum
// 0 1 2 3 4 5 6 7 8
//		+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
// | |E| VER | CHECKSUM | EXTRA_CHECKSUM(OPTIONAL) |
//		+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//		     HEADER
//		- HEADER
//		  - VER: version
// - E: has extra checksum
//		- CHECKSUM
//		  - little-endian CRC32(IEEE) when hdr.ver = 0 (old version, columns-level checksum)
//	   - little-endian CRC32(IEEE) when hdr.ver = 1 (default, bytes-level checksum)
// row 对应 Go 的内部 struct，用于按 row format 访问一段已编码 row bytes。
// Go 版本的 []byte/[]uint16/[]uint32 多数会别名到底层 rowData；暂用 Vec 保存，
// 生命周期和零拷贝别名语义尚未接通，后续需要按安全借用或原始指针策略重新审查。
/// 已编码 row 的可读写视图：flags、列 ID、offsets、data 与可选 checksum。
#[derive(Default, Clone)]
pub struct row {
    flags: u8,
    checksumHeader: u8,
    numNotNullCols: u16,
    numNullCols: u16,

    // for small row: colID []byte, offsets []uint16, optimized for most cases.
    // small row 路径中列 ID 是 1 字节、offset 是 2 字节，保持 Go 字段顺序便于对照。
    pub colIDs: Vec<u8>,
    offsets: Vec<u16>,

    // for large row: colID []uint32, offsets []uint32.
    // large row 路径中列 ID 和 offset 都扩展为 4 字节，用于大列号或大 row data。
    pub colIDs32: Vec<u32>,
    offsets32: Vec<u32>,

    data: Vec<u8>,
    checksum1: u32,
    checksum2: u32,
}

impl row {
    // large 对应 Go 的 flags 判断：只检查 rowFlagLarge 位，不解析其它 flags。
    /// 是否为 large 布局（4 字节列 ID / offset）。
    pub fn large(&self) -> bool {
        self.flags & rowFlagLarge > 0
    }

    // hasChecksum 对应 Go 的 rowFlagChecksum 位判断。
    fn hasChecksum(&self) -> bool {
        self.flags & rowFlagChecksum > 0
    }

    // hasExtraChecksum 对应 checksum header 里的 extra bit。
    fn hasExtraChecksum(&self) -> bool {
        self.checksumHeader & checksumFlagExtra > 0
    }

    // getOffsets 返回第 i 个非空列的数据区间。
    // Go 代码按 large/small 两种 offset 宽度分支读取，并让 i==0 时 start 保持 0。
    fn getOffsets(&self, i: usize) -> (u32, u32) {
        let mut start = 0;
        let end;
        if self.large() {
            if i > 0 {
                start = self.offsets32[i - 1];
            }
            end = self.offsets32[i];
        } else {
            if i > 0 {
                start = u32::from(self.offsets[i - 1]);
            }
            end = u32::from(self.offsets[i]);
        }
        (start, end)
    }

    // getData 返回第 i 个非空列的原始编码 bytes。
    // Rust 返回切片；它依赖 self.data 的生命周期，和 Go 返回 []byte view 的语义相近。
    /// 返回第 `i` 个非空列的原始编码字节切片。
    pub fn getData(&self, i: usize) -> &[u8] {
        let (start, end) = self.getOffsets(i);
        &self.data[start as usize..end as usize]
    }

    // fromBytes 从 rowData 解码 row header、列 ID、offset、data 和可选 checksum。
    // Go 原实现没有显式短输入检查，越界时会 panic；这里保留索引语义，未额外发明校验规则。
    /// 从 rowData 解析 header、列 ID、offset、data 与可选 checksum。
    pub fn fromBytes(&mut self, rowData: &[u8]) -> Result<(), errors::SharedError> {
        if rowData[0] != CodecVer {
            // 对应 common.go 中的 errInvalidCodecVer。
            return Err(errInvalidCodecVer());
        }
        self.flags = rowData[1];
        self.numNotNullCols = u16::from_le_bytes(rowData[2..4].try_into().unwrap());
        self.numNullCols = u16::from_le_bytes(rowData[4..6].try_into().unwrap());
        let mut cursor = 6usize;
        let mut lastOffset = 0usize;

        if self.large() {
            // large row 的 col id/offset 都是 4 字节；Go 通过 unsafe 把 bytes view 成 []uint32。
            // Rust 保留 bytesToU32Slice 占位，后续需要决定是否复制还是零拷贝借用。
            let colIDsLen = usize::from(self.numNotNullCols + self.numNullCols) * 4;
            self.colIDs32 = bytesToU32Slice(&rowData[cursor..cursor + colIDsLen]);
            cursor += colIDsLen;

            let offsetsLen = usize::from(self.numNotNullCols) * 4;
            self.offsets32 = bytesToU32Slice(&rowData[cursor..cursor + offsetsLen]);
            cursor += offsetsLen;
            if let Some(last) = self.offsets32.last() {
                lastOffset = *last as usize;
            }
        } else {
            // small row 直接复用 col id 的 byte slice；用 Vec 克隆以避开生命周期接线。
            let colIDsLen = usize::from(self.numNotNullCols + self.numNullCols);
            self.colIDs = rowData[cursor..cursor + colIDsLen].to_vec();
            cursor += colIDsLen;

            let offsetsLen = usize::from(self.numNotNullCols) * 2;
            self.offsets = bytes2U16Slice(&rowData[cursor..cursor + offsetsLen]);
            cursor += offsetsLen;
            if let Some(last) = self.offsets.last() {
                lastOffset = usize::from(*last);
            }
        }

        // data 长度由最后一个 offset 决定；Go 这里仍然只是切片，不做 checksum 前置校验。
        self.data = rowData[cursor..cursor + lastOffset].to_vec();
        cursor += lastOffset;

        if self.hasChecksum() {
            self.checksumHeader = rowData[cursor];
            let checksumVersion = self.ChecksumVersion();
            // make sure it can be read previous version checksum to support backward compatibility.
            // 保持 Go 的兼容性白名单：版本 0/1/2 可读，其它版本返回 errInvalidChecksumVer。
            match checksumVersion {
                0 | 1 | 2 => {}
                _ => return Err(errInvalidChecksumVer()),
            }
            cursor += 1;
            self.checksum1 = u32::from_le_bytes(rowData[cursor..cursor + 4].try_into().unwrap());
            if self.hasExtraChecksum() {
                cursor += 4;
                self.checksum2 =
                    u32::from_le_bytes(rowData[cursor..cursor + 4].try_into().unwrap());
            }
        } else {
            // 没有 checksum flag 时，Go 会显式清空 header 和两个 checksum 字段。
            self.checksumHeader = 0;
            self.checksum1 = 0;
            self.checksum2 = 0;
        }
        Ok(())
    }

    // toBytes 按 row format 重新拼回 header、列 ID、offset 和 data。
    // Go 版本接收并 append 到调用方提供的 buf；用 Vec 保留“追加到已有缓冲区”的形状。
    fn toBytes(&self, mut buf: Vec<u8>) -> Vec<u8> {
        buf.push(CodecVer);
        buf.push(self.flags);
        buf.push(self.numNotNullCols as u8);
        buf.push((self.numNotNullCols >> 8) as u8);
        buf.push(self.numNullCols as u8);
        buf.push((self.numNullCols >> 8) as u8);
        if self.large() {
            // u32SliceToBytes 对应 common.go 的 unsafe slice view；这里仍作为同包占位函数调用。
            buf.extend(u32SliceToBytes(&self.colIDs32));
            buf.extend(u32SliceToBytes(&self.offsets32));
        } else {
            buf.extend_from_slice(&self.colIDs);
            buf.extend(u16SliceToBytes(&self.offsets));
        }
        buf.extend_from_slice(&self.data);
        buf
    }

    // findColID 在非空列数组和空列数组中分别二分查找 colID。
    // 返回值顺序保持 Go 的 (idx, isNil, notFound)，idx 仅在找到非空列时有意义。
    /// 二分查找列 ID，返回 `(idx, isNil, notFound)`。
    pub fn findColID(&self, colID: i64) -> (usize, bool, bool) {
        // Search the column in not-null columns array.
        let mut i = 0usize;
        let mut j = usize::from(self.numNotNullCols);
        while i < j {
            // Go 使用 uint(i+j)>>1 避免溢出；这里用 i + (j-i)/2 表达同样的二分中点意图。
            let h = i + ((j - i) >> 1);
            // i ≤ h < j
            let v = if self.large() {
                i64::from(self.colIDs32[h])
            } else {
                i64::from(self.colIDs[h])
            };
            if v < colID {
                i = h + 1;
            } else if v == colID {
                return (h, false, false);
            } else {
                j = h;
            }
        }

        // Search the column in null columns array.
        i = usize::from(self.numNotNullCols);
        j = usize::from(self.numNotNullCols + self.numNullCols);
        while i < j {
            // i ≤ h < j
            let h = i + ((j - i) >> 1);
            let v = if self.large() {
                i64::from(self.colIDs32[h])
            } else {
                i64::from(self.colIDs[h])
            };
            if v < colID {
                i = h + 1;
            } else if v == colID {
                return (0, true, false);
            } else {
                j = h;
            }
        }
        (0, false, true)
    }

    // ChecksumVersion returns the version of checksum. Note that it's valid only if checksum has been encoded in the row
    // value (callers can check it by `GetChecksum`).
    // ChecksumVersion 提取 checksum header 低 3 位，保持 Go 对 checksumMaskVersion 的位运算。
    /// 提取 checksum 版本（仅当行已编码 checksum 时有效）。
    pub fn ChecksumVersion(&self) -> i32 {
        i32::from(self.checksumHeader & checksumMaskVersion)
    }

    // GetChecksum returns the checksum of row data (not null columns).
    // GetChecksum 对应 Go 的双返回值：(checksum, ok)。
    /// 返回主校验和及是否存在。
    pub fn GetChecksum(&self) -> (u32, bool) {
        if !self.hasChecksum() {
            return (0, false);
        }
        (self.checksum1, true)
    }

    // GetExtraChecksum returns the extra checksum which shall be calculated in the last stable schema version (whose
    // elements are all public).
    // GetExtraChecksum 只检查 extra bit；Go 代码不要求 hasChecksum 再次为真。
    fn GetExtraChecksum(&self) -> (u32, bool) {
        if !self.hasExtraChecksum() {
            return (0, false);
        }
        (self.checksum2, true)
    }

    // ColumnIsNull returns if the column value is null. Mainly used for count column aggregation.
    // this method will used in unistore.
    // ColumnIsNull 先按 rowData 重置当前 row，再通过 findColID 判断列是否为空或缺失。
    // defaultVal 在 Go 中用 nil slice 表示默认值为空；用 Option<&[u8]> 表达这个 nil 语义。
    fn ColumnIsNull(
        &mut self,
        rowData: &[u8],
        colID: i64,
        defaultVal: Option<&[u8]>,
    ) -> Result<bool, errors::SharedError> {
        self.fromBytes(rowData)?;
        let (_, isNil, notFound) = self.findColID(colID);
        if notFound {
            return Ok(defaultVal.is_none());
        }
        Ok(isNil)
    }

    // initColIDs 对应 Go 的容量复用逻辑：容量足够时只调整长度，否则重新分配。
    fn initColIDs(&mut self) {
        let numCols = usize::from(self.numNotNullCols + self.numNullCols);
        if self.colIDs.capacity() >= numCols {
            self.colIDs.resize(numCols, 0);
        } else {
            self.colIDs = vec![0; numCols];
        }
    }

    // initColIDs32 是 large row 的列 ID 缓冲区初始化。
    fn initColIDs32(&mut self) {
        let numCols = usize::from(self.numNotNullCols + self.numNullCols);
        if self.colIDs32.capacity() >= numCols {
            self.colIDs32.resize(numCols, 0);
        } else {
            self.colIDs32 = vec![0; numCols];
        }
    }

    // initOffsets 初始化 small row 的 offset 缓冲区，长度等于非空列数。
    fn initOffsets(&mut self) {
        let numNotNullCols = usize::from(self.numNotNullCols);
        if self.offsets.capacity() >= numNotNullCols {
            self.offsets.resize(numNotNullCols, 0);
        } else {
            self.offsets = vec![0; numNotNullCols];
        }
    }

    // initOffsets32 初始化 large row 的 offset 缓冲区，长度等于非空列数。
    fn initOffsets32(&mut self) {
        let numNotNullCols = usize::from(self.numNotNullCols);
        if self.offsets32.capacity() >= numNotNullCols {
            self.offsets32.resize(numNotNullCols, 0);
        } else {
            self.offsets32 = vec![0; numNotNullCols];
        }
    }

    // CalculateRawChecksum calculates the bytes-level checksum by using the given elements.
    // this is mainly used by the TiCDC to implement E2E checksum functionality.
    // CalculateRawChecksum 按给定 values 重写 row.data 中已编码的非空列，然后按 raw bytes 计算 checksum。
    fn CalculateRawChecksum(
        &mut self,
        loc: Option<&time::Location>,
        colIDs: &[i64],
        values: &[&types::Datum],
        key: kv::Key,
        handle: &dyn kv::Handle,
        mut buf: Vec<u8>,
    ) -> Result<u32, errors::SharedError> {
        for (idx, colID) in colIDs.iter().enumerate() {
            // encodeValueDatum 可能因为 Datum 类型不支持或时间转换失败返回错误；Go 直接向上返回。
            // Go 参数是 []*types.Datum；先用 &[&types::Datum] 表达“外部持有 Datum 指针”。
            let data = encodeValueDatum(loc, values[idx], Vec::new())?;
            let (index, isNil, notFound) = self.findColID(*colID);
            // some datum may not be found, since it's not encoded into the raw bytes,
            // such as handle key columns, or null columns.
            if !notFound && !isNil {
                let (start, end) = self.getOffsets(index);
                let dst = &mut self.data[start as usize..end as usize];
                // Go copy(dst, src) 只复制两者长度的较小值；Rust 不能直接 copy_from_slice 不等长切片。
                let n = std::cmp::min(dst.len(), data.len());
                dst[..n].copy_from_slice(&data[..n]);
            }
        }
        buf = self.toBytes(buf);
        buf.push(self.checksumHeader);
        let mut rawChecksum = crc32_update(0, &buf);
        // keep backward compatibility to v8.3.0
        // v8.3.0 使用 raw key 参与 checksum；之后版本改为使用 handle.Encoded()，保持 Go 兼容分支。
        if self.ChecksumVersion() == i32::from(checksumVersionRawKey) {
            rawChecksum = crc32_update(rawChecksum, key.as_ref());
        } else {
            rawChecksum = crc32_update(rawChecksum, &handle.Encoded());
        }
        Ok(rawChecksum)
    }
}
