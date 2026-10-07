# `br/pkg/glue/glue.rs`

## 文件定位

该文件是 Rust crate `astersql-br-pkg-glue` 的核心抽象门面，对应 Go 文件 [`br/pkg/glue/glue.go`](./glue.go)。[`br/pkg/glue/lib.rs`](./lib.rs) 以 `#[path = "glue.rs"]` 注册模块并用 `pub use glue::*` 从 crate 根重新导出其公开项，因此下游通常通过 `astersql_br_pkg_glue::{Glue, Session, Progress, ...}` 使用，而不需要知道本文件名。

它处在 BR 业务逻辑与具体 TiDB/TiKV 运行时之间：上游恢复、连接和任务代码依赖这里的 trait；[`br/pkg/gluetidb/glue.rs`](../gluetidb/glue.rs) 与 [`br/pkg/gluetikv/glue.rs`](../gluetikv/glue.rs) 分别提供 TiDB CLI 与纯 TiKV 实现。当前文件有意不接入完整 `kv`、`domain`、`sessionctx` 等重型 crate，而以本地占位类型保存 API 形状；这意味着它是迁移期接口层，不是完整数据库能力的实现层。

## 核心职责

- 定义入口类别 `GlueClient` 以及 `ClientCLP = 0`、`ClientSql = 1`，保持 Go `iota` 数值契约。
- 用 `Glue` 汇总 BR 所需的存储打开、Domain/Session 获取、进度创建、指标记录、版本查询和一次性会话生命周期能力。
- 用 `Session`、`BatchCreateTableSession`、`Progress` 分隔 SQL/DDL 会话、批量建表和进度汇报能力，避免业务代码绑定具体实现。
- 提供轻量 `Context`，让共享 clone 能观察显式取消；它只覆盖当前 Rust 调用点需要的取消标志。
- 用 `WithProgress` 固化“创建进度—执行回调—无条件关闭”的资源协议。
- 提供 `SecurityOption`、`Storage`、`Domain`、`DBInfo`、`TableInfo`、`PolicyInfo`、`RefreshMetaArgs`、`CIStr` 等迁移期边界类型，使本 crate 只需依赖 `astersql-errors`。

## 主要符号

- `Context { cancelled: Arc<AtomicBool> }`：`new` 创建未取消上下文，`cancel` 以 `Relaxed` 写入共享标志，`is_cancelled` 读取该标志。clone 共享同一个原子量。
- `GlueClient`、`ClientCLP`、`ClientSql`：入口类型及稳定判别值；同目录 `parity_test.rs::go_rust_public_contract_matches` 断言值为 0、1。
- `SecurityOption { CAPath, CertPath, KeyPath }`：传给 `Glue::Open` 的 TLS 路径集合。
- `Storage: Send + Sync`：存储占位 trait；默认 `name()` 为 `"storage"`，默认 `keyspace_id()` 为 `u32::MAX`，后者表示传统非 keyspace 存储的 nullspace 哨兵。
- `Domain`、`DBInfo`、`TableInfo`、`PolicyInfo`、`RefreshMetaArgs`：零字段占位结构；`TableMode = i32`。这些类型保存签名，但不携带 Go 对应对象的真实状态。
- `CIStr { O, L }`：保留原始字符串和小写字符串，字段布局对齐 Go `ast.CIStr` 的相关用法。
- `CreateTableOption = Box<dyn FnOnce() + Send>`：以一次性闭包近似 Go DDL option 函数；具体 DDL 参数语义由实现层承担。
- `SessionCtxHandle = Arc<dyn Any + Send + Sync>`：由于真实 Rust session context trait 不能作为 trait object，跨边界时进行类型擦除。
- `Glue: Send + Sync`：对象安全的 BR 能力入口。必需方法包括 `GetDomain`、`CreateSession`、`Open`、`OwnsStorage`、`StartProgress`、`Record`、`GetVersion`、`UseOneShotSession` 和 `GetClient`；`AsConsoleGlue` 是 Rust 额外适配点，默认返回 `None`。
- `Session: Send`：定义 SQL 执行、带参数内部 SQL、建库/建表/放置策略、全局变量读取、session context 获取、表模式变更、元数据刷新及 `Close`。
- `BatchCreateTableSession`：仅定义按库名分组的 `CreateTables`，没有继承 `Session`；`parity_test.rs::go_rust_public_contract_matches` 用只实现该 trait 的 `BatchOnlySession` 验证这种独立性。
- `Progress: Send + Sync`：定义 `Inc`、`IncBy`、`GetCurrent`、`Close`，允许实现被多个执行线程共享。
- `WithProgress`：本文件唯一组合业务流程的自由函数。它调用 `Glue::StartProgress`，建立局部 `CloseOnDrop` 守卫，再将 `&dyn Progress` 交给一次性回调。

## 执行流程

本文件的大部分 trait 方法没有默认执行体，真实流程由实现者决定。典型调用链是：BR 上游持有 `&dyn Glue`，通过 `Open` 得到 `Box<dyn Storage>`；依据 `OwnsStorage` 判断调用方是否拥有存储生命周期；需要 TiDB 元数据或 SQL 时调用 `GetDomain`、`CreateSession` 或 `UseOneShotSession`；执行过程通过 `StartProgress`/`Record` 对外报告。

