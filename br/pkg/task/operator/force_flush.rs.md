# `br/pkg/task/operator/force_flush.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-task-operator`。该 crate 由 `br/pkg/task/operator/Cargo.toml` 定义，以同目录 `lib.rs` 为入口；`lib.rs` 把本文件声明为公开模块 `force_flush`，并通过 `pub use force_flush::*` 将公开函数平铺到 crate 根。manifest 的 `package.metadata.porting.go-package = "br/pkg/task/operator"` 明确记录了它对应的 Go 包。

应用入口是 `br/cmd/br/operator.rs::newForceFlushCommand`：该函数注册 `force-flush` 子命令，解析 `ForceFlushConfig` 后调用 `RunForceFlush`。命令用于让地址匹配的 TiKV 节点立即刷新日志备份任务；TiFlash 不应收到该请求。

当前 Rust 路径仍处于本地边界桩接线阶段。`RunForceFlush` 依赖 `prepare_snap.rs::{dialPD, createStoreManager}`，而默认 `dialPD` 在没有测试 hook 时会明确返回“PD dial not configured”，不会连接真实 PD。因此本文描述的并发、筛选、错误聚合和清理逻辑是已经实现并可由契约测试验证的；真实集群 RPC 能力尚不能仅凭本文件宣称可用。

## 核心职责

本文件承担两层职责：

1. `getAllTiKVs` 从 `PDClient` 获取 store 列表并移除带 `engine=tiflash` 标签的节点，建立后续只对 TiKV 操作的第一道过滤。
2. `RunForceFlush` 建立 PD/StoreManager 资源，按 `ForceFlushConfig::StoresPattern` 二次筛选地址，为每个目标 store 启动一个线程并调用 `StoreManager::FlushNow`，汇总首个错误，等待全部 worker 后按顺序关闭资源。

本文件只编排操作，不定义网络协议、PD 客户端或日志备份响应类型。上述边界均来自 `stubs.rs`；尤其 `StoreManager::FlushNow` 当前读取测试注入的结果表，而非创建真实 gRPC `LogBackupClient`。因此它是 Go 流程的可测同步移植框架，不是已经接通生产网络的完整实现。

## 主要符号

- `pub fn getAllTiKVs(p: &dyn PDClient) -> Result<Vec<metapb::Store>>`：调用 `PDClient::GetAllStores`，保留所有非 TiFlash store。返回拥有所有权的 `Vec<Store>`，便于 `RunForceFlush` 把各 store 的 ID 和地址移入线程调度流程。
- `pub fn RunForceFlush(cfg: &ForceFlushConfig) -> Result<()>`：公开业务入口。它负责资源建立、store 获取与匹配、每 store 并发刷新、错误聚合及显式关闭。

文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项。`ForceFlushConfig` 定义在 `config.rs`，`PDClient`、`PdController`、`StoreManager`、`FlushResult`、`Error` 和 `metapb::Store` 都定义在 `stubs.rs`。源码导入的 `std::sync::Arc` 没有被显式写入类型位置；实际共享所有权由 `dialPD`/`createStoreManager` 返回的 `Arc` 及其 `clone` 表达。

## 执行流程

`RunForceFlush` 的执行顺序如下：

1. 调用 `dialPD(&cfg.Config)`。失败时直接返回，此时尚未取得资源。
2. 使用 `pdMgr.GetPDClient()` 调用 `createStoreManager`。若创建失败，显式关闭已经取得的 `pdMgr`，再返回原错误。
3. 调用 `getAllTiKVs`。若 PD 查询失败，先关闭 `stores`，再关闭 `pdMgr`，然后返回原错误。
4. 克隆 `cfg.StoresPattern`，遍历非 TiFlash store。地址不匹配或再次被 `IsTiFlash` 判断为真时记录跳过信息；二次 TiFlash 检查是防御性保护。
5. 对每个匹配节点克隆 `Arc<StoreManager>`，保存 `store_id`，用 `thread::spawn` 启动 worker。worker 调用 `stores.FlushNow(store_id)`；RPC/桩错误通过 `?` 返回。响应中的每个 `FlushResult` 都必须 `Success=true`，否则立即构造包含任务名、store ID 和服务端消息的错误。
6. 主线程逐个 `join` 全部 handle。它保存遇到的第一个业务错误；worker panic 被转换成固定错误 `force flush worker panicked`。即使已经出现错误，也继续等待其余 worker。
7. 所有 worker 结束后先 `stores.Close()`，再 `pdMgr.Close()`。最后返回首错；没有错误（包括没有 store 匹配、响应结果为空）时返回 `Ok(())`。

## 数据与状态

输入配置 `ForceFlushConfig` 包含公共 `Config` 与已编译的 `regex::Regex`。`StoresPattern` 默认是 `.*`，非法正则在 `config.rs::ForceFlushConfig::ParseFromFlags` 中被拒绝，本文件无需处理正则编译失败。匹配对象是 `metapb::Store::Address`，通常形如 `<host>:20160`。

