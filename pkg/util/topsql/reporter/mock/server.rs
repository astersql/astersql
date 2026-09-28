// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TopSQL Agent 的 tonic mock 服务端。
//
// 实现 `TopSqlAgent` 四类流式上报 RPC，缓存 SQL/Plan meta 与记录批次；
// 支持 Hang 窗口模拟慢下游，以及阻塞查询/计数等待，供 reporter 集成测试使用。

#![allow(non_snake_case, non_camel_case_types)]

use crate::tipb;
use crate::tipb::top_sql_agent_server::{TopSqlAgent, TopSqlAgentServer};
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, Streaming};

/// 挂起窗口：在 [beginTime, endTime) 内 `mayHang` 会 sleep 至 endTime。
#[derive(Clone, Copy)]
struct HangWindow {
    beginTime: Instant,
    endTime: Instant,
}

/// mock Agent 共享状态：hang、meta 映射与各类记录批次。
struct State {
    hang: RwLock<HangWindow>,
    sqlMetas: Mutex<HashMap<Vec<u8>, tipb::SqlMeta>>,
    planMetas: Mutex<HashMap<Vec<u8>, String>>,
    records: Mutex<Vec<Vec<tipb::TopSqlRecord>>>,
    ruRecords: Mutex<Vec<Vec<tipb::TopRuRecord>>>,
}

impl State {
    /// 构造空状态，hang 窗口初始为已结束。
    fn new() -> Self {
        let now = Instant::now();
        Self {
            hang: RwLock::new(HangWindow {
                beginTime: now,
                endTime: now,
            }),
            sqlMetas: Mutex::new(HashMap::with_capacity(5000)),
            planMetas: Mutex::new(HashMap::with_capacity(5000)),
            records: Mutex::new(Vec::new()),
            ruRecords: Mutex::new(Vec::new()),
        }
    }

    /// 若当前落在 hang 窗口内则异步等待到窗口结束。
    async fn mayHang(&self) {
        let window = *self.hang.read().expect("hang lock poisoned");
        let now = Instant::now();
        if now < window.endTime && now > window.beginTime {
            tokio::time::sleep_until(window.endTime.into()).await;
        }
    }
}

/// tonic 服务实现，持有共享 State。
#[derive(Clone)]
struct AgentService {
    state: Arc<State>,
}

#[tonic::async_trait]
impl TopSqlAgent for AgentService {
    // 读完流后整批压入 records，供 GetLatestRecords 取出。
    async fn report_top_sql_records(
        &self,
        request: Request<Streaming<tipb::TopSqlRecord>>,
    ) -> Result<Response<tipb::EmptyResponse>, Status> {
        let mut stream = request.into_inner();
        let mut records = Vec::with_capacity(10);
        loop {
            self.state.mayHang().await;
            match stream.message().await? {
                Some(record) => records.push(record),
                None => break,
            }
        }
        self.state
            .records
            .lock()
            .expect("records lock poisoned")
            .push(records);
        Ok(Response::new(tipb::EmptyResponse {}))
    }

    // 同样整批压入 ruRecords。
    async fn report_top_ru_records(
        &self,
        request: Request<Streaming<tipb::TopRuRecord>>,
    ) -> Result<Response<tipb::EmptyResponse>, Status> {
        let mut stream = request.into_inner();
        let mut records = Vec::with_capacity(10);
        loop {
            self.state.mayHang().await;
            match stream.message().await? {
                Some(record) => records.push(record),
                None => break,
            }
        }
        self.state
            .ruRecords
            .lock()
            .expect("RU records lock poisoned")
            .push(records);
        Ok(Response::new(tipb::EmptyResponse {}))
    }

    // 按 sql_digest 覆盖写入 sqlMetas。
    async fn report_sql_meta(
        &self,
        request: Request<Streaming<tipb::SqlMeta>>,
    ) -> Result<Response<tipb::EmptyResponse>, Status> {
        let mut stream = request.into_inner();
        loop {
            self.state.mayHang().await;
            match stream.message().await? {
                Some(meta) => {
                    self.state
                        .sqlMetas
                        .lock()
                        .expect("SQL metas lock poisoned")
                        .insert(meta.sql_digest.clone(), meta);
                }
                None => break,
            }
        }
        Ok(Response::new(tipb::EmptyResponse {}))
    }

    // 按 plan_digest 覆盖写入 planMetas（只存规范化 plan 文本）。
    async fn report_plan_meta(
        &self,
        request: Request<Streaming<tipb::PlanMeta>>,
    ) -> Result<Response<tipb::EmptyResponse>, Status> {
        let mut stream = request.into_inner();
        loop {
            self.state.mayHang().await;
            match stream.message().await? {
                Some(meta) => {
                    self.state
                        .planMetas
                        .lock()
                        .expect("plan metas lock poisoned")
                        .insert(meta.plan_digest, meta.normalized_plan);
                }
                None => break,
            }
        }
        Ok(Response::new(tipb::EmptyResponse {}))
    }
}

/// 可启停的 mock Agent：对外暴露 Address/Hang/查询与 Stop。
pub struct mockAgentServer {
    state: Arc<State>,
    addr: String,
    grpcServer: Option<oneshot::Sender<()>>,
}

