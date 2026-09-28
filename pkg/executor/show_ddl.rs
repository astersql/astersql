// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `ADMIN SHOW DDL` 执行器：输出 DDL Owner 与当前作业摘要。
//
// DDL（Data Definition Language，数据定义语言）作业由集群中的 DDL Owner 节点调度；
// 本执行器将 schema 版本、Owner ID/地址、作业描述与原始 SQL 拼成单行结果。

#![allow(non_snake_case)]

use astersql_util_chunk::Chunk;

/// 单个 DDL 作业的展示摘要（描述文案与原始查询）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DdlJobInfo {
    pub description: String,
    pub query: String,
}

/// 当前 DDL 子系统快照：schema 版本与作业列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DdlInfo {
    /// InfoSchema（信息系统）版本号，随 DDL 提交递增。
    pub schema_version: i64,
    pub jobs: Vec<DdlJobInfo>,
}

/// 运行时边界：写 Chunk、解析 Owner 服务地址。
pub trait ShowDdlRuntime {
    type Context;
    type Error;

    fn reset_chunk(&self, request: &mut Chunk);
    /// 根据 server_id 解析 Owner 的 IP 与端口。
    fn server_address(
        &mut self,
        context: &mut Self::Context,
        server_id: &str,
    ) -> Result<(String, u16), Self::Error>;
    fn append_int64(&self, request: &mut Chunk, column: usize, value: i64);
    fn append_string(&self, request: &mut Chunk, column: usize, value: &str);
}

/// `ADMIN SHOW DDL` 执行器；一次性写出 schema 版本、Owner 与作业摘要。
pub struct ShowDDLExec<R: ShowDdlRuntime> {
    pub runtime: R,
    /// 当前 DDL Owner 的实例 ID。
    pub ddl_owner_id: String,
    /// 本节点实例 ID。
    pub self_id: String,
    pub ddl_info: DdlInfo,
    pub done: bool,
}

impl<R: ShowDdlRuntime> ShowDDLExec<R> {
    /// 首次调用组装六列结果行，之后返回空块。
    pub fn Next(&mut self, context: &mut R::Context, request: &mut Chunk) -> Result<(), R::Error> {
        self.runtime.reset_chunk(request);
        if self.done {
            return Ok(());
        }

        // 多作业时用换行拼接描述与原始 SQL，保持与 Go 版展示格式一致。
        let ddl_jobs = self
            .ddl_info
            .jobs
            .iter()
            .map(|job| job.description.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let queries = self
            .ddl_info
            .jobs
            .iter()
            .map(|job| job.query.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let (ip, port) = self.runtime.server_address(context, &self.ddl_owner_id)?;
        let address = format!("{ip}:{port}");

        self.runtime
            .append_int64(request, 0, self.ddl_info.schema_version);
        self.runtime.append_string(request, 1, &self.ddl_owner_id);
        self.runtime.append_string(request, 2, &address);
        self.runtime.append_string(request, 3, &ddl_jobs);
        self.runtime.append_string(request, 4, &self.self_id);
        self.runtime.append_string(request, 5, &queries);
        self.done = true;
        Ok(())
    }
}
