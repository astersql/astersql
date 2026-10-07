# `pkg/domain/sqlsvrapi/server.rs`

## 文件定位

本文件是 `astersql-domain-sqlsvrapi` crate 的核心接口契约，源码见 [`server.rs`](server.rs)。它不创建监听器，也不处理 MySQL 协议；这里的 “SQL Server” 指向 Domain 暴露给 SQL 执行及分布式任务框架的运行时能力集合。crate 入口 [`lib.rs`](lib.rs) 将本模块公开为 `server`，并把 KV `Storage`、元数据 `AlterTableModeTarget`、DDL owner `Manager` 与系统 session pool `DestroyableSessionPool` 从各自 crate 重新导出。

[`Cargo.toml`](Cargo.toml) 表明该 crate 没有 feature 分支：生产依赖仅为 `astersql-kv`、`astersql-meta-model`、`astersql-owner`、`astersql-util` 和 `tokio-util 0.7`。本文件自身没有条件编译项、常量、结构体或具体函数实现，只有两个类型别名与三个公开 trait。

仓库当前的 Rust 接线要分两层理解：接口已被 mock crate 和 DXF 工具使用，例如 [`pkg/dxf/framework/dxfutil/util.rs`](../../dxf/framework/dxfutil/util.rs) 通过 `Server` 获取任务 Runtime；但对全仓 Rust 源码搜索未找到 Domain 或跨 keyspace 生产类型对本文件三个 trait 的实现。相应的完整生产实现目前可在 Go 的 [`domain.go`](../domain.go) 与 [`cross_ks.go`](../crossks/cross_ks.go) 中看到。因此本文件是迁移中的稳定边界，不能单凭 trait 声明推断 Rust 主程序已经完成生产接线。

## 核心职责

1. 用 `Runtime` 把某一 keyspace 的 KV store、系统 session pool 和 table-mode DDL 提交能力绑定为同一视图，避免调用方分别取得可能属于不同租户的组件。
2. 用 `KSRuntimeHandle: Runtime` 表示一次跨 keyspace 的借用，并通过显式 `Release` 约束持有计数与空闲回收生命周期。
3. 用 `Server` 同时提供当前实例 Runtime、目标 keyspace Runtime handle，以及 DDL owner 管理器。
4. 用 `Context` 和 `SqlSvrError` 统一动态分派边界上的取消令牌与错误类型，使调用者不依赖具体实现错误枚举。

该接口只规定能力和所有权形态，不负责验证 keyspace 一致性、构造 DDL job、选举 owner、创建/关闭 session pool，亦不自动调用 `Release`。这些行为属于实现者或上层资源管理代码；Rust 的一个直接消费者 [`AcquireTaskRuntime`](../../dxf/framework/dxfutil/util.rs) 会比较当前会话与任务 keyspace，并把释放操作封装为调用方必须执行的闭包。

## 主要符号

- `pub type Context = CancellationToken`：把 `tokio_util::sync::CancellationToken` 作为 Go `context.Context` 在此 API 的取消语义替代物。它只表达取消状态，不携带 Go context 的 deadline、键值或完整错误链。
- `pub type SqlSvrError = Box<dyn Error + Send + Sync + 'static>`：允许不同实现返回可跨线程、具有静态生命周期的任意标准错误。接口没有定义稳定的错误分类，调用方只能传播、显示或对具体错误做显式下转型。
- `pub trait Runtime: Send + Sync`：可作为跨线程共享的 trait object。
  - `Store(&self) -> Arc<dyn Storage + Send + Sync>` 返回 keyspace 作用域的 KV 存储。
  - `SysSessionPool(&self) -> Arc<dyn DestroyableSessionPool>` 返回内部 SQL/元数据访问所用的系统 session pool。
  - `AlterTableMode(&self, ctx, target) -> Result<(), SqlSvrError>` 提交内部 table-mode DDL 并等待结果。注释规定 `SchemaID`、`TableID`、`TargetMode` 必填；跨 keyspace 实现还应校验名称，实际 `CurrentMode` 应从元数据重新解析。
- `pub trait KSRuntimeHandle: Runtime`：通过 supertrait 继承上述三项 Runtime 能力，并增加 `Release(&self)`。`Release` 后继续使用 handle 违反接口契约；底层 Runtime 的生命周期独立于 handle。
- `pub trait Server: Send + Sync`：SQL Server 的对象安全共享接口。
  - `GetRuntime` 返回当前实例 Runtime。
  - `AcquireKSRuntime(targetKS, holderID)` 获取目标 keyspace 句柄；`holderID` 用于标识持有者。
  - `GetDDLOwnerMgr` 返回 owner 管理器，供选主状态和 DDL 调度相关代码使用。

