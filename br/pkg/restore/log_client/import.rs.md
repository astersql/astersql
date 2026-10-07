# `br/pkg/restore/log_client/import.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-log-client` crate（入口与依赖见 [`lib.rs`](./lib.rs) 和 [`Cargo.toml`](./Cargo.toml)），实现日志恢复阶段把外部存储中的 KV 日志文件按目标 TiKV Region 下载并 Apply 的导入器。`lib.rs` 通过 `pub mod import` 和 `pub use import::*` 暴露本模块；上层 [`client.rs`](./client.rs) 的 `LogRestoreManager.fileImporter` 持有 `LogFileImporter`，`LogClient::InitClients` 负责构造它，`LogClient::CleanUpKVFiles` 和 `LogRestoreManager::Close` 分别接入清理与关闭生命周期。

当前 Rust 迁移状态需要特别区分“实现存在”和“已接入生产主链”：仓库内非测试 Rust 源码会构造、清理和关闭 `LogFileImporter`，但代码搜索未发现生产 Rust 调用 `LogFileImporter::ImportKVFiles`。批量/逐文件编排函数 `ApplyKVFilesWithBatchMethod`、`ApplyKVFilesWithSingleMethod` 目前也只接受通用回调，未在生产 Rust 中绑定到本文件。因此本文件的 Apply 主路径已有实现并受独立测试覆盖，但不能据此声称 Rust PiTR 生产编排已经完整接线。

## 核心职责

- `LogFileImporter` 聚合 Region 拓扑客户端 `SplitClient`、TiKV ImportSST 客户端 `ImporterClient`、外部存储描述 `StorageBackend` 和会话级 `cacheKey`。
- `ImportKVFiles` 先计算所有文件经 rewrite 后的编码键范围，再通过 [`import_retry.rs`](./import_retry.rs) 的 `CreateRangeController` 遍历/重试 Region；每个 Region 仅处理与之相交的文件。
- `downloadAndApplyKVFileOwned` 为目标 Region 生成 `KVMeta`、`RewriteRule`、`kvrpcpb::Context` 和批量或单文件 `ApplyRequest`，并向 leader store 调用 `ImporterClient::ApplyKVFile`。
- `ClearFiles` 在所有 Up 状态 TiKV 上尽力清除指定前缀的 importer 临时文件；`Close` 释放 importer gRPC 客户端。
- `filterFilesByRegion` 维持文件和预计算范围的下标对应关系，并执行与 Go 版本一致的边界相交判断。

## 主要符号

- `pub struct LogFileImporter`：四个字段分别是 `metaClient: Arc<dyn SplitClient>`、`importClient: Arc<dyn ImporterClient>`、可选 `backend` 和字符串 `cacheKey`。字段是公开的，但语义上共同构成一次日志文件导入会话。
- `pub fn NewLogFileImporter(...) -> LogFileImporter`：构造导入器。Rust 使用 Unix 秒及其乘积生成 `BR-<sec>-<value>`；它只降低命名碰撞概率，不提供随机性或全局唯一性保证。
- `LogFileImporter::Close(&self) -> Result<()>`：直接转发 `CloseGrpcClient`，不吞掉关闭错误。
- `LogFileImporter::ClearFiles(...) -> Result<()>`：获取排除 TiFlash 的 TiKV 列表，只处理 `StoreState::Up`，逐店发送 `ClearRequest { Prefix }`。
- `LogFileImporter::ImportKVFiles(...) -> Result<()>`：公开 Apply 入口；参数包含文件、rewrite 规则、三个时间戳、批能力开关、文件加密信息和主密钥。
- `importKVFileForRegionOwned(...) -> RPCResult`：Region 级适配层；把“rewrite 规则缺失”和“空 KV 范围”降级为成功跳过，成功 Apply 时累计 `RegionInvolved`。
- `downloadAndApplyKVFileOwned(...) -> RPCResult`：请求组装与 RPC 边界。它是模块私有函数，拥有的参数由 `ImportKVFiles` 的 `'static` Region 回调捕获。
- `pub fn filterFilesByRegion(...) -> Result<Vec<LogDataFileInfo>>`：验证 `files.len() == ranges.len()`，保留与 Region 相交的文件；测试通过 [`export_test.rs`](./export_test.rs) 的 `FilterFilesByRegion` 别名访问。

本文件没有 trait 定义、模块级业务常量或条件编译项；重试参数 `45` 次、`100ms` 初始退避、`15s` 上限是在 `ImportKVFiles` 内部构造的策略值。

## 执行流程

