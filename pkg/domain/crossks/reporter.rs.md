# `pkg/domain/crossks/reporter.rs`

## 文件定位

本文件说明的真实源码为 [`reporter.rs`](./reporter.rs)。它属于 Cargo 包 `astersql-domain-crossks`，其 crate 根为 `pkg/domain/crossks/lib.rs`。模块入口通过 `pub mod reporter` 声明该模块，并以 `pub use reporter::*` 将其中的公开项重导出到 crate 根。文件当前只有 28 行，是跨 keyspace（多租户命名空间）域中的最小事务开始时间戳上报占位门面，而不是实际的时间戳计算或 etcd 写入实现。

从当前 Rust 接线看，本文件尚未进入 server-info 的运行链：`pkg/domain/crossks/cross_ks.rs::new_manager_with_server_info` 接受的是 `Arc<dyn astersql_domain_serverinfo::MinStartTSReporter>`，实际测试传入 `pkg/domain/serverinfo/syncer.rs::NoopMinStartTSReporter`；仓库内除模块导出外未发现 `crossks::MinStartTsReporter` 的构造或调用。因而本文件目前的准确定位是“公开、可构造，但未接线的 no-op 兼容占位”。

## 核心职责

1. 用 `MinStartTsReporter` 保留跨 keyspace 场景的“上报最小 start_ts”概念和 API 位置。
2. 用显式空操作 `report_min_start_ts` 表达当前 Rust 移植没有可报告的 TSO 数据，避免伪造时间戳或产生外部副作用。
3. 与 Go 同路径文件 `pkg/domain/crossks/reporter.go` 的空实现保持意图一致，为后续接入真实 reporter 留出稳定的模块边界。

它不负责扫描事务、读取 session、选择最小时间戳、更新内存状态或写入 etcd。上述能力不能从本文件推断为“已支持”。

## 主要符号

- `pub struct MinStartTsReporter;`：公开的零字段单元结构体，派生 `Default`。它不持有 store、session、时钟、客户端或缓存，因此构造和销毁均不涉及资源管理。
- `pub fn report_min_start_ts<S, T>(&self, _store: &S, _session: &T)`：公开泛型方法。`S`、`T` 没有 trait 约束，两个参数只为保留调用形态而存在；以下划线命名明确表示未读取。方法没有返回值、没有错误通道，也没有函数体行为。

本文件没有模块级常量、trait、枚举、条件编译项或私有辅助函数。`MinStartTsReporter` 虽通过 crate 根重导出，但没有实现 `pkg/domain/serverinfo/syncer.rs::MinStartTSReporter` trait；大小写风格也不同：本文件方法为 Rust 风格 `report_min_start_ts`，server-info trait 方法为 Go 兼容风格 `ReportMinStartTS`。

## 执行流程

若外部代码直接调用本文件的方法，流程只有三步：

1. 调用者持有一个 `MinStartTsReporter` 值或引用。
2. 调用 `report_min_start_ts(&store, &session)`；泛型参数由传入引用推导。
3. 方法立即返回 `()`，不读取参数、不改变状态、不中断调用者。

当前应用的真实周期上报链不经过该方法。真实 Rust 链由 `pkg/domain/serverinfo/syncer.rs::Syncer::ServerInfoSyncLoop` 按 `minTSReportInterval` 唤醒，在存在 session 时调用其字段 `reporter.ReportMinStartTS(store, session)`；reporter 是 `Arc<dyn serverinfo::MinStartTSReporter>`。跨 keyspace 管理器在 `pkg/domain/crossks/cross_ks.rs::RegisteredRuntimeFactory::create` 中把其持有的 trait object 传给 `NewCrossKSSyncer`。本文件类型没有实现该 trait，因此不能直接进入这条链。

## 数据与状态

`MinStartTsReporter` 是无字段单元结构体，不保存任何可变或不可变业务状态。`Default` 仅产生同一个语义上的空值，不执行初始化。

`report_min_start_ts` 的两个输入均为共享借用：

- `_store: &S`：只保留“存储对象”形状，未约束为 `astersql_kv` 或 server-info 的 `Storage` trait。
- `_session: &T`：只保留“会话对象”形状，未约束为 etcd concurrency session 或 `serverinfo::Session`。

因此本文件没有 start_ts 数值、最小值不变量、上次上报时间、租约 ID 或 keyspace ID。任何此类状态都必须由未来接入的真实实现或上游组件定义。

## 依赖与调用关系

直接依赖方面，本文件只使用 Rust 标准语言能力和 `#[derive(Default)]`，没有 `use` 语句，也没有调用外部 crate。所属 `pkg/domain/crossks/Cargo.toml` 声明了 `astersql-domain-serverinfo`、`astersql-kv` 等依赖，但本文件当前没有使用它们。

上游关系：

- `pkg/domain/crossks/lib.rs` 声明并重导出 `reporter` 模块。
- RustCodeGraph 对 `MinStartTsReporter` 和 `report_min_start_ts` 未给出有效的生产调用边；仓库文本检索也只在本文件和模块导出中找到该 Rust 类型。
- `pkg/domain/crossks/cross_ks.rs::new_manager_with_server_info` 与 `new_manager_with_server_info_provider` 接收 server-info crate 定义的 reporter trait object，而非本文件类型。

下游关系：`report_min_start_ts` 当前无被调用函数，因其函数体为空。概念上的相邻实现是 `pkg/domain/serverinfo/syncer.rs::MinStartTSReporter`、`NoopMinStartTSReporter` 和 `Syncer::ServerInfoSyncLoop`，但它们不是本文件的静态依赖。

## 错误处理与边界

