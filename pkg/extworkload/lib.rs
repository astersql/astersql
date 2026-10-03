// Copyright 2026 AsterSQL.

// 外部工作负载（external workload）包：协调 TiDB 后台作业与外部控制器。
//
// 本 crate 聚合 keyspace 元数据、角色配置、gRPC 客户端桩、以及
// `Manager` 接口与实现。后台作业包括：
// - GCV2：keyspace 级垃圾回收（Garbage Collection，清理过期版本数据）；
// - TTL：按表的存活时间删除过期行；
// - Auto Analyze：自动收集表统计信息。
//
// 迁移基线中大量 stub 类型用于脱离真实 PD/控制器依赖的单测。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_extworkload;

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// 精简版 `context.Context`：承载截止时间标志与类型化键值。
pub mod context {
    use super::*;

    /// 请求上下文：是否设置 deadline，以及按 TypeId 存放的附属值。
    #[derive(Clone, Default)]
    pub struct Context {
        /// 为 true 表示已通过 WithTimeout 设置截止时间。
        deadline: bool,
        /// 以 TypeId 为键的上下文值表（对应 Go context.WithValue）。
        values: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    }

    impl Context {
        /// 是否已设置请求超时/截止时间。
        pub fn Deadline(&self) -> bool {
            self.deadline
        }

        /// 按键类型取出上下文值；不存在或类型不匹配时返回 None。
        pub fn Value<K: 'static, V: Any + Clone + Send + Sync>(&self, _key: K) -> Option<V> {
            self.values
                .get(&TypeId::of::<K>())?
                .downcast_ref::<V>()
                .cloned()
        }
    }

    /// 取消函数：超时 context 结束时由调用方执行以释放定时器资源。
    pub type CancelFunc = Box<dyn FnOnce()>;

    /// 返回无截止时间、无附加值的背景上下文。
    pub fn Background() -> Context {
        Context::default()
    }

    /// 派生带超时标志的 context；桩实现不真正计时，仅置 deadline=true。
    pub fn WithTimeout(context: &Context, _timeout: Duration) -> (Context, CancelFunc) {
        let mut result = context.clone();
        // 标记已设置 deadline，供 Manager 单测断言请求超时已注入。
        result.deadline = true;
        (result, Box::new(|| {}))
    }

    /// 派生上下文并写入类型化键值（对应 Go context.WithValue）。
    pub fn WithValue<K, V>(context: &Context, _key: K, value: V) -> Context
    where
        K: 'static,
        V: Any + Clone + Send + Sync,
    {
        let mut result = context.clone();
        result.values.insert(TypeId::of::<K>(), Arc::new(value));
        result
    }
}

/// Keyspace（键空间）元数据的 protobuf 风格桩类型。
pub mod keyspacepb {
    /// 绑定到当前 TiDB 的 keyspace 标识：数值 ID 与名称。
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct KeyspaceMeta {
        /// keyspace 数值 ID。
        pub id: u32,
        /// keyspace 名称。
        pub name: String,
        /// PD keyspace settings, including GC management type.
        pub config: std::collections::BTreeMap<String, String>,
    }
    impl KeyspaceMeta {
        /// 返回 keyspace ID。
        pub fn GetId(&self) -> u32 {
            self.id
        }
        /// 返回 keyspace 名称副本。
        pub fn GetName(&self) -> String {
            self.name.clone()
        }
    }
}

/// 外部工作负载角色与全局安全配置桩。
pub mod config {
    /// 外部工作负载角色名（master / gcv2 / ttl / auto-analyze）。
    pub type ExternalWorkloadRole = String;
    /// 普通 TiDB 主角色：不专职跑某一类后台作业。
    pub const RoleMaster: &str = "master";
    /// 专职 keyspace 级 GC worker 角色。
    pub const RoleGCV2Worker: &str = "gcv2";
    /// 专职 TTL 作业 worker 角色。
    pub const RoleTTLTaskWorker: &str = "ttl";
    /// 专职自动分析 worker 角色。
    pub const RoleAutoAnalyzeWorker: &str = "auto-analyze";

