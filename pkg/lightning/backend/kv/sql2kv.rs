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
// Copyright 2026 AsterSQL.

// SQL 行 → TiKV 键值对编码。
//
// `tableKVEncoder` 将 Datum 行按表结构编码为记录键（record key，含 `_r`）与
// 索引键（index key），产出 `Pairs`；并提供行集合拆分、校验和分类、
// Datum 列表编解码等辅助函数。自增/自随机 ID 经 BaseKVEncoder 处理。

use std::any::Any;
use std::collections::BTreeMap;

use encode::{Datum, EncodeError, Encoder, EncodingConfig, Row, Rows};
use verification::{KVChecksum, KvPair};

use crate::{
    AllocatorType, AutoIDFieldType, BaseKVEncoder, CollectGeneratedColumnsFromTable, GeneratedCol,
    NewBaseKVEncoder, Session, TableDefinition,
};

/// 表级 KV 编码器：包装 BaseKVEncoder，实现 Encoder trait。
pub struct tableKVEncoder {
    pub BaseKVEncoder: BaseKVEncoder,
    closed: bool,
}

/// 测试用：从 dyn Encoder 取出内部 Session。
pub fn GetSession4test(encoder: &dyn Encoder) -> &Session {
    &encoder
        .as_any()
        .downcast_ref::<tableKVEncoder>()
        .expect("encoder was not created by NewTableKVEncoder")
        .BaseKVEncoder
        .SessionCtx
}

/// 按 EncodingConfig 构造表 KV 编码器。
pub fn NewTableKVEncoder(config: &EncodingConfig) -> Result<Box<dyn Encoder>, EncodeError> {
    Ok(Box::new(tableKVEncoder {
        BaseKVEncoder: NewBaseKVEncoder(config).map_err(EncodeError)?,
        closed: false,
    }))
}

/// 收集表上生成列（generated column）定义；`_se` 保留与 Go 签名对齐。
pub fn CollectGeneratedColumns(_se: &Session, table: &TableDefinition) -> Vec<GeneratedCol> {
    CollectGeneratedColumnsFromTable(table)
}

/// 一行编码结果：键值对列表及可比序的 RowID 字节。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pairs {
    pub Pairs: Vec<KvPair>,
    pub RowID: Vec<u8>,
}

/// 按索引 ID 分组的键值对集合（BTreeMap 保序）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GroupedPairs(pub BTreeMap<i64, Vec<KvPair>>);

impl GroupedPairs {
    /// 仅为满足 Rows 形状而保留；与 Go 一致，不支持切块。
    pub fn SplitIntoChunks(&self, _chunkSize: usize) -> Vec<Box<dyn Rows>> {
        panic!("not implemented")
    }
}