所有方法都使用 `&self`，返回值主要由 `Arc<dyn Trait>` 承载共享所有权；没有方法要求可变借用。命名保留 Go 风格的首字母大写，crate 入口通过 lint allow 支持这种迁移期 API 形态。

## 执行流程

当前 keyspace 的典型流程是：调用者从会话取得 `Server`，调用 `GetRuntime`，再从同一个 `Runtime` 取得 `Store` 或 `SysSessionPool`；需要切换表模式时，把 `Context` 和 `AlterTableModeTarget` 交给 `AlterTableMode`，同步等待实现返回成功或错误。

跨 keyspace 的典型流程在 [`AcquireTaskRuntime`](../../dxf/framework/dxfutil/util.rs) 中有直接 Rust 证据：

1. 新建 session 并从 `sessionctx::Context` 读取当前 store 的 keyspace。
2. 若任务 keyspace 与当前 keyspace 相同，调用 `Server::GetRuntime`，返回的释放闭包为空操作。
3. 若不同，调用 `Server::AcquireKSRuntime(taskKS, holderID)`；把所得 `Arc<dyn KSRuntimeHandle>` 向上转为 `Arc<dyn Runtime>` 供任务使用，同时保留原 handle。
4. 调用方结束使用时执行释放闭包；闭包只在跨 keyspace 分支调用 `Release`。
5. [`CheckTaskRuntime`](../../dxf/framework/dxfutil/util.rs) 进一步验证 Runtime store 的 keyspace 等于任务 keyspace，并从 session pool 借出 session，确认 session store 与 Runtime store 也属于同一 keyspace。

table-mode DDL 的细节由实现决定。本文件注释对应两条 Go 路径：当前 keyspace 的 `Domain.AlterTableMode` 从系统池借 session、调用本地 DDL executor、最后归还 session；跨 keyspace 的 `runtimeHandle.AlterTableMode` 委托给目标 `SessionManager`/DDL client，后者解析元数据、校验名称、补齐当前模式、提交 job、通知 owner 并等待历史 job 终态。Rust 仓库虽有独立的 [`crossks/ddl_submit.rs`](../crossks/ddl_submit.rs) 实现相似流程，但尚未发现它与本 trait 的生产适配实现，故不能视为已经通过本 API 接线。

## 数据与状态

本文件自身没有可变状态。状态位于接口返回或接收的对象中：

- `Arc<dyn Storage + Send + Sync>` 和 `Arc<dyn DestroyableSessionPool>` 共同定义 Runtime 的 keyspace 视图。接口类型系统不能证明二者属于同一 keyspace，因此消费方 [`CheckTaskRuntime`](../../dxf/framework/dxfutil/util.rs) 做运行时一致性检查。
- `AlterTableModeTarget` 携带 schema/table 的 ID、名称、当前模式和目标模式。接口注释明确：ID 与目标模式是调用方必要输入；跨 keyspace 路径需要名称进行元数据一致性校验；当前模式应由实现按最新元数据覆盖，而不应盲信请求值。
- `targetKS` 选择目标租户，`holderID` 标记借用者。二者是 `AcquireKSRuntime` 的值参数，接口不限制字符串格式；DXF 的 [`GenHolderID`](../../dxf/framework/dxfutil/util.rs) 采用 `DXF/{component}/{taskID}`。
- `CancellationToken` 可克隆并共享取消状态。trait 只把令牌传给实现，没有在入口预先拒绝已取消请求。
- `Arc` 只管理 Rust 对象引用计数，不替代业务层的 `Release`。`KSRuntimeHandle` 的持有登记可能已释放，而对象仍因 `Arc` 存活，这是两套不同生命周期。

## 依赖与调用关系

上游直接关系如下：