1. `NewLogFileImporter` 保存两个客户端和外部存储后端，并生成会话缓存键。`LogClient::InitClients` 将其放入 `LogRestoreManager`。
2. `ImportKVFiles` 首先拒绝 `supportBatch == false && files.len() > 1`，确保旧 TiKV 的单文件协议不会静默丢文件。
3. 对每个 `LogDataFileInfo` 调用 `GetRewriteEncodedKeys`；结果既保存到与 `files` 等长的 `ranges`，又归并为全局最小 `startKey` 和最大 `endKey`。
4. 函数创建 `RangeCtlMetricListener` 和指数退避状态，再用 `CreateRangeController(startKey, endKey, metaClient, retry_state)` 建立 Region 遍历器。
5. Region 回调先增加原子计数，通过 `filterFilesByRegion` 选出相交文件并记录文件数指标；空集合直接返回 `RPCResultOK`。
6. 非空集合进入 `importKVFileForRegionOwned`。它调用 `downloadAndApplyKVFileOwned`，并把错误码 `BR:KV:ErrKVRewriteRuleNotFound`、`BR:KV:ErrKVRangeIsEmpty` 解释为该 Region 无适用数据，记录告警后返回成功；其他错误原样交给 RangeController 的重试/失败策略。
7. `downloadAndApplyKVFileOwned` 要求 Region 和 leader 均存在。每个文件必须匹配 rewrite 规则；随后生成编码前缀规则和裁剪到当前 Region 边界的 `KVMeta`。
8. 批量能力开启时填充 `ApplyRequest.Metas/RewriteRules`；关闭时填充单个 `Meta/RewriteRule`。两种请求都会携带 backend、Region epoch/peer、cache key、cipher 和 master keys。
9. RPC 发往 leader 的 store ID。传输错误经 `RPCResultFromError` 包装，响应内 PB 错误经 `RPCResultFromPBError` 分类，无错误则成功。
10. RangeController 完成后记录本批回调涉及的 Region 数，`ImportKVFiles` 返回控制器最终结果。

独立的清理路径由 `LogClient::CleanUpKVFiles` 调用 `ClearFiles`：PD/store 枚举失败会返回错误；某个 Up store 的清理 RPC 失败只告警并继续其它 store。任务结束时 `LogRestoreManager::Close` 调用本文件的 `Close`，但上层管理器会把关闭错误降级为告警。

## 数据与状态

`LogFileImporter` 自身没有锁或可变集合；共享客户端通过 `Arc<dyn ...>` 持有，方法均借用 `&self`。`backend` 会复制进每个 Apply 请求，`cacheKey` 用作 TiKV 侧 `StorageCacheId`，将不同导入会话的临时对象命名空间隔离开。

`ImportKVFiles` 的关键不变量是 `files[i]` 与 `ranges[i]` 对应。`filterFilesByRegion` 在长度不等时返回 `ErrInvalidArgument`，避免错误的范围套到另一文件上。相交条件是 `region.start <= file.end` 且 `region.end` 为空或 `region.end >= file.start`；比较使用编码后的字节键。这里按 Go 现有实现保留端点相等时相交的行为，不能擅自替换为一般半开区间公式。

`KVMeta.StartTs` 有列族相关语义：`DefaultCF` 使用 `shiftStartTS`，其它 CF 使用 `startTS`；所有文件使用同一 `restoreTS`。`KVMeta` 还携带路径、CF、range offset/length、删除标记、SHA-256、压缩类型及文件加密信息。请求级别另带 `CipherInfo` 与 `MasterKeys`。

Region 回调次数通过 `Arc<AtomicI64>` 以 `Ordering::Relaxed` 累加；该计数仅用于 `KV_APPLY_BATCH_REGIONS` 观测，不参与正确性决策，所以不要求跨线程同步其它状态。

## 依赖与调用关系

上游接线关系如下：

- [`lib.rs`](./lib.rs) 声明并再导出 `import` 模块。
- [`client.rs`](./client.rs) 的 `LogClient::InitClients -> NewLogFileImporter -> NewLogRestoreManager` 建立所有权；`LogClient::CleanUpKVFiles -> LogFileImporter::ClearFiles` 清理临时文件；`LogRestoreManager::Close -> LogFileImporter::Close` 释放客户端。
- [`import_test.rs`](./import_test.rs) 直接调用构造、清理、批/非批导入和关闭；[`parity_test.rs`](./parity_test.rs) 与 [`export_test.rs`](./export_test.rs) 验证公开契约及测试可见过滤函数。
- 当前代码搜索未找到非测试 Rust 对 `ImportKVFiles` 的调用边；这是迁移接线缺口，不应把 Go 的调用链自动投射成 Rust 现状。

