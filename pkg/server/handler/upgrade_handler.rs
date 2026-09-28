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

// 集群升级（cluster upgrade）状态机 HTTP 接口。
//
// 对齐 Go `ClusterUpgradeHandler`：通过 POST + `op` 路径变量执行
// start（进入升级态）、finish（恢复 normal）、show（查询进度）。
// 升级态用于滚动升级期间协调多 TiDB 节点版本与 DDL owner。

use std::sync::{Arc, Mutex};

use crate::util::{Error, JsonValue, ResponseWriter, WriteData, WriteError, json_string};

/// ClusterUpgradeHandler 对应 Go handler，持有 kv.Storage 以创建 session/domain。
pub struct ClusterUpgradeHandler {
    /// 底层存储，用于创建 session 并读写集群升级状态。
    pub store: Storage,
}

/// 构造集群升级 handler。
pub fn NewClusterUpgradeHandler(store: Storage) -> ClusterUpgradeHandler {
    ClusterUpgradeHandler { store }
}

/// ServeHTTP 仅接受 POST，并按路由 op 分发 start/finish/show。
pub fn serve_http(h: &ClusterUpgradeHandler, w: &mut ResponseWriter, req: &Request) {
    if req.method() != METHOD_POST {
        WriteError(w, Error::new("This API only support POST method"));
        return;
    }

    let op = req.path_var(OPERATION);
    let mut has_done = false;
    // 按 op 分发；start/finish 返回是否已是目标态（重复操作）。
    let result = match op.as_str() {
        "start" => h.StartUpgrade().map(|v| {
            has_done = v;
        }),
        "finish" => h.FinishUpgrade().map(|v| {
            has_done = v;
        }),
        "show" => h.showUpgrade(w),
        _ => {
            WriteError(w, Error::new(format!("wrong operation:{}", op)));
            return;
        }
    };

    if let Err(err) = result {
        WriteError(w, err);
        log_info(req.context(), "upgrade operation failed", &op, has_done);
        return;
    }
    // 重复操作给出明确提示，避免误判为失败。
    if has_done {
        match op.as_str() {
            "start" => WriteData(
                w,
                "It's a duplicated operation and the cluster is already in upgrading state.",
            ),
            "finish" => WriteData(
                w,
                "It's a duplicated operation and the cluster is already in normal state.",
            ),
            _ => {}
        }
    } else {
        WriteData(w, "success!");
    }
    log_info(req.context(), "upgrade operation success", &op, has_done);
}

impl ClusterUpgradeHandler {
    /// HTTP 入口，转发到 `serve_http`。
    pub fn ServeHTTP(&self, writer: &mut ResponseWriter, request: &Request) {
        serve_http(self, writer, request);
    }

    /// StartUpgrade 对应 Go：创建 session，检查升级态，未升级则同步升级状态。
    pub fn StartUpgrade(&self) -> Result<bool, Error> {
        let mut se = create_session(&self.store)?;
        let is_upgrading = is_upgrading_cluster_state(&mut se)?;
        // 已在升级态则视为重复操作，返回 true。
        if is_upgrading {
            se.close();
            return Ok(true);
        }
        let result = sync_upgrade_state(&mut se, Duration::seconds(10));
        se.close();
        result.map(|_| false)
    }

    /// FinishUpgrade 对应 Go：若已经是 normal state，则报告重复操作；否则同步 normal running。
    pub fn FinishUpgrade(&self) -> Result<bool, Error> {
        let mut se = create_session(&self.store)?;
        let is_upgrading = is_upgrading_cluster_state(&mut se)?;
        // 已是 normal 则视为重复 finish。
        if !is_upgrading {
            se.close();
            return Ok(true);
        }
        let result = sync_normal_running(&mut se);
        se.close();
        result.map(|_| false)
    }