该方法返回 `()`，不可能通过返回值传播错误；空函数体也不会主动产生业务错误。泛型参数无约束意味着几乎任意两个引用都能通过类型检查，这使它适合占位，但也无法在编译期保证调用者传入真实 store/session。

边界需要明确区分：

- “调用成功”只表示 no-op 正常返回，不表示时间戳已计算、已持久化或已上报。
- 方法不处理 session 缺失、租约过期、store 读取失败、etcd 写入失败或取消信号。
- 类型没有实现 server-info reporter trait，所以把它传给当前 `NewCrossKSSyncer` 会在编译期失败；不能把同名和相似签名视为接口实现。
- 当前没有针对本文件的独立 Rust 测试；相关 server-info/crossks 测试覆盖的是 `NoopMinStartTSReporter` 及同步器生命周期，不是此方法本身。

## 并发与资源生命周期

本类型无字段、只接受共享引用且不访问参数，本身没有锁、原子变量、通道、任务、线程、事务或文件/网络句柄。每次调用彼此独立，没有内部竞态，也没有需要显式关闭的资源。

不过本类型未声明或实现 `Send`/`Sync` 相关业务 trait；单元结构体通常可由编译器自动满足自动 trait，并不等于它已满足 `serverinfo::MinStartTSReporter: Send + Sync` 的接口要求。周期调度、退出通道、session 重启和租约撤销由 `pkg/domain/serverinfo/syncer.rs::Syncer::ServerInfoSyncLoop` 及其生命周期方法负责，不能归因于本文件。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/domain/crossks/reporter.go`：

- `type minStartTSReporter struct{}` 对应 Rust 的无状态 `MinStartTsReporter`。
- `func (*minStartTSReporter) ReportMinStartTS(kv.Storage, *concurrency.Session)` 同样为空，仅保留 TODO；这与 Rust no-op 的业务意图一致。
- Go 类型依靠结构化接口的隐式实现满足 `pkg/domain/serverinfo/syncer.go::MinStartTSReporter`，并在 `pkg/domain/crossks/cross_ks.go` 创建 `serverinfo.NewCrossKSSyncer` 时传入 `&minStartTSReporter{}`。

Rust 移植并未完全对齐这段接线：`MinStartTsReporter` 的参数是无约束泛型，方法名不同，且没有显式实现 `astersql_domain_serverinfo::MinStartTSReporter`。Rust 跨 keyspace 构造链改为由调用者提供该 trait object，现有测试使用 server-info crate 自己的 `NoopMinStartTSReporter`。因此“空操作语义”已对齐，“本地类型参与同步器构造”尚未对齐。

## 扩展指南

若只需继续保持空实现，优先复用 `pkg/domain/serverinfo/syncer.rs::NoopMinStartTSReporter`，避免出现两个行为相同但接口不兼容的类型。

若要让本文件承担 Go 对应角色，最小接入点是为 `MinStartTsReporter` 实现 `astersql_domain_serverinfo::MinStartTSReporter`，使用 trait 要求的 `&dyn Storage` 与 `&Session` 签名，并在跨 keyspace manager 的组装位置明确选择该实现。不要仅修改当前泛型方法后便宣称已接线；还需验证 `new_manager_with_server_info` 的 reporter 来源和 `Syncer::ServerInfoSyncLoop` 的周期调用。

若将来实现真实最小 start_ts 计算，需要先确定 Go 上游的最终语义，再设计 store/session 读取、无活跃事务时的哨兵值、并发快照、租约失效和写入失败策略。对应 Rust 测试必须放在独立测试文件中：本模块可新增 `pkg/domain/crossks/reporter_test.rs` 并从 `lib.rs` 以 `#[cfg(test)]` 引入；跨组件接线可扩展 `cross_ks_test.rs`，周期上报与 session 行为应扩展 `pkg/domain/serverinfo/syncer_test.rs`。兼容风险主要是改变 Go 当前 no-op 行为，性能风险主要是周期扫描事务或同步访问存储。

## 验证依据

- 目标源码：`pkg/domain/crossks/reporter.rs`，确认只有 `MinStartTsReporter` 与 `report_min_start_ts`，函数体为空。
- crate 边界：`pkg/domain/crossks/Cargo.toml` 与 `pkg/domain/crossks/lib.rs`，确认包名、server-info 依赖、模块声明和公开重导出。
- Rust 调用链：`pkg/domain/crossks/cross_ks.rs::RegisteredRuntimeFactory::create`、`new_manager_with_server_info`、`new_manager_with_server_info_provider`；`pkg/domain/serverinfo/syncer.rs::MinStartTSReporter`、`NoopMinStartTSReporter`、`Syncer::ServerInfoSyncLoop`。
- Go 对照：`pkg/domain/crossks/reporter.go`、`pkg/domain/crossks/cross_ks.go`、`pkg/domain/serverinfo/syncer.go`，确认 Go 类型的空实现、接口匹配和构造时注入。
- 测试证据：`pkg/domain/crossks/cross_ks_test.rs` 与 `pkg/domain/serverinfo/syncer_test.rs` 使用 `NoopMinStartTSReporter` 验证同步器注册/清理等生命周期；检索未发现本文件类型或方法的专门测试。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/domain/crossks` 收录 `reporter.rs`，`node --file pkg/domain/crossks/reporter.rs` 返回完整 28 行源码，`query MinStartTsReporter` 定位本类型及相邻 Go/trait 符号。限定 callers/callees 未得到本方法的有效调用边，随后以仓库文本检索核对未接线事实。
- 文档结构按任务要求以 11 个固定二级标题校验；本任务是纯文档分析，未运行 Cargo 或代码测试。
