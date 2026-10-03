// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// External workload manager construction and request forwarding.
//
// 外部工作负载 Manager 的构造与请求转发实现。
//
// `NewManager` 在启用配置下拨号控制器、Ping 探活并安装 `manager`；
// 各业务方法为 RPC 注入超时 context 与指标标签后转发到底层客户端。

use std::time::Duration;

use crate::{Manager, client, config, context, grpc, keyspacepb, logutil, metrics, zap};

/// 同时限制管理器创建阶段的同步拨号和 Ping。
// dialTimeout 同时限制管理器创建阶段的同步拨号和 Ping。
#[allow(non_upper_case_globals)]
const dialTimeout: Duration = Duration::from_secs(30);

/// 限制管理器发出的每一次控制器请求。
// requestTimeout 限制管理器发出的每一次控制器请求。
#[allow(non_upper_case_globals)]
const requestTimeout: Duration = Duration::from_secs(30);

/// Manager 操作错误类型。
pub type ManagerError = Box<dyn std::error::Error + Send + Sync>;

/// Manager 具体实现：持有控制器客户端、角色与 keyspace 元数据。
// manager 对应 Go 具体实现，持有控制器客户端、当前角色和不可为空的 keyspace 元数据。
#[allow(non_camel_case_types)]
pub struct manager {
    /// 底层控制器客户端。
    pub cli: Box<dyn client::Client>,
    /// 当前外部工作负载角色。
    pub role: config::ExternalWorkloadRole,
    /// 绑定的 keyspace 元数据（构造时必非空）。
    pub meta: keyspacepb::KeyspaceMeta,
}

/// 写入 context 的指标维度，仅由拦截器读取。
// metricLabels 是写入 context 的指标维度，仅由拦截器读取。
#[allow(non_camel_case_types)]
#[derive(Clone)]
pub struct metricLabels {
    /// worker 类型标签（如 gcv2 / ttl）。
    pub workerType: String,
    /// 动作标签（init / abort / register / recycle）。
    pub action: String,
}

/// context 值键：零尺寸独立类型，避免与其他键冲突。
// metricLabelsKey 使用零尺寸独立类型，避免与 context 中其他值发生键冲突。
#[allow(non_camel_case_types)]
#[derive(Clone, Copy)]
pub struct metricLabelsKey;

/// 为启用 external-workload 的配置创建管理器。
///
/// 配置关闭时返回 `Ok(None)`（对应 Go `nil, nil`）；启用时 `keyspaceMeta` 必须非空。
// NewManager 对应 Go 构造函数，为启用 external-workload 的配置创建管理器。
// 配置关闭时返回 Ok(None)，显式表达 Go 的 nil, nil；启用时 keyspaceMeta 必须非空。
#[allow(non_snake_case)]
pub fn NewManager(
    context: &context::Context,
    keyspaceMeta: Option<&keyspacepb::KeyspaceMeta>,
    config: config::ExternalWorkload,
) -> Result<Option<Box<dyn Manager>>, ManagerError> {
    NewManagerWithTLS(context, keyspaceMeta, config, None)
}

/// Construct the production manager with the cluster's CA and client identity.
#[allow(non_snake_case)]
pub fn NewManagerWithTLS(
    context: &context::Context,
    keyspaceMeta: Option<&keyspacepb::KeyspaceMeta>,
    config: config::ExternalWorkload,
    tls_files: Option<(&str, &str, &str)>,
) -> Result<Option<Box<dyn Manager>>, ManagerError> {
    if !config.Enable {
        return Ok(None);
    }
    let keyspace_meta = keyspaceMeta.ok_or_else(|| {
        boxedError("external workload controller requires a non-nil keyspace meta")
    })?;

    let client = dialClient(context, keyspace_meta, &config, tls_files)
        .map_err(|error| annotateError("init external workload client", error))?;
    let result = manager {
        cli: client,
        role: config.Role.clone(),
        meta: keyspace_meta.clone(),
    };

    // 与 Go 一样在安装成功后记录角色、keyspace 名称和 ID；日志字段本身不改变管理器状态。
    logutil::BgLogger().Info(
        "external workload manager installed",
        vec![
            zap::String("role", result.role.to_string()),
            zap::String("keyspace", result.meta.GetName()),
            zap::Uint32("keyspace-id", result.meta.GetId()),
        ],
    );
    Ok(Some(Box::new(result)))
}

