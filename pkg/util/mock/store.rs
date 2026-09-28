// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// util 包专用的模拟 Storage（存储引擎抽象）实现。
//
// 对应 Go `pkg/util/mock` 的 `Store`：只保留可空的 KV Client，其余
// Storage 接口方法返回固定空值/占位，供 util 包单测注入，不连真实 TiKV。

use std::any::Any;
use std::sync::Arc;

use crate::kv;

/// Store keeps Go's nullable client and fixed mock-storage return values.
/// 模拟存储：可挂一个可选的 `kv::Client`，其余 Storage 方法返回固定占位结果。
#[derive(Default)]
pub struct Store {
    /// 可选的 KV 客户端句柄；未设置时 `GetClient` 返回 `None`。
    pub Client: Option<Arc<dyn kv::Client + Send + Sync>>,
}

impl Store {
    /// 返回已配置的 KV Client 克隆；未配置则为 `None`。
    pub fn GetClient(&self) -> Option<Arc<dyn kv::Client + Send + Sync>> {
        self.Client.clone()
    }

    /// MPP（大规模并行处理）客户端：mock 不提供，恒为 `None`。
    pub fn GetMPPClient(&self) -> Option<&dyn kv::MPPClient> {
        None
    }

    /// Oracle（全局时间戳服务）句柄：mock 不提供，恒为 `None`。
    pub fn GetOracle(&self) -> Option<&dyn kv::oracle::Oracle> {
        None
    }

    /// 开启事务：mock 不创建真实事务，成功返回 `Ok(None)`。
    pub fn Begin(
        &self,
        _opts: &[kv::tikv::TxnOption],
    ) -> Result<Option<Box<dyn kv::Transaction>>, kv::errors::SharedError> {
        Ok(None)
    }

    /// 按版本取快照（MVCC 读视图）：mock 恒返回 `None`。
    pub fn GetSnapshot(&self, _version: kv::Version) -> Option<Box<dyn kv::Snapshot>> {
        None
    }

    /// 关闭存储：无资源可释，恒成功。
    pub fn Close(&self) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }

    /// 返回固定 UUID `"mock"`，便于测试识别。
    pub fn UUID(&self) -> String {
        "mock".to_owned()
    }

    /// 当前版本号：mock 固定返回 `Ver: 0`。
    pub fn CurrentVersion(&self, _txn_scope: &str) -> Result<kv::Version, kv::errors::SharedError> {
        Ok(kv::Version { Ver: 0 })
    }

    /// 是否支持 DeleteRange：mock 不支持。
    pub fn SupportDeleteRange(&self) -> bool {
        false
    }

    /// 存储实现名称，对齐 Go 的 `"UtilMockStorage"`。
    pub fn Name(&self) -> String {
        "UtilMockStorage".to_owned()
    }

    /// 人类可读描述，说明仅供 util 包单测使用。
    pub fn Describe(&self) -> String {
        "UtilMockStorage is a mock Store implementation, only for unittests in util package"
            .to_owned()
    }

    /// 内存缓存管理器：mock 不提供。
    pub fn GetMemCache(&self) -> Option<&dyn kv::MemManager> {
        None
    }

    /// ShowStatus 诊断接口：mock 无状态可查，返回 `Ok(None)`。
    pub fn ShowStatus(
        &self,
        _ctx: &kv::context::Context,
        _key: &str,
    ) -> Result<Option<Box<dyn Any>>, kv::errors::SharedError> {
        Ok(None)
    }

    /// 最小安全时间戳（safe TS）：mock 固定返回 0。
    pub fn GetMinSafeTS(&self, _txn_scope: &str) -> u64 {
        0
    }

    /// 锁等待信息（死锁检测）：mock 无锁，返回 `Ok(None)`。
    pub fn GetLockWaits(
        &self,
    ) -> Result<Option<Vec<kv::deadlockpb::WaitForEntry>>, kv::errors::SharedError> {
        Ok(None)
    }

    /// 键编解码器：mock 不提供。
    pub fn GetCodec(&self) -> Option<kv::tikv::Codec> {
        None
    }

    /// 读取存储选项：恒返回 `(None, false)` 表示未设置。
    pub fn GetOption(&self, _key: &dyn Any) -> (Option<&dyn Any>, bool) {
        (None, false)
    }

    /// 设置存储选项：mock 忽略写入，空实现。
    pub fn SetOption(&self, _key: Box<dyn Any>, _value: Box<dyn Any>) {}

    /// 集群 ID：mock 固定返回 1。
    pub fn GetClusterID(&self) -> u64 {
        1
    }

    /// Keyspace（多租户键空间名）：mock 返回空串。
    pub fn GetKeyspace(&self) -> String {
        String::new()
    }
}
