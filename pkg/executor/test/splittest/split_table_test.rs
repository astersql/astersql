// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 对应 `pkg/executor/test/splittest/split_table_test.go`。
//
// Go 版本的 `TestShowTableRegion`/`TestClusterIndexShowTableRegion` 通过完整的
// `testkit.TestKit` 执行 `SPLIT TABLE ... REGIONS` / `SHOW TABLE ... REGIONS` SQL，
// 依赖一个支持 region 元数据（PD/TiKV mock、`ddl.EnableSplitTableRegion`、分区表
// DDL、placement policy）的完整 mock store。当前 `astersql-testkit` 的
// `AnalyzeStatsStore`/`ConcreteSession` 还没有实现这一整套 region/分区/
// placement-policy 基础设施（只做谓词列统计，不建 region），因此本文件改为
// 直接对真实生产代码做单元级验证：
//
//   1. `astersql-util-regionsplit`（`pkg/util/regionsplit/split_handle.rs`）的
//      `GetSplitTableKeys`/`GetSplitIndexKeys` —— 真实的分裂点插值算法，直接用
//      Go 测试里 `between (-10000) and (10000) regions 4` 之类的边界和数量核对
//      分裂点是否与 Go 期望的数值（-5000/0/5000 等）完全一致，以及
//      `MinRegionStepValue` 触发的错误信息与 Go 完全一致；
//   2. `astersql-executor::split`（`pkg/executor/split.rs`）的
//      `SplitTableRegionExec`/`SplitIndexRegionExec`/`getPhysicalTableRegions`/
//      `decodeRegionsKey` —— 真实的执行器与 region 去重/解码逻辑，通过实现一个
//      `SplitRuntime`（`MockPdRuntime`）驱动：mock 只负责 PD/TiKV 语义的 region
//      分裂与加载（在内存里维护一份全局按 key 排序的 region 列表，语义上等价于
//      真实 PD 的 region 分裂：以给定 key 为界把覆盖该 key 的 region 一分为二），
//      key 编码/prefix 计算等纯函数部分则是本文件按 Go tablecodec 的记录/索引
//      key 布局手写的最小实现（`record key = t_<id>_r_<handle>`，
//      `index key = t_<id>_i_<idxid>_<value>`，大端 8 字节整数并翻转符号位），
//      与 `regionKeyDecoder::decodeRegionKey`（真实生产代码）严格对应，
//      使得分裂后 `show table regions` 的字符串输出与 Go 断言完全一致。
//
// `ddl.EnableSplitTableRegion` 在 Go 里的效果是"建表时自动为该表预留一个独立
// region"，`MockPdRuntime::register_table` 直接在表注册时对 `table_prefix(id)`/
// `table_prefix(id+1)` 做一次预分裂来复现这一效果（而不是引入完整的 DDL 建表
// 流程）。分区表 DDL、placement policy 调度信息（`show table regions` 的第 11/12
// 列）依赖尚不存在的 Rust DDL/placement 基础设施，本文件不假造。

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use astersql_executor::split::{
    RegionDescriptor, RegionStatistics, SplitIndexRegionExec, SplitRuntime, SplitTableRegionExec,
    getPhysicalTableRegions, regionKeyDecoder, splitRegionResult,
};
use astersql_util_regionsplit::{
    Datum as RsDatum, GetSplitIndexKeys, GetSplitTableKeys, IndexInfo, MinRegionStepValue,
    SplitError, StatementContext, TableInfo, commonHandleCols, intHandleCols,
};

// ---------------------------------------------------------------------------
// 1. `astersql-util-regionsplit` 分裂点插值算法的直接单测。
// ---------------------------------------------------------------------------

/// Go 测试不使用 `t.Parallel()`；串行化会读写全局 `MinRegionStepValue` 的 Rust 测试，
/// 避免测试线程之间互相改变最小步长、产生顺序相关的偶发失败。
static MIN_REGION_STEP_TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_min_region_step() -> MutexGuard<'static, ()> {
    MIN_REGION_STEP_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct RestoreMinRegionStep(i64);

impl Drop for RestoreMinRegionStep {
    fn drop(&mut self) {
        MinRegionStepValue.store(self.0, std::sync::atomic::Ordering::Release);
    }
}

/// Go tablecodec 用符号位翻转把有符号 int64 编码成按字节可比较的 8 字节大端序，
/// `regionsplit::encode_i64` 就是这个编码；这里写一个仅供断言用的反翻转解码器，
/// 用来把 `GetSplitTableKeys`/`GetSplitIndexKeys` 生成的真实 key 还原成十进制值，
/// 不修改、不依赖任何生产代码的内部私有函数。
fn unflip_i64(bytes: &[u8]) -> i64 {
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    (u64::from_be_bytes(array) ^ (1u64 << 63)) as i64
}