`WithProgress(ctx, g, cmdName, total, redirectLog, cc)` 的确定流程如下：

1. 将上下文、命令名、总量和输出方式原样传给 `g.StartProgress`。
2. 为返回的 `Box<dyn Progress>` 建立借用式 `CloseOnDrop` 守卫。
3. 以 `p.as_ref()` 调用 `FnOnce` 回调，直接返回回调的 `Result<(), SharedError>`，不包装或改写错误。
4. 函数正常返回、错误返回或 panic 展开离开作用域时，守卫的 `Drop` 调用一次 `Progress::Close`。

RustCodeGraph 对 `WithProgress` 的下游轨迹确认了 `StartProgress` 调用和 `CloseOnDrop` 构造；同目录测试分别验证成功、错误及 unwind 三条退出路径。

## 数据与状态

本文件自身仅持有两类实际状态。`Context` 的取消位位于 `Arc<AtomicBool>` 中，clone 不复制状态而是共享状态；取消是单向布尔变化，本文件没有恢复、父子上下文、截止时间或值传递机制。`WithProgress` 的局部状态是实现方创建的进度对象和借用该对象的 RAII 守卫，守卫先于进度对象析构。

其余结构主要是边界数据或空占位：TLS 三路径和 `CIStr` 拥有各自字符串；元数据结构没有字段；`SessionCtxHandle` 只保证共享所有权及 `Any + Send + Sync`，消费方必须知道实际类型才能向下转型。`CreateTableOption` 是一次性消费的闭包，因此调用 `CreateTable`/`CreateTables` 后不能重用。

## 依赖与调用关系

[`br/pkg/glue/Cargo.toml`](./Cargo.toml) 声明该包为 library，入口是 `lib.rs`，端口元数据指向 Go 包 `br/pkg/glue`；唯一直接 Cargo 依赖是本地 `astersql-errors`，本文件从中使用 `SharedError`。标准库依赖为 `Any`、`HashMap`、`Arc` 和原子布尔。

文件内依赖 `crate::console_glue::ConsoleGlue`，供 `Glue::AsConsoleGlue` 返回可选控制台能力；[`console_glue.rs`](./console_glue.rs) 反向依赖 `Glue`，用该扩展点选择控制台实现。RustCodeGraph 将本文件标记为被 11 个文件使用，并明确列出包括 `br/pkg/conn/conn.rs`、`br/pkg/restore/snap_client/client.rs`、`br/pkg/task/common.rs`、`br/pkg/task/restore.rs`、`br/pkg/task/stream.rs` 在内的上游。

具体实现关系可由 crate 引用核验：`br/pkg/gluetidb/glue.rs` 的 `Glue` 将 Domain/Session 生命周期交给 `DomainHooks`，并将打开存储、进度、记录和版本能力委托给 TiKV glue；`br/pkg/gluetikv/glue.rs` 实现纯 TiKV 入口，SQL/Domain 能力仍是防误用或占位行为。`br/pkg/conn/conn.rs` 直接使用 `Glue`、`Storage`、`SecurityOption` 和 `Domain`，说明该抽象位于连接建立与具体后端之间。

## 错误处理与边界

所有可能失败的 Glue/Session 操作统一返回 `Result<_, SharedError>`；接口不规定具体错误枚举，也不在此处增加上下文。`WithProgress` 原样传播 `StartProgress` 之后回调产生的错误，同时依靠 `Drop` 保证清理；如果 `StartProgress` 自身无法通过返回值报告失败（其签名直接返回进度对象），失败策略必须由实现方决定。

当前边界的关键限制是占位类型：`Domain` 和元数据类型不能表达真实 TiDB 数据；`Storage` 默认方法也不等价于完整 KV storage。`SessionCtxHandle` 的 `Any` 擦除把类型正确性推迟到运行时。`Context` 不等价于完整 Go `context.Context`，没有 deadline、值、完成通道或取消原因；只能把显式取消布尔状态提供给主动轮询方。`Progress::IncBy` 未在 trait 层拒绝负数，`Close` 后能否继续递增也由实现者落实。

## 并发与资源生命周期

`Glue`、`Storage` 和 `Progress` 要求 `Send + Sync`，可被并发任务共享；`Session` 只要求 `Send`，其方法需要 `&mut self`，调用方不能无同步地并发操作同一会话。`Context` 使用原子布尔避免共享锁，`Relaxed` 顺序只承诺该标志自身的原子读写，不承载其他内存状态的同步关系。

Go 注释要求 `Progress::Inc`/`IncBy` 可从任意 goroutine 调用；Rust trait 的 `Send + Sync` 保存了可跨线程共享的类型约束，但每个实现仍必须自行保证内部更新线程安全。`WithProgress` 的守卫在回调结束后关闭进度：`parity_test.rs::go_rust_public_contract_matches` 验证成功和 `Err`，`parity_test.rs::with_progress_closes_during_unwinding` 验证 panic unwind。若进程 abort 或 `Progress::Close` 自身 panic，RAII 无法提供更强保证。

