// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Checksum executor — port of `br/pkg/checksum/executor.go`.
//!
//! 构建并执行 DistSQL checksum 请求，校验表与索引数据一致性。
//! 数据流：`ExecutorBuilder` 收集表/TS/并发/keyspace → `Build` 展开为
//! 每表（及分区）的 table + public 索引 `Request` → `Executor::Execute`
//! 经 DistSQL 发送并 XOR 聚合 `ChecksumResponse`。
//!
//! 约束：若提供 `old_table`，新表每个 public 索引必须在旧表同名存在，
//! 否则 `panic`（对齐 Go `log.Panic`）；非 public 索引跳过。
//! Common Handle 用 `FullNotNullRange`，整型句柄用 `FullIntRange`。
//! 请求优先级默认 Low，减少对在线流量的干扰。
//! rewrite 时 OldPrefix/NewPrefix 由 keyspace 与表/索引编码前缀拼接而成。
//!
//! 与 Go 对齐要点：
//! - rewrite 前缀 = keyspace ‖ record/index 前缀，供跨集群校验同一逻辑行；
//! - 请求优先级固定 `PriorityLow`，降低对在线业务的干扰；
//! - `sendChecksumRequest` 的 Close 错误覆盖成功结果（对齐 Go named-return defer）；
//! - `Execute` 内 `WithRetry` + failpoint `checksumRetryErr` 注入首错重试路径；
//! - 全部请求聚合后再 `checkContextDone`，避免取消竞态污染已算结果。
//! 建造者字段均为可选覆盖；未设置时并发取 `DefDistSQLScanConcurrency`，
//! backoff_weight=0 表示沿用 session 默认退避。
//! 分区展开按定义顺序；旧分区 ID 由 `GetPartitionByName` 解析，找不到则报错。
//! 非 StatePublic 索引跳过，避免对构建中索引做无意义校验。
//! `updateChecksumResponse`：Checksum XOR，TotalKvs/TotalBytes 累加，不可改为求和校验和。
//! `RawRequests` 仅反序列化内嵌 ChecksumRequest，不触发 DistSQL。
//! `Each` 供测试窥视 Request 列表，回调错误经 Trace 上抛。
//! DistSQL/checksum 相关类型来自本 crate stubs，非完整 TiKV 客户端。

use crate::stubs::{
    self, ChecksumAlgorithm, ChecksumRequest, ChecksumResponse, ChecksumRewriteRule,
    ChecksumScanOn, Client, Context, DefDistSQLScanConcurrency, DistSQLChecksum,
    EncodeTableIndexPrefix, Error, FullIntRange, FullNotNullRange, FullRange, GenTableRecordPrefix,
    GetPartitionByName, MetaTable, NewChecksumBackoffStrategy, NewVariables, PriorityLow, Request,
    RequestBuilder, RequestSource, Result, TableInfo, WithRetry, take_checksum_retry_err,
};

/// 构造 checksum `kv.Request` 列表的建造者。
/// ExecutorBuilder is used to build a "kv.Request".
pub struct ExecutorBuilder {
    /// 目标（新）表元信息
    table: TableInfo,
    /// 读快照 TS
    ts: u64,
    /// 可选旧表：启用前缀 rewrite
    old_table: Option<MetaTable>,
    /// DistSQL 扫描并发
    concurrency: u32,
    /// 重试退避权重
    backoff_weight: i32,
    /// 旧 keyspace 前缀
    old_keyspace: Vec<u8>,
    /// 新 keyspace 前缀
    new_keyspace: Vec<u8>,
    /// 资源组名
    resource_group_name: String,
    /// 请求来源标记
    request_source: RequestSource,
}

/// 以默认 DistSQL 并发创建建造者；其它字段空/零。
/// NewExecutorBuilder returns a new executor builder.
pub fn NewExecutorBuilder(table: TableInfo, ts: u64) -> ExecutorBuilder {
    ExecutorBuilder {
        table,
        ts,
        old_table: None,
        // 默认并发取自 DistSQL 扫描常量
        concurrency: DefDistSQLScanConcurrency,
        // 0 表示使用 Variables 默认退避
        backoff_weight: 0,
        old_keyspace: Vec::new(),
        new_keyspace: Vec::new(),
        resource_group_name: String::new(),
        request_source: RequestSource::default(),
    }
}

