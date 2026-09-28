// Copyright 2026 AsterSQL.

// simplesst 辅助工具：区间重叠、去重与统计偏移推算。
//
// 提供端点扫描求最大重叠、已排序输入去重、按统计属性估算 seek 偏移，
// 以及枚举分区/非分区目录下的对象路径。对应 Go `util.go`。

use crate::stat_reader::StatsReader;
use crate::writer::IsValidPartition;
use crate::{MemoryStorage, Result};

/// 区间端点类型；枚举序同时决定同 key 时的处理顺序（与 Go iota 一致）。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EndpointTp {
    /// 开区间右端 `..., key)`，同 key 时最先处理，保证该点不计入重叠。
    ExclusiveEnd = 0,
    /// 闭区间左端 `[key, ...`。
    InclusiveStart = 1,
    /// 闭区间右端 `..., key]`，同 key 时最后减去权重，保证该点仍计入重叠。
    InclusiveEnd = 2,
}
/// 带权重的区间端点，用于扫描线求最大重叠。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    /// 端点键。
    pub Key: Vec<u8>,
    /// 端点类型（开/闭）。
    pub Tp: EndpointTp,
    /// 正权重；起点加、终点减。
    pub Weight: i64,
}

/// 原地按 key、类型排序后扫描，返回任一点处最大累计权重。
pub fn get_max_overlapping(points: &mut [Endpoint]) -> i64 {
    // key 相同时按 ExclusiveEnd → InclusiveStart → InclusiveEnd 次序处理。
    points.sort_by(|a, b| a.Key.cmp(&b.Key).then(a.Tp.cmp(&b.Tp)));
    let mut current = 0;
    let mut maximum = 0;
    for point in points {
        match point.Tp {
            EndpointTp::InclusiveStart => current += point.Weight,
            EndpointTp::ExclusiveEnd | EndpointTp::InclusiveEnd => current -= point.Weight,
        }
        maximum = maximum.max(current);
    }
    maximum
}
/// Go 风格别名：等同 `get_max_overlapping`。
pub fn GetMaxOverlapping(points: &mut [Endpoint]) -> i64 {
    get_max_overlapping(points)
}

/// Removes every member of a duplicate group from sorted input. When
/// `record_removed` is true the removed values retain their original order.
///
/// 从已排序输入移除整组重复键（keep=0）；`record_removed` 为 true 时保留被删元素顺序。
pub fn remove_duplicates<T: Clone, F>(
    input: &mut Vec<T>,
    key: F,
    record_removed: bool,
) -> (Vec<T>, Vec<T>, usize)
where
    F: Fn(&T) -> &[u8],
{
    remove_duplicates_with_keep(input, key, 0, record_removed)
}
/// 每个重复组保留前两个元素，其余记入 removed；第三返回值为重复组元素总数。
pub fn remove_duplicates_more_than_two<T: Clone, F>(
    input: &mut Vec<T>,
    key: F,
) -> (Vec<T>, Vec<T>, usize)
where
    F: Fn(&T) -> &[u8],
{
    remove_duplicates_with_keep(input, key, 2, true)
}
/// 去重核心：扫描相邻同 key 组；`keep` 为 0 或 2。
fn remove_duplicates_with_keep<T: Clone, F>(
    input: &mut Vec<T>,
    key: F,
    keep: usize,
    record: bool,
) -> (Vec<T>, Vec<T>, usize)
where
    F: Fn(&T) -> &[u8],
{
    let mut output = Vec::with_capacity(input.len());
    let mut removed = Vec::new();
    let mut duplicates = 0;
    let mut cursor = 0;
    while cursor < input.len() {
        // 扩展到同 key 连续区间的右边界。
        let mut end = cursor + 1;
        while end < input.len() && key(&input[end]) == key(&input[cursor]) {
            end += 1;
        }
        let count = end - cursor;
        if count >= 2 {
            duplicates += count;
            output.extend(input[cursor..end].iter().take(keep).cloned());
            if record {
                removed.extend(input[cursor + keep.min(count)..end].iter().cloned());
            }
        } else {
            output.push(input[cursor].clone());
        }
        cursor = end;
    }
    (output, removed, duplicates)
}

