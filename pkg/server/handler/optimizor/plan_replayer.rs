// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Plan Replayer HTTP 处理器：下载执行计划回放（Plan Replayer）导出包。
//
// Plan Replayer 用于捕获某次 SQL 的执行计划、统计信息与 schema，便于离线复现与诊断。
// 本模块对齐 Go 侧 `optimizor.PlanReplayerHandler`：先查本地 dump 文件，不存在则向集群其他
// TiDB 节点转发；对 capture 类包还会按快照（snapshot）注入历史统计信息。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use astersql_domain::plan_replayer_dump::{
    PLAN_REPLAYER_SCHEMA_META_FILE, PLAN_REPLAYER_SQL_META_FILE,
};

#[derive(Debug, Clone, PartialEq, Eq)]
/// 内存中的 zip 归档：条目名到内容字节的有序映射。
pub struct Archive {
    pub entries: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 表的基本元数据：表 ID、库名与表名。
pub struct TableMeta {
    pub id: i64,
    pub database: String,
    pub table: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 归档内一张表的信息，可附带历史统计 JSON。
pub struct tblInfo {
    pub info: TableMeta,
    pub json_stats: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 集群中一个 TiDB 节点的状态服务地址（IP + status 端口）。
pub struct Topology {
    pub ip: String,
    pub status_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 对远程节点 HTTP GET 的响应：状态码与正文。
pub struct RemoteResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 下载 dump 文件所需的路径、转发 URL 与响应文件名等上下文。
pub struct downloadFileHandler {
    pub scheme: String,
    pub file_path: PathBuf,
    pub file_name: String,
    pub address: String,
    pub status_port: u16,
    pub url_path: String,
    pub downloaded_filename: String,
}

/// Plan Replayer 运行时依赖：目录、拓扑、HTTP、zip 编解码与统计导出等。
pub trait PlanReplayerRuntime {
    type Error;

