# [`pkg/workloadlearning/cache.rs`](cache.rs)

## 文件定位

本文件属于 Cargo 包 `astersql-workloadlearning`（`pkg/workloadlearning/Cargo.toml`），由 `pkg/workloadlearning/lib.rs` 的 `mod cache` 纳入模块，并通过 `pub use cache::*` 对外导出。根 `Cargo.toml` 又以 `facade_workloadlearning` 引用该包，`pkg/lib.rs` 将其公开项重新导出。它位于“指标已经由 `Handle::SaveTableReadCostMetrics` 写入版本化存储”之后，负责把某一最新版本的表读代价加载成进程内快照并提供按表 ID 查询。

当前 Rust 仓库中，`NewWLCacheWorker`、`UpdateTableReadCostCache` 和 `GetTableReadCostMetrics` 的直接使用只出现在独立测试 `pkg/workloadlearning/cache_test.rs`；未检索到 Rust 生产调用者。因此，这个文件已经实现缓存能力并纳入 workspace/facade，但尚不能据此断言它已接入 Rust 服务的后台运行主链。完整应用中的定时接线目前可在 Go 的 `pkg/domain/domain.go::{SetupWorkloadBasedLearningWorker,readTableCostWorker}` 找到。

## 核心职责

- `TableReadCostCache` 把一个存储版本号与该版本的 `table_id -> TableReadCostMetrics` 映射绑定为同一快照。
- `WLCacheWorker` 持有共享的 `Arc<dyn WorkloadStore>`，用它查询最新版本及加载该版本的 JSON 行；缓存本身由 `RwLock<TableReadCostCache>` 保护。
- `UpdateTableReadCostCache` 只在存储版本严格大于本地版本时刷新；存储访问失败会向调用者返回错误，单行 JSON 解析失败则跳过该行。
- `GetTableReadCostMetrics` 返回值类型而不是缓存内部引用，并且只投影成本相关四个字段，避免暴露缓存条目以及用于落盘的库名、表名。

本文件不负责分析 SQL、计算成本、生成版本或把指标写入存储；这些职责属于 `pkg/workloadlearning/handle.rs`。它也不提供刷新定时器或后台任务生命周期，调用时机由上层负责。

## 主要符号

- `pub struct TableReadCostCache`：可克隆、可比较的缓存快照。公开字段 `TableReadCostMetrics: HashMap<i64, TableReadCostMetrics>` 以表 ID 为键，`Version: u64` 记录该映射来自哪个存储版本；`Default` 产生空映射和版本 `0`。
- `pub struct WLCacheWorker`：缓存工作器。`store: Arc<dyn WorkloadStore>` 允许工作器共享线程安全的存储实现；`tableReadCostCache: RwLock<TableReadCostCache>` 是私有的并发状态。
- `pub fn NewWLCacheWorker(store: Arc<dyn WorkloadStore>) -> WLCacheWorker`：以给定存储构造版本为 `0` 的空缓存。
- `pub fn WLCacheWorker::UpdateTableReadCostCache(&self) -> Result<bool, String>`：刷新入口。`Ok(true)` 表示已用最新版本整体替换缓存，`Ok(false)` 表示存储版本没有前进，`Err(String)` 表示查询版本或加载指标失败。
- `pub fn WLCacheWorker::updateTableReadCostCacheWithMetrics(...)`：以一次写锁整体替换映射和版本。尽管名称采用内部风格，它在 Rust 中声明为 `pub`，因此通过模块再导出后是外部可调用 API；现有测试也直接用它构造快照。
- `pub fn WLCacheWorker::GetTableReadCostMetrics(&self, tableID: i64) -> Option<TableReadCostMetrics>`：只读查询。命中时复制 `TableScanTime`、`TableMemUsage`、`ReadFrequency`、`TableReadCost`，其余字段由 `Default` 填充；未命中返回 `None`。

文件没有模块级常量、trait、条件编译项或异步函数。

## 执行流程

刷新流程由 `UpdateTableReadCostCache` 驱动：

1. 调用 `WorkloadStore::latest_version()` 取得存储最新版本；失败立即以 `Err(String)` 返回，缓存不变。
2. 短暂取得读锁比较版本。若 `latestVersionInStorage <= cache.Version`，返回 `Ok(false)`，不调用 `load_metrics`。
3. 调用 `WorkloadStore::load_metrics(latestVersionInStorage)` 取得 `(table_id, JSON)` 行；失败返回错误，缓存不变。
4. 逐行执行 `serde_json::from_str::<TableReadCostMetrics>`。成功的值插入新 `HashMap`；失败的行被静默跳过。同一表 ID 多次出现时，后插入的成功值覆盖先前值。
5. 调用 `updateTableReadCostCacheWithMetrics`，在写锁内一次性替换整个 `TableReadCostCache`，使映射和版本一起对读者可见，然后返回 `Ok(true)`。

