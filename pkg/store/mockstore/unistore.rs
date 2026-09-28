// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 嵌入式 UniStore 适配层：根据 MockOptions 组装 MockStorage。
//
// UniStore 是进程内 KV 实现，提供与 TiKV 协议兼容的测试后端；本模块负责
// 创建集群、应用 inspector/hijacker，并填充 keyspace 与 latch 配置。

use crate::embedded_unistore;
use crate::mockstore::{MockOptions, MockStorage, Result, StoreError, StoreType};

/// 以 EmbedUnistore 后端创建 MockStorage。
pub fn new_unistore(options: &MockOptions) -> Result<MockStorage> {
    build_embedded(options, StoreType::EmbedUnistore)
}

/// 共用构建逻辑：启动嵌入式集群，再按选项劫持客户端并组装门面。
pub(crate) fn build_embedded(options: &MockOptions, backend: StoreType) -> Result<MockStorage> {
    // 将测试侧 keyspace 元数据转为嵌入式 PD 使用的形式。
    let keyspaces = options
        .cluster_keyspaces
        .iter()
        .map(|meta| meta.to_embedded())
        .collect();
    let (mut client, mut pd_client, cluster) = embedded_unistore::New(
        &options.path,
        options.pd_addresses.clone(),
        options.current_keyspace_id,
        keyspaces,
    )
    .map_err(|error| StoreError(error.to_string()))?;
    // 允许测试检查或改写刚创建的集群拓扑（如添加 TiFlash peer）。
    (options.cluster_inspector)(&cluster);
    if let Some(hijacker) = &options.client_hijacker {
        client = hijacker(client);
    }
    if let Some(hijacker) = &options.pd_client_hijacker {
        pd_client = hijacker(pd_client);
    }
    Ok(MockStorage {
        backend,
        client,
        pd_client,
        cluster,
        current_keyspace: options.current_keyspace_meta(),
        txn_local_latches: options.txn_local_latches,
        ddl_checked: false,
    })
}

/// Go 风格命名别名，等价于 `new_unistore`。
pub fn newUnistore(options: &MockOptions) -> Result<MockStorage> {
    new_unistore(options)
}