    fn route_file_name(&self) -> String;
    fn forwarded(&self) -> bool;
    fn plan_replayer_directory(&self) -> PathBuf;
    fn internal_http_scheme(&self) -> String;
    fn read_local_file(&mut self, path: &Path) -> Result<Option<Vec<u8>>, Self::Error>;
    fn topology(&mut self) -> Result<Vec<Topology>, Self::Error>;
    fn http_get(&mut self, url: &str) -> Result<RemoteResponse, Self::Error>;
    fn decode_zip(&mut self, content: &[u8]) -> Result<Archive, Self::Error>;
    fn encode_zip(&mut self, archive: &Archive) -> Result<Vec<u8>, Self::Error>;
    fn resolve_table(&mut self, database: &str, table: &str) -> Result<TableMeta, Self::Error>;
    fn dump_historical_stats(
        &mut self,
        table: &TableMeta,
        snapshot: u64,
    ) -> Result<Vec<u8>, Self::Error>;
    fn invalid_data(&mut self, message: &str) -> Self::Error;
    fn set_header(&mut self, name: &str, value: &str);
    fn write_status(&mut self, status: u16);
    fn write_body(&mut self, body: &[u8]) -> Result<(), Self::Error>;
    fn write_error(&mut self, error: Self::Error);
    fn log(&mut self, message: &str, file: &str, address: &str, forwarded: bool);
    fn log_forward_error(&mut self, address: &str, error: &Self::Error);
}

/// Plan Replayer dump 下载的 HTTP 入口；持有本机地址与 status 端口。
pub struct PlanReplayerHandler {
    pub address: String,
    pub status_port: u16,
}

/// 构造 PlanReplayerHandler。
pub fn NewPlanReplayerHandler(address: String, status_port: u16) -> PlanReplayerHandler {
    PlanReplayerHandler {
        address,
        status_port,
    }
}

impl PlanReplayerHandler {
    /// 处理 dump 下载请求：组装 downloadFileHandler 并调用 handleDownloadFile。
    pub fn ServeHTTP<R: PlanReplayerRuntime>(&self, runtime: &mut R) {
        let file_name = runtime.route_file_name();
        let handler = downloadFileHandler {
            scheme: runtime.internal_http_scheme(),
            file_path: filepath_join(&runtime.plan_replayer_directory(), &file_name),
            file_name: file_name.clone(),
            address: self.address.clone(),
            status_port: self.status_port,
            url_path: format!("plan_replayer/dump/{file_name}"),
            downloaded_filename: "plan_replayer".to_owned(),
        };
        if let Err(error) = handleDownloadFile(&handler, runtime) {
            runtime.write_error(error);
        }
    }
}

/// 先读本地文件；若无且非转发请求则遍历拓扑向其他节点拉取；均失败则 404。
pub fn handleDownloadFile<R: PlanReplayerRuntime>(
    handler: &downloadFileHandler,
    runtime: &mut R,
) -> Result<(), R::Error> {
    let forwarded = runtime.forwarded();
    let local_address = join_host_port(&handler.address, handler.status_port);
    // 本地命中：capture 包需注入历史统计后再作为 zip 返回。
    if let Some(mut content) = runtime.read_local_file(&handler.file_path)? {
        if handler.downloaded_filename == "plan_replayer"
            && handler.file_name.starts_with("capture_replayer")
        {
            content = handlePlanReplayerCaptureFile(&content, runtime)?;
        }
        write_zip_response(runtime, &handler.downloaded_filename, &content)?;
        runtime.log(
            "return dump file successfully",
            &handler.file_name,
            &local_address,
            forwarded,
        );
        return Ok(());
    }

    // 已是转发请求仍找不到文件，说明全链路无此 dump，直接 404。
    if forwarded {
        runtime.write_status(404);
        runtime.log(
            "failed to find dump file",
            &handler.file_name,
            &local_address,
            true,
        );
        return Ok(());
    }

    // 向其他节点转发拉取；成功即返回，失败则继续尝试下一节点。
    for topology in runtime.topology()? {
        if topology.ip == handler.address && topology.status_port == handler.status_port {
            continue;
        }
        let remote = join_host_port(&topology.ip, topology.status_port);
        let url = format!(
            "{}://{}/{}?forward=true",
            handler.scheme, remote, handler.url_path
        );
        match runtime.http_get(&url) {
            Ok(response) if response.status == 200 => {
                write_zip_response(runtime, &handler.downloaded_filename, &response.body)?;
                runtime.log(
                    "return dump file successfully in remote server",
                    &handler.file_name,
                    &remote,
                    false,
                );
                return Ok(());
            }
            Ok(_) => runtime.log(
                "can't find file in remote server",
                &handler.file_name,
                &remote,
                false,
            ),
            Err(error) => runtime.log_forward_error(&remote, &error),
        }
    }
    runtime.write_status(404);
    runtime.write_body(
        format!(
            "can't find dump file {} in any remote server",
            handler.file_name
        )
        .as_bytes(),
    )?;
    Ok(())
}

/// 设置 zip 附件响应头并写出正文。
fn write_zip_response<R: PlanReplayerRuntime>(
    runtime: &mut R,
    downloaded_filename: &str,
    content: &[u8],
) -> Result<(), R::Error> {
    runtime.set_header("Content-Type", "application/zip");
    runtime.set_header(
        "Content-Disposition",
        &format!("attachment; filename=\"{downloaded_filename}.zip\""),
    );
    runtime.write_body(content)
}

/// 处理 capture_replayer 包：解析 SQL meta 中的 start_ts，导出历史统计并写回 zip。
pub fn handlePlanReplayerCaptureFile<R: PlanReplayerRuntime>(
    content: &[u8],
    runtime: &mut R,
) -> Result<Vec<u8>, R::Error> {
    let mut archive = runtime.decode_zip(content)?;
    let snapshot = loadSQLMetaFile(&archive, runtime)?;
    // 无有效快照时间戳则无需注入历史统计，原样返回。
    if snapshot == 0 {
        return Ok(content.to_vec());
    }
    let mut tables = loadSchemaMeta(&archive, runtime)?;
    for table in tables.values_mut() {
        table.json_stats = Some(runtime.dump_historical_stats(&table.info, snapshot)?);
    }
    dumpJSONStatsIntoZipInMemory(&mut archive, &tables, runtime)
}

/// 从 `sql_meta.toml` 解析 `startTS`（事务开始时间戳，用于历史统计快照）；缺失则返回 0。
pub fn loadSQLMetaFile<R: PlanReplayerRuntime>(
    archive: &Archive,
    runtime: &mut R,
) -> Result<u64, R::Error> {
    let Some(content) = archive.entries.get(PLAN_REPLAYER_SQL_META_FILE) else {
        return Ok(0);
    };
    let text =
        std::str::from_utf8(content).map_err(|_| runtime.invalid_data("invalid SQL meta UTF-8"))?;
    let mut start_ts = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // startTS 对应该次捕获的事务开始时间戳（TSO）。
        if key.trim() == "startTS" {
            start_ts = Some(
                value
                    .trim()
                    .trim_matches(['\'', '"'])
                    .parse()
                    .map_err(|_| runtime.invalid_data("invalid startTS"))?,
            );
            break;
        }
    }
    start_ts.ok_or_else(|| runtime.invalid_data("missing startTS"))
}

/// 从 `schema/schema_meta.txt` 解析库表列表并解析为 TableMeta。
pub fn loadSchemaMeta<R: PlanReplayerRuntime>(
    archive: &Archive,
    runtime: &mut R,
) -> Result<HashMap<i64, tblInfo>, R::Error> {
    let Some(content) = archive
        .entries
        .get(&format!("schema/{PLAN_REPLAYER_SCHEMA_META_FILE}"))
    else {
        return Ok(HashMap::new());
    };
    let text = std::str::from_utf8(content)
        .map_err(|_| runtime.invalid_data("invalid schema meta UTF-8"))?;
    let mut tables = HashMap::new();
    for row in text.lines() {
        let mut fields = row.split(';');
        let (Some(database), Some(table)) = (fields.next(), fields.next()) else {
            continue;
        };
        let info = runtime.resolve_table(database, table)?;
        tables.insert(
            info.id,
            tblInfo {
                info,
                json_stats: None,
            },
        );
    }
    Ok(tables)
}

/// 将各表历史统计 JSON 写入归档的 `stats/` 目录并重新编码为 zip。
pub fn dumpJSONStatsIntoZipInMemory<R: PlanReplayerRuntime>(
    archive: &mut Archive,
    tables: &HashMap<i64, tblInfo>,
    runtime: &mut R,
) -> Result<Vec<u8>, R::Error> {
    for table in tables.values() {
        let stats = table
            .json_stats
            .as_ref()
            .ok_or_else(|| runtime.invalid_data("missing historical statistics"))?;
        archive.entries.insert(
            format!("stats/{}.{}.json", table.info.database, table.info.table),
            stats.clone(),
        );
    }
    runtime.encode_zip(archive)
}

/// 按 Go `filepath.Join` 语义合并并清理路径组件。
fn filepath_join(directory: &Path, name: &str) -> PathBuf {
    let mut path = directory.to_path_buf();
    for component in Path::new(name).components() {
        match component {
            std::path::Component::CurDir | std::path::Component::RootDir => {}
            std::path::Component::ParentDir => {
                path.pop();
            }
            std::path::Component::Normal(value) => path.push(value),
            std::path::Component::Prefix(_) => path.push(component.as_os_str()),
        }
    }
    path
}

/// 按 Go `net.JoinHostPort` 格式化主机与端口，IPv6 地址使用方括号。
fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
