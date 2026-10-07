# `pkg/lightning/common/conn.rs`

## 文件定位

本文件属于 `astersql-lightning-common` crate 的连接复用层。crate 入口 `pkg/lightning/common/lib.rs` 通过 `mod conn` 编译本模块，并以 `pub use conn::*` 将其公开类型和函数再导出；`pkg/lightning/common/Cargo.toml` 则把该 crate 映射到 Go 包 `pkg/lightning/common`。本文件不负责真实拨号或 RPC：当前 `ClientConn` 只保存目标字符串和关闭标志，是 `*grpc.ClientConn` 的轻量本地替身。

RustCodeGraph 的文件查询显示 `conn.rs` 已被索引并包含 21 个符号；精确符号查询能定位 `ClientConn`、`ConnPool`、`GRPCConns`、`NewConnPool`、`GetGrpcConn` 和 `NewGRPCConns`。仓库范围的精确名称检索未发现独立 Rust 生产模块调用这些 API，当前可确认的直接 Rust 使用者是 `pkg/lightning/common/conn_test.rs`，因此不能把它描述为已经接入真实 Lightning/TiKV gRPC 主链。

## 核心职责

文件提供三个逐级封装的职责：

1. `ClientConn` 表示一个可共享、可关闭的目标连接句柄，供池化逻辑观察目标与关闭状态。
2. `ConnPool` 使用固定容量延迟创建连接；池未满时调用 `ConnFactory`，池满后按 `next` 游标轮询复用。
3. `GRPCConns` 以 TiKV `storeID` 为键维护多个 `ConnPool`，首次访问某个 store 时才用该次传入的并发度和工厂建池。

这些职责对应 `pkg/lightning/common/conn.go` 的 `ConnPool`/`GRPCConns` 设计，但当前 Rust 层只实现池和生命周期语义，不包含地址解析、TLS、负载均衡、健康检查或网络 I/O。

## 主要符号

- `ClientConn { target, closed }`（第 28 行）：公开句柄类型；`target: String` 在构造后不变，`closed: AtomicBool` 支持跨线程观察关闭状态。`new` 构造句柄，`Target` 返回目标，`Close` 以 Release 顺序置位，`IsClosed` 以 Acquire 顺序读取。
- `ConnFactory`（第 60 行）：`Arc<dyn Fn(&Context) -> Result<Arc<ClientConn>, CommonError> + Send + Sync>`。工厂可跨线程共享，创建错误使用公共 `CommonError` 返回。
- `ConnPool`（第 63 行）：公开池对象；私有 `state` 把连接列表和轮询游标作为同一个互斥临界区，`cap` 和 `newConn` 在构造后固定。
- `ConnPoolState`（第 70 行）：私有状态，包含 `conns: Vec<Arc<ClientConn>>` 与 `next: usize`。
- `ConnPool::TakeConns`（第 77 行）：在锁内把全部连接移出，并把游标复位为 0。
- `ConnPool::Close`（第 84 行）：先清空池，再逐个调用 `ClientConn::Close`；返回类型为 `()`。
- `ConnPool::get`（第 91 行）：私有取连接入口。未满时在持锁状态调用工厂并入池；已满时复制当前 `Arc`，再以模容量推进游标。
- `NewConnPool`（第 108 行）：公开构造器，预分配容量但不创建连接。
- `GRPCConns`（第 120 行）：公开的 store 到池映射，`HashMap<u64, Arc<ConnPool>>` 由互斥锁保护。
- `GRPCConns::GetGrpcConn`（第 140 行）：按 store 懒建池，然后调用该池的私有 `get`。
- `GRPCConns::Close`（第 126 行）与 `NewGRPCConns`（第 161 行）：分别关闭当前映射中的池、创建空映射。

本文件没有模块级常量、trait 或条件编译项。

## 执行流程

典型获取流程从 `NewGRPCConns` 开始：它创建空的 store 映射；调用者执行 `GetGrpcConn(ctx, storeID, tcpConcurrency, newConn)` 后，方法先锁住映射，并通过 `entry(storeID).or_insert_with(...)` 保证同一 store 只安装一个池。若是首次访问，`NewConnPool` 保存这次调用的容量与工厂。随后外层映射锁被释放，进入 `ConnPool::get`。

`get` 在整个选择/创建阶段持有池状态锁。若 `conns.len() < cap`，它调用工厂；成功时追加并返回同一个 `Arc`，失败时通过 `?` 原样返回错误且不改变列表。池满时，它返回 `conns[next]` 的共享引用，并把 `next` 更新为 `(next + 1) % cap`。因此容量为 `N > 0` 时，前 `N` 次成功获取会创建连接，后续请求按创建顺序循环复用。

关闭流程由 `GRPCConns::Close` 先在映射锁内克隆所有池的 `Arc`，释放映射锁后逐池调用 `ConnPool::Close`。每个池通过 `TakeConns` 原子地移走列表并复位游标，再在锁外关闭句柄。`pkg/lightning/common/conn_test.rs::close_waits_for_in_flight_get_and_closes_its_connection` 证明：当首次 `GetGrpcConn` 已把池放入映射且正在池锁内执行阻塞工厂时，`Close` 会在该池锁后等待，工厂完成后取得并关闭刚创建的连接。