主要下游依赖是：`astersql-br-pkg-restore-utils` 提供 rewrite 查找、编码和范围换算；`import_retry` 提供 `RangeController`、Region 回调与 RPC 错误分类；`stubs::split_client`、`stubs::pd`、`stubs::importclient` 抽象 PD/TiKV/importer 边界；`stubs` 中的 proto 同形类型承载请求数据。`Cargo.toml` 将 restore-utils、restore-split、restore、checkpoint、stream 和 utils 等本地 crate 纳入该库；本文件直接使用的是 restore-utils 加 crate 内部模块/桩。

## 错误处理与边界

- 非批模式传多个文件立即返回 `ErrInvalidArgument`；这发生在范围计算和 RPC 前。
- `GetRewriteEncodedKeys` 的错误由 `?` 直接上抛。空文件列表没有本文件内的显式拒绝；它会以空全局范围进入 RangeController，其最终语义依赖 [`import_retry.rs`](./import_retry.rs)，调用方不应假定空输入必然成功。
- `filterFilesByRegion` 对文件/范围数量不一致返回分类错误；Region 元数据为空时保守返回所有文件，真正 Apply 时再由 `downloadAndApplyKVFileOwned` 返回 `region is nil`。
- leader 缺失返回 `ErrPDLeaderNotFound`，让上层重试策略有机会重新定位；Region 缺失是普通本地错误。
- 单文件找不到 rewrite 规则返回 `ErrKVRewriteRuleNotFound`。Region 适配层会把该错误与 `ErrKVRangeIsEmpty` 一并当成可跳过条件。Rust 当前通过单个 `Error.code` 判断，而 Go 会遍历 `multierr.Errors`；若 Rust 后续引入复合错误，必须补齐多错误展开语义。
- `ApplyKVFile` 的传输失败和响应内 PB 错误走不同转换函数，以便 [`import_retry.rs`](./import_retry.rs) 按错误类型决定重试 Region、重试范围或终止。
- `ClearFiles` 只让获取 store 列表失败中止调用；单店清理失败是尽力而为。`Close` 本身返回错误，但 `LogRestoreManager::Close` 选择只告警。
- 非批请求用 `next().unwrap_or_default()` 取得 rewrite rule；当前入口保证非批最多一个文件，但没有在此私有函数中单独保证至少一个。调用链通过空子文件提前返回来维持该前置条件，未来若新增直接调用必须保留它。

## 并发与资源生命周期

`ImportKVFiles` 为 `RegionFunc` 复制文件、规则、加密材料和 backend，并克隆 `Arc` 客户端，使闭包满足控制器需要的拥有型、可发送生命周期。回调是否并行由 RangeController 实现决定；本文件不创建线程或 Tokio 任务，也不持有互斥锁。

客户端资源从 `LogClient::InitClients` 开始，由 `LogRestoreManager` 持有，到 `LogRestoreManager::Close` 结束。`Close` 只关闭 importer gRPC 客户端；`Arc<dyn SplitClient>` 与其余克隆按引用计数释放。`cacheKey` 在导入器生命周期内稳定，所有 Region 请求共用它；构造器仅以秒级时间生成后缀，并发创建多个实例时理论上可能碰撞，若要强化唯一性需同时核对 TiKV 缓存协议和 Go 兼容性。

清理没有并发 fan-out，而是顺序遍历 Up stores；好处是失败隔离简单，代价是 store 数量增长时清理延迟线性增加。Apply 的重试预算为最多 45 次、100ms 到 15s 的指数退避，长时间阻塞和取消行为由 `Context`、RangeController 及底层客户端共同决定。

## 与 Go 版本的对应关系

直接对照文件是 [`import.go`](./import.go)，回归对照是 [`import_test.go`](./import_test.go)。Rust 保留了 Go 的核心结构：`LogFileImporter` 字段、非批多文件拒绝、rewrite 后全局范围、Region 过滤、RangeController 指标与重试、DefaultCF 时间戳选择、批/单 ApplyRequest、leader 路由、两类可跳过错误、逐 Up-store 清理和关闭客户端。

已确认的差异与迁移限制：

