# `lightning/pkg/importer/checksum_helper.rs`

## 文件定位

`WithChecksumManager` 与 `CHECKSUM_MANAGER_KEY` 定义模块内部的 context 传递协议，使策略构造与按表执行解耦。

## 核心职责

1. `NewChecksumManager` 根据配置和集群能力选择 checksum 策略：TiDB backend 或 checksum 关闭时跳过；其余场景先读取 PD 版本，再选 TiKV manager 或 TiDB SQL executor。
2. 在 TiKV 分支读取 `tidb_backoff_weight`，只有成功取得且不小于 `ingestctrl::DefaultBackoffWeight` 的值才保留，否则回退到默认值并记录日志。
3. `WithChecksumManager` 与 `CHECKSUM_MANAGER_KEY` 定义模块内部的 context 传递协议，使策略构造与按表执行解耦。
4. `DoChecksum` 取得 manager、调用 trait 方法、结束带表名的日志任务，并在 metrics 存在时记录一次耗时；manager 的成功值或错误原样返回。

本模块不负责 checksum 结果的可选/必选错误策略、重复数据跳过条件或结果比较。Go 主链的直接证据分别位于 `lightning/pkg/importer/import.go`（构造并放入 context）、`table_import.go`（执行并按 `OpLevelOptional` 处理错误、比较 checksum）和 `meta_manager.go`（导入前的基线 checksum）。

## 主要符号

- `pub const CHECKSUM_MANAGER_KEY: &str = "checksumManagerKey"`：Rust context 的字符串键。写入和读取必须使用完全相同的键和 holder 类型。
- `pub fn NewChecksumManager(ctx: Context, rc: &Controller, store: &kv::Storage) -> Result<Option<Arc<dyn ChecksumManager>>>`：策略工厂。`Ok(None)` 是合法的“无需校验”，不是错误；`Ok(Some(...))` 返回线程安全的共享 trait object。
- `pub fn DoChecksum(ctx: Context, table: &importdef::TableInfo) -> Result<ingestctrl::RemoteChecksum>`：单表执行入口。它要求 context 中存在 `ChecksumManagerHolder`，并将 `table.Name` 加入远端 checksum 日志。
- `pub struct ChecksumManagerHolder(pub Arc<dyn ChecksumManager>)`：为动态 trait object 提供可放入 `Any` context 的具体包装类型；读取时通过 `downcast_ref` 恢复类型。
- `pub fn WithChecksumManager(ctx: Context, mgr: Arc<dyn ChecksumManager>) -> Context`：不可变风格地派生新 context，并把 holder 写入 `CHECKSUM_MANAGER_KEY`。

文件没有条件编译项、内部私有函数或本地算法状态；所有顶层符号均为 `pub`，再由 crate 根统一再导出。

## 执行流程

`NewChecksumManager` 的决策顺序如下：

1. 检查 `rc.cfg.TikvImporter.Backend == config::BackendTiDB` 或 `rc.cfg.PostRestore.Checksum == config::OpLevelOff`；任一成立就立即返回 `Ok(None)`，不会访问 PD、DB 或 storage。
2. 调用 `pdutil::FetchPDVersion(ctx.clone(), rc.pdHTTPCli.clone().unwrap_or_default())`。错误经过 `errors::Trace` 返回，不继续选择执行器。
3. 当 `pdVersion.Major >= 4` 且没有启用 `ChecksumViaSQL` 时进入 TiKV 分支：
   - 用 `rc.db.as_ref().unwrap()` 调用 `common::GetBackoffWeightFromDB`；
   - 查询成功且值大于等于默认权重时沿用该值，否则使用 `ingestctrl::DefaultBackoffWeight`；两条路径都写 info 日志；
   - 调用 `store.GetClient()`，但当前返回值被丢弃；
   - 用两个 `()` 占位参数、DistSQL 扫描并发度、backoff 权重、resource group 和 task type 构造 `NewTiKVChecksumManager`。
4. PD 主版本小于 4，或显式启用 `ChecksumViaSQL` 时，用 `rc.db.as_ref().unwrap().clone()` 构造 `NewTiDBChecksumExecutor`。
5. 将具体执行器擦除为 `Arc<dyn ChecksumManager>` 并返回 `Ok(Some(manager))`。