每个 store 的静态数据是 `Id`、`Address` 和标签列表。`IsTiFlash` 在当前 stub 中只检查是否存在严格等于 `("engine", "tiflash")` 的标签。`getAllTiKVs` 会新建过滤后的向量；`RunForceFlush` 随后消费该向量，不缓存拓扑，也不修改 PD 状态。

跨线程共享状态集中在 `Arc<StoreManager>`。测试桩内部以 `Mutex<HashMap<u64, Vec<FlushResult>>>` 和 `Mutex<HashMap<u64, String>>` 保存每个 store 的响应与错误，并以 `AtomicBool` 记录关闭状态。`first_err` 只由主线程在 `join` 后修改，不需要锁；错误选择取决于 handle 的创建/遍历顺序，而不是 worker 实际完成时间。

## 依赖与调用关系

上游调用链为：

- `br/cmd/br/operator.rs::newForceFlushCommand` 解析 flags 后调用 crate 根再导出的 `RunForceFlush`，这是 Rust 生产 CLI 的直接入口。
- `br/pkg/task/operator/lib.rs` 声明并再导出本模块。
- `br/pkg/task/operator/parity_test.rs` 直接调用 `getAllTiKVs` 与 `RunForceFlush`，验证 TiFlash 过滤、建 StoreManager 失败时关闭 PD，以及成功刷新后的资源关闭。

直接下游为：

- `config.rs::ForceFlushConfig` 提供公共连接配置和地址正则。
- `prepare_snap.rs::{dialPD, createStoreManager}` 创建 `Arc<PdController>` 与 `Arc<StoreManager>`；两者优先使用 `stubs.rs` 中的全局测试 hook。
- `stubs.rs::{PDClient::GetAllStores, IsTiFlash, StoreManager::FlushNow, Error, Result}` 提供拓扑查询、节点分类、刷新边界和统一错误类型。
- 标准库 `std::thread` 提供每 store 一个原生线程的并发模型。

`Cargo.toml` 没有 feature 声明。本文件直接涉及的第三方 crate 只有 `regex`，并且正则类型经 `ForceFlushConfig` 间接传入；PD、kvproto 和 gRPC 没有作为本 crate 的外部依赖出现，而是由本地 stub 类型代替。

## 错误处理与边界

- `dialPD` 错误直接传播。当前无 hook 的默认实现即使配置了 PD 地址也会拒绝假成功，因此真实 CLI 路径会停在该边界。
- `createStoreManager` 失败时确保 PD 关闭；`getAllTiKVs` 失败时确保 StoreManager 和 PD 都关闭。相应的 PD 清理分支已有独立契约断言。
- `FlushNow` 返回错误时，stub 会添加 `failed to flush store <id>` 上下文；`RunForceFlush` 保留该错误作为候选首错。
- `FlushNow` 返回多个任务结果时，任意 `Success=false` 都使该 worker 失败，错误包含任务名、store ID 和 `ErrorMessage`；该 worker 不再检查其后的结果。
- worker panic 不向主线程继续展开，而被归一为 `Error::new("force flush worker panicked")`。其他 worker 仍会被等待。
- 无匹配 store、PD 返回空列表或某 store 返回空结果数组都被视为成功。这符合当前 Rust 和 Go 控制流，但独立测试尚未逐项覆盖这些空路径。
- `Close` 返回 `()`，关闭失败没有表达或传播通道。代码依靠所有已知提前返回分支手工关闭资源，而不是 RAII guard；未来增加新的 `?` 分支时必须同步维护清理。
- 当前 `PDClient::GetAllStores()` 没有 Go 调用的 `opt.WithExcludeTombstone()` 参数，stub store 也没有节点状态字段，所以 Rust 不能证明已排除 tombstone store。这是明确的移植边界，不能把“非 TiFlash”误写成“活跃 TiKV”。

## 并发与资源生命周期

并发粒度是“每个匹配 store 一个 `std::thread`”，没有线程池或并发上限。store 数量增大时会线性创建操作系统线程；扩展生产实现时应评估资源开销，但不能在没有保持 Go 并行语义和错误行为的情况下随意串行化。

worker 通过克隆 `Arc<StoreManager>` 共享管理器，仅捕获 `store_id`，不借用循环中的 `Store` 或配置。主线程持有原始 `Arc` 并在全部 `join` 后调用 `Close`，因此不会在刷新仍在途时主动关闭管理器。线程按 handle 创建顺序被 join；这不阻止其他线程并发运行，但决定了多个失败中哪个错误成为 `first_err`。

与 Go 的 `errgroup.WithContext` 不同，Rust `RunForceFlush` 不接受 `Context`，CLI 中取得的 `_ctx` 也没有传入本函数。一个 worker 失败不会取消其余 worker，外部命令取消也不会传播到 `FlushNow`。当前实现选择等待所有线程并返回创建顺序上的首错；这是源码事实，不应表述为与 Go 的取消语义完全等价。