/// 构造带/不带二级索引的最小 `TableInfo`，供分裂点插值单测复用。
fn table_for_split(has_index: bool) -> TableInfo {
    TableInfo {
        name: "t_regions".to_owned(),
        pk_is_handle: true,
        pk_is_unsigned: false,
        is_common_handle: false,
        indices: if has_index {
            vec![IndexInfo {
                id: 1,
                name: "idx".to_owned(),
            }]
        } else {
            Vec::new()
        },
    }
}

/// 对应 Go `split table t_regions between (-10000) and (10000) regions 4`
/// 之后 `show table t_regions regions` 断言的三个内部分裂点：-5000/0/5000。
#[test]
fn get_split_table_keys_matches_go_expected_bound_split_points() {
    let _serial = lock_min_region_step();
    let table = table_for_split(true);
    let statement_context = StatementContext::default();
    let keys = GetSplitTableKeys(
        &statement_context,
        &table,
        &intHandleCols,
        9,
        &[RsDatum::Int(-10000)],
        &[RsDatum::Int(10000)],
        4,
        Vec::new(),
    )
    .expect("GetSplitTableKeys must succeed for a valid bound range");

    // 表带二级索引时，Go/`regionsplit::GetSplitTableKeys` 都会额外插入一个
    // `record_prefix` 边界（对应 Go `tablecodec.GenTableRecordPrefix`），
    // 因此 4 个 region 需要 4 个 key：record_prefix + 3 个内部分裂点。
    assert_eq!(keys.len(), 4);
    assert!(
        keys[0].ends_with(b"_r"),
        "first key must be the bare record prefix"
    );
    let split_points: Vec<i64> = keys[1..]
        .iter()
        .map(|key| unflip_i64(&key[key.len() - 8..]))
        .collect();
    assert_eq!(split_points, vec![-5000, 0, 5000]);
}

/// 使用 `regionsplit` 生成的真实 tablecodec 记录键验证 `SHOW TABLE REGIONS` 的
/// 解码契约。Go `TestShowTableRegion` 精确断言首个内部分裂点为
/// `t_<id>_r_-5000`，不能用与生产编码不同的 mock key 绕过该路径。
#[test]
fn real_regionsplit_record_key_decodes_to_go_region_name() {
    let _serial = lock_min_region_step();
    let table = table_for_split(true);
    let keys = GetSplitTableKeys(
        &StatementContext::default(),
        &table,
        &intHandleCols,
        9,
        &[RsDatum::Int(-10000)],
        &[RsDatum::Int(10000)],
        4,
        Vec::new(),
    )
    .expect("GetSplitTableKeys must produce real tablecodec keys");

    let record_prefix = keys[0].clone();
    let decoder = regionKeyDecoder {
        physicalTableID: 9,
        tablePrefix: record_prefix[..9].to_vec(),
        recordPrefix: record_prefix,
        indexPrefix: Vec::new(),
        indexID: 0,
        hasUnsignedIntHandle: false,
    };

    assert_eq!(decoder.decodeRegionKey(&keys[1]), "t_9_r_-5000");
}

/// 覆盖 Go `regionKeyDecoder.decodeRegionKey` 的其余编码分支：无符号句柄、
/// 非当前索引 ID、其它表 ID 与无法解码的短 key。
#[test]
fn region_key_decoder_matches_go_codec_decode_int_branches() {
    let decoder = regionKeyDecoder {
        physicalTableID: 9,
        tablePrefix: table_prefix(9),
        recordPrefix: record_prefix(9),
        indexPrefix: index_prefix(9, 1),
        indexID: 1,
        hasUnsignedIntHandle: false,
    };
    assert_eq!(decoder.decodeRegionKey(&record_key(9, -1)), "t_9_r_-1");

    let unsigned_decoder = regionKeyDecoder {
        hasUnsignedIntHandle: true,
        ..decoder
    };
    assert_eq!(
        unsigned_decoder.decodeRegionKey(&record_key(9, -1)),
        format!("t_9_r_{}", u64::MAX)
    );

    let mut other_index = index_prefix(9, 2);
    other_index.push(0xab);
    assert_eq!(unsigned_decoder.decodeRegionKey(&other_index), "t_9_i_2_ab");

    let mut other_table = table_prefix(10);
    other_table.extend_from_slice(b"_r");
    assert_eq!(unsigned_decoder.decodeRegionKey(&other_table), "t_10_5f72");
    assert_eq!(unsigned_decoder.decodeRegionKey(b"t\x01\x02"), "t_0102");
    assert_eq!(unsigned_decoder.decodeRegionKey(&[0xab]), "ab");
}

