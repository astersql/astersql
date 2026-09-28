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

// DDL ingest 配置模块。
//
// ingest（导入）是指在创建索引等 DDL（Data Definition Language，数据定义语言）
// 操作时，先把待写入的键值对（KV）排序、缓存到本地引擎，再批量导入存储层的加速方式。
// 本模块负责生成并调整这些导入过程的参数，例如并发度、内存缓存大小、
// 排序中间文件目录，以及导入时的会话变量默认值。

use std::collections::BTreeMap;

use crate::mem_root::MemRoot;

/// ingest（本地导入）过程的配置参数集合。
///
/// 描述索引回填等 DDL 任务在本地排序、缓存并批量导入 KV 时使用的并发度与内存预算。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestConfig {
    /// 导入 worker（工作线程）的并发数量。
    pub worker_concurrency: usize,
    /// 处理数据区间（range）的并发数量。
    pub range_concurrency: usize,
    /// 本地引擎（引擎指承载排序后 KV 的本地存储）内存缓存大小，单位字节。
    pub engine_memory_cache_size: i64,
    /// 本地写入器（writer）的内存缓存大小，单位字节。
    pub local_writer_memory_cache_size: i64,
    /// 允许同时打开的文件句柄上限。
    pub max_open_files: usize,
    /// 排序后 KV 中间文件的存放目录。
    pub sorted_kv_dir: String,
}

/// 根据目录、并发数与内存配额生成一份默认的 ingest 配置。
///
/// 内存配额（memory_quota）会在本地引擎与各写入器之间进行分配，
/// 以避免导入过程占用过多内存。
pub fn generate_config(
    path: impl Into<String>,
    concurrency: usize,
    _memory_quota: i64,
) -> IngestConfig {
    IngestConfig {
        worker_concurrency: concurrency.wrapping_mul(2),
        range_concurrency: concurrency,
        engine_memory_cache_size: 512 * 1024 * 1024,
        local_writer_memory_cache_size: 128 * 1024 * 1024,
        max_open_files: 1024,
        sorted_kv_dir: path.into(),
    }
}
/// 计算 coprocessor（协处理器，下推到存储节点执行的读取算子）读取批大小。
///
/// 正数提示值原样采用；零值回退为默认 DDL reorg 批大小的十倍。
pub fn cop_read_batch_size(hint_size: usize) -> usize {
    if hint_size > 0 { hint_size } else { 10 * 256 }
}
/// 生成本地引擎的配置项。
///
/// `ts` 为时间戳（timestamp），用于标识数据版本；同时开启 compact（压缩合并）。
pub fn generate_local_engine_config(ts: u64) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("ts".into(), ts.to_string()),
        ("compact".into(), "true".into()),
        (
            "compact_threshold".into(),
            (1024_i64 * 1024 * 1024).to_string(),
        ),
        ("compact_concurrency".into(), "4".into()),
        ("block_size".into(), (16 * 1024).to_string()),
        ("keep_sort_dir".into(), "true".into()),
    ])
}
/// 根据内存根（MemRoot，追踪整体内存使用的组件）当前的剩余额度收缩导入缓存。
///
/// 当可用内存不足时，把引擎与写入器缓存下调，避免导入过程触发内存超限。
pub fn adjust_import_memory(mem_root: &dyn MemRoot, config: &mut IngestConfig) {
    if try_aggressive_memory(mem_root, config) {
        return;
    }

    let writer_memory = config
        .local_writer_memory_cache_size
        .wrapping_mul(config.worker_concurrency as i64)
        / 2;
    let default_memory =
        writer_memory.wrapping_add(config.engine_memory_cache_size.wrapping_mul(4));
    let scale = default_memory / mem_root.max_memory_quota();
    if scale == 0 || scale == 1 {
        return;
    }
    config.local_writer_memory_cache_size /= scale;
    config.engine_memory_cache_size /= scale;
}
/// 尝试为导入申请较激进（尽量多）的内存额度。
///
/// 若当前用量加默认配置不超过上限则返回 true；该检查本身不改变内存记账。
pub fn try_aggressive_memory(mem_root: &dyn MemRoot, config: &mut IngestConfig) -> bool {
    let writer_memory = config
        .local_writer_memory_cache_size
        .wrapping_mul(config.worker_concurrency as i64)
        / 2;
    let default_memory = writer_memory.wrapping_add(config.engine_memory_cache_size);
    default_memory.wrapping_add(mem_root.current_usage()) <= mem_root.max_memory_quota()
}
/// 返回导入内部会话所需的一组重要系统变量默认值。
///
/// 这些变量（如时区、精度、字符集相关设置）需固定为确定值，
/// 以保证 DDL 导入生成的数据与正常写入路径行为一致。
pub fn default_important_variables() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("max_allowed_packet", "67108864"),
        ("div_precision_increment", "4"),
        ("time_zone", "SYSTEM"),
        ("lc_time_names", "en_US"),
        ("default_week_format", "0"),
        ("block_encryption_mode", "aes-128-ecb"),
        ("group_concat_max_len", "1024"),
        ("tidb_row_format_version", "1"),
    ])
}