## 数据与状态

核心不变量是 `state.conns.len() <= cap`，前提是容量为正且状态只经本实现修改；连接只有在工厂成功后才追加。`next` 只在池满分支推进，并在 `TakeConns` 中归零。对于同一个 `storeID`，首次 `GetGrpcConn` 决定该池长期保存的 `tcpConcurrency` 和 `newConn`；后续调用传入的不同容量或工厂不会替换已有配置。

连接与池均通过 `Arc` 共享。`TakeConns`/`Close` 只移除池持有的引用，不会使调用者已持有的 `Arc<ClientConn>` 失效；本实现会把这些句柄的原子关闭标志置为真。`GRPCConns::Close` 不删除 store 到池的映射，所以关闭后再次获取会在原池中重新创建连接。

零容量只保证 `NewConnPool(0, factory)` 能构造，这由 `zero_capacity_is_accepted_when_constructing_pool` 覆盖。首次 `get` 时 `len() < cap` 为假，随后访问空向量 `conns[0]`，因此会 panic；源码注释也要求实际调用者传入正容量。

## 依赖与调用关系

向上，`pkg/lightning/common/lib.rs` 编译并再导出本模块；当前精确仓库检索只确认 `pkg/lightning/common/conn_test.rs` 调用 `NewConnPool`、`NewGRPCConns`、`GetGrpcConn` 和 `ClientConn::new`。RustCodeGraph 对整个文件给出的“used by”候选还包含 `pkg/lightning/backend/tidb/tidb.rs`、`pkg/lightning/common/common.rs` 与 `pkg/lightning/mydump/parser_generated.rs`，但逐符号检查未发现这些文件调用本模块 API，应视为图索引的同名/文件级候选，而不是可靠调用边。

向下，本文件仅依赖标准库的 `HashMap`、`Arc`、`Mutex` 和原子布尔值，以及 crate 内的 `Context` 与 `CommonError`。`Context` 的实现位于 `pkg/lightning/common/pause.rs`，提供取消/截止时间状态；连接池本身不读取这些状态，只把引用透传给 `ConnFactory`。`CommonError` 定义于 `pkg/lightning/common/errors.rs`，本文件仅把它作为工厂和 `ClientConn::Close` 的错误类型。虽然 crate 的 Cargo 依赖还包括 `astersql-lightning-log` 和 `libc`，`conn.rs` 没有直接使用它们。

Go 对照的直接调用关系同样封闭在 `pkg/lightning/common/conn.go` 内：`GetGrpcConn -> NewConnPool -> ConnPool.get`，`GRPCConns.Close -> ConnPool.Close -> TakeConns -> grpc.ClientConn.Close`。仓库检索没有找到该 Go API 的独立测试或包外精确调用。

## 错误处理与边界

唯一正常返回的业务错误来自 `ConnFactory`：`ConnPool::get` 使用 `?` 传播 `CommonError`，创建失败的连接不会入池，下一次调用仍可重试创建。`ClientConn::Close` 当前恒定返回 `Ok(())`；`ConnPool::Close` 即使未来收到关闭错误也以 `let _ = ...` 丢弃，调用者无法获知部分关闭失败。

两把互斥锁均使用 `expect(... mutex poisoned)`，因此持锁线程 panic 后的 poison 会导致后续操作再次 panic，而不是转换为 `CommonError`。容量为零时的首次获取也会因空向量索引而 panic。`tcpConcurrency` 极大时 `Vec::with_capacity` 还可能因内存分配失败终止进程；本模块没有参数上限检查。

工厂在持有池状态锁时执行，保证不会为同一池并发超额创建连接，但慢工厂会串行化该 store 的所有获取和关闭。工厂可以检查透传的 `Context`，然而本模块既不主动检查 `Context::Done`，也不为锁等待提供取消机制。

## 并发与资源生命周期

`ConnFactory` 的 `Send + Sync`、`Arc<ClientConn>`、`Mutex` 和原子字段使这些类型可跨线程共享。每个 store 的连接创建与轮询由各自的 `ConnPool::state` 独立串行化；不同 store 在完成映射查找后可以并发执行工厂。映射锁只负责 store 到 pool 的一致性，首次并发访问同一 store 不会创建两个池。

`ClientConn::Close`/`IsClosed` 使用 Release/Acquire，确保观察到关闭标志的线程也能获得关闭前的内存可见性；重复关闭是幂等置位。池没有 `Drop` 实现，遗忘显式 `Close` 只会在最后一个 `Arc` 释放时丢弃本地对象，不执行额外网络清理（当前对象本来也不持有真实网络资源）。

需要注意一个由源码可推导、但现有测试未覆盖的竞态窗口：对于已存在的 pool，`GetGrpcConn` 释放外层映射锁后才竞争池锁；`GRPCConns::Close` 可能先完成该池的 `TakeConns`，随后获取线程再创建一条新连接，使这次 `Close` 不包含该连接。现有并发测试覆盖的是“首次建池后工厂正在执行”的相反顺序。若未来赋予真正网络资源，应明确 Close 与后续 Get 的生命周期契约，并为此竞态增加独立测试。