资源所有权顺序为 PD 后 StoreManager，关闭顺序为 StoreManager 后 PD。成功、查询失败和 StoreManager 创建失败路径都显式覆盖了各自已取得的资源；线程完成后的关闭已由 `contract_resource_cleanup` 验证。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/task/operator/force_flush.go`。两版的主阶段一致：连接 PD、创建 StoreManager、获取 store、过滤 TiFlash、按地址正则选择、并行调用 `FlushNow`、检查每个任务结果、等待结束并清理资源。错误文本 `failed to flush task ... at store ...` 也保持相同关键信息。

需要明确的差异如下：

1. Go `getAllTiKVs` 调用 `GetAllStores(ctx, opt.WithExcludeTombstone())`；Rust trait 没有 context、查询选项或 store 状态字段，只能过滤 TiFlash，不能表达 tombstone 排除。
2. Go `RunForceFlush(ctx, cfg)` 使用 `errgroup.WithContext(ctx)`，首个 goroutine 错误会取消派生 context，PD、建连和 `FlushNow` 都接收 context。Rust 签名没有 context，worker 失败后仍等待其他独立线程自然结束。
3. Go 通过 `StoreManager::WithConn` 构造真实 `logbackuppb::LogBackupClient` 并发 RPC；Rust 直接调用本地 stub `StoreManager::FlushNow`。当前 manifest 也没有 kvproto/gRPC 依赖，所以真实 RPC 尚未接线。
4. Go 的 `defer` 保证取得资源后的所有返回路径自动关闭；Rust 手工覆盖现有分支，并在 worker 全部 join 后关闭。现有契约测试验证了建 StoreManager 失败和成功路径，但没有穷举所有 worker 错误/崩溃路径的清理。
5. Go `errgroup.Wait` 返回按运行时竞争到的首个非空错误；Rust 按 handle 创建顺序扫描 join 结果，所以并发多错时的具体错误选择可能不同。

## 扩展指南

- 接通真实集群时，最关键的修改点是 `prepare_snap.rs::{dialPD, createStoreManager}` 与 `stubs.rs::StoreManager::FlushNow` 的边界；应引入正式 PD/日志备份 client，而不是让默认 hook 或注入表伪装生产成功。
- 补齐 Go 查询语义时，应让 `getAllTiKVs` 能请求排除 tombstone，并以真实 store 状态测试；仍需保留 TiFlash 标签过滤。若调整函数签名，应同步修改 `RunForceFlush`、CLI 和独立测试。
- 补齐取消语义时，应从 `br/cmd/br/operator.rs::newForceFlushCommand` 把 context 传到 `RunForceFlush`，再传到 PD、连接和 FlushNow 边界；需验证首错触发取消、其他 worker 收敛及资源关闭，而不是仅增加一个未使用参数。
- 若增加并发上限或线程池，应保留“所有已启动工作结束后再关闭资源”“任一任务结果失败即 store 失败”和可诊断错误上下文，并评估大量 store 下的性能与公平性。
- 建议在同目录新增独立的 `force_flush_test.rs`，并从 `lib.rs` 以 `#[cfg(test)]` 挂载；不要把测试嵌入生产文件。至少覆盖地址不匹配、TiFlash/tombstone、PD 查询错误、FlushNow RPC 错误、响应内失败、多 store 多错、worker panic、空拓扑、清理和取消。
- 更改日志或错误时应保留 store ID、地址或任务名等诊断信息，避免把服务端错误压成无上下文的通用失败。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/task/operator` 定位目标、Go 对照与测试；`node --file br/pkg/task/operator/force_flush.rs --offset 1 --limit 240` 核对了目标文件全部 128 行；`query RunForceFlush`、`query getAllTiKVs` 识别 Rust/Go 对照定义；`callees RunForceFlush` 给出 Rust `RunForceFlush → getAllTiKVs`；索引同时报告目标文件由 `br/pkg/task/operator/parity_test.rs` 使用。
- Rust 源码与入口：`br/pkg/task/operator/force_flush.rs`、`br/pkg/task/operator/lib.rs`、`br/pkg/task/operator/config.rs`、`br/pkg/task/operator/prepare_snap.rs`、`br/pkg/task/operator/stubs.rs`、`br/cmd/br/operator.rs::newForceFlushCommand`。
- crate 边界：`br/pkg/task/operator/Cargo.toml` 确认 crate 名、`lib.rs` 入口、Go 包映射、无 feature，以及本地依赖/stub 定位。
- Go 对照：`br/pkg/task/operator/force_flush.go::{getAllTiKVs, RunForceFlush}`、`br/cmd/br/operator.go::newForceFlushCommand`。
- 独立 Rust 测试：`br/pkg/task/operator/parity_test.rs::contract_normal_config_and_helpers` 验证 TiFlash 过滤；`contract_error_paths` 验证 StoreManager 创建失败时关闭 PD；`contract_resource_cleanup` 验证成功刷新并关闭 PD/StoreManager。未发现同名 `force_flush_test.rs`，现有测试也未覆盖 tombstone、正则跳过、FlushNow 错误、响应失败、panic 和取消传播。
- 本任务是纯文档分析，按总计划不运行 Cargo；交付前执行任务指定的结构命令，验证本文存在且恰好包含 11 个固定二级章节。