    /// 外部工作负载开关与连接参数。
    #[derive(Clone, Default)]
    pub struct ExternalWorkload {
        /// 是否启用外部工作负载控制器对接。
        pub Enable: bool,
        /// 当前实例承担的角色。
        pub Role: ExternalWorkloadRole,
        /// TiDB 连接池名称，写入请求头供控制器识别。
        pub TidbPool: String,
        /// 控制器地址；测试中可用 stub:// 协议触发桩行为。
        pub ControllerAddr: String,
    }

    /// 集群 TLS 相关安全配置。
    #[derive(Clone, Default)]
    pub struct Security {
        /// 集群 CA 证书路径；非空时 Manager 才会构建 TLS。
        pub ClusterSSLCA: String,
    }
    /// 由 Security 导出的集群安全视图。
    pub struct ClusterSecurity;
    impl Security {
        /// 取得集群安全配置视图。
        pub fn ClusterSecurity(&self) -> ClusterSecurity {
            ClusterSecurity
        }
    }
    impl ClusterSecurity {
        /// 转为客户端 TLS 配置；桩实现恒成功返回空配置。
        pub fn ToTLSConfig(&self) -> Result<crate::client::TlsConfig, crate::client::ClientError> {
            Ok(crate::client::TlsConfig(
                tonic::transport::ClientTlsConfig::new(),
            ))
        }
    }
    /// 全局配置容器。
    #[derive(Clone, Default)]
    pub struct Config {
        /// 安全子配置。
        pub Security: Security,
    }
    /// 读取全局配置；桩实现返回默认值。
    pub fn GetGlobalConfig() -> Config {
        Config::default()
    }
}

/// gRPC 一元调用拦截器相关桩类型。
pub mod grpc {
    use crate::context;
    /// 客户端连接句柄桩。
    pub struct ClientConn;
    /// 单次调用选项桩。
    pub struct CallOption;
    /// 下游一元调用执行器：由拦截器在观测后转发。
    pub type UnaryInvoker = Box<
        dyn Fn(
                &context::Context,
                &str,
                &dyn std::any::Any,
                &mut dyn std::any::Any,
                &ClientConn,
                Vec<CallOption>,
            ) -> Result<(), crate::client::ClientError>
            + Send
            + Sync,
    >;
    /// 一元客户端拦截器：可在 RPC 前后注入指标等横切逻辑。
    pub type UnaryClientInterceptor = Box<
        dyn Fn(
                &context::Context,
                &str,
                &dyn std::any::Any,
                &mut dyn std::any::Any,
                &ClientConn,
                UnaryInvoker,
                Vec<CallOption>,
            ) -> Result<(), crate::client::ClientError>
            + Send
            + Sync,
    >;
}

/// 外部工作负载任务计数器与动作标签常量。
pub mod metrics {
    /// Prometheus 风格计数器桩。
    #[derive(Clone)]
    pub struct Counter;
    impl Counter {
        /// 按 worker 类型与动作取带标签的计数器视图。
        pub fn WithLabelValues(&self, _worker: &str, _action: &str) -> &Self {
            self
        }
        /// 计数加一。
        pub fn Inc(&self) {}
    }
    /// 全局任务计数器；未初始化时拦截器跳过打点。
    pub static ExternalWorkloadTaskCounter: Option<Counter> = None;
    /// 初始化动作标签。
    pub const WorkerActionInit: &str = "init";
    /// 中止动作标签。
    pub const WorkerActionAbort: &str = "abort";
    /// 注册动作标签。
    pub const WorkerActionRegister: &str = "register";
    /// 回收动作标签。
    pub const WorkerActionRecycle: &str = "recycle";
}

