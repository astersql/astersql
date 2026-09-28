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

// 后端管理器（backend manager）模块。
//
// 本模块负责为 DDL ingest（快速加索引数据摄入）流程构建后端上下文
// `BackendContext`。ingest 是指在执行 ADD INDEX 等 DDL 时，绕过常规事务
// 写入路径、直接批量生成并导入索引数据（类似 lightning 的本地导入模式），
// 以显著加速索引创建。
//
// 模块内容包括：
// - `BackendContextBuilder`：按 DDL 作业（job）构建后端上下文的建造者，
//   可选开启重复键检测与检查点（checkpoint）恢复能力；
// - 排序目录路径与后端标签（backend tag）的编码/解码辅助函数，
//   用于把作业 ID 映射到磁盘上的临时排序目录名。
// 普通后端与查重后端共享目录命名规则，靠 backend tag 后缀区分不同用途。

use crate::backend::BackendContext;
use crate::checkpoint::{CheckpointManager, CheckpointStorage, Key};
use crate::config::IngestConfig;
use crate::disk_root::DiskRoot;
use crate::mem_root::MemRoot;
use std::path::Path;
use std::sync::Arc;
/// `BackendContext` 的建造者（Builder 模式）。
///
/// 每个 DDL 作业对应一个后端上下文；通过链式方法按需配置
/// 重复键检测、检查点存储等可选能力，最后调用 [`Self::build`] 构建。
pub struct BackendContextBuilder {
    /// DDL 作业 ID，用于标识本次 ingest 所属的作业。
    pub job_id: i64,
    /// 是否开启重复键检测（唯一索引场景需要检查导入数据中的重复键）。
    pub check_duplicates: bool,
    /// 导入时间戳（TSO，全局单调递增的逻辑时间），作为导入数据的版本号。
    pub import_ts: u64,
    /// 可选的检查点存储后端；配置后可在任务中断时从检查点恢复进度。
    pub checkpoint_storage: Option<Arc<dyn CheckpointStorage>>,
    /// 检查点恢复的起始键（本次导入从该键之后继续处理）。
    pub start_key: Key,
    /// 当前实例地址，记录在检查点中用于识别检查点归属的节点。
    pub instance_addr: String,
}
impl BackendContextBuilder {
    /// 以指定作业 ID 创建建造者，其余字段取默认值（不查重、无检查点）。
    pub fn new(job_id: i64) -> Self {
        Self {
            job_id,
            check_duplicates: false,
            import_ts: 0,
            checkpoint_storage: None,
            start_key: Vec::new(),
            instance_addr: String::new(),
        }
    }
    /// 开启重复键检测模式（用于唯一索引的冲突校验流程）。
    pub fn for_duplicate_check(mut self) -> Self {
        self.check_duplicates = true;
        self
    }
    /// 配置检查点能力：指定检查点存储、恢复起始键、导入时间戳与实例地址。
    ///
    /// 配置后 ingest 过程会周期性持久化进度，任务中断（如节点重启）
    /// 时可以从最近的检查点继续，避免整个索引回填从头重做。
    pub fn with_checkpoint(
        mut self,
        storage: Arc<dyn CheckpointStorage>,
        start_key: Key,
        import_ts: u64,
        instance_addr: impl Into<String>,
    ) -> Self {
        self.checkpoint_storage = Some(storage);
        self.start_key = start_key;
        self.import_ts = import_ts;
        self.instance_addr = instance_addr.into();
        self
    }
    /// 消耗建造者并构建 `BackendContext`。
    ///
    /// 参数说明：
    /// - `config`：ingest 配置（当前实现暂未使用，仅保留接口）；
    /// - `mem_root`：内存配额跟踪器，控制 ingest 过程的内存使用上限；
    /// - `disk_root`：磁盘配额跟踪器，控制临时排序文件占用的磁盘空间。
    ///
    /// 若配置了检查点存储，则先创建 `CheckpointManager`；
    /// 创建失败时向调用方返回错误字符串。
    pub fn build(
        self,
        config: &IngestConfig,
        mem_root: Arc<dyn MemRoot>,
        disk_root: DiskRoot,
    ) -> Result<BackendContext, String> {
        // 仅在配置了检查点存储时构建检查点管理器；
        // map + transpose 把 Option<Result<T>> 转成 Result<Option<T>>，
        // 以便用 `?` 直接向上传播构建错误。
        let checkpoint = self
            .checkpoint_storage
            .map(|storage| {
                CheckpointManager::new(storage, self.start_key, self.import_ts, self.instance_addr)
            })
            .transpose()?;
        // config 目前未参与构建逻辑，显式忽略以避免未使用参数告警。
        let _ = config;
        Ok(BackendContext::new(
            self.job_id,
            mem_root,
            disk_root,
            checkpoint,
        ))
    }
}
/// 生成指定作业的排序目录路径：`{base}/{backend_tag}`。
///
/// ingest 过程中生成的键值数据需要先写入本地临时目录做外部排序，
/// 每个作业按后端标签划分独立子目录，避免相互干扰。
pub fn generate_job_sort_path(base: &str, job_id: i64, check_duplicates: bool) -> String {
    Path::new(base)
        .join(encode_backend_tag(job_id, check_duplicates))
        .to_string_lossy()
        .into_owned()
}
/// 把作业 ID 编码为后端标签：普通模式为 `{job_id}`，
/// 重复键检测模式追加 `-dup` 后缀（如 `123-dup`）。
pub fn encode_backend_tag(job_id: i64, check_duplicates: bool) -> String {
    // 查重后端与普通后端使用不同目录，后缀用于区分二者。
    if check_duplicates {
        format!("{job_id}-dup")
    } else {
        job_id.to_string()
    }
}
/// 从后端标签解析出作业 ID（`encode_backend_tag` 的逆操作）。
///
/// 与 Go `strconv.ParseInt(name, 10, 64)` 一致，仅接受完整的十进制整数；
/// `-dup` 后缀属于查重后端目录，不应被识别为普通作业目录。
pub fn decode_backend_tag(name: &str) -> Result<i64, String> {
    name.parse()
        .map_err(|_| format!("invalid backend tag {name}"))
}