查询流程由 `GetTableReadCostMetrics` 驱动：取得读锁、按 `tableID` 查找、构造一个独立的投影值并释放读锁。返回值不借用锁内数据，调用者不能通过它修改缓存。

## 数据与状态

缓存唯一的可变状态是 `RwLock<TableReadCostCache>`。快照的关键不变量是 `Version` 与 `TableReadCostMetrics` 必须描述同一次整体替换；`updateTableReadCostCacheWithMetrics` 通过赋值一个完整结构体维持这一点，而不是分别更新两个字段。

版本比较依赖存储版本单调递增：相等或倒退均被视为“不更新”。`Handle::SaveTableReadCostMetrics` 的相邻实现使用 `latest_version()?.saturating_add(1)` 生成版本，并把序列化行交给 `save_metrics`，这是当前生产者侧的直接依据。缓存不会合并新旧映射；每次有效刷新都完全丢弃旧快照，因此存储返回的指定版本必须是完整快照而非增量。

`TableReadCostMetrics` 定义在 `pkg/workloadlearning/metrics.rs`，JSON 包含库名、表名、纳秒表示的扫描时长、内存、频率和综合代价。加载时保存完整指标；查询时刻意把 `DbName`、`TableName` 留为默认空值。空版本也可以成为有效快照：若版本前进但 `load_metrics` 返回空列表或所有行均解析失败，方法仍安装空映射、推进版本并返回 `Ok(true)`。

## 依赖与调用关系

向下依赖如下：

- `crate::WorkloadStore`（定义于 `pkg/workloadlearning/handle.rs`）：本文件只调用 `latest_version` 和 `load_metrics`。
- `crate::TableReadCostMetrics`（定义于 `pkg/workloadlearning/metrics.rs`）：作为 JSON 反序列化目标、缓存值和查询返回值。
- `serde_json::from_str`：解析存储中的指标 JSON；`serde_json = "1"` 由本 crate 的 `Cargo.toml` 声明。
- 标准库 `Arc`、`RwLock`、`HashMap`：分别承担共享所有权、并发快照保护和表 ID 索引。

向上调用方面，RustCodeGraph 确认了本文件的结构体和函数节点，但对这些 Rust 方法的 `callers`/`callees` 查询均未给出调用边。文本检索补充确认：Rust 直接调用仅见 `pkg/workloadlearning/cache_test.rs`；`pkg/workloadlearning/lib.rs` 和根 facade 提供导出接线。Go 对照实现则由 `pkg/domain/domain.go::readTableCostWorker` 在工作负载学习开关开启且当前节点为 owner 时，先分析/保存指标，再调用缓存刷新。

## 错误处理与边界

- `latest_version` 或 `load_metrics` 返回的 `String` 错误通过 `?` 原样传播；这两类失败不会安装部分结果。
- 单条 JSON 无法反序列化时不返回错误，也不记录诊断信息，而是跳过该表；这与 Go 版本“记录警告后继续”的容错方向一致，但 Rust 版本缺少日志可观测性。
- 未命中的表 ID 返回 `None`。版本 `0` 的空缓存因此可安全查询。
- `RwLock::read()` 和 `write()` 都直接 `unwrap()`；若持锁线程 panic 导致锁中毒，后续刷新或查询也会 panic，而不是转换成 `Result`。
- 方法不校验负表 ID、数值范围、NaN/无穷代价或业务字段一致性；合法性主要由 `TableReadCostMetrics` 的 serde 类型约束和上游生产者保证。
- 版本检查与最终写入之间没有二次比较。若多个线程并发刷新且读取到不同版本，较旧请求可能在较新请求之后取得写锁并覆盖较新快照。这意味着调用方应串行调度刷新，或扩展实现时在写锁内再次做版本单调性检查。

## 并发与资源生命周期

`WorkloadStore: Send + Sync` 加上 `Arc<dyn WorkloadStore>` 允许存储在多个工作器或线程间共享；`WLCacheWorker` 自身没有显式 `Drop`，资源释放遵循 `Arc` 和锁的 RAII。构造函数把存储所有权计数移入工作器，工作器销毁时释放这一份引用。

刷新不会在存储 I/O 或 JSON 解析期间持有缓存锁：版本比较只持短读锁，准备好完整 `HashMap` 后才取一次写锁。这减少读路径阻塞，并保证读者只能观察旧快照或完整新快照，不能观察逐行构建状态。代价是存在前述并发刷新乱序覆盖窗口。