/// 对应 Go `MinRegionStepValue`/`TestStepShouldLargeThanMinStep` 的错误路径：
/// 分裂步长小于 `MinRegionStepValue` 时返回的错误信息需要与 Go 完全一致。
#[test]
fn get_split_table_keys_rejects_step_below_min_region_step() {
    let _serial = lock_min_region_step();
    let previous = MinRegionStepValue.swap(1000, std::sync::atomic::Ordering::AcqRel);
    let _restore = RestoreMinRegionStep(previous);
    let table = table_for_split(false);
    let statement_context = StatementContext::default();
    let error = GetSplitTableKeys(
        &statement_context,
        &table,
        &intHandleCols,
        9,
        &[RsDatum::Int(0)],
        &[RsDatum::Int(1000)],
        10,
        Vec::new(),
    )
    .expect_err("a step of 100 must be rejected when MinRegionStepValue is 1000");

    assert_eq!(
        error,
        SplitError::InvalidRanges(
            "the region size is too small, expected at least 1000, but got 100".to_owned()
        )
    );
}

/// 对应 Go `split table t_regions index idx between (-1000) and (1000) regions 4`：
/// 校验索引分裂点插值（`GetSplitIdxPhysicalStartAndOtherIdxKeys` + 区间插值）
/// 同样对齐 Go 的数值语义：4 个 region 需要 4 个 key（起始边界 + 3 个分裂点）。
#[test]
fn get_split_index_keys_matches_go_expected_bound_split_point_count() {
    let _serial = lock_min_region_step();
    let table = table_for_split(true);
    let index = IndexInfo {
        id: 1,
        name: "idx".to_owned(),
    };
    let statement_context = StatementContext::default();
    let keys = GetSplitIndexKeys(
        &statement_context,
        &table,
        &index,
        9,
        &[RsDatum::Int(-1000)],
        &[RsDatum::Int(1000)],
        4,
        Vec::new(),
    )
    .expect("GetSplitIndexKeys must succeed for a valid bound range");
    assert_eq!(keys.len(), 4);
}

/// `commonHandleCols`/`is_common_handle` 路径：非整数句柄表用
/// `SplitHandleCols::BuildHandleByDatums` 编码 lower/upper，
/// 对应 Go 聚簇索引（`primary key(a, b)`）表的分裂路径。
#[test]
fn get_split_table_keys_supports_common_handle_tables() {
    let _serial = lock_min_region_step();
    let mut table = table_for_split(false);
    table.is_common_handle = true;
    let statement_context = StatementContext::default();
    let keys = GetSplitTableKeys(
        &statement_context,
        &table,
        &commonHandleCols,
        9,
        &[RsDatum::Int(1), RsDatum::Int(0)],
        &[RsDatum::Int(2), RsDatum::Int(3)],
        2,
        Vec::new(),
    )
    .expect("GetSplitTableKeys must succeed for a valid common-handle bound range");
    // `regions = 2` 只需要 1 个内部分裂点，且没有额外的 record_prefix
    // （`is_common_handle && indices.len()==1` 的分支在这里 indices 为空，走
    // 一般的 `contains_index=false` 路径）。
    assert_eq!(keys.len(), 1);
}

/// Go `GetSplitTableKeys` 对 common handle 且只有一个索引的聚簇表不插入
/// 独立 record-prefix 边界；该索引本身就是聚簇主键的键空间。
#[test]
fn split_table_common_handle_single_index_omits_record_prefix() {
    let runtime = MockPdRuntime::new(30, "t").with_index(1, "primary");
    let mut executor = SplitTableRegionExec {
        runtime,
        partitionNames: Vec::new(),
        lower: vec![1, 0],
        upper: vec![2, 3],
        num: 2,
        handleCols: MockHandleCols::Common,
        valueLists: Vec::new(),
        splitKeys: Vec::new(),
        done: false,
        splitRegionResult: splitRegionResult::default(),
    };

    let mut context = ();
    executor.Open(&mut context).expect("Open must succeed");

    assert_eq!(
        executor.splitKeys.len(),
        1,
        "a single clustered index must not add a record-prefix boundary"
    );
}

// ---------------------------------------------------------------------------
// 2. `astersql-executor::split` 执行器 + region 去重/解码逻辑的单测。
// ---------------------------------------------------------------------------

/// Go `codec.EncodeInt` 的可比较 int64 编码：翻转符号位后大端写出。
fn encode_i64_comparable(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes()
}

/// 表前缀：`t` + 8 字节表 ID，对应 tablecodec 的 table prefix。
fn table_prefix(id: i64) -> Vec<u8> {
    let mut key = vec![b't'];
    key.extend_from_slice(&encode_i64_comparable(id));
    key
}

/// 记录前缀：`t_<id>_r`，标记该表行数据（row/handle）key 区间起点。
fn record_prefix(id: i64) -> Vec<u8> {
    let mut key = table_prefix(id);
    key.extend_from_slice(b"_r");
    key
}

/// 索引前缀：`t_<id>_i_<index_id>`，标记二级索引 key 区间起点。
fn index_prefix(id: i64, index_id: i64) -> Vec<u8> {
    let mut key = table_prefix(id);
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&encode_i64_comparable(index_id));
    key
}