impl ExecutorBuilder {
    /// 设置旧表以启用 checksum rewrite（跨 ID/keyspace 校验）。
    /// SetOldTable set a old table info to the builder.
    pub fn SetOldTable(mut self, old_table: MetaTable) -> Self {
        self.old_table = Some(old_table);
        self
    }

    /// 覆盖默认扫描并发。
    /// SetConcurrency set the concurrency of the checksum executing.
    pub fn SetConcurrency(mut self, conc: u32) -> Self {
        self.concurrency = conc;
        self
    }

    /// 设置重试退避权重。
    /// SetBackoffWeight set the backoffWeight of the checksum executing.
    pub fn SetBackoffWeight(mut self, backoff_weight: i32) -> Self {
        self.backoff_weight = backoff_weight;
        self
    }

    /// 旧集群 keyspace 字节前缀。
    pub fn SetOldKeyspace(mut self, keyspace: Vec<u8>) -> Self {
        self.old_keyspace = keyspace;
        self
    }

    /// 新集群 keyspace 字节前缀。
    pub fn SetNewKeyspace(mut self, keyspace: Vec<u8>) -> Self {
        self.new_keyspace = keyspace;
        self
    }

    /// 绑定资源组，限制对在线流量影响。
    pub fn SetResourceGroupName(mut self, name: String) -> Self {
        self.resource_group_name = name;
        self
    }

    /// 设置完整 RequestSource。
    pub fn SetRequestSource(mut self, req_source: RequestSource) -> Self {
        self.request_source = req_source;
        self
    }

    /// 仅覆盖 ExplicitRequestSourceType。
    pub fn SetExplicitRequestSourceType(mut self, name: String) -> Self {
        self.request_source.ExplicitRequestSourceType = name;
        self
    }

    /// 只读访问当前 RequestSource。
    pub fn request_source(&self) -> &RequestSource {
        &self.request_source
    }

    /// 展开请求并得到可执行的 `Executor`。
    /// Build builds a checksum executor.
    pub fn Build(self) -> Result<Executor> {
        let reqs = buildChecksumRequest(
            &self.table,
            self.old_table.as_ref(),
            self.ts,
            self.concurrency,
            &self.old_keyspace,
            &self.new_keyspace,
            &self.resource_group_name,
            &self.request_source,
        )
        .map_err(Error::Trace)?;
        Ok(Executor {
            reqs,
            backoff_weight: self.backoff_weight,
        })
    }
}

/// 为整表及每个分区各生成一组 table+index 请求。
/// 容量预估：`(索引数+1)*(分区数+1)`。
fn buildChecksumRequest(
    new_table: &TableInfo,
    old_table: Option<&MetaTable>,
    start_ts: u64,
    concurrency: u32,
    old_keyspace: &[u8],
    new_keyspace: &[u8],
    resource_group_name: &str,
    request_source: &RequestSource,
) -> Result<Vec<Request>> {
    // 无分区时为空，仅处理表级 ID
    let part_defs = new_table
        .Partition
        .as_ref()
        .map(|p| p.Definitions.clone())
        .unwrap_or_default();

    // 预分配避免分区×索引场景下反复扩容
    let mut reqs = Vec::with_capacity((new_table.Indices.len() + 1) * (part_defs.len() + 1));
    // 无旧表时 id=0，rewrite 规则不会启用
    let old_table_id = old_table.map(|t| t.Info.ID).unwrap_or(0);
    let mut rs = buildRequest(
        new_table,
        new_table.ID,
        old_table,
        old_table_id,
        start_ts,
        concurrency,
        old_keyspace,
        new_keyspace,
        resource_group_name,
        request_source,
    )
    .map_err(Error::Trace)?;
    reqs.append(&mut rs);

    // 按分区名在旧表查找对应 partition id
    for part_def in part_defs {
        // 无旧表时保持 0
        let mut old_part_id = 0i64;
        if let Some(old) = old_table {
            // 旧分区必须同名存在
            old_part_id = GetPartitionByName(&old.Info, &part_def.Name).map_err(Error::Trace)?;
        }
        let mut rs = buildRequest(
            new_table,
            part_def.ID,
            old_table,
            old_part_id,
            start_ts,
            concurrency,
            old_keyspace,
            new_keyspace,
            resource_group_name,
            request_source,
        )
        .map_err(Error::Trace)?;
        reqs.append(&mut rs);
    }

    Ok(reqs)
}

