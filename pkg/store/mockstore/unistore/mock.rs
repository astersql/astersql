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

// 嵌入式 unistore 一站式启动入口（对应 Go `New`）。
//
// 在进程内创建 mock TiKV server、RPC 客户端、PD 门面与 Cluster 元数据控制器，
// 供 TiDB 单测不依赖真实集群。空路径时使用临时目录；临时路径启用易失模式。

use crate::cluster::Cluster;
use crate::pd::{KeyspaceMeta, PdClient, PdError};
use crate::rpc::RPCClient;
use crate::{config, server};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// `New` 可能返回的错误：IO、Server 启动或 PD 构造失败。
#[derive(Debug)]
pub enum NewError {
    Io(std::io::Error),
    Server(server::ServerError),
    Pd(PdError),
}

impl std::fmt::Display for NewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Server(error) => write!(formatter, "{error}"),
            Self::Pd(error) => write!(formatter, "{error}"),
        }
    }
}
impl std::error::Error for NewError {}
impl From<std::io::Error> for NewError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<server::ServerError> for NewError {
    fn from(value: server::ServerError) -> Self {
        Self::Server(value)
    }
}
impl From<PdError> for NewError {
    fn from(value: PdError) -> Self {
        Self::Pd(value)
    }
}

/// Creates a functional embedded unistore server, its in-process RPC client,
/// a PD facade and the cluster metadata controller.
/// 创建嵌入式 unistore：返回进程内 RPCClient、PdClient 与 Cluster。
///
/// - `path` 为空则创建临时目录；位于 `tidb-unistore-temp` 下时走易失引擎配置。
/// - `pd_addresses` / `current_keyspace_id` / `cluster_keyspaces` 交给 mock PD。
/// Keyspace 是多租户隔离的逻辑命名空间。
pub fn New(
    path: impl AsRef<Path>,
    pd_addresses: Vec<String>,
    current_keyspace_id: u32,
    cluster_keyspaces: Vec<KeyspaceMeta>,
) -> Result<(Arc<RPCClient>, Arc<PdClient>, Arc<Cluster>), NewError> {
    let requested = path.as_ref();
    let path = if requested.as_os_str().is_empty() {
        create_temp_directory()?
    } else {
        requested.to_path_buf()
    };
    let temp_prefix = std::env::temp_dir().join("tidb-unistore-temp");
    // Go uses strings.HasPrefix here. Path::starts_with compares whole path
    // components, so it does not match names such as `tidb-unistore-temp-123`.
    let persistent = !path
        .to_string_lossy()
        .starts_with(temp_prefix.to_string_lossy().as_ref());
    fs::create_dir_all(&path)?;

    let mut conf = config::DefaultConf.clone();
    // ValueThreshold=0：所有 value 进 LSM，避免 value log 路径差异。
    conf.Engine.ValueThreshold = 0;
    conf.Engine.DBPath = path.to_string_lossy().into_owned();
    // 单测关闭真实 Raft，使用进程内 mock。
    conf.Server.Raft = false;
    if !persistent {
        // 临时目录：缩小 memtable、关闭 sync、减少 compact 开销。
        conf.Engine.VolatileMode = true;
        conf.Engine.MaxMemTableSize = 12 << 20;
        conf.Engine.SyncWrite = false;
        conf.Engine.NumCompactors = 1;
        conf.Engine.CompactL0WhenClose = false;
        conf.Engine.VlogFileSize = 16 << 20;
    }

    let (server, region_manager, mock_pd) = server::new_mock(&conf, 1)?;
    let cluster = Arc::new(Cluster::new(region_manager));
    let client = Arc::new(RPCClient::new(
        server,
        Arc::clone(&cluster),
        path,
        persistent,
    ));
    let pd = PdClient::new(
        mock_pd,
        pd_addresses,
        current_keyspace_id,
        cluster_keyspaces,
    )?;
    Ok((client, Arc::new(pd), cluster))
}

/// 在系统临时目录下创建带纳秒后缀的 `tidb-unistore-temp-*` 目录。
fn create_temp_directory() -> std::io::Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tidb-unistore-temp-{nonce}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}
