// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 测试用全局 ServerInfo 管理与序列化结构。
//
// `ServerInfo` 描述 TiDB 节点拓扑字段（版本、地址、DDL ID、labels 等）；
// `MockGlobalServerInfoManager` 以单例形式登记/查询/删除假服务器信息。

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// TiDB 服务器拓扑信息，字段名通过 serde rename 对齐 Go JSON。
pub struct ServerInfo {
    #[serde(rename = "version")]
    /// 服务器版本字符串。
    pub Version: String,
    #[serde(rename = "git_hash")]
    /// 构建时的 Git commit hash。
    pub GitHash: String,
    #[serde(rename = "ddl_id")]
    /// DDL owner 选举用的节点 ID（常称 ddl_id）。
    pub ID: String,
    #[serde(rename = "ip")]
    /// 监听 IP。
    pub IP: String,
    #[serde(rename = "listening_port")]
    /// SQL 服务端口。
    pub Port: u32,
    #[serde(rename = "status_port")]
    /// HTTP 状态端口。
    pub StatusPort: u32,
    #[serde(rename = "lease")]
    /// Owner lease 时长描述（如 `"1s"`）。
    pub Lease: String,
    #[serde(rename = "start_timestamp")]
    /// 进程启动时间戳（秒）。
    pub StartTimestamp: i64,
    #[serde(rename = "server_id")]
    /// 数值型 server ID（JSON 字段 `server_id`）。
    pub JSONServerID: u64,
    #[serde(rename = "labels")]
    /// 节点标签（如 zone、host），用于调度亲和。
    pub Labels: HashMap<String, String>,
}

/// 延迟获取数值 server ID 的回调类型。
type ServerIdGetter = Arc<dyn Fn() -> u64 + Send + Sync>;

/// 进程内 mock 的全局服务器信息登记表。
pub struct MockGlobalServerInfoManager {
    state: Mutex<MockGlobalServerInfoManagerState>,
}

struct MockServerInfo {
    info: ServerInfo,
    getter: ServerIdGetter,
}

struct MockGlobalServerInfoManagerState {
    infos: Vec<MockServerInfo>,
    mock_server_port: u32,
}

impl Default for MockGlobalServerInfoManager {
    fn default() -> Self {
        Self {
            state: Mutex::new(MockGlobalServerInfoManagerState {
                infos: Vec::new(),
                mock_server_port: 4000,
            }),
        }
    }
}

impl MockGlobalServerInfoManager {
    /// 登记一台服务器及其 ID 获取回调。
    pub fn Add(&self, id: String, getter: ServerIdGetter) {
        let mut state = self.state.lock().unwrap();
        let info = ServerInfo {
            ID: id,
            IP: "127.0.0.1".into(),
            Port: state.mock_server_port,
            StartTimestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            JSONServerID: getter(),
            ..Default::default()
        };
        state.mock_server_port += 1;
        state.infos.push(MockServerInfo { info, getter });
    }
    /// 按索引删除；越界返回错误。
    pub fn Delete(&self, idx: usize) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if idx >= state.infos.len() {
            return Err(Error::External("server idx out of bound".into()));
        }
        state.infos.remove(idx);
        Ok(())
    }
    /// 按执行器地址 `ip:port` 删除首个匹配项。
    pub fn DeleteByExecID(&self, exec_id: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(index) = state
            .infos
            .iter()
            .position(|entry| format!("{}:{}", entry.info.IP, entry.info.Port) == exec_id)
        {
            state.infos.remove(index);
        }
    }
    /// 构造当前已登记节点的 `ServerInfo` 快照。
    pub fn GetAllServerInfo(&self) -> HashMap<String, ServerInfo> {
        self.state
            .lock()
            .unwrap()
            .infos
            .iter()
            .map(|entry| {
                let mut info = entry.info.clone();
                info.JSONServerID = (entry.getter)();
                (info.ID.clone(), info)
            })
            .collect()
    }
    /// 清空全部登记项，并将下一个 mock 端口重置为 4000。
    pub fn Close(&self) {
        let mut state = self.state.lock().unwrap();
        state.infos.clear();
        state.mock_server_port = 4000;
    }
}

/// 返回进程级单例 `MockGlobalServerInfoManager`。
pub fn MockGlobalServerInfoManagerEntry() -> &'static MockGlobalServerInfoManager {
    static INSTANCE: std::sync::OnceLock<MockGlobalServerInfoManager> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(MockGlobalServerInfoManager::default)
}
