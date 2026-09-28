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

// 行编码器：将列 ID 与 Datum 整理为新格式 row 字节，并按策略追加校验和。
//
// 流程为装载列值 → 非空/空列排序 → 编码非空 Datum → `Checksum` 策略写出最终字节。

// rowcodec 的行编码流程：接收列 ID 与 Datum，整理空/非空列顺序，
// 编码非空列数据，并按 Go 版本的 Checksum 策略追加校验和字节。
// Encoder is used to encode a row.
// Encoder 对应 Go 的行编码器；它内嵌 row 状态，并在单次 Encode 中复用临时列 ID 与 Datum 缓冲。
/// 行编码器：内嵌 `row` 状态，复用临时列 ID / Datum 缓冲完成一次 Encode。
pub struct Encoder {
    // Go 里是匿名嵌入 row；显式命名，表示复用 row.go 迁移出的底层格式字段。
    row: row,
    temp_col_ids: Vec<i64>,
    // Rust 持有 Datum；重排时保持列 ID 与值的配对关系。
    values: Vec<types::Datum>,
    // Enable indicates whether this encoder should be use.
    // Enable 保留 Go 的导出字段名，用于表示调用方是否启用该 encoder。
    pub Enable: bool,
}

impl Encoder {
    /// 创建编码器；`enable` 对应 Go 导出字段 `Enable`。
    pub fn new(enable: bool) -> Self {
        Self {
            row: row::default(),
            temp_col_ids: Vec::new(),
            values: Vec::new(),
            Enable: enable,
        }
    }

    /// 返回已编码的行校验和及是否存在。
    pub fn GetChecksum(&self) -> (u32, bool) {
        self.row.GetChecksum()
    }

    // Encode encodes a row from a datums slice.
    // `buf` is not truncated before encoding.
    // This function may return both a valid encoded bytes and an error (actually `"pingcap/errors".ErrorGroup`). If the caller
    // expects to handle these errors according to `SQL_MODE` or other configuration, please refer to `pkg/errctx`.
    // the caller needs to ensure the key is not nil if checksum is required.
    // Encode 对应 Go 的入口方法：重置内部状态、装载列值、整理列布局、编码 Datum，最后交给 checksum 策略生成字节。
    /// 编码一行：重置、装载、整理布局、编码 Datum，再经 checksum 策略写出。
    pub fn Encode(
        &mut self,
        loc: Option<&time::Location>,
        col_ids: Vec<i64>,
        values: Vec<types::Datum>,
        checksum: Option<Box<dyn Checksum>>,
        mut buf: Vec<u8>,
    ) -> Result<Vec<u8>, errors::SharedError> {
        self.reset();
        self.appendColVals(col_ids, values);
        let (num_cols, not_null_idx) = self.reformatCols();
        // 编码非空列时会累计单列错误；Go 里遇到错误仍可能继续处理剩余列以收集 ErrorGroup。
        if let Err(err) = self.encodeRowCols(loc, num_cols, not_null_idx) {
            return Err(err);
        }
        // Go nil 接口默认替换为 NoChecksum；用 Option 表达该默认分支。
        let checksum = checksum.unwrap_or_else(|| Box::new(NoChecksum {}));
        checksum.encode(self, buf)
    }

    // reset 清空 Encoder 可复用状态，对应 Go 在每次 Encode 开头复位 row 与临时 slice。
    fn reset(&mut self) {
        self.row.flags = 0;
        self.row.numNotNullCols = 0;
        self.row.numNullCols = 0;
        self.row.data.truncate(0);
        self.temp_col_ids.truncate(0);
        self.values.truncate(0);
        self.row.offsets32.truncate(0);
        self.row.offsets.truncate(0);
        self.row.checksumHeader = 0;
        self.row.checksum1 = 0;
        self.row.checksum2 = 0;
    }

    // appendColVals 按传入顺序把 colID 与 Datum 配对写入临时缓冲。
    fn appendColVals(&mut self, col_ids: Vec<i64>, values: Vec<types::Datum>) {
        assert!(
            col_ids.len() <= values.len(),
            "values must contain an entry for every column ID"
        );
        for (col_id, datum) in col_ids.into_iter().zip(values) {
            self.appendColVal(col_id, datum);
        }
    }

    // appendColVal 记录单列，并根据列 ID 和 Datum 是否为空更新 row flags 与计数。
    fn appendColVal(&mut self, col_id: i64, d: types::Datum) {
        if col_id > 255 {
            self.row.flags |= rowFlagLarge;
        }
        if d.IsNull() {
            self.row.numNullCols += 1;
        } else {
            self.row.numNotNullCols += 1;
        }
        self.temp_col_ids.push(col_id);
        self.values.push(d);
    }