/// 扫描各统计文件，为升序 job key 计算可安全 seek 的最大数据偏移；结果按 `[job_key][path]`。
pub fn get_read_range_from_props(
    job_keys: &[Vec<u8>],
    paths: &[String],
    storage: &MemoryStorage,
) -> Result<Vec<Vec<u64>>> {
    get_read_range_from_props_with_limit(job_keys, paths, storage, 64)
}

/// 与 `get_read_range_from_props` 相同，但允许测试收紧同时打开的统计文件数。
pub fn get_read_range_from_props_with_limit(
    job_keys: &[Vec<u8>],
    paths: &[String],
    storage: &MemoryStorage,
    concurrency: usize,
) -> Result<Vec<Vec<u64>>> {
    if job_keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut result = vec![vec![0; paths.len()]; job_keys.len()];
    let concurrency = concurrency.max(1);
    for (batch_index, batch) in paths.chunks(concurrency).enumerate() {
        let batch_result = std::thread::scope(|scope| {
            let handles = batch
                .iter()
                .map(|path| scope.spawn(move || read_offsets_for_path(job_keys, path, storage)))
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().map_err(|_| {
                        crate::Error::InvalidData("stats reader worker panicked".into())
                    })?
                })
                .collect::<Result<Vec<_>>>()
        })?;
        for (offset_in_batch, offsets) in batch_result.into_iter().enumerate() {
            let path_index = batch_index * concurrency + offset_in_batch;
            for (key_index, offset) in offsets.into_iter().enumerate() {
                result[key_index][path_index] = offset;
            }
        }
    }
    Ok(result)
}

fn read_offsets_for_path(
    job_keys: &[Vec<u8>],
    path: &str,
    storage: &MemoryStorage,
) -> Result<Vec<u64>> {
    let mut reader = match StatsReader::from_storage(storage, path, 250 * 1024) {
        Ok(reader) => reader,
        Err(error) if error.is_eof() => return Ok(vec![0; job_keys.len()]),
        Err(error) => return Err(error),
    };
    let mut offsets = vec![0; job_keys.len()];
    let mut key_index = 0;
    loop {
        let property = match reader.next_prop() {
            Ok(property) => property,
            Err(error) if error.is_eof() => {
                let offset = offsets[key_index];
                for remaining in offsets.iter_mut().skip(key_index + 1) {
                    *remaining = offset;
                }
                let _ = reader.close();
                return Ok(offsets);
            }
            Err(error) => {
                let _ = reader.close();
                return Err(error);
            }
        };

        while property.FirstKey > job_keys[key_index] {
            key_index += 1;
            if key_index >= job_keys.len() {
                // 与 Go 的 goroutine 一致：所有 key 已确定后立即停止读取该文件。
                let _ = reader.close();
                return Ok(offsets);
            }
            offsets[key_index] = offsets[key_index - 1];
        }
        offsets[key_index] = property.Offset;
    }
}
/// Go 风格别名：等同 `get_read_range_from_props`。
pub fn GetReadRangeFromProps(
    job_keys: &[Vec<u8>],
    paths: &[String],
    storage: &MemoryStorage,
) -> Result<Vec<Vec<u64>>> {
    get_read_range_from_props(job_keys, paths, storage)
}

/// 枚举属于目标非分区目录的对象路径（含 `pXXXXXXXX/dir/...` 分区前缀）。
pub fn get_all_file_names(
    storage: &MemoryStorage,
    non_partitioned_dir: &str,
) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for path in storage.list()? {
        let mut parts = path.split('/');
        let Some(first) = parts.next() else { continue };
        if first == non_partitioned_dir {
            // 直接挂在目标目录下的对象。
            if parts.next().is_some() {
                result.push(path);
            }
            continue;
        }
        // 第一段须为合法 `p[01]{8}` 分区前缀。
        if !IsValidPartition(first.as_bytes()) {
            continue;
        }
        if parts.next() == Some(non_partitioned_dir) && parts.next().is_some() {
            result.push(path);
        }
    }
    // 列表顺序不稳定，显式排序以提供确定性结果。
    result.sort();
    Ok(result)
}
/// Go 风格别名：等同 `get_all_file_names`。
pub fn GetAllFileNames(storage: &MemoryStorage, dir: &str) -> Result<Vec<String>> {
    get_all_file_names(storage, dir)
}
