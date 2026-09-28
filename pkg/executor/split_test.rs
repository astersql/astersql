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

// `split` 模块单元测试：校验 Region 键解码对 record/index/无符号 handle 的格式化。
//
// Region 是 TiKV 的键空间分片单位；此处解码结果用于 SHOW 类语句的可读展示。

use crate::split::{
    RegionDescriptor, RegionStatistics, SplitRuntime, regionKeyDecoder, selectedPhysicalIDs,
};
use astersql_util_codec::EncodeInt;
use std::time::Duration;

/// 覆盖行键、无符号 handle 与索引键三种解码路径。
#[test]
fn split_region_key_decoder_handles_record_index_and_unsigned_handles() {
    let decoder = regionKeyDecoder {
        physicalTableID: 9,
        tablePrefix: b"t".to_vec(),
        recordPrefix: b"tr".to_vec(),
        indexPrefix: b"ti".to_vec(),
        indexID: 3,
        hasUnsignedIntHandle: true,
    };
    // 行记录前缀 + 8 字节 handle：无符号时按 u64 展示
    let record_key = EncodeInt(vec![b't', b'r'], 7);
    assert_eq!(decoder.decodeRegionKey(&record_key), "t_9_r_7");
    // 索引前缀后的剩余字节以十六进制拼接
    assert_eq!(decoder.decodeRegionKey(b"tiab"), "t_9_i_3_6162");
}

struct PartitionRuntime;

impl SplitRuntime for PartitionRuntime {
    type Context = ();
    type Chunk = ();
    type Datum = ();
    type HandleColumns = ();
    type Error = String;

    fn reset_chunk(&mut self, _: &mut Self::Chunk) {}
    fn append_int64(&mut self, _: &mut Self::Chunk, _: usize, _: i64) {}
    fn append_float64(&mut self, _: &mut Self::Chunk, _: usize, _: f64) {}
    fn partition_ids(&self) -> Vec<(String, i64)> {
        vec![("p0".into(), 101), ("p1".into(), 102), ("p2".into(), 103)]
    }
    fn table_id(&self) -> i64 {
        100
    }
    fn table_name(&self) -> String {
        "t".into()
    }
    fn index_id(&self) -> i64 {
        0
    }
    fn index_name(&self) -> String {
        String::new()
    }
    fn public_index_ids(&self) -> Vec<i64> {
        Vec::new()
    }
    fn unsigned_int_handle(&self) -> bool {
        false
    }
    fn index_start_and_boundary_keys(
        &mut self,
        _: i64,
        _: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error> {
        unreachable!()
    }
    fn encode_index_value_key(
        &mut self,
        _: i64,
        _: &[Self::Datum],
    ) -> Result<Vec<u8>, Self::Error> {
        unreachable!()
    }
    fn split_index_bound_keys(
        &mut self,
        _: i64,
        _: &[Self::Datum],
        _: &[Self::Datum],
        _: i32,
        _: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error> {
        unreachable!()
    }
    fn encode_table_value_key(
        &mut self,
        _: i64,
        _: &Self::HandleColumns,
        _: &[Self::Datum],
    ) -> Result<Vec<u8>, Self::Error> {
        unreachable!()
    }
    fn split_table_bound_keys(
        &mut self,
        _: i64,
        _: &Self::HandleColumns,
        _: &[Self::Datum],
        _: &[Self::Datum],
        _: i32,
        _: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error> {
        unreachable!()
    }
    fn split_regions(
        &mut self,
        _: &mut Self::Context,
        _: Vec<Vec<u8>>,
        _: i64,
    ) -> Result<Vec<u64>, Self::Error> {
        unreachable!()
    }
    fn wait_split_timeout(&self) -> Duration {
        Duration::ZERO
    }
    fn wait_split_region_finish(&self) -> bool {
        false
    }
    fn context_done(&self, _: &Self::Context) -> bool {
        false
    }
    fn wait_scatter_region_finish(
        &mut self,
        _: &mut Self::Context,
        _: u64,
        _: i32,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }
    fn warn_split_failed(&self, _: &str, _: Option<&str>, _: &Self::Error) {}
    fn warn_scatter_failed(&self, _: u64, _: &str, _: Option<&str>, _: &Self::Error) {}
    fn table_handle_key_range(&self, _: i64) -> (Vec<u8>, Vec<u8>) {
        unreachable!()
    }
    fn table_index_key_range(&self, _: i64, _: i64) -> (Vec<u8>, Vec<u8>) {
        unreachable!()
    }
    fn load_regions(
        &mut self,
        _: Vec<u8>,
        _: Vec<u8>,
    ) -> Result<Vec<RegionDescriptor>, Self::Error> {
        unreachable!()
    }
    fn region_is_scattering(&mut self, _: u64) -> Result<bool, Self::Error> {
        unreachable!()
    }
    fn region_statistics(&mut self, _: u64) -> Result<Option<RegionStatistics>, Self::Error> {
        unreachable!()
    }
    fn table_prefix(&self, _: i64) -> Vec<u8> {
        unreachable!()
    }
    fn record_prefix(&self, _: i64) -> Vec<u8> {
        unreachable!()
    }
    fn index_prefix(&self, _: i64, _: i64) -> Vec<u8> {
        unreachable!()
    }
}

#[test]
fn selected_partitions_preserve_statement_order() {
    assert_eq!(
        selectedPhysicalIDs(&PartitionRuntime, &["p2".into(), "P0".into()]).unwrap(),
        vec![103, 101],
    );
}

#[test]
fn selected_partitions_reject_unknown_name() {
    assert_eq!(
        selectedPhysicalIDs(&PartitionRuntime, &["missing".into()]).unwrap_err(),
        "unknown partition 'missing' in table 't'",
    );
}
