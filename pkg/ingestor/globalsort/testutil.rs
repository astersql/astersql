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

// 全局排序测试辅助：构造 `MultipleFilesStat` 与读回比对。
//
// `testReadAndCompare` 用 `RangeSplitter` 切分后调用 `read_all_data`，
// 将加载结果排序后与期望 KV 逐条比较，是端到端往返的共用断言。

use crate::reader::{CancellationToken, MemKvsAndBuffers, read_all_data};
use crate::split::NewRangeSplitter;
use crate::{Error, FilePair, KvPair, MultipleFilesStat, Result, Storage, next_key};

/// 将等长的 data/stat 路径列表包装为单个 `MultipleFilesStat` 组。
pub fn mockOneMultiFileStat(data: &[String], stat: &[String]) -> Result<Vec<MultipleFilesStat>> {
    if data.len() != stat.len() {
        return Err(Error::InvalidArgument(
            "data and stat file counts differ".into(),
        ));
    }
    Ok(vec![MultipleFilesStat {
        filenames: data
            .iter()
            .zip(stat)
            .map(|(data_file, stat_file)| FilePair {
                data_file: data_file.clone(),
                stat_file: stat_file.clone(),
                properties: Vec::new(),
            })
            .collect(),
    }])
}

/// 经 RangeSplitter + read_all_data 读回全部 KV，须与 `kvs` 完全一致。
pub fn testReadAndCompare(
    token: &CancellationToken,
    kvs: &[KvPair],
    store: &dyn Storage,
    data_files: &[String],
    stat_files: &[String],
    start_key: Vec<u8>,
    memory_size_limit: usize,
) -> Result<()> {
    // Go 辅助函数以最后一条 KV 计算最终上界，因此空期望值不是有效调用。
    // 显式报错保留该前置条件，同时避免 Rust 版本静默跳过文件与内容校验。
    if kvs.is_empty() {
        return Err(Error::InvalidArgument(
            "expected KVs must not be empty".into(),
        ));
    }
    let stats = mockOneMultiFileStat(data_files, stat_files)?;
    let mut splitter = NewRangeSplitter(
        &stats,
        store,
        memory_size_limit as i64,
        i64::MAX,
        4_i64 * 1024 * 1024 * 1024,
        i64::MAX,
        i64::MAX,
        i64::MAX,
    )?;
    let mut current_start = start_key;
    let mut actual = Vec::new();
    // 逐个 ranges group 读取 [current_start, current_end)，拼接后与期望比对。
    loop {
        let group = splitter.SplitOneRangesGroup()?;
        let current_end = if group.end_key_of_group.is_empty() {
            next_key(&kvs.last().unwrap().key)
        } else {
            group.end_key_of_group.clone()
        };
        let starts = vec![0; group.data_files.len()];
        let ends = group
            .data_files
            .iter()
            .map(|file| store.read(file).map(|data| data.len() as u64))
            .collect::<Result<Vec<_>>>()?;
        let mut loaded = MemKvsAndBuffers::default();
        read_all_data(
            token,
            store,
            &group.data_files,
            &group.stat_files,
            &current_start,
            &current_end,
            &starts,
            &ends,
            memory_size_limit,
            &mut loaded,
        )?;
        loaded.build();
        loaded.kvs.sort_by(|left, right| left.key.cmp(&right.key));
        actual.extend(loaded.kvs);
        current_start = current_end;
        if group.end_key_of_group.is_empty() {
            break;
        }
    }
    splitter.Close()?;
    // 行数或内容不一致时返回 InvalidData，便于测试定位。
    if actual != kvs {
        return Err(Error::InvalidData(format!(
            "loaded KV mismatch: got {} rows, expected {}",
            actual.len(),
            kvs.len()
        )));
    }
    Ok(())
}