    /// showUpgrade 对应 Go 的集群升级进度查询。
    pub fn showUpgrade(&self, w: &mut ResponseWriter) -> Result<(), Error> {
        let mut se = create_session(&self.store)?;
        let is_upgrading = is_upgrading_cluster_state(&mut se)?;
        if !is_upgrading {
            se.close();
            WriteData(w, "The cluster state is normal.");
            return Ok(());
        }

        // 汇总各节点版本，计算最高版本占比作为升级进度。
        let dom = get_domain(&self.store)?;
        let all_servers = get_all_server_info(&self.store, background_context())?;
        let owner_id =
            dom.ddl_owner_id(timeout_context(background_context(), Duration::seconds(3)))?;
        let mut version_counts = VersionCounter::new();
        for info in &all_servers {
            version_counts.add(info.version.clone());
        }
        let max_version = version_counts.max_version();
        let upgraded_percent = version_counts.count(&max_version) * 100 / all_servers.len().max(1);
        let mut upgrade_info = ClusterUpgradeInfo {
            servers_num: all_servers.len() as i32,
            owner_id,
            upgraded_percent: upgraded_percent as i32,
            is_all_upgraded: upgraded_percent == 100,
            all_servers_diff_infos: Vec::new(),
        };
        // Go 在版本不一致时附带所有 TiDB server 的简化信息，便于定位落后节点。
        if !upgrade_info.is_all_upgraded {
            upgrade_info.all_servers_diff_infos = all_servers
                .iter()
                .map(SimpleServerInfo::from_server_info)
                .collect();
        }
        WriteData(w, upgrade_info);
        se.close();
        Ok(())
    }
}

/// SimpleServerInfo 对应 Go 中返回给 HTTP 的单节点版本/地址信息。
#[derive(Clone)]
pub struct SimpleServerInfo {
    /// 节点版本信息。
    pub version_info: VersionInfo,
    /// 节点唯一 ID。
    pub id: String,
    /// 节点 IP。
    pub ip: String,
    /// 节点端口。
    pub port: u32,
    /// JSON 中的 server id 字段。
    pub json_server_id: u64,
}

impl SimpleServerInfo {
    /// 从 infosync 的 ServerInfo 投影出 HTTP 响应字段。
    fn from_server_info(info: &ServerInfo) -> SimpleServerInfo {
        SimpleServerInfo {
            version_info: info.version.clone(),
            id: info.id.clone(),
            ip: info.ip.clone(),
            port: info.port,
            json_server_id: info.json_server_id,
        }
    }
}

/// ClusterUpgradeInfo 对应 Go HTTP 响应体，记录 owner、进度和差异节点。
pub struct ClusterUpgradeInfo {
    /// 集群 TiDB 节点总数。
    pub servers_num: i32,
    /// 当前 DDL owner 节点 ID。
    pub owner_id: String,
    /// 已升到最高版本的节点百分比。
    pub upgraded_percent: i32,
    /// 是否全部节点已升级到最高版本。
    pub is_all_upgraded: bool,
    /// 版本不一致时的各节点简要信息。
    pub all_servers_diff_infos: Vec<SimpleServerInfo>,
}

impl JsonValue for ClusterUpgradeInfo {
    fn to_json(&self) -> String {
        let mut fields = vec![
            format!("\"owner_id\":{}", json_string(&self.owner_id)),
            format!("\"upgraded_percent\":{}", self.upgraded_percent),
        ];
        if self.servers_num != 0 {
            fields.insert(0, format!("\"servers_num\":{}", self.servers_num));
        }
        if self.is_all_upgraded {
            fields.push("\"is_all_server_version_consistent\":true".into());
        }
        if !self.all_servers_diff_infos.is_empty() {
            let servers = self
                .all_servers_diff_infos
                .iter()
                .map(JsonValue::to_json)
                .collect::<Vec<_>>()
                .join(",");
            fields.push(format!("\"all_servers_diff_info\":[{}]", servers));
        }
        format!("{{{}}}", fields.join(","))
    }
}

impl JsonValue for SimpleServerInfo {
    fn to_json(&self) -> String {
        format!(
            "{{\"version\":{},\"git_hash\":{},\"ddl_id\":{},\"ip\":{},\"listening_port\":{},\"server_id\":{}}}",
            json_string(&self.version_info.version),
            json_string(&self.version_info.git_hash),
            json_string(&self.id),
            json_string(&self.ip),
            self.port,
            self.json_server_id,
        )
    }
}

/// 集群升级状态与节点信息的最小存储抽象。
#[derive(Clone, Default)]
pub struct Storage {
    state: Arc<Mutex<ClusterState>>,
}

#[derive(Default)]
struct ClusterState {
    upgrading: bool,
    owner_id: String,
    servers: Vec<ServerInfo>,
}

impl Storage {
    pub fn new() -> Storage {
        Storage::default()
    }

    pub fn set_servers(&self, servers: Vec<ServerInfo>) {
        self.state.lock().unwrap().servers = servers;
    }

    pub fn set_owner_id(&self, owner_id: impl Into<String>) {
        self.state.lock().unwrap().owner_id = owner_id.into();
    }
}