impl Rows for GroupedPairs {
    fn Clear(self: Box<Self>) -> Box<dyn Rows> {
        panic!("not implemented")
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// 从裸 KvPair 列表构造 Rows。
pub fn MakeRowsFromKvPairs(pairs: Vec<KvPair>) -> Box<dyn Rows> {
    Box::new(Pairs {
        Pairs: pairs,
        RowID: Vec::new(),
    })
}

/// 从裸 KvPair 列表构造单行 Row。
pub fn MakeRowFromKvPairs(pairs: Vec<KvPair>) -> Box<dyn Row> {
    Box::new(Pairs {
        Pairs: pairs,
        RowID: Vec::new(),
    })
}

/// 将 Rows（Pairs 或 GroupedPairs）还原为扁平 KvPair 列表。
pub fn Rows2KvPairs(rows: &dyn Rows) -> Vec<KvPair> {
    if let Some(pairs) = rows.as_any().downcast_ref::<Pairs>() {
        return pairs.Pairs.clone();
    }
    if let Some(groups) = rows.as_any().downcast_ref::<GroupedPairs>() {
        return groups.0.values().flatten().cloned().collect();
    }
    panic!("unknown Rows type")
}

/// 将 Row（须为 Pairs）还原为 KvPair 列表。
pub fn Row2KvPairs(row: &dyn Row) -> Vec<KvPair> {
    row.as_any()
        .downcast_ref::<Pairs>()
        .expect("row was not constructed from KV pairs")
        .Pairs
        .clone()
}

/// 清空 Row 内的键值对与 RowID。
pub fn ClearRow(row: &mut dyn Row) {
    if let Some(pairs) = row.as_any_mut().downcast_mut::<Pairs>() {
        pairs.Pairs.clear();
        pairs.RowID.clear();
    }
}

impl Encoder for tableKVEncoder {
    fn Close(&mut self) {
        self.BaseKVEncoder.SessionCtx.Close();
        self.closed = true;
    }

    /// 将一行 Datum 编码为记录键与索引键。
    ///
    /// `columnPermutation[i]` 表示表第 i 列对应输入行下标，-1 表示缺列（由自增等填充）。
    fn Encode(
        &mut self,
        row: &[Datum],
        rowID: i64,
        columnPermutation: &[i32],
        _offset: i64,
    ) -> Result<Box<dyn Row>, EncodeError> {
        if self.closed {
            return Err(EncodeError("encoder is closed".into()));
        }
        let mut record = self.BaseKVEncoder.GetOrCreateRecord();
        // 按列置换填充记录；缺列走 ProcessColDatum 的默认/自增逻辑。
        for index in 0..self.BaseKVEncoder.Columns.len() {
            let source = columnPermutation.get(index).copied().unwrap_or(-1);
            let datum = if source >= 0 {
                row.get(source as usize)
            } else {
                None
            };
            let value = self
                .BaseKVEncoder
                .ProcessColDatum(index, rowID, datum, true)
                .map_err(EncodeError)?;
            record.push(value);
        }
        // 生成列：基于已填列求值表达式。
        if !self.BaseKVEncoder.GenCols.is_empty() {
            self.BaseKVEncoder
                .EvalGeneratedColumns(&mut record)
                .map_err(|(index, error)| {
                    EncodeError(self.BaseKVEncoder.LogEvalGenExprFailed(
                        row,
                        &self.BaseKVEncoder.Columns[index].name,
                        &error,
                    ))
                })?;
        }
        // 非整数主键时，用 rowID rebase 隐式 RowID 分配器上限。
        if !self.BaseKVEncoder.table.pk_is_handle {
            self.BaseKVEncoder
                .TableAllocators()
                .Get(AllocatorType::RowIDAllocType)
                .Rebase(rowID, false);
        }
        let mut pairs = self
            .BaseKVEncoder
            .Record2KV(record, row, rowID)
            .map_err(EncodeError)?;
        pairs.RowID = comparableI64(rowID);
        Ok(Box::new(pairs))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 判断列是否为 AUTO_INCREMENT。
pub fn IsAutoIncCol(column: &encode::Column) -> bool {
    column.auto_increment
}

/// 经编码器的 AutoIDFn 将逻辑 id 映射为实际增量/分片 ID。
pub fn GetEncoderIncrementalID(encoder: &dyn Encoder, id: i64) -> i64 {
    let encoder = encoder.as_any().downcast_ref::<tableKVEncoder>().unwrap();
    (encoder.BaseKVEncoder.AutoIDFn)(id)
}

/// 取得编码器内部 Session（测试/诊断用）。
pub fn GetEncoderSe(encoder: &dyn Encoder) -> &Session {
    GetSession4test(encoder)
}

/// 计算列在编码时的最终 Datum（含缺省与自增填充）。
pub fn GetActualDatum(
    encoder: &dyn Encoder,
    column: usize,
    rowID: i64,
    inputDatum: Option<&Datum>,
) -> Result<Datum, String> {
    encoder
        .as_any()
        .downcast_ref::<tableKVEncoder>()
        .unwrap()
        .BaseKVEncoder
        .getActualDatum(column, rowID, inputDatum, true)
}

/// 从 Datum 提取自增记录 ID；浮点目标时四舍五入。
pub fn GetAutoRecordID(datum: &Datum, target: AutoIDFieldType) -> i64 {
    match (target, datum) {
        (AutoIDFieldType::Float, Datum::Float(value)) => value.round() as i64,
        (AutoIDFieldType::Integer, Datum::Int(value)) => *value,
        (AutoIDFieldType::Integer, Datum::UInt(value)) => *value as i64,
        _ => panic!("unsupported auto-increment field type"),
    }
}

impl Pairs {
    /// 所有键值对的字节总长。
    pub fn Size(&self) -> u64 {
        self.Pairs
            .iter()
            .map(|pair| (pair.key.len() + pair.val.len()) as u64)
            .sum()
    }
}

impl Row for Pairs {
    /// 按键形态拆分到 data（记录）与 indices（索引），并更新各自校验和。
    fn ClassifyAndAppend(
        &self,
        data: &mut Box<dyn Rows>,
        dataChecksum: &mut KVChecksum,
        indices: &mut Box<dyn Rows>,
        indexChecksum: &mut KVChecksum,
    ) {
        let data = data
            .as_any_mut()
            .downcast_mut::<Pairs>()
            .expect("data rows must be Pairs");
        let indices = indices
            .as_any_mut()
            .downcast_mut::<Pairs>()
            .expect("index rows must be Pairs");
        for pair in &self.Pairs {
            if isRecordKey(&pair.key) {
                dataChecksum.UpdateOne(pair);
                data.Pairs.push(pair.clone());
            } else {
                indexChecksum.UpdateOne(pair);
                indices.Pairs.push(pair.clone());
            }
        }
    }
    fn Size(&self) -> u64 {
        self.Size()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Rows for Pairs {
    fn Clear(mut self: Box<Self>) -> Box<dyn Rows> {
        self.Pairs.clear();
        self.RowID.clear();
        self
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// 按 tablecodec 固定 discriminator 位置区分记录键与索引键。
pub(crate) fn isRecordKey(key: &[u8]) -> bool {
    const TABLE_SPLIT_KEY_LEN: usize = 9;
    key.get(TABLE_SPLIT_KEY_LEN + 1) == Some(&b'r')
}

/// 将 i64 转为可比较的大端字节（符号位翻转，与 TiDB 编码约定一致）。
fn comparableI64(value: i64) -> Vec<u8> {
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes().to_vec()
}

/// 使用 TiDB 旧行格式编码 Datum 列表。
///
/// 该辅助函数没有列元数据，仅供内部算法测试；生产路径使用
/// `encodeCanonicalRow` 并传入真实列定义。
pub(crate) fn encodeDatumList(datums: &[Datum]) -> Vec<u8> {
    crate::encodeCanonicalRow(datums, &[], false)
        .expect("test Datum list must be canonical-encodable")
}

/// 解码 `encodeDatumList` 产出的 TiDB 旧行格式。
pub(crate) fn decodeDatumList(input: &[u8]) -> Result<Vec<Datum>, String> {
    let values = tablecodec::codec::Decode(input.to_vec(), 0).map_err(|error| error.to_string())?;
    if values.len() % 2 != 0 {
        return Err("canonical row contains an unmatched column id".into());
    }
    values
        .chunks_exact(2)
        .map(|pair| crate::fromCanonicalDatum(&pair[1], None))
        .collect()
}