    // reformatCols 将临时列按 Go rowcodec 布局改排：非空列在前、空列在后，并分别按列 ID 排序。
    // 返回值 numCols 是总列数，notNullIdx 是非空列数量，也是后续 encodeRowCols 的上界。
    fn reformatCols(&mut self) -> (usize, usize) {
        let num_cols = self.temp_col_ids.len();
        let mut not_null = Vec::with_capacity(self.row.numNotNullCols as usize);
        let mut null_ids = Vec::with_capacity(self.row.numNullCols as usize);
        for (col_id, datum) in self.temp_col_ids.drain(..).zip(self.values.drain(..)) {
            if datum.IsNull() {
                null_ids.push(col_id);
            } else {
                not_null.push((col_id, datum));
            }
        }
        not_null.sort_by_key(|(col_id, _)| *col_id);
        null_ids.sort_unstable();
        self.values = not_null.iter().map(|(_, datum)| datum.clone()).collect();
        let ordered_ids = not_null
            .iter()
            .map(|(col_id, _)| *col_id)
            .chain(null_ids)
            .collect::<Vec<_>>();
        if self.row.large() {
            self.row.initColIDs32();
            self.row.initOffsets32();
            for (slot, col_id) in self.row.colIDs32.iter_mut().zip(ordered_ids) {
                *slot = col_id as u32;
            }
        } else {
            self.row.initColIDs();
            self.row.initOffsets();
            for (slot, col_id) in self.row.colIDs.iter_mut().zip(ordered_ids) {
                *slot = col_id as u8;
            }
        }
        (num_cols, self.values.len())
    }

    // encodeRowCols 编码所有非空列的 Datum，并在每列编码完成后记录 offset。
    // Go 版本会累计错误到 multierr，同时在数据超过 u16 offset 上限时把 row 动态升级为 large。
    fn encodeRowCols(
        &mut self,
        loc: Option<&time::Location>,
        num_cols: usize,
        not_null_idx: usize,
    ) -> Result<(), errors::SharedError> {
        let r = &mut self.row;
        let mut errs: Vec<errors::SharedError> = Vec::new();
        for i in 0..not_null_idx {
            let d = &self.values[i];
            match encodeValueDatum(loc, d, std::mem::take(&mut r.data)) {
                Ok(next_data) => {
                    r.data = next_data;
                }
                Err(err) => {
                    // Go 使用 multierr.Append 继续累计错误；这里保留累计意图，具体 ErrorGroup 类型待后续接线。
                    errs.push(err);
                }
            }
            // handle convert to large
            // 数据区长度超过 MaxUint16 时，小行必须迁移到 large 布局，并复制已写入的列 ID 与 offsets。
            if r.data.len() > u16::MAX as usize && !r.large() {
                r.initColIDs32();
                for j in 0..num_cols {
                    r.colIDs32[j] = r.colIDs[j] as u32;
                }
                r.initOffsets32();
                for j in 0..=i {
                    r.offsets32[j] = r.offsets[j] as u32;
                }
                r.flags |= rowFlagLarge;
            }
            if r.large() {
                r.offsets32[i] = r.data.len() as u32;
            } else {
                r.offsets[i] = r.data.len() as u16;
            }
        }
        // Go 返回 nil 或 ErrorGroup；暂用 Option 转 Result 表示最终错误状态。
        match errs.len() {
            0 => Ok(()),
            1 => Err(errs.pop().expect("one error")),
            _ => Err(errors::New(
                errs.into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            )),
        }
    }
}

// encodeValueDatum encodes one row datum entry into bytes.
// due to encode as value, this method will flatten value type like tablecodec.flatten
// encodeValueDatum 对应 Go 的单 Datum 编码函数：按 Datum Kind 选择 row value 内部编码方式。
// 这里保留各分支原始语义；具体 Datum API、MySQL 时间/JSON/vector 类型与 codec 依赖还未 Rust 化。
fn encodeValueDatum(
    loc: Option<&time::Location>,
    d: &types::Datum,
    mut buffer: Vec<u8>,
) -> Result<Vec<u8>, errors::SharedError> {
    match d.Kind() {
        types::KindInt64 => {
            buffer = encodeInt(buffer, d.GetInt64());
        }
        types::KindUint64 => {
            buffer = encodeUint(buffer, d.GetUint64());
        }
        types::KindString | types::KindBytes => {
            buffer.extend_from_slice(&d.GetBytes());
        }
        types::KindMysqlTime => {
            // for mysql datetime, timestamp and date type
            // Timestamp 在 Go 中按 loc 转成 UTC 后打包；loc 为 nil 或 UTC 时不转换。
            let mut t = d.GetMysqlTime();
            if t.Type() == mysql::TypeTimestamp
                && loc.is_some_and(|location| *location != time::UTC)
            {
                t.ConvertTimeZone(*loc.expect("checked above"), time::UTC)
                    .map_err(|error| errors::New(error.to_string()))?;
            }
            let v = match t.ToPackedUint() {
                Ok(v) => v,
                Err(err) => return Err(errors::New(err.to_string())),
            };
            buffer = encodeUint(buffer, v);
        }
        types::KindMysqlDuration => {
            buffer = encodeInt(buffer, d.GetMysqlDuration().Duration);
        }
        types::KindMysqlEnum => {
            buffer = encodeUint(buffer, d.GetMysqlEnum().Value);
        }
        types::KindMysqlSet => {
            buffer = encodeUint(buffer, d.GetMysqlSet().Value);
        }
        types::KindBinaryLiteral | types::KindMysqlBit => {
            // We don't need to handle errors here since the literal is ensured to be able to store in uint64 in convertToMysqlBit.
            // Go 仍会检查 ToInt 返回错误；保持该错误返回路径。
            let val = match d
                .GetBinaryLiteral()
                .ToInt((*types::DefaultStmtNoWarningContext).clone())
            {
                Ok(val) => val,
                Err(err) => return Err(errors::New(err.to_string())),
            };
            buffer = encodeUint(buffer, val);
        }
        types::KindFloat32 | types::KindFloat64 => {
            buffer = codec::EncodeFloat(buffer, d.GetFloat64());
        }
        types::KindMysqlDecimal => {
            buffer = match codec::EncodeDecimal(buffer, &d.GetMysqlDecimal(), d.Length(), d.Frac())
            {
                Ok(buffer) => buffer,
                Err(err) => return Err(err),
            };
        }
        types::KindMysqlJSON => {
            let j = d.GetMysqlJSON();
            buffer.push(j.TypeCode);
            buffer.extend_from_slice(&j.Value);
        }
        types::KindVectorFloat32 => {
            let v = d.GetVectorFloat32();
            buffer = v.SerializeTo(buffer);
        }
        _ => {
            return Err(errors::New(format!("unsupport encode type {}", d.Kind())));
        }
    }
    Ok(buffer)
}