## 与 Go 版本的对应关系

`pkg/lightning/common/conn.go` 是直接语义基准。两边都采用：按 store ID 懒建池、池未满时创建、满后 round-robin、取走全部连接时重置游标、关闭后保留 store 映射。Rust 用 `Arc` 替代 Go 指针共享，用 `Mutex<ConnPoolState>` 把 Go 的 `mu + conns + next` 聚合为受保护状态，用 `ConnFactory` trait object 替代 Go 函数值。

已验证的差异如下：

- Go 存储真实 `*grpc.ClientConn`，Rust `ClientConn` 只是目标与关闭位，不进行拨号。
- Go `NewConnPool` 还接收 logger；连接关闭失败时记录 target 和短错误。Rust 构造器没有 logger，`Close` 忽略错误。
- Go `get` 用 `errors.Trace` 包装工厂错误；Rust 原样传播 `CommonError`。
- Go `GRPCConns::GetGrpcConn` 在调用 pool `get` 期间继续持有外层锁，Rust 在取得 `Arc<ConnPool>` 后释放外层锁，因而允许不同 store 并行获取，但具有上一节所述 Close/Get 窗口。
- Go `GRPCConns::Close` 在遍历关闭期间持有外层锁；Rust 先克隆池列表再释放外层锁，以缩短映射临界区。
- Go 构造器返回指针；Rust `NewGRPCConns`/`NewConnPool` 返回值，由调用者决定是否包入 `Arc`。

仓库中不存在 `pkg/lightning/common/conn_test.go`，也未检索到其他 Go 测试直接覆盖这些符号；当前边界行为证据主要来自 Go 实现本身和独立 Rust 测试，不能宣称 Go 测试完全对齐。

## 扩展指南

若要接入真实 gRPC，最可能修改 `ClientConn` 与 `ConnFactory`：应让句柄拥有真实 channel/transport，并保留可观测的关闭结果；不要把网络实现复制进池状态机。同步更新独立的 `pkg/lightning/common/conn_test.rs`，增加拨号失败、关闭失败、目标地址和资源释放测试。若引入外部 Rust 依赖，必须遵守仓库规则，在独立上游仓库移植、提交并打 tag，再以统一 tag 的 Git 依赖引用。

若要改变池策略，应集中修改 `ConnPool::get`、`TakeConns` 和 `NewConnPool`，并测试容量填充、严格轮询次序、工厂失败不占容量、关闭后重建、零容量获取行为。测试逻辑保持在 `conn_test.rs`，不要内嵌进生产文件。

若要定义严格的并发关闭协议，应同时审查 `GRPCConns::GetGrpcConn` 与 `GRPCConns::Close` 的锁顺序。至少新增“既有池上 Close 先取得池锁、Get 随后进入”的回归测试，并决定是禁止 Close 后 Get、允许重开，还是用全局 closing/closed 状态阻止漏关连接。锁顺序必须固定为外层映射锁后内层池锁，避免反向获取造成死锁。

任何语义变化都应与 `pkg/lightning/common/conn.go` 对照：若是 Rust 特有优化，要记录并发、兼容与性能理由；若最终接入生产调用，还需从真实调用方验证 `tcpConcurrency > 0`，因为当前 API 对零容量获取没有可恢复错误。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/lightning/common` 确认 `conn.rs`、`conn_test.rs`、`conn.go` 与 crate 同目录；`node --file pkg/lightning/common/conn.rs --offset 1 --limit 400` 读取完整 165 行源码；`query` 精确定位 `ClientConn`、`ConnPool`、`GRPCConns`、`NewConnPool`、`TakeConns`、`GetGrpcConn` 和 `NewGRPCConns`。精确 ID 的 callers/callees 命令未返回可靠限定结果（callees 将 ID 当成模糊名称并产生无关结果），因此调用边又用仓库精确名称检索核验，未将噪声当作证据。
- 源码与模块边界：`pkg/lightning/common/conn.rs`、`pkg/lightning/common/lib.rs`、`pkg/lightning/common/Cargo.toml`、`pkg/lightning/common/pause.rs`、`pkg/lightning/common/errors.rs`。
- Go 对照：`pkg/lightning/common/conn.go`；仓库中没有同目录 `conn_test.go`，精确检索也未发现其他 Go 测试调用这些 API。
- Rust 独立测试：`pkg/lightning/common/conn_test.rs`，覆盖零容量构造，以及首次获取正在工厂内执行时 Close 等待并最终关闭该连接。
- 调用检索：对 `pkg/lightning`、`lightning` 和 `br` 下 Rust/Go 文件检索 `NewGRPCConns|GetGrpcConn|NewConnPool|TakeConns|ClientConn|GRPCConns|ConnPool`，并对 crate 名称/导入形式做补充检索；除本实现、Go 对照和 Rust 测试外，未确认本模块 API 的 Rust 生产调用方。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前另运行任务指定的 11 章节结构校验，并人工复核文档对定位、运行流程和安全扩展入口的回答。