/// Starts a real tonic server on an OS-assigned loopback port.
/// 在回环动态端口启动真实 tonic 服务并返回句柄。
pub async fn StartMockAgentServer() -> io::Result<mockAgentServer> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?.to_string();
    let state = Arc::new(State::new());
    let service = AgentService {
        state: state.clone(),
    };
    let (stop, stopped) = oneshot::channel();
    // serve 直至 oneshot 触发 shutdown。
    tokio::spawn(async move {
        let result = tonic::transport::Server::builder()
            .add_service(TopSqlAgentServer::new(service))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = stopped.await;
            })
            .await;
        if let Err(error) = result {
            eprintln!("mock agent server serve failed (category=top-sql): {error}");
        }
    });
    Ok(mockAgentServer {
        state,
        addr,
        grpcServer: Some(stop),
    })
}

impl mockAgentServer {
    /// 从当前时刻起设置 hang 窗口长度。
    pub fn HangFromNow(&self, duration: Duration) {
        let now = Instant::now();
        *self.state.hang.write().expect("hang lock poisoned") = HangWindow {
            beginTime: now,
            endTime: now + duration,
        };
    }

    /// TopSQL 记录批次数。
    pub fn RecordsCnt(&self) -> usize {
        self.state
            .records
            .lock()
            .expect("records lock poisoned")
            .len()
    }

    /// 已收集的 SQL meta 条数。
    pub fn SQLMetaCnt(&self) -> usize {
        self.state
            .sqlMetas
            .lock()
            .expect("SQL metas lock poisoned")
            .len()
    }

    /// 等待 TopSQL 批次增量达到 cnt（相对 old）或超时。
    pub fn WaitCollectCnt(&self, old: usize, cnt: usize, timeout: Duration) {
        self.waitUntil(timeout, || self.RecordsCnt().saturating_sub(old) >= cnt);
    }

    /// 等待 SQL meta 增量达到 cnt（相对 old）或超时。
    pub fn WaitCollectCntOfSQLMeta(&self, old: usize, cnt: usize, timeout: Duration) {
        self.waitUntil(timeout, || self.SQLMetaCnt().saturating_sub(old) >= cnt);
    }

    /// 自旋等待谓词成立或超时（1ms 间隔）。
    fn waitUntil(&self, timeout: Duration, predicate: impl Fn() -> bool) {
        let start = Instant::now();
        loop {
            if predicate() || start.elapsed() > timeout {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// 按 digest 阻塞查询 SQL meta；超时则返回默认值与 false。
    pub fn GetSQLMetaByDigestBlocking(
        &self,
        digest: &[u8],
        timeout: Duration,
    ) -> (tipb::SqlMeta, bool) {
        let start = Instant::now();
        loop {
            if let Some(meta) = self
                .state
                .sqlMetas
                .lock()
                .expect("SQL metas lock poisoned")
                .get(digest)
                .cloned()
            {
                return (meta, true);
            }
            if start.elapsed() > timeout {
                return (tipb::SqlMeta::default(), false);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// 按 digest 阻塞查询规范化 plan；超时返回空串与 false。
    pub fn GetPlanMetaByDigestBlocking(&self, digest: &[u8], timeout: Duration) -> (String, bool) {
        let start = Instant::now();
        loop {
            if let Some(plan) = self
                .state
                .planMetas
                .lock()
                .expect("plan metas lock poisoned")
                .get(digest)
                .cloned()
            {
                return (plan, true);
            }
            if start.elapsed() > timeout {
                return (String::new(), false);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// 取出并清空所有 TopSQL 批次，返回最后一批（对齐 Go 语义）。
    pub fn GetLatestRecords(&self) -> Option<Vec<tipb::TopSqlRecord>> {
        let records =
            std::mem::take(&mut *self.state.records.lock().expect("records lock poisoned"));
        records.into_iter().last()
    }

    /// 取出并清空所有 TopRU 批次，返回最后一批。
    pub fn GetLatestRURecords(&self) -> Option<Vec<tipb::TopRuRecord>> {
        let records = std::mem::take(
            &mut *self
                .state
                .ruRecords
                .lock()
                .expect("RU records lock poisoned"),
        );
        records.into_iter().last()
    }

    /// TopRU 记录批次数。
    pub fn RURecordsCnt(&self) -> usize {
        self.state
            .ruRecords
            .lock()
            .expect("RU records lock poisoned")
            .len()
    }

    /// 返回当前全部 SQL meta 的快照。
    pub fn GetTotalSQLMetas(&self) -> Vec<tipb::SqlMeta> {
        self.state
            .sqlMetas
            .lock()
            .expect("SQL metas lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// 监听地址。
    pub fn Address(&self) -> String {
        self.addr.clone()
    }

    /// 通过 oneshot 关闭 tonic serve 循环。
    pub fn Stop(&mut self) {
        if let Some(stop) = self.grpcServer.take() {
            let _ = stop.send(());
        }
    }
}

impl Drop for mockAgentServer {
    fn drop(&mut self) {
        self.Stop();
    }
}