存储与会话所有权由契约显式表达：`Open` 返回拥有所有权的 `Box`，但是否应由调用方负责其外部关闭语义取决于 `OwnsStorage`；`CreateSession` 返回拥有型对象，`Session::Close` 是显式清理入口；`UseOneShotSession` 要求具体 Glue 实现落实回调后的 session/domain 收尾。该文件只定义规则，不执行这些清理。

## 与 Go 版本的对应关系

Rust 公开 trait 基本逐项对应 `glue.go` 的 `Glue`、`Session`、`BatchCreateTableSession` 和 `Progress` 接口；`ClientCLP`/`ClientSql` 数值、`WithProgress` 的 defer-close 语义及主要方法名称均保持 Go 风格。Rust 的 `CloseOnDrop` 对应 Go `defer p.Close()`，并额外由测试明确覆盖 panic unwind。

为适应 Rust 所有权和当前移植状态，存在以下差异：Go 的具体 `kv.Storage`、`domain.Domain`、model/DDL/sessionctx 类型被本地 trait、空结构、别名或 `Any` 句柄代替；可变 Session 方法使用 `&mut self`；可变参数转为 slice/vector；Go option 函数转为 `Box<dyn FnOnce() + Send>`；接口值转为 `Box`/`Arc` trait object。Rust 的 `Glue::AsConsoleGlue` 是 Go 侧类型断言的显式替代，Go `Glue` 接口中没有该方法。

尤其不能把签名相似理解为完整能力已落地：文件头注释明确重型依赖仍由未来/下游接线；例如纯 TiKV Rust 实现用占位对象保存非 SQL 路径的接口形状。扩展或替换这些类型时必须同时检查两个具体 Glue 实现和调用方，而不能只改本文件。

## 扩展指南

新增 BR 跨后端能力时，先判断它是否真属于所有 Glue 实现的最小公共面。若是，应同时修改 `Glue` 或 `Session` trait、Go 对照接口（若同步移植要求适用）、`gluetidb`、`gluetikv` 及 mock 实现；新增必需方法会让所有实现立即出现编译缺口。只属于某种后端的能力应留在具体 crate，避免继续扩大迁移期门面。

改变生命周期语义时优先扩展 `WithProgress` 一类集中式守卫，并在独立测试文件增加成功、错误和 unwind/提前退出覆盖；不要把测试写入 `glue.rs`。改变 `Context` 时要说明新的同步保证，不能依赖 `Relaxed` 取消位传递其他数据可见性。替换占位类型为真实 crate 类型前，需要评估当前文件头记录的 arm64/grpc 构建边界以及所有 Cargo 依赖，避免把重型依赖无意传播给整个 BR 工具链。

需要同步关注的测试包括 [`br/pkg/glue/parity_test.rs`](./parity_test.rs)（公开契约、进度生命周期、常量和独立 trait）、[`br/pkg/gluetidb/glue_test.rs`](../gluetidb/glue_test.rs) 与 [`br/pkg/gluetidb/parity_test.rs`](../gluetidb/parity_test.rs)（TiDB 实现）、[`br/pkg/gluetikv/glue_test.rs`](../gluetikv/glue_test.rs) 与 [`br/pkg/gluetikv/parity_test.rs`](../gluetikv/parity_test.rs)（TiKV 实现），以及使用该 crate 的连接/恢复任务测试。主要兼容风险是 trait 破坏性变更和 Go 语义漂移；性能风险主要来自把轻量占位替换成重型依赖，或在高频进度/取消路径引入锁和额外分配。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/glue` 找到本 crate 的 Rust/Go 实现和独立测试。
- RustCodeGraph `node --file br/pkg/glue/glue.rs --offset 1 --limit 400`：读取目标文件全部 283 行，并报告 11 个使用文件；用于核对所有类型、trait、默认方法和 `WithProgress` 实现。
- RustCodeGraph `query Glue --kind trait`、`query Session --kind trait`、`query GetDomain --kind function`、`query OwnsStorage --kind function`：核对抽象及 TiDB/TiKV 实现候选。
- RustCodeGraph `node WithProgress`：确认 Rust 实现的调用轨迹为 `StartProgress` 及 `CloseOnDrop` 构造。限定文件的通用 `callers/callees` 查询未返回额外可用输出，因此上游以 crate 引用搜索核验，未臆造静态调用边。
- 已阅读路径：`br/pkg/glue/glue.rs`、`br/pkg/glue/Cargo.toml`、`br/pkg/glue/lib.rs`、`br/pkg/glue/glue.go`、`br/pkg/glue/parity_test.rs`、`br/pkg/gluetidb/glue.rs`、`br/pkg/gluetikv/glue.rs`；并以 `rg` 核对 `astersql-br-pkg-glue` 的 Cargo 使用方和 `astersql_br_pkg_glue` 的 Rust 引用。
- 本任务是纯文档分析，按计划不运行 Cargo。结构检查要求本文恰好包含上述 11 个固定二级标题。