/// 对单个 table/partition id：先 table 请求，再每个 StatePublic 索引。
/// 索引按 `Name.L` 匹配旧表；缺省同名索引时 panic，防止 silent 错校验。
fn buildRequest(
    table_info: &TableInfo,
    table_id: i64,
    old_table: Option<&MetaTable>,
    old_table_id: i64,
    start_ts: u64,
    concurrency: u32,
    old_keyspace: &[u8],
    new_keyspace: &[u8],
    resource_group_name: &str,
    request_source: &RequestSource,
) -> Result<Vec<Request>> {
    let mut reqs = Vec::new();
    // 先推表请求，再推索引请求
    let req = buildTableRequest(
        table_info,
        table_id,
        old_table,
        old_table_id,
        start_ts,
        concurrency,
        old_keyspace,
        new_keyspace,
        resource_group_name,
        request_source,
    )
    .map_err(Error::Trace)?;
    reqs.push(req);

    // 仅遍历 public 索引
    for index_info in &table_info.Indices {
        // 非 public 索引不参与 checksum（与 Go 一致）
        if index_info.State != stubs::StatePublic {
            continue;
        }
        let mut old_index_info: Option<&stubs::IndexInfo> = None;
        if let Some(old) = old_table {
            for old_index in &old.Info.Indices {
                // Go 直接比较 ast.CIStr，O/L 都必须一致。
                if old_index.Name == index_info.Name {
                    old_index_info = Some(old_index);
                    break;
                }
            }
            if old_index_info.is_none() {
                // 旧表缺同名索引：无法构造 rewrite，直接 panic
                // Go log.Panic — restore table must share index info with origin.
                panic!(
                    "index not found in origin table, please check the restore table has the same index info with origin table: \
                     table id={}, table name={}, origin table id={}, origin table name={}, index name={}",
                    table_id, table_info.Name, old_table_id, old.Info.Name, index_info.Name
                );
            }
        }
        let req = buildIndexRequest(
            table_id,
            index_info,
            old_table_id,
            old_index_info,
            start_ts,
            concurrency,
            old_keyspace,
            new_keyspace,
            resource_group_name,
            request_source,
        )
        .map_err(Error::Trace)?;
        reqs.push(req);
    }

    Ok(reqs)
}

/// 表数据 checksum；Common Handle→FullNotNullRange，否则 FullIntRange。
fn buildTableRequest(
    table_info: &TableInfo,
    table_id: i64,
    old_table: Option<&MetaTable>,
    old_table_id: i64,
    start_ts: u64,
    concurrency: u32,
    old_keyspace: &[u8],
    new_keyspace: &[u8],
    resource_group_name: &str,
    request_source: &RequestSource,
) -> Result<Request> {
    // 拼接 keyspace + record 前缀形成 rewrite 规则
    let rule = if old_table.is_some() {
        let mut old_prefix = old_keyspace.to_vec();
        old_prefix.extend_from_slice(&GenTableRecordPrefix(old_table_id));
        let mut new_prefix = new_keyspace.to_vec();
        new_prefix.extend_from_slice(&GenTableRecordPrefix(table_id));
        // Old/New 前缀成对出现
        Some(ChecksumRewriteRule {
            OldPrefix: old_prefix,
            NewPrefix: new_prefix,
        })
    } else {
        None
    };

    // 表扫描 + Crc64_Xor 算法
    let checksum = ChecksumRequest {
        ScanOn: ChecksumScanOn::Table,
        Algorithm: ChecksumAlgorithm::Crc64_Xor,
        Rule: rule,
    };

    // 句柄类型决定扫描范围
    let ranges = if table_info.IsCommonHandle {
        FullNotNullRange()
    } else {
        FullIntRange(false)
    };

    // RequestBuilder 组装 DistSQL 请求外壳
    let mut builder = RequestBuilder::default();
    // 低优先级，降低对在线请求影响
    // Use low priority to reducing impact to other requests.
    // 写入低优先级后再链式 Set*
    builder.Request.Priority = PriorityLow;
    builder
        // 绑定句柄范围、TS、checksum 载荷与并发
        .SetHandleRanges(None, table_id, table_info.IsCommonHandle, ranges)
        .SetStartTS(start_ts)
        .SetChecksumRequest(&checksum)
        .SetConcurrency(concurrency as i32)
        .SetResourceGroupName(resource_group_name)
        .SetRequestSource(request_source.clone())
        .Build()
}