- Go `cacheKey` 使用格式化时间加 `rand.Int63()`；Rust 使用 Unix 秒和确定性乘积，格式与随机性并不完全一致。
- Go `Close` 对 nil importer/client 安全返回；Rust 类型构造后两个客户端不可为空，因此直接调用 `CloseGrpcClient`。
- Go Region 指针或 `Region` 为空时过滤函数返回全部文件；Rust 没有空的 `RegionInfo` 引用，但在 `r.Region == None` 时保持同样回退。
- Go 通过 `multierr.Errors` 检查嵌套错误；Rust 只检查当前 `Error.code`。
- Go 的生产日志恢复链会把文件批处理回调接到 importer；当前 Rust 仓库中 `ImportKVFiles` 只在测试中直接调用，生产接线尚未由搜索证据确认。
- Rust 使用本 crate `stubs` 中的 PD、proto、import client 和指标外观；这说明当前 crate 为精简移植边界，不能把内存桩测试等同于真实 TiKV 集成验证。

Rust 独立测试 [`import_test.rs`](./import_test.rs) 对齐 Go 三组意图：非批多文件报 `ErrInvalidArgument`；多个 Region 边界（含空 `EndKey`）的过滤结果；内存 PD/importer 下 Clear、批/非批 Import、Close 生命周期。测试代码与生产文件分离，符合仓库 Rust 测试组织要求。

## 扩展指南

- 修改 Region 选择或键边界时，优先改 `filterFilesByRegion` 和 `ImportKVFiles` 的范围生成，并同步扩展 [`import_test.rs`](./import_test.rs) 与 Go [`import_test.go`](./import_test.go) 的贴边、空末端、长度不匹配用例。边界相等的既有语义属于兼容风险。
- 新增 `KVMeta` 或 `ApplyRequest` 字段时，接入点是 `downloadAndApplyKVFileOwned`；批量与非批量两个构造分支必须同步，且应覆盖 DefaultCF/WriteCF、delete 文件、加密与 backend 透传。
- 改错误分类时，同时检查 `importKVFileForRegionOwned` 与 [`import_retry.rs`](./import_retry.rs) 的 `RPCResult`/重试策略。尤其不要把不可重试的参数错误误标成 Region 拓扑重试，也不要遗漏 Go 的复合错误语义。
- 若完成 Rust 生产接线，应从 [`client.rs`](./client.rs) 的 `ApplyKVFilesWithBatchMethod`/`ApplyKVFilesWithSingleMethod` 回调边界连接 `LogRestoreManager.fileImporter.ImportKVFiles`，保持 put 先于 delete、批能力探测、TS/rewrite/cipher/master key 传递；同时新增独立测试文件中的生产编排回归，不能只依赖本文件单元烟雾测试。
- 若并行化 `ClearFiles` 或 Region Apply，需证明 `ImporterClient`/`SplitClient` 的线程安全契约、取消传播、指标计数和 cache namespace 不变，并评估 store 数/Region 数放大后的资源占用。
- 若调整 cache key 生成，需同时核对 Go 格式、TiKV `StorageCacheId` 的作用域和并发会话碰撞风险；不要仅为测试方便使用固定全局值。
- 本任务只创建说明文档。未来修改 Rust 源码时，应保持顶部 AsterSQL/PingCAP 版权注释，并按仓库要求将测试继续放在独立 `*_test.rs` 文件中。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/restore/log_client` 确认目标及 Go/Rust 测试均已索引。
- RustCodeGraph `node --file br/pkg/restore/log_client/import.rs --offset 1 --limit 500`：读取目标文件 445 行及其 34 个符号，并报告直接使用文件为 `client.rs`、`export_test.rs`、`import_test.rs`、`parity_test.rs`、`br/pkg/utiltest/crr/harness.rs`。
- RustCodeGraph `query LogFileImporter`、`query ImportKVFiles`、`query filterFilesByRegion`：确认 Rust/Go 同名符号及测试导出位置。精确 `callers/callees` 查询两次在 30 秒窗口内未返回结果，因此未把缺失图输出当成事实依据。
- 已读生产/配置路径：[`import.rs`](./import.rs)、[`client.rs`](./client.rs)、[`lib.rs`](./lib.rs)、[`import_retry.rs`](./import_retry.rs) 的图调用关系、[`Cargo.toml`](./Cargo.toml)，以及仓库范围 `rg` 对非测试 Rust 调用点的核查。
- 已读对照与测试路径：[`import.go`](./import.go)、[`import_test.go`](./import_test.go)、[`import_test.rs`](./import_test.rs)、[`export_test.rs`](./export_test.rs)、[`parity_test.rs`](./parity_test.rs)。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证只检查固定章节、目标文件存在、链接/符号事实和提交范围；真实 PD/TiKV 行为、取消时序及未接线的生产 Apply 主链未在本任务本地执行验证。