/// 构造 TLS、指标拦截器和客户端，并在受限 context 中执行首次 Ping。
// dialClient 构造 TLS、指标拦截器和客户端，并在受限 context 中执行首次 Ping。
#[allow(non_snake_case)]
fn dialClient(
    context: &context::Context,
    keyspaceMeta: &keyspacepb::KeyspaceMeta,
    config: &config::ExternalWorkload,
    tls_files: Option<(&str, &str, &str)>,
) -> Result<Box<dyn client::Client>, ManagerError> {
    let security = config::GetGlobalConfig().Security;
    let tls_config = if let Some((ca_path, cert_path, key_path)) = tls_files {
        let mut tls = tonic::transport::ClientTlsConfig::new();
        if !ca_path.is_empty() {
            let ca = std::fs::read(ca_path)?;
            tls = tls.ca_certificate(tonic::transport::Certificate::from_pem(ca));
        }
        if !cert_path.is_empty() || !key_path.is_empty() {
            let cert = std::fs::read(cert_path)?;
            let key = std::fs::read(key_path)?;
            tls = tls.identity(tonic::transport::Identity::from_pem(cert, key));
        }
        Some(client::TlsConfig(tls))
    } else if !security.ClusterSSLCA.is_empty() {
        // 仅配置了集群 CA 时才建立 TLS 配置；证书解析错误在创建客户端前返回。
        let cluster_security = security.ClusterSecurity();
        Some(
            cluster_security
                .ToTLSConfig()
                .map_err(|error| annotateError("build external workload TLS config", error))?,
        )
    } else {
        None
    };

    // Go 使用 defer cancel；这里保存结果后显式取消，保证成功和所有错误路径都会释放定时器资源。
    let (dial_context, cancel) = context::WithTimeout(context, dialTimeout);
    let option = client::Option {
        KeyspaceID: keyspaceMeta.GetId(),
        KeyspaceName: keyspaceMeta.GetName(),
        TiDBPool: config.TidbPool.clone(),
        ControllerAddr: config.ControllerAddr.clone(),
        TLSConfig: tls_config,
        Interceptors: vec![metricsInterceptor()],
    };
    let mut client = match client::New(Some(&option)) {
        Ok(client) => client,
        Err(error) => {
            // 构造失败也要执行 Go defer cancel 对应的定时器收尾。
            cancel();
            return Err(Box::new(error) as ManagerError);
        }
    };

    // Ping 使用同一个 30 秒拨号 context；失败时先尝试关闭已创建连接，关闭错误只记录警告。
    let ping_result = client.Ping(&dial_context);
    cancel();
    if let Err(error) = ping_result {
        if let Err(close_error) = client.Close() {
            logutil::BgLogger().Warn(
                "failed to close external workload client after ping failure",
                vec![zap::Error(close_error)],
            );
        }
        return Err(annotateError("ping external workload controller", error));
    }
    Ok(client)
}

impl Manager for manager {
    /// 透传到底层客户端，释放 gRPC 连接。
    // Close 透传到底层客户端，释放 gRPC 连接。
    fn Close(&mut self) -> Result<(), ManagerError> {
        self.cli
            .Close()
            .map_err(|error| Box::new(error) as ManagerError)
    }

    /// 返回构造时保存的 TiDB 外部工作负载角色。
    // Role 返回构造时保存的 TiDB 外部工作负载角色。
    fn Role(&self) -> config::ExternalWorkloadRole {
        self.role.clone()
    }

    /// 返回构造时绑定的 keyspace 元数据借用。
    // Meta 返回构造时绑定的 keyspace 元数据借用。
    fn Meta(&self) -> Option<&keyspacepb::KeyspaceMeta> {
        Some(&self.meta)
    }

