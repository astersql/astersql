// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 索引预切分（pre-split）模块。
//
// 在创建索引或建表时，为了避免所有写入集中到单个 Region（TiKV 中数据分片
// 的基本单位，每个 Region 负责一段连续的 key 范围）造成写热点，可以预先
// 按照用户指定的切分点或上下界把索引的 key 范围切分成多个 Region。
// 本模块负责：
// - 根据 SQL 语句中给出的切分值列表或上下界与数量，计算切分用的索引 key；
// - 调用外部提供的 split/scatter 回调执行 Region 切分并等待打散完成。

use crate::backfilling::Key;
use crate::index_cop::Datum;

/// 预切分过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SplitError {
    /// 切分份数非法（按上下界切分时必须大于 0）。
    InvalidCount,
    /// 上下界非法（如下界大于等于上界，或区间过小无法切分）。
    InvalidBounds,
    /// 切分表达式求值失败（存在无法求值的表达式）。
    Evaluation,
    /// 底层 Region 切分操作失败，携带具体错误信息。
    Split(String),
}
/// 切分参数，来自 SQL 语句中的 `SPLIT TABLE ... INDEX ...` 子句。
///
/// 两种使用方式二选一：
/// - `value_lists` 非空：按显式给出的每组索引值作为切分点；
/// - 否则：在 `lower`/`upper` 界定的范围内均匀切分成 `num` 份。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SplitArguments {
    /// 显式切分点列表，每个元素是一组索引列的值（Datum 为运行时数据值）。
    pub value_lists: Vec<Vec<Datum>>,
    /// 按范围切分时的下界索引值。
    pub lower: Vec<Datum>,
    /// 按范围切分时的上界索引值。
    pub upper: Vec<Datum>,
    /// 按范围切分时期望切分出的 Region 份数。
    pub num: usize,
}

/// 根据切分参数计算索引的切分 key 列表。
///
/// 优先使用显式值列表；若为空则退化为按上下界均匀切分。
pub fn get_split_index_keys(
    table_id: i64,
    index_id: i64,
    args: &SplitArguments,
) -> Result<Vec<Key>, SplitError> {
    if !args.value_lists.is_empty() {
        return get_split_keys_from_value_list(table_id, index_id, &args.value_lists);
    }
    get_split_keys_from_bound(table_id, index_id, &args.lower, &args.upper, args.num)
}
/// 由显式值列表生成切分 key，保持用户指定的顺序和重复值。
pub fn get_split_keys_from_value_list(
    table_id: i64,
    index_id: i64,
    values: &[Vec<Datum>],
) -> Result<Vec<Key>, SplitError> {
    let keys: Vec<Key> = values
        .iter()
        .map(|values| encode_index_key(table_id, index_id, values))
        .collect();
    Ok(keys)
}
/// 在 `[lower, upper)` 索引 key 范围内均匀生成 `count - 1` 个切分点。
///
/// 做法是把两端 key 的前缀字节视为大整数，按数值等距插值，
/// 因此得到的切分点在字节序上大致均匀分布。
pub fn get_split_keys_from_bound(
    table_id: i64,
    index_id: i64,
    lower: &[Datum],
    upper: &[Datum],
    count: usize,
) -> Result<Vec<Key>, SplitError> {
    // Go 调用方只要求 region 数大于 0；一份 Region 合法且无需切分点。
    if count == 0 {
        return Err(SplitError::InvalidCount);
    }
    let lower = encode_index_key(table_id, index_id, lower);
    let upper = encode_index_key(table_id, index_id, upper);
    // 编码后的下界必须严格小于上界。
    if lower >= upper {
        return Err(SplitError::InvalidBounds);
    }
    let common = lower
        .iter()
        .zip(&upper)
        .take_while(|(left, right)| left == right)
        .count();
    let mut lo = padded_u64(&lower[common..], 0);
    let hi = padded_u64(&upper[common..], 0xff);
    let step = hi.wrapping_sub(lo) / count as u64;
    let prefix = lower[..common].to_vec();
    Ok((1..count)
        .map(|_| {
            lo = lo.wrapping_add(step);
            let mut key = prefix.clone();
            key.extend_from_slice(&lo.to_be_bytes());
            key
        })
        .collect())
}
/// 将一组索引列的值编码为索引 key。
///
/// key 布局为 `t{table_id}i{index_id}` 前缀加上各列值的调试格式编码，
/// 每个值以 0 字节结尾（简化版编码，仅用于生成有序的切分 key）。
fn encode_index_key(table_id: i64, index_id: i64, values: &[Datum]) -> Key {
    let mut key = b"t".to_vec();
    key.extend_from_slice(&table_id.to_be_bytes());
    key.push(b'i');
    key.extend_from_slice(&index_id.to_be_bytes());
    for value in values {
        key.extend_from_slice(format!("{value:?}").as_bytes());
        key.push(0);
    }
    key
}
/// 按 Go `getUint64FromBytes` 的规则取前八字节，不足部分以 `pad` 补齐。
fn padded_u64(key: &[u8], pad: u8) -> u64 {
    let mut bytes = [pad; 8];
    let copied = key.len().min(8);
    bytes[..copied].copy_from_slice(&key[..copied]);
    u64::from_be_bytes(bytes)
}

/// 按给定切分 key 执行 Region 切分并统计打散（scatter）完成的 Region 数量。
///
/// - `split`：执行切分的回调，返回新产生的 Region ID 列表；
/// - `scatter_finished`：查询某个 Region 是否已完成打散（即副本已被
///   调度器均匀分布到不同节点）的回调。
///
/// 返回已完成打散的 Region 个数；切分 key 为空时直接返回 0。
pub fn split_index_region_and_wait(
    keys: &[Key],
    mut split: impl FnMut(&[Key]) -> Result<Vec<u64>, String>,
    mut scatter_finished: impl FnMut(u64) -> bool,
) -> Result<usize, SplitError> {
    if keys.is_empty() {
        return Ok(0);
    }
    let regions = split(keys).map_err(SplitError::Split)?;
    // 只统计已经完成打散的 Region。
    Ok(regions
        .into_iter()
        .filter(|region| scatter_finished(*region))
        .count())
}
/// 收集切分表达式的求值结果；只要有一个表达式求值失败（None）即报错。
pub fn eval_split_datums(expressions: &[Option<Datum>]) -> Result<Vec<Datum>, SplitError> {
    expressions
        .iter()
        .cloned()
        .collect::<Option<Vec<_>>>()
        .ok_or(SplitError::Evaluation)
}