/// Session 句柄；Drop 保证与 Go defer se.Close() 相同的收尾语义。
pub struct Session {
    state: Arc<Mutex<ClusterState>>,
    closed: bool,
}

/// Domain（服务域）视图，读取同一集群状态。
pub struct Domain {
    state: Arc<Mutex<ClusterState>>,
}

/// HTTP 请求的最小可测试表示。
pub struct Request {
    method: String,
    operation: String,
    context: Context,
}

impl Request {
    pub fn new(method: impl Into<String>, operation: impl Into<String>) -> Request {
        Request {
            method: method.into(),
            operation: operation.into(),
            context: Context,
        }
    }

    pub fn post(operation: impl Into<String>) -> Request {
        Request::new(METHOD_POST, operation)
    }
}

/// 请求上下文占位。
pub struct Context;
/// 版本字符串包装。
#[derive(Clone, Default, Eq, PartialEq)]
pub struct VersionInfo {
    pub version: String,
    pub git_hash: String,
}
/// infosync 单节点信息占位。
#[derive(Clone, Default)]
pub struct ServerInfo {
    pub version: VersionInfo,
    pub id: String,
    pub ip: String,
    pub port: u32,
    pub json_server_id: u64,
}
/// 版本出现次数计数器，用于计算升级进度。
pub struct VersionCounter {
    values: Vec<VersionInfo>,
}
/// 时长占位（秒）。
pub struct Duration(i64);

/// HTTP POST 方法字面量。
pub const METHOD_POST: &str = "POST";
/// 路径变量：操作名。
pub const OPERATION: &str = "op";

impl Duration {
    /// 由秒数构造时长。
    fn seconds(v: i64) -> Duration {
        Duration(v)
    }
}
impl Request {
    fn method(&self) -> &str {
        &self.method
    }
    fn path_var(&self, name: &str) -> String {
        if name == OPERATION {
            self.operation.clone()
        } else {
            String::new()
        }
    }
    fn context(&self) -> Context {
        Context
    }
}
impl Session {
    fn close(&mut self) {
        self.closed = true;
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}
impl Domain {
    /// 查询当前 DDL owner ID（带超时上下文）。
    fn ddl_owner_id(&self, _: Context) -> Result<String, Error> {
        Ok(self.state.lock().unwrap().owner_id.clone())
    }
}
impl VersionCounter {
    fn new() -> VersionCounter {
        VersionCounter { values: Vec::new() }
    }
    fn add(&mut self, version: VersionInfo) {
        self.values.push(version);
    }
    /// 按版本字符串字典序取最大版本。
    fn max_version(&self) -> VersionInfo {
        self.values
            .iter()
            .cloned()
            .max_by(|a, b| a.version.cmp(&b.version))
            .unwrap_or(VersionInfo {
                version: String::new(),
                git_hash: String::new(),
            })
    }
    fn count(&self, target: &VersionInfo) -> usize {
        self.values.iter().filter(|v| *v == target).count()
    }
}

/// 创建 session，并共享存储状态。
fn create_session(storage: &Storage) -> Result<Session, Error> {
    Ok(Session {
        state: storage.state.clone(),
        closed: false,
    })
}
/// 判断集群是否处于升级态。
fn is_upgrading_cluster_state(session: &mut Session) -> Result<bool, Error> {
    Ok(session.state.lock().unwrap().upgrading)
}
/// 同步写入升级状态（带超时）。
fn sync_upgrade_state(session: &mut Session, _: Duration) -> Result<(), Error> {
    session.state.lock().unwrap().upgrading = true;
    Ok(())
}
/// 同步恢复 normal running 状态。
fn sync_normal_running(session: &mut Session) -> Result<(), Error> {
    session.state.lock().unwrap().upgrading = false;
    Ok(())
}
/// 由 storage 取 Domain。
fn get_domain(storage: &Storage) -> Result<Domain, Error> {
    Ok(Domain {
        state: storage.state.clone(),
    })
}
/// 拉取全部 TiDB server 信息。
fn get_all_server_info(storage: &Storage, _: Context) -> Result<Vec<ServerInfo>, Error> {
    Ok(storage.state.lock().unwrap().servers.clone())
}
fn background_context() -> Context {
    Context
}
fn timeout_context(_: Context, _: Duration) -> Context {
    Context
}
fn log_info(_: Context, _: &str, _: &str, _: bool) {}
