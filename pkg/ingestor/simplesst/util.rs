// Copyright 2026 AsterSQL.
/*
// Copyright 2026 PingCAP, Inc.
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


/// EndpointTp 对应 Go 的端点类型；枚举顺序同时决定同 key 端点的处理顺序。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EndpointTp {
    /// ExclusiveEnd 表示 `..., key)`，排序时先于起点，保证该点不计入重叠。
    ExclusiveEnd = 0,
    /// InclusiveStart 表示 `[key, ...`。
    InclusiveStart = 1,
    /// InclusiveEnd 表示 `..., key]`，排序时最后减去权重，保证该点仍计入重叠。
    InclusiveEnd = 2,
}
*/

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
/*

/// Endpoint 对应 Go 区间端点，所有端点类型都携带正权重。
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub Key: Vec<u8>,
    pub Tp: EndpointTp,
    pub Weight: i64,
}

/// GetMaxOverlapping 原地排序端点，并返回任一点处最大的累计区间权重。
pub fn GetMaxOverlapping(points: &mut [Endpoint]) -> i64 {
    // key 相同时按 ExclusiveEnd、InclusiveStart、InclusiveEnd 排序，完全沿用 Go iota 次序。
    points.sort_by(|i, j| i.Key.cmp(&j.Key).then(i.Tp.cmp(&j.Tp)));
    let mut maxWeight = 0_i64;
    let mut curWeight = 0_i64;
    for p in points {
        match p.Tp {
            EndpointTp::InclusiveStart => curWeight += p.Weight,
            EndpointTp::ExclusiveEnd | EndpointTp::InclusiveEnd => curWeight -= p.Weight,
        }
        maxWeight = maxWeight.max(curWeight);
    }
    maxWeight
}

/// RemoveDuplicates 对应 Go 泛型入口：从已排序切片中移除所有属于重复组的元素。
/// recordRemoved 为 true 时同时返回被移除元素；第三项是重复组中元素总数而非组数。
pub fn RemoveDuplicates<E: Clone>(
    input: &mut [E],
    keyGetter: impl Fn(&E) -> &[u8],
    recordRemoved: bool,
) -> (Vec<E>, Vec<E>, usize) {
    doRemoveDuplicates(input, keyGetter, 0, recordRemoved)
}

/// removeDuplicatesMoreThanTwo 保留每个重复组前两个元素，其余记录到 removed。
fn removeDuplicatesMoreThanTwo<E: Clone>(
    input: &mut [E],
    keyGetter: impl Fn(&E) -> &[u8],
) -> (Vec<E>, Vec<E>, usize) {
    doRemoveDuplicates(input, keyGetter, 2, true)
}

/// doRemoveDuplicates 对应 Go 核心算法，扫描相邻同 key 的组并用 fillIdx 原地压缩结果。
fn doRemoveDuplicates<E: Clone>(
    input: &mut [E],
    keyGetter: impl Fn(&E) -> &[u8],
    keptDupCnt: usize,
    recordRemoved: bool,
) -> (Vec<E>, Vec<E>, usize) {
    assert!(keptDupCnt == 0 || keptDupCnt == 2, "keptDupCnt must be 0 or 2");
    if input.len() <= 1 {
        return (input.to_vec(), Vec::new(), 0);
    }

    let mut fillIdx = 0;
    let mut pivotIdx = 0;
    let mut removed = Vec::new();
    let mut totalDup = 0;
    // idx == len 是 Go 使用的哨兵迭代，用来统一刷新最后一组。
    for idx in 1..=input.len() {
        if idx < input.len() && keyGetter(&input[pivotIdx]) == keyGetter(&input[idx]) {
            continue;
        }

        let dupCount = idx - pivotIdx;
        if dupCount >= 2 {
            totalDup += dupCount;
            // keep=0 会删除整组，keep=2 只复制前两个；输入已排序是算法成立的前提。
            for startIdx in pivotIdx..pivotIdx + keptDupCnt {
                if startIdx != fillIdx {
                    input[fillIdx] = input[startIdx].clone();
                }
                fillIdx += 1;
            }
            if recordRemoved {
                removed.extend_from_slice(&input[pivotIdx + keptDupCnt..idx]);
            }
        } else {
            if pivotIdx != fillIdx {
                input[fillIdx] = input[pivotIdx].clone();
            }
            fillIdx += 1;
        }
        pivotIdx = idx;
    }
    (input[..fillIdx].to_vec(), removed, totalDup)
}

/// 限制同时扫描的统计文件数；元数据读取收益低于数据读取，因此默认并发度较保守。
pub const getReadRangeFromPropsConcurrency: usize = 64;

/// GetReadRangeFromProps 扫描每个统计文件，为多组升序 job key 计算可安全 seek 的最大数据偏移。
/// 返回矩阵按 `[job_key][path]` 排列，可分别作为 `[keyA,keyB)` 的估算起止偏移。
pub fn GetReadRangeFromProps(
    ctx: &Context,
    jobKeys: &[Vec<u8>],
    paths: &[String],
    exStorage: &Storage,
) -> Result<Vec<Vec<u64>>, Error> {
    let logger = Logger::from_context(ctx);
    let task = BeginTask(&logger, "seek props offsets");

    let starts: Vec<Key> = jobKeys.iter().cloned().map(Key::from).collect();
    if starts.is_empty() {
        // 空任务不创建错误组，也不会触发任何对象存储读取。
        task.End(None);
        return Ok(Vec::new());
    }
    let readRangesPerKey = Shared::new(vec![vec![0_u64; paths.len()]; starts.len()]);

    let mut eg = NewErrorGroupWithRecoverWithCtx(ctx);
    eg.SetLimit(getReadRangeFromPropsConcurrency);
    for (pathIdx, path) in paths.iter().cloned().enumerate() {
        let starts = starts.clone();
        let ranges = readRangesPerKey.clone();
        let storage = exStorage.clone();
        eg.Go(move |egCtx| {
            let mut reader = match StatsReader::NewStatsReader(egCtx, &storage, &path, 250 * 1024) {
                Ok(reader) => reader,
                // 空统计文件的 EOF 不是失败，该数据文件所有偏移保持零。
                Err(err) if err.is_eof() => return Ok(()),
                Err(err) => return Err(err.trace()),
            };

            // 对应 Go defer：任务无论从哪个分支返回都尽力关闭 reader，关闭错误不覆盖主错误。
            let result = scanPropOffsets(&mut reader, &starts, pathIdx, &ranges);
            let _ = reader.Close();
            result
        });
    }

    let result = match eg.Wait() {
        Ok(()) => Ok(readRangesPerKey.into_inner()),
        Err(err) => Err(err),
    };
    // Go defer 在正常与错误路径都结束计时任务，并以 ErrorLevel 记录错误。
    task.End(result.as_ref().err());
    result
}

/// scanPropOffsets 是 Go goroutine 主体的顺序迁移，逐属性推进当前 job key。
fn scanPropOffsets(
    reader: &mut StatsReader,
    starts: &[Key],
    pathIdx: usize,
    ranges: &Shared<Vec<Vec<u64>>>,
) -> Result<(), Error> {
    let mut keyIdx = 0;
    let mut curKey = &starts[keyIdx];
    let mut next = reader.NextProp();
    let mut firstKey = next.as_ref().ok().map(|p| Key::from(p.FirstKey.clone()));

    loop {
        let prop = match next {
            Ok(prop) => prop,
            Err(err) if err.is_eof() => {
                // 文件结束后，后续所有 key 沿用当前 key 已找到的最后偏移。
                let mut guard = ranges.lock();
                let offset = guard[keyIdx][pathIdx];
                for row in guard.iter_mut().skip(keyIdx + 1) {
                    row[pathIdx] = offset;
                }
                return Ok(());
            }
            Err(err) => return Err(err.trace()),
        };

        while firstKey.as_ref().unwrap().Cmp(curKey) > 0 {
            keyIdx += 1;
            if keyIdx >= starts.len() {
                return Ok(());
            }
            // 尚未覆盖新 key 时先继承上一 key 的偏移，保持 seek 不越过目标。
            let mut guard = ranges.lock();
            guard[keyIdx][pathIdx] = guard[keyIdx - 1][pathIdx];
            curKey = &starts[keyIdx];
        }
        ranges.lock()[keyIdx][pathIdx] = prop.Offset;
        next = reader.NextProp();
        if let Ok(ref prop) = next {
            firstKey = Some(Key::from(prop.FirstKey.clone()));
        }
    }
}

/// GetAllFileNames 返回属于同一非分区目录的元数据文件和分区前缀下的数据/统计文件。
pub fn GetAllFileNames(
    ctx: &Context,
    store: &Storage,
    nonPartitionedDir: &str,
) -> Result<Vec<String>, Error> {
    let mut data = Vec::new();
    store.WalkDir(ctx, WalkOption::default(), |path, _size| {
        // 第一段可能直接是目标目录，也可能是 randPartitionedPrefix 生成的 pXXXXXXXX。
        let Some((firstDir, rest)) = path.split_once('/') else {
            return Ok(());
        };
        if firstDir == nonPartitionedDir {
            data.push(path.to_owned());
            return Ok(());
        }
        if !IsValidPartition(firstDir.as_bytes()) {
            return Ok(());
        }

        // 合法分区目录下再取第二段；不存在第二个斜线的对象不属于目标目录树。
        let Some((secondDir, _)) = rest.split_once('/') else {
            return Ok(());
        };
        if secondDir == nonPartitionedDir {
            data.push(path.to_owned());
        }
        Ok(())
    })?;

    // 外部存储 WalkDir 不保证顺序，Go 在返回前显式排序以提供稳定结果。
    data.sort();
    Ok(data)
}
*/