`DoChecksum` 的流程是：用键取 context 值，向下转型成 `ChecksumManagerHolder` 并克隆其中的 `Arc`；若任一步失败则返回初始化错误。成功时创建带表名字段的日志 task，调用 `manager.Checksum(ctx.clone(), table)`，不论成功或失败都用错误引用结束 task；若 `metric::FromContext(ctx)` 返回指标对象，则观察 `dur.as_secs_f64()`；最后原样返回调用结果。

## 数据与状态

该文件自身没有可变全局状态。长期共享状态是 `Arc<dyn ChecksumManager>`：`ChecksumManager` 在 `stubs.rs` 中要求 `Send + Sync`，holder 和派生 context 通过克隆 `Arc` 共享同一执行器，不复制执行器内部状态。

`Context` 当前由 `stubs.rs` 实现为 `Arc<HashMap<String, Arc<dyn Any + Send + Sync>>>` 加取消状态。`WithValue` 克隆 map 后生成新的 `Context`，因此 `WithChecksumManager` 不会就地修改传入 context；同名键会在新 context 中覆盖。值的运行时类型同时构成协议的一部分：即使键存在，若不是 `ChecksumManagerHolder`，`DoChecksum` 仍按“manager 缺失”返回错误。

`RemoteChecksum` 包含 `Schema`、`Table`、`Checksum`、`TotalKVs`、`TotalBytes`。不过 importer stub 中的 TiKV/TiDB 实现当前只复制 schema/table，其余字段为零；不能据此声称 Rust 路径已经执行真实远端 checksum。当前 `DefaultBackoffWeight` 也来自 importer stub，值为 2，与独立生产 crate `pkg/ingestor/ingestctrl/checksum.rs` 中的 15 不同，扩展时必须先确认应接入哪套实现。

## 依赖与调用关系

上游关系：

- RustCodeGraph 的文件关系只识别到 `lightning/pkg/importer/parity_test.rs`；其中 `contract_parity` 验证 TiDB backend 返回 `None`，`contract_error` 验证 context 缺失 manager 时返回错误。
- Rust 文本检索没有发现 `WithChecksumManager` 的调用点，也没有发现除上述 parity test 外的 `NewChecksumManager`/`DoChecksum` 调用点。这表示 Rust 生产编排尚未把本文件接入完整主链。
- Go 对照主链由 `import.go` 构造 manager 并放入 context，`table_import.go` 与 `meta_manager.go` 调用 `DoChecksum`；这些是理解预期位置的证据，而不是 Rust 已接线的证据。

下游关系：

- 配置与控制器字段：`config::{BackendTiDB, OpLevelOff}`、`Controller::{cfg, pdHTTPCli, db, resourceGroupName, taskType}`。
- 能力与参数探测：`pdutil::FetchPDVersion`、`common::GetBackoffWeightFromDB`、`kv::Storage::GetClient`。
- 执行器边界：`ingestctrl::{ChecksumManager, NewTiKVChecksumManager, NewTiDBChecksumExecutor, RemoteChecksum, DefaultBackoffWeight}`。
- 观测边界：`log::Wrap`、`logutil::Logger`、`zap` 字段以及 `metric::FromContext(...).checksum_hist().Observe(...)`。
- 错误边界：crate 的 `errors::{Trace, New}` 和 `Result`。

RustCodeGraph 对本文件的精确 `node` 查询可展示全部源码，但 `callers` 没有返回调用者，`callees` 又因符号解析歧义报告“No callees found”；因此调用证据按技能规则由模块入口、`rg` 结果和直接依赖源码补足。

## 错误处理与边界

- checksum 被关闭或 backend 为 TiDB 时返回 `Ok(None)`，调用者必须把它当作跳过状态，不能当成构造失败。
- PD 版本读取失败会被 `errors::Trace` 包装后传播，是构造阶段显式错误。
- backoff 权重查询失败、解析失败形成的低值，或值低于默认阈值都不会中止构造，而是统一回退默认值；这种容错会隐藏具体查询错误，只以“set ... to default”日志呈现。
- `rc.db` 在两种实际执行器分支中均用 `unwrap()`。只要 checksum 未被提前跳过，控制器就必须已经初始化 DB，否则会 panic；这是当前 API 的隐含前置条件。
- `pdHTTPCli` 缺失时使用默认 client，而非报初始化错误。当前 stub 的 `FetchPDVersion` 固定成功返回 6.0.0，因而无法在这里真实覆盖旧 PD 或网络失败。
- `DoChecksum` 将“键不存在”和“值类型错误”折叠成同一错误，文本仍沿用 Go 的 `No gcLifeTimeManager found...`，名称与 checksum manager 不一致但属于当前兼容行为。
- manager 的 `Checksum` 错误不在本层转换或吞掉；日志结束和耗时记录仍会执行，然后返回原错误。
- 本层不校验 `TableInfo` 名称格式，也不判断返回统计是否与本地 checksum 一致。