/// 完整记录 key：`record_prefix` + handle（主键句柄）。
fn record_key(id: i64, handle: i64) -> Vec<u8> {
    let mut key = record_prefix(id);
    key.extend_from_slice(&encode_i64_comparable(handle));
    key
}

/// 完整索引 key：`index_prefix` + 索引列编码值。
fn index_key(id: i64, index_id: i64, value: i64) -> Vec<u8> {
    let mut key = index_prefix(id, index_id);
    key.extend_from_slice(&encode_i64_comparable(value));
    key
}

/// 对应 Go `kv.Key.PrefixNext()`：给定前缀的下一个最小 key（末字节 +1）。
/// 用来把"记录区间"精确限制为 `[record_prefix, record_prefix.PrefixNext())`，
/// 不包含该表的索引 key 区间（`_i` 在 `_r` 之前），与真实 tablecodec 的
/// `tablecodec.GetTableHandleKeyRange` 语义一致。
fn prefix_next(prefix: &[u8]) -> Vec<u8> {
    let mut next = prefix.to_vec();
    *next.last_mut().expect("prefix must be non-empty") += 1;
    next
}

/// 真实 tablecodec key 使用翻转符号位的可比较整数编码。为了让 mock PD 按数值
/// 语义维护 region 插入位置/排序，这里使用一个只在 mock PD 内部使用的
/// "逻辑 key"表示（表/索引 id +
/// 语义分类 + 数值），按真实语义的数值大小比较，只在跨越
/// `SplitRuntime` 的 `Vec<u8>` 边界时才与本文件的字节编码互相转换，
/// 与真实 PD 维护 region 边界、但数值语义仍需符合插值算法的效果一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogicalKey {
    TablePrefix(i64),
    IndexPrefix(i64, i64),
    Index(i64, i64, i64),
    RecordPrefix(i64),
    Record(i64, i64),
    /// 记录区间的排他上界（`record_prefix.PrefixNext()`），只作为查询边界使用，
    /// 从不会真正成为某个 region 的起始 key。
    RecordPrefixEnd(i64),
}

impl LogicalKey {
    /// 逻辑排序键：(表 ID, 类别, 索引 ID, 数值)，保证记录/索引区间相对顺序正确。
    fn sort_tuple(&self) -> (i64, i64, i64, i64) {
        match *self {
            Self::TablePrefix(id) => (id, 0, 0, i64::MIN),
            Self::IndexPrefix(id, index_id) => (id, 1, index_id, i64::MIN),
            Self::Index(id, index_id, value) => (id, 1, index_id, value),
            Self::RecordPrefix(id) => (id, 2, 0, i64::MIN),
            Self::Record(id, value) => (id, 2, 0, value),
            Self::RecordPrefixEnd(id) => (id, 3, 0, i64::MIN),
        }
    }

    /// 转回本文件约定的字节编码，供 `RegionDescriptor` 与执行器消费。
    fn to_bytes(&self) -> Vec<u8> {
        match *self {
            Self::TablePrefix(id) => table_prefix(id),
            Self::IndexPrefix(id, index_id) => index_prefix(id, index_id),
            Self::Index(id, index_id, value) => index_key(id, index_id, value),
            Self::RecordPrefix(id) => record_prefix(id),
            Self::Record(id, value) => record_key(id, value),
            Self::RecordPrefixEnd(id) => prefix_next(&record_prefix(id)),
        }
    }

    /// 从 mock 字节 key 还原逻辑 key；布局须与 `to_bytes` 互逆。
    fn decode(key: &[u8]) -> Self {
        assert_eq!(key.first(), Some(&b't'), "mock keys always start with 't'");
        let id = unflip_i64(&key[1..9]);
        if key.len() == 9 {
            return Self::TablePrefix(id);
        }
        match &key[9..11] {
            b"_r" if key.len() == 11 => Self::RecordPrefix(id),
            b"_r" if key.len() == 19 => Self::Record(id, unflip_i64(&key[11..19])),
            b"_s" if key.len() == 11 => Self::RecordPrefixEnd(id),
            b"_i" => {
                let index_id = unflip_i64(&key[11..19]);
                if key.len() == 19 {
                    Self::IndexPrefix(id, index_id)
                } else {
                    Self::Index(id, index_id, unflip_i64(&key[19..27]))
                }
            }
            other => panic!("unrecognized mock key layout: {other:?} (key={key:?})"),
        }
    }
}

/// 内存中的单个 Region：半开区间 `[start, end)`，`None` 表示 ±∞。
#[derive(Clone, Debug)]
struct MockRegion {
    id: u64,
    /// `None` 表示 -inf（keyspace 的全局起点）。
    start: Option<LogicalKey>,
    /// `None` 表示 +inf（keyspace 的全局终点）。
    end: Option<LogicalKey>,
}