/// 结构化日志字段构造桩（对应 go.uber.org/zap）。
pub mod zap {
    /// 单条日志字段。
    pub struct Field;
    /// 字符串字段。
    pub fn String(_key: &str, _value: String) -> Field {
        Field
    }
    /// u32 字段。
    pub fn Uint32(_key: &str, _value: u32) -> Field {
        Field
    }
    /// 错误字段。
    pub fn Error<E>(_error: E) -> Field {
        Field
    }
}

/// 后台日志工具桩。
pub mod logutil {
    /// 日志记录器。
    pub struct Logger;
    impl Logger {
        /// 输出 Info 级别日志。
        pub fn Info(&self, _message: &str, _fields: Vec<crate::zap::Field>) {}
        /// 输出 Warn 级别日志。
        pub fn Warn(&self, _message: &str, _fields: Vec<crate::zap::Field>) {}
    }
    /// 取得进程级后台 Logger。
    pub fn BgLogger() -> Logger {
        Logger
    }
}

/// 控制器客户端接口与可配置桩实现（供 Manager 单测注入）。
pub mod client {
    use crate::{context, grpc};
    use std::fmt;

    /// TLS configuration forwarded to the production gRPC client.
    #[derive(Clone)]
    pub struct TlsConfig(pub tonic::transport::ClientTlsConfig);
    /// 创建客户端所需的连接与身份选项。
    pub struct Option {
        /// keyspace 数值 ID，写入请求头。
        pub KeyspaceID: u32,
        /// keyspace 名称。
        pub KeyspaceName: String,
        /// TiDB 池名。
        pub TiDBPool: String,
        /// 控制器地址；`stub://ping-error` 使 Ping 失败。
        pub ControllerAddr: String,
        /// 可选 TLS。
        pub TLSConfig: std::option::Option<TlsConfig>,
        /// 一元调用拦截器链。
        pub Interceptors: Vec<grpc::UnaryClientInterceptor>,
    }
    /// 客户端错误，消息字符串对应 Go error 文本。
    #[derive(Clone, Debug)]
    pub struct ClientError(pub String);
    impl fmt::Display for ClientError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl std::error::Error for ClientError {}

    /// 与外部工作负载控制器通信的客户端契约。
    pub trait Client: Send {
        /// 向下转型为具体类型（测试取 FakeClient 状态）。
        fn as_any(&self) -> &dyn std::any::Any;
        /// 关闭连接并释放资源。
        fn Close(&mut self) -> Result<(), ClientError>;
        /// 探测控制器可达性。
        fn Ping(&mut self, context: &context::Context) -> Result<(), ClientError>;
        /// 注册 keyspace 级 GC 轮次（safe_point + gc_life_time）。
        fn RegisterGCV2(
            &mut self,
            context: &context::Context,
            safe_point: u64,
            gc_life_time: i64,
        ) -> Result<(), ClientError>;
        /// 回收截至 safe_point 的 GC 任务。
        fn RecycleGCV2(
            &mut self,
            context: &context::Context,
            safe_point: u64,
        ) -> Result<(), ClientError>;
        /// 上报 gc_life_time 配置变更。
        fn UpdateGCLifeTime(
            &mut self,
            context: &context::Context,
            gc_life_time: i64,
        ) -> Result<(), ClientError>;
        /// 注册或更新 TTL 表任务。
        fn RegisterTTLTask(
            &mut self,
            context: &context::Context,
            table_id: i64,
            enabled: bool,
        ) -> Result<(), ClientError>;
        /// 删除表的 TTL 任务信息。
        fn DeleteTTLTableInfo(
            &mut self,
            context: &context::Context,
            table_id: i64,
        ) -> Result<(), ClientError>;
        /// 回收已完成的 TTL 作业。
        fn RecycleTTLTask(
            &mut self,
            context: &context::Context,
            create_time: u64,
        ) -> Result<(), ClientError>;
        /// 上报全局 TTL 作业开关变更。
        fn UpdateTTLJobEnable(
            &mut self,
            context: &context::Context,
            enabled: bool,
        ) -> Result<(), ClientError>;
        /// 注册自动分析任务。
        fn RegisterAutoAnalyze(
            &mut self,
            context: &context::Context,
            task_id: u64,
        ) -> Result<(), ClientError>;
        /// 回收已完成的自动分析任务。
        fn RecycleAutoAnalyze(
            &mut self,
            context: &context::Context,
            task_id: u64,
        ) -> Result<(), ClientError>;
    }