## 并发与资源生命周期

manager 以 `Arc<dyn ChecksumManager + Send + Sync>` 的约束形式跨 context 和潜在并发任务共享；`DoChecksum` 仅短暂克隆 `Arc`，函数结束时释放该引用，不独占或关闭 manager。`WithChecksumManager` 派生的新 context 持有另一份强引用，因此 manager 至少存活到所有相关 context 和临时克隆被释放。

当前 Rust trait 没有 Go `ChecksumManager.Close()` 对应的生命周期方法，本文件也没有显式清理逻辑。Go `import.go` 会在 manager 非 nil 时 `defer manager.Close()`；因此真实客户端、GC TTL 或后台任务若迁入 Rust，必须明确把清理能力放入 trait、RAII 类型或拥有者，而不能假设本模块已经对齐资源释放。

本文件不创建线程、异步任务、锁或通道。并发度只是把 `DistSQLScanConcurrency as u32` 传给 TiKV manager；负数或超出 `u32` 的值会按 Rust `as` 规则转换，本层没有范围校验。日志 task 与 metric observation 都限定在一次 `DoChecksum` 调用内，且耗时包含 manager 返回错误前消耗的时间。

## 与 Go 版本的对应关系

主要差异如下：

- Go 用 `nil` 表示跳过；Rust 用 `Result<Option<Arc<dyn ChecksumManager>>>` 明确区分跳过、成功对象和错误。
- Go 将接口值直接存入以私有地址 `&checksumManagerKey` 为键的标准 context；Rust 使用公开字符串键、`ChecksumManagerHolder` 和 `Any` 向下转型，并提供 `WithChecksumManager` helper。
- Go TiKV 构造器接收真实 `store.GetClient()` 与 `rc.pdCli`；当前 Rust 调用了 `GetClient` 却丢弃结果，并传入两个 `()`。Go PD 版本读取访问真实 HTTP client；Rust stub 固定返回主版本 6。
- Go 执行器包含真实 TiKV/SQL checksum 行为和 `Close` 生命周期；当前 importer Rust stub 只返回 schema/table 与零值统计，且 trait 无 `Close`。
- Go 的返回值为指针，可表示 nil checksum；Rust 返回实体 `RemoteChecksum`，成功时不能为 `None`。
- Go 的专项行为由 `table_import_test.go`、`meta_manager_test.go` 以及 `pkg/ingestor/ingestctrl/checksum_test.go` 间接或直接覆盖；Rust 当前只有 `parity_test.rs` 的两个边界断言，没有成功执行、TiKV/SQL 分支、backoff、指标或并发测试。

## 扩展指南

- 若补齐 Rust 生产接线，优先在 importer 顶层运行流程中调用 `NewChecksumManager`，只在返回 `Some` 时调用 `WithChecksumManager`；所有进入 `DoChecksum` 的派生 context 都必须保留该值。不要把 `None` 写成 holder。
- 若替换 slim-port checksum 实现，应让 `NewTiKVChecksumManager` 接收真实 KV/PD client，并核对 `DefaultBackoffWeight`、取消协议、resource group、task type、GC TTL 与 Go 一致；同时设计确定性的关闭/RAII 语义。
- 修改策略分支时，应在独立 Rust 测试文件（当前可扩展 `lightning/pkg/importer/parity_test.rs`，不要把测试嵌入源文件）覆盖：两种跳过条件、PD `<4`、`ChecksumViaSQL`、合法/过低/查询失败的 backoff、缺失 DB 的前置条件，以及构造错误传播。
- 修改 `DoChecksum` 时，应增加一个可注入的 fake `ChecksumManager`，验证成功值、错误传播、错误场景也记录耗时、键存在但类型错误、context 派生和并发共享；并对照 Go `table_import_test.go` 与 `meta_manager_test.go` 的上层行为。
- 若改变 key 或 holder，必须同时更新所有写入和读取点；字符串键有碰撞风险，迁移到类型化 context 时应一次性调整协议并保留兼容测试。
- 性能风险主要来自远端 checksum、并发度、backoff 和额外 Arc/context map 克隆；兼容风险主要来自选择不同执行器、改变可选错误处理的上层约定、默认权重漂移和遗漏资源清理。