查询仅在查找和复制四个成本字段时持读锁，返回后不再依赖锁。文件自身不创建线程、任务、通道、定时器、事务或会话，也不管理刷新重试；这些都属于调用方或 `WorkloadStore` 实现的生命周期。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/workloadlearning/cache.go`：两者都有 `TableReadCostCache`、`WLCacheWorker`、构造、版本门控刷新、整体替换和按表 ID 读取，并都在 JSON 行损坏时跳过该行、在查询时只复制四个成本字段。

关键差异如下：

- Go 工作器直接持有 `util.DestroyableSessionPool`，执行两条 restricted SQL，并负责会话归还/销毁与日志；Rust 把持久化细节抽象成 `WorkloadStore`，所以本文件没有 SQL、会话或日志依赖。
- Go `UpdateTableReadCostCache` 无返回值，通过日志表达错误或无需更新；Rust 返回 `Result<bool, String>`，把失败和“版本未前进”显式交给调用者。
- Go 缓存值是指针映射，Rust 使用拥有所有权的值映射；两者的 getter 都生成独立结果，避免调用方改写缓存。
- Go 运行路径已由 `Domain` 周期性调度；当前 Rust 搜索只发现测试调用，不能把 Go 的接线状态等同于 Rust 已接线。
- Go 在坏 JSON、查询异常和版本未变化时记录日志；Rust 的坏 JSON 是静默跳过，存储错误仅返回字符串。

`pkg/workloadlearning/cache_test.go` 验证落盘后刷新与空缓存查询；Rust 的 `cache_test.rs` 保留这些意图，并额外验证第二次相同版本刷新返回 `false`，以及 getter 不返回库名和表名。

## 扩展指南

- 若增加缓存字段或改变查询投影，优先修改 `TableReadCostCache`、`GetTableReadCostMetrics` 及 `pkg/workloadlearning/metrics.rs::TableReadCostMetrics`，并同步独立测试 `pkg/workloadlearning/cache_test.rs`；若要求 Go/Rust 等价，还需核对 `cache.go` 与 `cache_test.go`。
- 若改变版本策略或支持增量更新，应集中调整 `UpdateTableReadCostCache` 与 `updateTableReadCostCacheWithMetrics`，明确存储返回的是完整快照还是增量，并新增乱序版本、空版本、重复表 ID 的测试。
- 若允许多个刷新者并发，必须防止旧版本覆盖新版本；可在写锁内重新比较版本，只有更大版本才能安装。需要用独立测试通过可控 `WorkloadStore` 制造加载顺序反转。
- 若增强坏数据处理，需决定“跳过并推进版本”“整批失败”或“保留旧条目”的兼容语义，并为全坏、部分坏 JSON 增加测试。日志或错误聚合不应延长写锁持有时间。
- 若接入 Rust 服务主链，应在上层生命周期组件中创建工作器和定时任务，而不是把调度塞入本文件；同时验证关闭信号、owner 语义、重试和错误观测。当前 Go 接线可作为行为参照，但不能直接假定 Rust 已具备相同基础设施。
- Rust 单元测试继续放在 `pkg/workloadlearning/cache_test.rs`，通过 `lib.rs` 的 `#[cfg(test)] #[path = "cache_test.rs"]` 挂载，不要内嵌到生产源文件。

## 验证依据

- 目标实现：`pkg/workloadlearning/cache.rs`，核对了两个结构体、构造函数和三个方法的完整源码。
- crate 与模块边界：`pkg/workloadlearning/Cargo.toml`、`pkg/workloadlearning/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`；确认 workspace 成员、serde 依赖和 facade 再导出。
- 直接依赖：`pkg/workloadlearning/handle.rs::{WorkloadStore,Handle::SaveTableReadCostMetrics}` 与 `pkg/workloadlearning/metrics.rs::TableReadCostMetrics`。
- Rust 测试：`pkg/workloadlearning/cache_test.rs::{TestUpdateTableCostCache,TestGetTableReadCacheMetricsWithNoData,get_table_read_cost_metrics_matches_go_projection}`。
- Go 对照与运行接线：`pkg/workloadlearning/cache.go`、`pkg/workloadlearning/cache_test.go`、`pkg/domain/domain.go::{SetupWorkloadBasedLearningWorker,readTableCostWorker}`。
- RustCodeGraph：`status` 显示索引包含 `pkg/workloadlearning/cache.rs`；`files --filter pkg/workloadlearning` 列出相关 Rust/Go 文件；`query` 确认 `WLCacheWorker`、`NewWLCacheWorker`、`UpdateTableReadCostCache`、`updateTableReadCostCacheWithMetrics`、`GetTableReadCostMetrics` 的 Rust 节点。方法调用边查询为空，因此调用位置和“暂无 Rust 生产调用者”由 `rg` 全仓 Rust 检索补证，未把空图边单独当作结论。
- 本任务是纯文档分析，按任务约束未运行 Cargo；最终以固定十一章节结构命令和人工事实复核验收。