    /// 可配置 Ping 成败的控制器桩。
    struct StubController {
        /// 为 true 时 Ping 返回错误。
        ping_error: bool,
    }
    impl Client for StubController {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn Close(&mut self) -> Result<(), ClientError> {
            Ok(())
        }
        fn Ping(&mut self, _: &context::Context) -> Result<(), ClientError> {
            if self.ping_error {
                Err(ClientError("boom".into()))
            } else {
                Ok(())
            }
        }
        fn RegisterGCV2(
            &mut self,
            _: &context::Context,
            _: u64,
            _: i64,
        ) -> Result<(), ClientError> {
            unreachable!()
        }
        fn RecycleGCV2(&mut self, _: &context::Context, _: u64) -> Result<(), ClientError> {
            unreachable!()
        }
        fn UpdateGCLifeTime(&mut self, _: &context::Context, _: i64) -> Result<(), ClientError> {
            unreachable!()
        }
        fn RegisterTTLTask(
            &mut self,
            _: &context::Context,
            _: i64,
            _: bool,
        ) -> Result<(), ClientError> {
            unreachable!()
        }
        fn DeleteTTLTableInfo(&mut self, _: &context::Context, _: i64) -> Result<(), ClientError> {
            unreachable!()
        }
        fn RecycleTTLTask(&mut self, _: &context::Context, _: u64) -> Result<(), ClientError> {
            unreachable!()
        }
        fn UpdateTTLJobEnable(&mut self, _: &context::Context, _: bool) -> Result<(), ClientError> {
            unreachable!()
        }
        fn RegisterAutoAnalyze(&mut self, _: &context::Context, _: u64) -> Result<(), ClientError> {
            unreachable!()
        }
        fn RecycleAutoAnalyze(&mut self, _: &context::Context, _: u64) -> Result<(), ClientError> {
            unreachable!()
        }
    }
    /// 按选项构造客户端；地址为 stub://ping-error 时 Ping 将失败。
    pub fn New(option: std::option::Option<&Option>) -> Result<Box<dyn Client>, ClientError> {
        if let Some(option) = option {
            if !option.ControllerAddr.starts_with("stub://") {
                return Ok(Box::new(crate::real_client::RealController::connect(
                    option,
                )?));
            }
        }
        // 测试用特殊地址：创建阶段 Ping 失败，验证 NewManager 错误路径。
        let ping_error = option
            .map(|option| option.ControllerAddr == "stub://ping-error")
            .unwrap_or(false);
        Ok(Box::new(StubController { ping_error }))
    }
}

#[path = "real_client.rs"]
mod real_client;

/// Manager 接口定义（Close / Role / GC·TTL·Analyze 上报）。
#[path = "external_workload.rs"]
mod external_workload;
pub use external_workload::*;
/// Manager 具体实现与 NewManager 构造。
#[path = "manager.rs"]
pub mod manager_impl;
pub use manager_impl::{NewManager, NewManagerWithTLS, manager};
/// 角色谓词工具（IsMaster / IsGCV2Worker 等）。
#[path = "util.rs"]
mod util;
pub use util::*;

/// Manager 生命周期与方法转发单测。
#[cfg(test)]
#[path = "manager_test.rs"]
mod manager_test;
/// 与 Go 行为对齐的迁移回归测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
/// 角色谓词单测。
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