// Checksum is used to calculate and append checksum data into the raw bytes
// Checksum 对应 Go 接口：不同实现决定是否以及如何把校验和追加到原始 row bytes。
/// 校验和策略：决定是否以及如何把 checksum 追加到 row 字节。
pub trait Checksum {
    fn encode(&self, encoder: &mut Encoder, buf: Vec<u8>) -> Result<Vec<u8>, errors::SharedError>;
}

// NoChecksum indicates no checksum is encoded into the returned raw bytes.
// NoChecksum 对应 Go 的空校验实现，只负责清除 checksum flag 并返回 row bytes。
/// 不写校验和：清除 checksum flag 后返回 row 字节。
pub struct NoChecksum {}

impl Checksum for NoChecksum {
    fn encode(&self, encoder: &mut Encoder, buf: Vec<u8>) -> Result<Vec<u8>, errors::SharedError> {
        // revert checksum flag
        // Go 的 &^= 是按位清除；这里保留“撤销 checksum 标记”的语义。
        encoder.row.flags &= !rowFlagChecksum;
        Ok(encoder.row.toBytes(buf))
    }
}

// introduced since v8.3.0
// checksumVersionRawKey 保留 Go 常量，表示旧的 raw key checksum 版本。
const checksumVersionRawKey: u8 = 1;

// introduced since v8.4.0
// checksumVersionRawHandle 保留 Go 常量，表示基于 raw handle 的 checksum 版本。
/// 基于 raw handle 的 checksum 版本（自 v8.4.0）。
pub const checksumVersionRawHandle: u8 = 2;

// RawChecksum indicates encode the raw bytes checksum and append it to the raw bytes.
// RawChecksum 对应 Go 的 raw bytes checksum 实现；Handle 提供追加到 CRC32 输入中的行句柄编码。
/// Raw bytes 校验和：对 value+handle 做 CRC32 并追加到行尾。
pub struct RawChecksum {
    /// 参与 CRC32 的行句柄（Handle）。
    pub Handle: Box<dyn kv::Handle>,
}

impl Checksum for RawChecksum {
    fn encode(&self, encoder: &mut Encoder, buf: Vec<u8>) -> Result<Vec<u8>, errors::SharedError> {
        encoder.row.flags |= rowFlagChecksum;
        // revert extra checksum flag
        // RawChecksum 只写单 checksum，因此先清除 extra checksum 标记。
        encoder.row.checksumHeader &= !checksumFlagExtra;
        // revert checksum version
        // 再清除版本位，避免复用 encoder 时残留旧版本。
        encoder.row.checksumHeader &= !checksumMaskVersion;
        // set checksum version
        // Go 当前把版本设置为 RawHandle(v8.4.0)。
        encoder.row.checksumHeader |= checksumVersionRawHandle;
        let mut value_bytes = encoder.row.toBytes(buf);
        value_bytes.push(encoder.row.checksumHeader);
        // Go 使用 crc32.Checksum(valueBytes, crc32.IEEETable) 初始化 checksum1。
        encoder.row.checksum1 = crc32_update(0, &value_bytes);
        // Handle.Encoded() 是外部 kv 依赖；保留调用形状，表示句柄字节也纳入 CRC32。
        encoder.row.checksum1 = crc32_update(encoder.row.checksum1, &self.Handle.Encoded());
        // Go 使用 binary.LittleEndian.AppendUint32 追加 checksum1。
        value_bytes.extend_from_slice(&encoder.row.checksum1.to_le_bytes());
        Ok(value_bytes)
    }
}