    /// 注册 safePoint=0 的初始任务，并使用调用方加载的有效 GC 生命周期。
    // InitializeGCV2 注册 safePoint=0 的初始任务，并使用调用方加载的有效 GC 生命周期。
    fn InitializeGCV2(
        &mut self,
        context: &context::Context,
        gc_life_time: std::time::Duration,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let context = withMetric(
            &context,
            config::RoleGCV2Worker.to_string(),
            metrics::WorkerActionInit.to_owned(),
        );
        let result = self
            .cli
            .RegisterGCV2(&context, 0, gc_life_time.as_secs_f64() as i64);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 以 MaxUint64 请求控制器回收全部 GCV2 任务。
    // AbortGCV2 以 MaxUint64 请求控制器回收全部 GCV2 任务。
    fn AbortGCV2(&mut self, context: &context::Context) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let context = withMetric(
            &context,
            config::RoleGCV2Worker.to_string(),
            metrics::WorkerActionAbort.to_owned(),
        );
        let result = self.cli.RecycleGCV2(&context, u64::MAX);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 上报指定 safePoint 和调用方当前 gcLifeTime，并记录 register 指标。
    // RegisterGCV2 上报指定 safePoint 和调用方当前 gcLifeTime，并记录 register 指标。
    fn RegisterGCV2(
        &mut self,
        context: &context::Context,
        safePoint: u64,
        gcLifeTime: std::time::Duration,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let context = withMetric(
            &context,
            config::RoleGCV2Worker.to_string(),
            metrics::WorkerActionRegister.to_owned(),
        );
        let result = self
            .cli
            .RegisterGCV2(&context, safePoint, gcLifeTime.as_secs_f64() as i64);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 上报已处理到 safePoint，并记录 recycle 指标。
    // RecycleGCV2 上报已处理到 safePoint，并记录 recycle 指标。
    fn RecycleGCV2(
        &mut self,
        context: &context::Context,
        safePoint: u64,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let context = withMetric(
            &context,
            config::RoleGCV2Worker.to_string(),
            metrics::WorkerActionRecycle.to_owned(),
        );
        let result = self.cli.RecycleGCV2(&context, safePoint);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 只转发生命周期变化；Go 原实现没有为该动作附加指标标签。
    // UpdateGCLifeTime 只转发生命周期变化；Go 原实现没有为该动作附加指标标签。
    fn UpdateGCLifeTime(
        &mut self,
        context: &context::Context,
        gcLifeTime: std::time::Duration,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let result = self
            .cli
            .UpdateGCLifeTime(&context, gcLifeTime.as_secs_f64() as i64);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 转发 TTL 表状态并记录 TTL worker 的 register 指标。
    // RegisterTTLTask 转发 TTL 表状态并记录 TTL worker 的 register 指标。
    fn RegisterTTLTask(
        &mut self,
        context: &context::Context,
        tableID: i64,
        ttlJobEnable: bool,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let context = withMetric(
            &context,
            config::RoleTTLTaskWorker.to_string(),
            metrics::WorkerActionRegister.to_owned(),
        );
        let result = self.cli.RegisterTTLTask(&context, tableID, ttlJobEnable);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 转发表 ID；Go 原实现没有为删除动作附加指标标签。
    // DeleteTTLTableInfo 转发表 ID；Go 原实现没有为删除动作附加指标标签。
    fn DeleteTTLTableInfo(
        &mut self,
        context: &context::Context,
        tableID: i64,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let result = self.cli.DeleteTTLTableInfo(&context, tableID);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 转发已完成作业创建时间，并记录 TTL worker 的 recycle 指标。
    // RecycleTTLTask 转发已完成作业创建时间，并记录 TTL worker 的 recycle 指标。
    fn RecycleTTLTask(
        &mut self,
        context: &context::Context,
        completedJobCreateTime: u64,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let context = withMetric(
            &context,
            config::RoleTTLTaskWorker.to_string(),
            metrics::WorkerActionRecycle.to_owned(),
        );
        let result = self.cli.RecycleTTLTask(&context, completedJobCreateTime);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 只转发全局开关；Go 原实现没有为该动作附加指标标签。
    // UpdateTTLJobEnable 只转发全局开关；Go 原实现没有为该动作附加指标标签。
    fn UpdateTTLJobEnable(
        &mut self,
        context: &context::Context,
        ttlJobEnable: bool,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let result = self.cli.UpdateTTLJobEnable(&context, ttlJobEnable);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 转发任务 ID，并记录 auto-analyze worker 的 register 指标。
    // RegisterAutoAnalyze 转发任务 ID，并记录 auto-analyze worker 的 register 指标。
    fn RegisterAutoAnalyze(
        &mut self,
        context: &context::Context,
        taskID: u64,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let context = withMetric(
            &context,
            config::RoleAutoAnalyzeWorker.to_string(),
            metrics::WorkerActionRegister.to_owned(),
        );
        let result = self.cli.RegisterAutoAnalyze(&context, taskID);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }

    /// 转发任务 ID，并记录 auto-analyze worker 的 recycle 指标。
    // RecycleAutoAnalyze 转发任务 ID，并记录 auto-analyze worker 的 recycle 指标。
    fn RecycleAutoAnalyze(
        &mut self,
        context: &context::Context,
        taskID: u64,
    ) -> Result<(), ManagerError> {
        let (context, cancel) = withRequestTimeout(context);
        let context = withMetric(
            &context,
            config::RoleAutoAnalyzeWorker.to_string(),
            metrics::WorkerActionRecycle.to_owned(),
        );
        let result = self.cli.RecycleAutoAnalyze(&context, taskID);
        cancel();
        result.map_err(|error| Box::new(error) as ManagerError)
    }
}

/// 在 context 带有 metricLabels 且计数器已初始化时递增指标，然后始终调用下游 invoker。
// metricsInterceptor 在 context 带有 metricLabels 且计数器已初始化时递增对应指标，然后始终调用下游 invoker。
#[allow(non_snake_case)]
fn metricsInterceptor() -> grpc::UnaryClientInterceptor {
    Box::new(
        |context, method, request, reply, connection, invoker, options| {
            if let Some(labels) = context.Value::<metricLabelsKey, metricLabels>(metricLabelsKey) {
                if let Some(counter) = &metrics::ExternalWorkloadTaskCounter {
                    counter
                        .WithLabelValues(&labels.workerType, &labels.action)
                        .Inc();
                }
            }
            // 指标采集不能吞掉 RPC；无论是否存在标签，都将原参数完整传给 invoker。
            invoker(context, method, request, reply, connection, options)
        },
    )
}

/// 返回派生 context，并以独立键保存 worker 类型和动作标签。
// withMetric 返回派生 context，并以独立键保存 worker 类型和动作标签。
#[allow(non_snake_case)]
fn withMetric(context: &context::Context, workerType: String, action: String) -> context::Context {
    context::WithValue(
        context,
        metricLabelsKey,
        metricLabels { workerType, action },
    )
}

/// 为单次请求创建 30 秒超时 context；调用方必须在 RPC 返回后执行 CancelFunc。
// withRequestTimeout 为单次请求创建 30 秒超时 context；调用方必须在 RPC 返回后执行 CancelFunc。
#[allow(non_snake_case)]
fn withRequestTimeout(context: &context::Context) -> (context::Context, context::CancelFunc) {
    context::WithTimeout(context, requestTimeout)
}

/// 构造带消息的 boxed 错误（对应 Go `errors.New`）。
// boxedError 和 annotateError 对应 Go errors.New/Annotate，只保留错误消息链的可读形状。
#[allow(non_snake_case)]
fn boxedError(message: &str) -> ManagerError {
    Box::new(std::io::Error::new(std::io::ErrorKind::Other, message))
}

/// 在错误消息前附加注解前缀（对应 Go `errors.Annotate`）。
#[allow(non_snake_case)]
fn annotateError<E>(message: &str, error: E) -> ManagerError
where
    E: std::fmt::Display + Send + Sync + 'static,
{
    boxedError(&format!("{message}: {error}"))
}