/// 索引 checksum：ScanOn=Index；rewrite 用 EncodeTableIndexPrefix。
fn buildIndexRequest(
    table_id: i64,
    index_info: &stubs::IndexInfo,
    old_table_id: i64,
    old_index_info: Option<&stubs::IndexInfo>,
    start_ts: u64,
    concurrency: u32,
    old_keyspace: &[u8],
    new_keyspace: &[u8],
    resource_group_name: &str,
    request_source: &RequestSource,
) -> Result<Request> {
    let rule = if let Some(old_index) = old_index_info {
        let mut old_prefix = old_keyspace.to_vec();
        old_prefix.extend_from_slice(&EncodeTableIndexPrefix(old_table_id, old_index.ID));
        let mut new_prefix = new_keyspace.to_vec();
        new_prefix.extend_from_slice(&EncodeTableIndexPrefix(table_id, index_info.ID));
        Some(ChecksumRewriteRule {
            OldPrefix: old_prefix,
            NewPrefix: new_prefix,
        })
    } else {
        None
    };

    // 索引扫描 + Crc64_Xor 算法
    let checksum = ChecksumRequest {
        ScanOn: ChecksumScanOn::Index,
        Algorithm: ChecksumAlgorithm::Crc64_Xor,
        Rule: rule,
    };

    let ranges = FullRange();

    let mut builder = RequestBuilder::default();
    // Use low priority to reducing impact to other requests.
    builder.Request.Priority = PriorityLow;
    builder
        .SetIndexRanges(None, table_id, index_info.ID, ranges)
        .SetStartTS(start_ts)
        .SetChecksumRequest(&checksum)
        .SetConcurrency(concurrency as i32)
        .SetResourceGroupName(resource_group_name)
        .SetRequestSource(request_source.clone())
        .Build()
}

/// 经 DistSQLChecksum 发送单请求；Variables 注入 backoff_weight。
/// 可注入 take_checksum_retry_err 以模拟失败重试路径。
fn sendChecksumRequest(
    ctx: &Context,
    client: &dyn Client,
    req: &Request,
    vars: &VariablesWrap,
) -> Result<ChecksumResponse> {
    let mut res = DistSQLChecksum(ctx, client, req, &vars.inner).map_err(Error::Trace)?;
    let mut resp = ChecksumResponse::default();
    let mut close_err: Option<Error> = None;

    // 循环消费分片，直到流结束或出错。
    loop {
        let data = match res.NextRaw(ctx) {
            Ok(d) => d,
            Err(err) => {
                // Go named-return defer: Close 错误覆盖已有读错误。
                return match res.Close() {
                    Err(close_err) => Err(close_err),
                    Ok(()) => Err(Error::Trace(err)),
                };
            }
        };
        let Some(data) = data else {
            break;
        };
        let checksum = match ChecksumResponse::Unmarshal(&data) {
            Ok(c) => c,
            Err(err) => {
                // Go named-return defer: Close 错误同样覆盖 Unmarshal 错误。
                return match res.Close() {
                    Err(close_err) => Err(close_err),
                    Ok(()) => Err(Error::Trace(err)),
                };
            }
        };
        updateChecksumResponse(&mut resp, &checksum);
    }

    // Go named-return defer: Close error overwrites success / prior err.
    // 对齐 Go：即便分片已成功聚合，Close 失败仍覆盖为错误返回。
    if let Err(err) = res.Close() {
        close_err = Some(err);
    }
    if let Some(err) = close_err {
        return Err(err);
    }
    Ok(resp)
}

/// 包装 session Variables，供重试策略读取 backoff。
struct VariablesWrap {
    inner: stubs::Variables,
}

/// 聚合响应：Checksum XOR，TotalKvs/TotalBytes 累加。
/// updateChecksumResponse aggregates checksum XOR and kv/bytes sums.
pub fn updateChecksumResponse(resp: &mut ChecksumResponse, update: &ChecksumResponse) {
    // Crc64 用 XOR；计数与字节累加
    resp.Checksum ^= update.Checksum;
    // Go uint64 arithmetic wraps modulo 2^64 in every build mode.
    resp.TotalKvs = resp.TotalKvs.wrapping_add(update.TotalKvs);
    resp.TotalBytes = resp.TotalBytes.wrapping_add(update.TotalBytes);
}