/// 判断 region 与查询区间 `[start, end)` 是否相交（两端均可为 ±∞）。
fn region_overlaps(
    region: &MockRegion,
    start: Option<LogicalKey>,
    end: Option<LogicalKey>,
) -> bool {
    let ends_after_start = match (region.end, start) {
        (None, _) | (_, None) => true,
        (Some(region_end), Some(query_start)) => query_start.sort_tuple() < region_end.sort_tuple(),
    };
    let starts_before_end = match (region.start, end) {
        (None, _) | (_, None) => true,
        (Some(region_start), Some(query_end)) => region_start.sort_tuple() < query_end.sort_tuple(),
    };
    ends_after_start && starts_before_end
}

/// 句柄列类型：有符号整型 / 无符号整型 / 公共句柄（聚簇索引多列主键）。
#[derive(Clone, Copy)]
enum MockHandleCols {
    Int,
    Unsigned,
    Common,
}

/// 对应 Go 里由真实 PD/TiKV 承担的 region 元数据与分裂能力：内存中维护一份
/// 按 key 排序、覆盖整个 keyspace 的 region 列表；`split_regions` 在给定 key
/// 处把覆盖该 key 的 region 一分为二（与真实 PD `SplitRegion` 语义一致）。
/// key 编码/prefix 计算是本文件按 tablecodec 记录/索引布局写的最小实现，见
/// 文件头注释；`SplitRuntime` 里其余的错误上报/统计等方法保持最小真实实现。
struct MockPdRuntime {
    table_id: i64,
    table_name: String,
    partitions: Vec<(String, i64)>,
    index_ids: Vec<(i64, String)>,
    unsigned_handle: bool,
    current_index_id: i64,
    current_index_name: String,
    regions: Vec<MockRegion>,
    next_region_id: u64,
    split_calls: Vec<(i64, usize)>,
}

impl MockPdRuntime {
    /// 创建覆盖整个 keyspace 的初始 region，并为该表做一次前缀预分裂。
    fn new(table_id: i64, table_name: &str) -> Self {
        let mut runtime = Self {
            table_id,
            table_name: table_name.to_owned(),
            partitions: Vec::new(),
            index_ids: Vec::new(),
            unsigned_handle: false,
            current_index_id: 0,
            current_index_name: String::new(),
            regions: vec![MockRegion {
                id: 1,
                start: None,
                end: None,
            }],
            next_region_id: 2,
            split_calls: Vec::new(),
        };
        runtime.register_table(table_id);
        runtime
    }

    /// 复现 Go `ddl.EnableSplitTableRegion` 打开时建表自动预留独立 region 的效果。
    fn register_table(&mut self, physical_id: i64) {
        self.split_at(vec![
            LogicalKey::TablePrefix(physical_id),
            LogicalKey::TablePrefix(physical_id + 1),
        ]);
    }

    /// 注册二级索引元数据，供索引分裂与 `public_index_ids` 使用。
    fn with_index(mut self, index_id: i64, name: &str) -> Self {
        self.index_ids.push((index_id, name.to_owned()));
        self
    }

    /// 注册分区名与物理表 ID，并对每个分区物理 ID 执行预分裂。
    fn with_partitions(mut self, partitions: Vec<(&str, i64)>) -> Self {
        for (name, id) in partitions {
            self.partitions.push((name.to_owned(), id));
            self.register_table(id);
        }
        self
    }

    /// 在给定逻辑 key 处分裂覆盖该 key 的 region；已存在的起点会被跳过。
    fn split_at(&mut self, mut keys: Vec<LogicalKey>) -> Vec<u64> {
        keys.sort_by_key(|key| key.sort_tuple());
        keys.dedup();
        let mut created = Vec::new();
        for key in keys {
            if self.regions.iter().any(|region| region.start == Some(key)) {
                continue;
            }
            if let Some(position) = self.regions.iter().position(|region| {
                let after_start = region
                    .start
                    .is_none_or(|start| start.sort_tuple() <= key.sort_tuple());
                let before_end = region
                    .end
                    .is_none_or(|end| key.sort_tuple() < end.sort_tuple());
                after_start && before_end
            }) {
                let end = self.regions[position].end;
                self.regions[position].end = Some(key);
                let id = self.next_region_id;
                self.next_region_id += 1;
                self.regions.insert(
                    position + 1,
                    MockRegion {
                        id,
                        start: Some(key),
                        end,
                    },
                );
                created.push(id);
            }
        }
        created
    }
}

impl SplitRuntime for MockPdRuntime {
    type Context = ();
    type Chunk = splitRegionResult;
    type Datum = i64;
    type HandleColumns = MockHandleCols;
    type Error = String;

    fn reset_chunk(&mut self, chunk: &mut Self::Chunk) {
        *chunk = splitRegionResult::default();
    }

    fn append_int64(&mut self, chunk: &mut Self::Chunk, column: usize, value: i64) {
        if column == 0 {
            chunk.splitRegions = value as i32;
        }
    }

