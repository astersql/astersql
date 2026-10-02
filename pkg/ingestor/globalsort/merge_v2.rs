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

// 全局排序合并算子 V2：按键范围分组读入、内存排序后写出单一 data/stat 文件对。
//
// 与 V1 不同，V2 借助 `RangeSplitter`（按 RangeProperty 切分 key 范围）
// 迭代每个 ranges group，再调用 `read_all_data` 加载 `[start, end)` 窗口内的 KV，
// 最终写回外部存储（对象存储或本地文件抽象）。

use crate::merge::OnWriterClose;
use crate::merge::StreamSummary;
use crate::reader::{CancellationToken, MemKvsAndBuffers, read_all_data};
use crate::split::NewRangeSplitter;
use crate::{Error, MultipleFilesStat, Result, Storage};

/// 4GiB，用作 RangeSplitter 与读入缓冲的默认大容量上限。
const FOUR_GIB: i64 = 4 * 1024 * 1024 * 1024;

/// 合并重叠的多文件统计组：按范围切分、读入排序、写出合并结果。
///
/// `multi_file_stat` 描述输入 data/stat 文件对；`start_key`/`end_key` 为半开区间；
/// 成功返回写出的 data 文件路径，可通过 `on_writer_close` 回调上报 `WriterSummary`。
#[allow(clippy::too_many_arguments)]
pub fn MergeOverlappingFilesV2(
    token: &CancellationToken,
    multi_file_stat: &[MultipleFilesStat],
    store: &dyn Storage,
    start_key: &[u8],
    end_key: &[u8],
    _part_size: i64,
    new_file_prefix: &str,
    writer_id: &str,
    _block_size: usize,
    _write_batch_count: u64,
    property_size_distance: u64,
    property_keys_distance: u64,
    on_writer_close: Option<&OnWriterClose>,
    concurrency: usize,
    _check_hotspot: bool,
) -> Result<String> {
    // Go 仅把 concurrency 用于排序并发；writeBatchCount 在 V2 路径中不参与校验。
    if concurrency == 0 {
        return Err(Error::InvalidArgument(
            "merge concurrency must be positive".into(),
        ));
    }
    // 用 4GiB 等宽松阈值构造 RangeSplitter，按属性边界迭代 ranges group。
    let mut splitter = NewRangeSplitter(
        multi_file_stat,
        store,
        FOUR_GIB,
        i64::MAX,
        FOUR_GIB,
        i64::MAX,
        i64::MAX,
        i64::MAX,
    )?;
    let mut current_start = start_key.to_vec();
    let prefix = new_file_prefix.trim_end_matches('/');
    let data_file = format!("{prefix}/{writer_id}.data");
    let stat_file = format!("{prefix}/{writer_id}.stat");
    let mut writer = store.create(&data_file)?;
    let mut summary = StreamSummary::new(
        store.record_format(),
        property_size_distance,
        property_keys_distance,
    );
    // Each range window is sorted, streamed to the output, then released.
    loop {
        // 取消令牌（CancellationToken）已触发则中止，避免无意义的 I/O。
        if token.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let group = splitter.SplitOneRangesGroup()?;
        let current_end = if group.end_key_of_group.is_empty() {
            end_key.to_vec()
        } else {
            group.end_key_of_group.clone()
        };
        let offsets = crate::reader::get_read_ranges_from_props(
            token,
            store,
            &[current_start.clone(), current_end.clone()],
            &group.stat_files,
        )?;
        let start_offsets = &offsets[0];
        let end_offsets = &offsets[1];
        let mut loaded = MemKvsAndBuffers::default();
        read_all_data(
            token,
            store,
            &group.data_files,
            &group.stat_files,
            &current_start,
            &current_end,
            start_offsets,
            end_offsets,
            FOUR_GIB as usize,
            &mut loaded,
        )?;
        loaded.build();
        loaded.kvs.sort_by(|left, right| left.key.cmp(&right.key));
        // Release each bounded range window after writing it. Keeping prior
        // windows would turn the entire external merge into a table-sized Vec.
        for pair in &loaded.kvs {
            summary.write(writer.as_mut(), pair)?;
        }
        current_start = current_end;
        if group.end_key_of_group.is_empty() {
            break;
        }
    }
    splitter.Close()?;
    writer.finish()?;
    summary.write_stats(store, &stat_file)?;
    let summary = summary.finish(data_file.clone(), stat_file);
    if let Some(callback) = on_writer_close {
        callback(&summary);
    }
    Ok(data_file)
}