/// Executor is a checksum executor.
/// 已构建的请求集 + 退避权重；`Execute` 时发送并聚合。
pub struct Executor {
    /// DistSQL 请求列表
    reqs: Vec<Request>,
    /// 重试退避权重
    backoff_weight: i32,
}

impl Executor {
    /// Len returns the total number of checksum requests.
    /// 请求条数（表/分区 × (1+public 索引)）。
    pub fn Len(&self) -> usize {
        self.reqs.len()
    }

    /// Each executes the function to each requests in the executor.
    /// 遍历底层 Request，供测试或上层检查。
    pub fn Each<F>(&self, mut f: F) -> Result<()>
    where
        F: FnMut(&Request) -> Result<()>,
    {
        // 逐请求发送；任一步失败则整体返回
        for req in &self.reqs {
            f(req).map_err(Error::Trace)?;
        }
        Ok(())
    }

    /// 反序列化各 Request 内嵌的 ChecksumRequest，便于断言。
    /// RawRequests extracts the raw requests associated with this executor.
    pub fn RawRequests(&self) -> Result<Vec<ChecksumRequest>> {
        let mut res = Vec::with_capacity(self.reqs.len());
        for req in &self.reqs {
            let raw_req = ChecksumRequest::Unmarshal(&req.Data).map_err(Error::Trace)?;
            res.push(raw_req);
        }
        Ok(res)
    }

    /// 对全部请求执行 checksum 并聚合；支持 ctx 取消与重试。
    /// Execute executes a checksum executor.
    /// `update_fn` 在每个 Request 成功后回调，供上层推进进度条（对齐 Go）。
    pub fn Execute<F>(
        &self,
        ctx: &Context,
        client: &dyn Client,
        mut update_fn: F,
    ) -> Result<ChecksumResponse>
    where
        F: FnMut(),
    {
        let mut checksum_resp = ChecksumResponse::default();
        // 逐请求执行：单请求失败立即返回，不继续后续请求。
        for req in &self.reqs {
            // Pointer to SessionVars.Killed — reserved slot in BR.
            // killed 指针槽位预留给 session kill；BR 路径当前固定 0。
            let killed: u32 = 0;
            let mut resp: Option<ChecksumResponse> = None;
            // 单请求带退避重试
            let err = WithRetry(
                ctx,
                || {
                    // 新建 Variables 并写入 backoff
                    let mut vars = NewVariables(&killed);
                    // 仅当建造者显式设置正权重时覆盖退避。
                    if self.backoff_weight > 0 {
                        vars.BackOffWeight = self.backoff_weight;
                    }
                    let wrap = VariablesWrap { inner: vars };
                    let mut inner_err: Option<Error> = None;
                    match sendChecksumRequest(ctx, client, req, &wrap) {
                        Ok(r) => resp = Some(r),
                        Err(e) => inner_err = Some(e),
                    }
                    // failpoint checksumRetryErr — first hit returns inject error
                    // 测试注入：首次命中则伪装发送失败，验证重试路径。
                    if take_checksum_retry_err() {
                        inner_err = Some(Error::new("inject checksum error"));
                    }
                    if let Some(err) = inner_err {
                        return Err(Error::Trace(err));
                    }
                    Ok(())
                },
                NewChecksumBackoffStrategy(),
            );
            if let Err(err) = err {
                return Err(Error::Trace(err));
            }
            // 合并本请求结果到总量
            updateChecksumResponse(&mut checksum_resp, resp.as_ref().unwrap());
            update_fn();
        }
        // 全部请求完成后再次确认未被取消
        checkContextDone(ctx)?;
        Ok(checksum_resp)
    }
}

/// 若上下文已取消则返回其错误；防止 CONTEXT DONE 后继续产生校验结果。
/// checkContextDone makes sure the result is not affected by CONTEXT DONE.
pub fn checkContextDone(ctx: &Context) -> Result<()> {
    // Done 时透传取消原因
    if let Some(ctx_err) = ctx.Err() {
        return Err(Error::Annotate(
            ctx_err,
            "context is cancelled by other error",
        ));
    }
    Ok(())
}