    fn append_float64(&mut self, chunk: &mut Self::Chunk, column: usize, value: f64) {
        if column == 1 {
            chunk.finishScatterNum = (value * chunk.splitRegions as f64).round() as i32;
        }
    }

    fn partition_ids(&self) -> Vec<(String, i64)> {
        self.partitions.clone()
    }

    fn table_id(&self) -> i64 {
        self.table_id
    }

    fn table_name(&self) -> String {
        self.table_name.clone()
    }

    fn index_id(&self) -> i64 {
        self.current_index_id
    }

    fn index_name(&self) -> String {
        self.current_index_name.clone()
    }

    fn public_index_ids(&self) -> Vec<i64> {
        self.index_ids.iter().map(|(id, _)| *id).collect()
    }

    fn unsigned_int_handle(&self) -> bool {
        self.unsigned_handle
    }

    fn index_start_and_boundary_keys(
        &mut self,
        physical_id: i64,
        mut keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error> {
        let index_id = self.current_index_id;
        let is_first_index = self
            .index_ids
            .first()
            .is_none_or(|(first_id, _)| *first_id == index_id);
        if !is_first_index {
            keys.push(index_prefix(physical_id, index_id));
        }
        keys.push(index_prefix(physical_id, index_id + 1));
        Ok(keys)
    }

    fn encode_index_value_key(
        &mut self,
        physical_id: i64,
        values: &[Self::Datum],
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(index_key(physical_id, self.current_index_id, values[0]))
    }

    fn split_index_bound_keys(
        &mut self,
        physical_id: i64,
        lower: &[Self::Datum],
        upper: &[Self::Datum],
        number: i32,
        mut keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error> {
        keys = self.index_start_and_boundary_keys(physical_id, keys)?;
        let step = (upper[0] - lower[0]) / number as i64;
        let mut current = lower[0];
        for _ in 1..number {
            current += step;
            keys.push(index_key(physical_id, self.current_index_id, current));
        }
        Ok(keys)
    }

    fn encode_table_value_key(
        &mut self,
        physical_id: i64,
        _handle_columns: &Self::HandleColumns,
        values: &[Self::Datum],
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(record_key(physical_id, values[0]))
    }

    fn split_table_bound_keys(
        &mut self,
        physical_id: i64,
        handle_columns: &Self::HandleColumns,
        lower: &[Self::Datum],
        upper: &[Self::Datum],
        number: i32,
        mut keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error> {
        let has_record_region_boundary = !self.index_ids.is_empty()
            && !(matches!(handle_columns, MockHandleCols::Common) && self.index_ids.len() == 1);
        if has_record_region_boundary {
            keys.push(record_prefix(physical_id));
        }
        let step = (upper[0] - lower[0]) / number as i64;
        let mut current = lower[0];
        for _ in 1..number {
            current += step;
            keys.push(record_key(physical_id, current));
        }
        Ok(keys)
    }

    fn split_regions(
        &mut self,
        _context: &mut Self::Context,
        keys: Vec<Vec<u8>>,
        table_id: i64,
    ) -> Result<Vec<u64>, Self::Error> {
        self.split_calls.push((table_id, keys.len()));
        let logical_keys = keys.iter().map(|key| LogicalKey::decode(key)).collect();
        Ok(self.split_at(logical_keys))
    }

    fn wait_split_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn wait_split_region_finish(&self) -> bool {
        true
    }

    fn context_done(&self, _context: &Self::Context) -> bool {
        false
    }

    fn wait_scatter_region_finish(
        &mut self,
        _context: &mut Self::Context,
        _region_id: u64,
        _timeout_milliseconds: i32,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn warn_split_failed(&self, _table: &str, _index: Option<&str>, _error: &Self::Error) {}

    fn warn_scatter_failed(
        &self,
        _region_id: u64,
        _table: &str,
        _index: Option<&str>,
        _error: &Self::Error,
    ) {
    }

    fn table_handle_key_range(&self, physical_id: i64) -> (Vec<u8>, Vec<u8>) {
        let record_prefix = record_prefix(physical_id);
        let end = prefix_next(&record_prefix);
        (record_prefix, end)
    }

    fn table_index_key_range(&self, physical_id: i64, index_id: i64) -> (Vec<u8>, Vec<u8>) {
        (
            index_prefix(physical_id, index_id),
            index_prefix(physical_id, index_id + 1),
        )
    }

    fn load_regions(
        &mut self,
        start: Vec<u8>,
        end: Vec<u8>,
    ) -> Result<Vec<RegionDescriptor>, Self::Error> {
        let query_start = if start.is_empty() {
            None
        } else {
            Some(LogicalKey::decode(&start))
        };
        let query_end = if end.is_empty() {
            None
        } else {
            Some(LogicalKey::decode(&end))
        };
        let mut regions: Vec<&MockRegion> = self
            .regions
            .iter()
            .filter(|region| region_overlaps(region, query_start, query_end))
            .collect();
        regions.sort_by_key(|region| {
            region
                .start
                .map(|key| key.sort_tuple())
                .unwrap_or((i64::MIN, 0, 0, 0))
        });
        Ok(regions
            .into_iter()
            .map(|region| RegionDescriptor {
                id: region.id,
                leader_id: region.id,
                store_id: 1,
                start_key: region.start.map(|key| key.to_bytes()).unwrap_or_default(),
                end_key: region.end.map(|key| key.to_bytes()).unwrap_or_default(),
            })
            .collect())
    }

    fn region_is_scattering(&mut self, _region_id: u64) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn region_statistics(
        &mut self,
        _region_id: u64,
    ) -> Result<Option<RegionStatistics>, Self::Error> {
        Ok(None)
    }

    fn table_prefix(&self, physical_id: i64) -> Vec<u8> {
        table_prefix(physical_id)
    }

    fn record_prefix(&self, physical_id: i64) -> Vec<u8> {
        record_prefix(physical_id)
    }

    fn index_prefix(&self, physical_id: i64, index_id: i64) -> Vec<u8> {
        index_prefix(physical_id, index_id)
    }
}

/// 对应 Go `TestShowTableRegion` 里核心的
/// `split table t_regions between (-10000) and (10000) regions 4` +
/// `show table t_regions regions` 断言：表带两个索引（idx/idx2），4 个记录
/// region + 1 个共享（未分裂）索引 region（`idx`/`idx2` 的 key 区间在分裂前
/// 属于同一个 region，`getPhysicalTableRegions` 靠 `unique_region_ids` 去重后
/// 只出现一次），与 Go 断言的 5 行、每行字符串完全一致。
#[test]
fn split_table_region_between_bounds_matches_go_expected_regions() {
    let runtime = MockPdRuntime::new(9, "t_regions")
        .with_index(1, "idx")
        .with_index(2, "idx2");
    let mut executor = SplitTableRegionExec {
        runtime,
        partitionNames: Vec::new(),
        lower: vec![-10000],
        upper: vec![10000],
        num: 4,
        handleCols: MockHandleCols::Int,
        valueLists: Vec::new(),
        splitKeys: Vec::new(),
        done: false,
        splitRegionResult: splitRegionResult::default(),
    };

    let mut context = ();
    executor.Open(&mut context).expect("Open must succeed");
    let mut chunk = splitRegionResult::default();
    executor
        .Next(&mut context, &mut chunk)
        .expect("Next must succeed");

    // 对应 Go `Check(testkit.Rows("4 1"))`：4 个新 region，全部 scatter 完成。
    assert_eq!(chunk.splitRegions, 4);
    assert_eq!(chunk.finishScatterNum, 4);

    let mut unique_region_ids = HashSet::new();
    let regions = getPhysicalTableRegions(&mut executor.runtime, 9, &mut unique_region_ids)
        .expect("getPhysicalTableRegions must succeed");

    assert_eq!(
        regions.len(),
        5,
        "4 record regions + 1 deduplicated shared index region"
    );
    assert_eq!(regions[0].start, "t_9_r");
    assert_eq!(regions[1].start, "t_9_r_-5000");
    assert_eq!(regions[2].start, "t_9_r_0");
    assert_eq!(regions[3].start, "t_9_r_5000");
    assert_eq!(regions[4].end, "t_9_r");
    assert!(regions[4].start.starts_with("t_9_"));
}

/// 对应 Go 无符号主键的分裂场景：
/// `split table t_regions by (2500),(5000),(7500)`（值列表分裂，非区间分裂），
/// 断言 `Check(testkit.Rows("3 1"))` 且 `show table regions` 返回 4 行，
/// 中间三行精确等于 `t_<id>_r_2500`/`_5000`/`_7500`。
#[test]
fn split_table_region_by_value_list_matches_go_expected_regions() {
    let mut runtime = MockPdRuntime::new(20, "t_regions").with_index(1, "idx");
    runtime.unsigned_handle = true;
    let mut executor = SplitTableRegionExec {
        runtime,
        partitionNames: Vec::new(),
        lower: Vec::new(),
        upper: Vec::new(),
        num: 0,
        handleCols: MockHandleCols::Unsigned,
        valueLists: vec![vec![2500], vec![5000], vec![7500]],
        splitKeys: Vec::new(),
        done: false,
        splitRegionResult: splitRegionResult::default(),
    };

    let mut context = ();
    executor.Open(&mut context).expect("Open must succeed");
    let mut chunk = splitRegionResult::default();
    executor
        .Next(&mut context, &mut chunk)
        .expect("Next must succeed");

    assert_eq!(chunk.splitRegions, 3);
    assert_eq!(chunk.finishScatterNum, 3);

    let mut unique_region_ids = HashSet::new();
    let regions = getPhysicalTableRegions(&mut executor.runtime, 20, &mut unique_region_ids)
        .expect("getPhysicalTableRegions must succeed");

    // 值列表分裂不会像区间分裂那样额外插入 record_prefix 边界，因此紧邻记录区间
    // 前的一段（表前缀到 2500 之间，覆盖了索引 idx 的整个 key 区间）在记录范围
    // 查询里就已经被计入，索引区间查询命中的是同一个 region，被去重，不产生
    // 新行，故总数与 Go `require.Len(t, rows, 4)` 一致。
    assert_eq!(regions.len(), 4);
    assert_eq!(regions[1].start, "t_20_r_2500");
    assert_eq!(regions[2].start, "t_20_r_5000");
    assert_eq!(regions[3].start, "t_20_r_7500");
}

/// 对应 Go `split table t_regions index idx between (-1000) and (1000) regions 4`
/// + `show table t_regions index idx regions`：4 行，除第一行外都以
/// `t_<id>_i_1_` 为前缀（Go 用 `require.Regexp` 只校验前缀，不校验具体数值）。
#[test]
fn split_index_region_between_bounds_matches_go_expected_regions() {
    let mut runtime = MockPdRuntime::new(9, "t_regions").with_index(1, "idx");
    runtime.current_index_id = 1;
    runtime.current_index_name = "idx".to_owned();
    let mut executor = SplitIndexRegionExec {
        runtime,
        partitionNames: Vec::new(),
        lower: vec![-1000],
        upper: vec![1000],
        num: 4,
        valueLists: Vec::new(),
        splitIdxKeys: Vec::new(),
        done: false,
        splitRegionResult: splitRegionResult::default(),
    };

    let mut context = ();
    executor.Open(&mut context).expect("Open must succeed");
    let mut chunk = splitRegionResult::default();
    executor
        .Next(&mut context, &mut chunk)
        .expect("Next must succeed");

    assert_eq!(chunk.splitRegions, 4);
    assert_eq!(chunk.finishScatterNum, 4);

    let mut unique_region_ids = HashSet::new();
    let (start, end) = executor.runtime.table_index_key_range(9, 1);
    let descriptors = executor
        .runtime
        .load_regions(start, end)
        .expect("load_regions must succeed");
    let regions = astersql_executor::split::getRegionMeta(
        &mut executor.runtime,
        descriptors,
        &mut unique_region_ids,
        9,
        1,
        false,
    )
    .expect("getRegionMeta must succeed");
    assert_eq!(regions.len(), 4);
    assert!(regions[0].start.starts_with("t_9_"));
    for region in &regions[1..] {
        assert!(
            region.start.starts_with("t_9_i_1_"),
            "expected t_9_i_1_ prefix, got {}",
            region.start
        );
    }
}

/// 对应 Go 分区表分裂时按 `partition (pX)` 过滤物理 ID 的语义
/// （`selectedPhysicalIDs`）：只对被选中的分区物理 ID 生成分裂 key 并调用
/// `split_regions`，未选中的分区完全不受影响。
#[test]
fn split_table_region_respects_selected_partition_names() {
    let runtime =
        MockPdRuntime::new(100, "t").with_partitions(vec![("p0", 101), ("p1", 102), ("p2", 103)]);
    let mut executor = SplitTableRegionExec {
        runtime,
        partitionNames: vec!["p1".to_owned()],
        lower: vec![0],
        upper: vec![4_000_000],
        num: 4,
        handleCols: MockHandleCols::Int,
        valueLists: Vec::new(),
        splitKeys: Vec::new(),
        done: false,
        splitRegionResult: splitRegionResult::default(),
    };

    let mut context = ();
    executor.Open(&mut context).expect("Open must succeed");
    let mut chunk = splitRegionResult::default();
    executor
        .Next(&mut context, &mut chunk)
        .expect("Next must succeed");

    // 只选中了 p1（physical id 102），region_between(0,4000000,4) 产生 3 个
    // 分裂点（表没有索引，不额外插入 record_prefix 边界）。
    assert_eq!(chunk.splitRegions, 3);
    assert_eq!(executor.runtime.split_calls, vec![(100, 3)]);

    let mut unique_region_ids = HashSet::new();
    let p0_regions = getPhysicalTableRegions(&mut executor.runtime, 101, &mut unique_region_ids)
        .expect("p0 must still be queryable");
    assert_eq!(p0_regions.len(), 1, "p0 was never split");

    let mut unique_region_ids = HashSet::new();
    let p1_regions = getPhysicalTableRegions(&mut executor.runtime, 102, &mut unique_region_ids)
        .expect("p1 must reflect the new split points");
    assert_eq!(p1_regions.len(), 4, "p1 was split into 4 regions");
    assert_eq!(p1_regions[1].start, "t_102_r_1000000");
    assert_eq!(p1_regions[2].start, "t_102_r_2000000");
    assert_eq!(p1_regions[3].start, "t_102_r_3000000");
}