- [`pkg/dxf/framework/dxfutil/lib.rs`](../../dxf/framework/dxfutil/lib.rs) 重新导出本文件的接口；其 `AcquireTaskRuntime`、`releaseTaskRuntime` 与 `CheckTaskRuntime` 分别消费 `Server`、`KSRuntimeHandle` 和 `Runtime`。
- [`pkg/domain/sqlsvrapi/mock`](mock/) 使用 `mockall` 为三个 trait 生成测试替身，保留 `expect_*`、调用次数、参数匹配和返回行为。
- [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 用手写 `RecordingRuntime`、`RecordingHandle`、`RecordingServer` 验证 trait object 组合、参数转发、错误传播和释放调用。
- Cargo 清单显示 `pkg/session`、`pkg/domain`、DXF scheduler/taskexecutor/dxfutil 等 crate 声明了该接口 crate 或其 mock 依赖；依赖声明并不等于每个 crate 当前都有源码调用。

下游类型来自：`astersql-kv::Storage`、`astersql-util::session_pool::DestroyableSessionPool`、`astersql-meta-model::AlterTableModeTarget`、`astersql-owner::Manager` 和 `tokio-util::CancellationToken`。本文件只引用其抽象接口，不调用任何具体方法，因此 RustCodeGraph 对三个 trait 定义本身没有内部 callees；具体调用边出现在实现与消费代码。

Go 侧的真实上游包括 DXF runtime 获取、IMPORT INTO 清理/调度与 BR 日志恢复等；RustCodeGraph 的同名跨语言结果也列出了这些 Go 调用者。它们可说明接口设计用途，但不能作为 Rust 调用已迁移完成的证据。

## 错误处理与边界

- `Store`、`SysSessionPool`、`GetRuntime`、`GetDDLOwnerMgr` 与 `Release` 无返回错误通道；实现若遇到内部故障，只能保证返回有效对象、内部处理，或发生 panic。新增实现不应返回与 Runtime keyspace 不一致的 store/pool。
- `AlterTableMode` 与 `AcquireKSRuntime` 使用 `SqlSvrError`，允许原样传播实现错误，但牺牲了静态错误分类。迁移测试验证字符串为 `ddl rejected` 与 `keyspace missing` 的错误不会被接口层吞掉。
- `AcquireKSRuntime` 成功后的 `Release` 不是 RAII trait 保证：接口没有 `Drop` 要求，也没有返回守卫类型。所有成功路径、提前返回与错误路径都必须确保释放。DXF 当前通过 `FnOnce` 闭包转交该责任；若调用方遗失闭包，trait 本身无法补救。
- `Context` 是否被尊重取决于实现。接口注释明确，跨 keyspace 的提交/等待路径应响应取消；Go 当前 keyspace 的本地 executor 在调用开始后不响应取消。Rust 测试只验证取消状态能传入 mock，不证明生产 DDL 路径已响应取消。
- `AlterTableModeTarget` 的语义校验不在 trait 默认实现中。特别是名称、ID、当前模式与允许的模式迁移，必须由具体实现或其 DDL 层验证。
- trait object 均要求 `Send + Sync` 或由所引用 trait 自身提供相应约束，错误也要求 `Send + Sync + 'static`；这允许跨线程共享，但不保证业务操作无竞争或可重入。

## 并发与资源生命周期

三个 trait 都以共享引用调用，`Runtime` 与 `Server` 显式要求 `Send + Sync`，主要返回值使用 `Arc`。因此实现内部的可变状态（如持有者计数、session pool、owner 状态或调用记录）必须自行同步；手写测试用 `Mutex` 和 `AtomicUsize` 展示了符合该接口的同步方式。

Runtime 不拥有其所有下游资源的关闭权：`KSRuntimeHandle` 注释明确它只是视图，释放 handle 不等于销毁底层 Runtime。Go 的 `runtimeHandle.Release` 使用 `sync.Once` 保证幂等，并把持有计数交回 manager；Go manager 可在最后释放后按空闲超时回收 Runtime。Rust 本文件没有规定幂等性，也没有 `Drop` 默认行为，安全实现应仿照这一语义避免重复减计数，并让 handle 释放与底层关闭解耦。

系统 session pool 的单次资源借还也不由接口自动完成。Go `Domain.AlterTableMode` 使用 defer 保证归还；Rust 消费者从 `SysSessionPool` 获取资源后同样必须在所有路径执行 `Put` 或按池契约 `Destroy`。`CancellationToken` 可在多个线程/任务间共享，但本 API 是同步方法签名，`AlterTableMode` 是否阻塞线程以及如何轮询由实现决定。

## 与 Go 版本的对应关系

[`server.go`](server.go) 是逐项对照来源：Go 的 `Runtime`、嵌入 `Runtime` 的 `KSRuntimeHandle`、以及 `Server` 分别映射为 Rust 同名 trait；方法顺序与名称保持一致。主要类型映射为：Go `context.Context` → Rust `CancellationToken`，Go `error` → boxed 标准错误，Go 接口值 → `Arc<dyn Trait>`，Go interface embedding → Rust supertrait。

语义上的重要差异是：

- Rust `Context` 只覆盖取消，不完整等价于 Go context 的 deadline/value/cause。
- Go 接口返回值依赖垃圾回收管理引用，Rust 显式用 `Arc`；但两者都仍要求业务层调用 `Release`。
- Go `Domain` 已实现 `Server` 和当前 keyspace `Runtime`：`GetRuntime` 返回自身，`GetDDLOwnerMgr` 返回 DDL owner，`AcquireKSRuntime` 委托跨 keyspace manager，`AlterTableMode` 使用本地系统 session 与 DDL executor。
- Go `crossks.runtimeHandle` 已实现跨 keyspace Runtime 与幂等释放，并由 `cross_ks_test.go` 覆盖目标 store/session pool、DDL 提交、等待取消和 `defer Release`。Rust 同名接口目前只有测试实现/mock 与消费者，未检索到对应生产 trait impl。
- Rust trait 的 `AlterTableMode` 明确接收完整 `AlterTableModeTarget`，与当前 Go 接口一致；不能把独立 `crossks` Rust crate 中形状相似但类型不同的 target/cancellation 自动视为可互换。

## 扩展指南

新增 Runtime 或 Server 生产实现时，应在独立源文件中实现，不要把测试或具体业务逻辑塞入本接口文件。实现顺序建议为：

1. 先确定 Runtime 的唯一 keyspace，并确保 `Store` 与 `SysSessionPool` 始终来自同一运行时；同步扩展 [`dxfutil` 的独立测试](../../dxf/framework/dxfutil/util_test.rs) 覆盖一致与不一致分支。
2. 为 `AlterTableMode` 明确两种路径：本地 executor 路径需要保证 session 借还；跨 keyspace 路径需要按 ID 重新读取元数据、校验名称、补齐当前模式、验证模式转换并响应等待阶段取消。
3. `AcquireKSRuntime` 应把 `targetKS` 与 `holderID` 原样交给 manager，失败时不得留下持有登记；成功 handle 的 `Release` 应幂等，最好再以 `Drop` 作泄漏兜底，但不能因此省略显式释放契约。
4. 适配现有 [`crossks`](../crossks/) Rust 实现时，需要显式转换两边的 Storage/session pool/target/cancellation/error 类型，并确认 DDL owner 与 Runtime 关闭顺序；不要仅因方法同名而做无验证强转。
5. 若新增 trait 方法，必须同步 Go [`server.go`](server.go)（若仍要求双版本一致）、三个 Rust mock 文件、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，以及所有生产实现和直接消费者。测试仍应放在独立 `*_test.rs`/迁移测试文件中。
6. 若需要可判别的恢复策略，优先引入稳定错误枚举或辅助分类 API；继续使用字符串匹配会使兼容性脆弱。若改变 `Context`，需评估 deadline、取消原因与同步/异步等待的兼容影响。

性能风险主要来自每次调用克隆 `Arc`、跨 keyspace Runtime 的重复创建/持有、系统 session 借用，以及 `AlterTableMode` 的阻塞等待；兼容风险集中于 Go 风格公开方法名、trait object 对象安全和所有实现者必须同步新增方法。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`pkg/domain/sqlsvrapi/server.rs` 被识别为 109 行、11 个符号。`query` 精确定位了本文件的 `Runtime`、`KSRuntimeHandle`、`Server`，`node --file` 复核了完整源码。
- RustCodeGraph 调用查询：`Runtime` 定义无内部 callee；`AlterTableMode` 的直接结果包含本 crate 的 `runtime_uses_real_dependencies_and_preserves_context_target_and_error`；`AcquireKSRuntime` 的直接 Rust 结果包含 `server_acquires_keyspace_handle_reports_errors_and_exposes_owner`。常见方法名的跨语言结果存在噪声，因此实际 Rust 消费边又由 crate 依赖和源码读取核实。
- 已读 Rust 边界：[`server.rs`](server.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、三个 [`mock`](mock/) 实现、[`dxfutil/util.rs`](../../dxf/framework/dxfutil/util.rs) 及其 [`util_test.rs`](../../dxf/framework/dxfutil/util_test.rs)。
- 已读 Go 对照与测试：[`server.go`](server.go)、[`domain.go`](../domain.go) 的 Server/Runtime 实现、[`cross_ks.go`](../crossks/cross_ks.go) 的 handle 实现，以及 [`cross_ks_test.go`](../crossks/cross_ks_test.go) 的获取、释放与取消场景。
- 生产接线限制：对全仓 Rust 的 `impl Runtime/Server/KSRuntimeHandle`、crate 名和 `sqlsvrapi` 引用进行搜索，只发现本 crate 测试、mock 与 DXF 消费层相关实现，未发现 Domain/跨 keyspace 的生产 trait impl；该结论描述的是当前仓库搜索结果，而不是未来设计保证。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核文档区分了接口事实、Go 生产行为和 Rust 当前迁移状态。
