# `br/pkg/task/operator/checksum_table.rs`

## 文件定位

本文件属于 `astersql-br-pkg-task-operator` library crate，是 BR `operator` 子命令的表级 checksum 实现，对照 Go 文件 [`checksum_table.go`](checksum_table.go)。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod checksum_table` 声明模块，并通过 `pub use checksum_table::*` 平铺导出；上层 [`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 分别在 checksum、PiTR checksum 和 upstream checksum 命令处理中调用 `RunChecksumTable`、`RunPitrChecksumTable`、`RunUpstreamChecksumTable`。

[`Cargo.toml`](Cargo.toml) 将该目录定义为独立 library crate，直接依赖 `serde`/`serde_json`，而连接、元数据、checksum executor、GC manager 等数据库能力由同 crate 的 [`stubs.rs`](stubs.rs) 抽象提供。该 crate 的元数据明确对应 Go package `br/pkg/task/operator`；当前实现不是生成代码或仅转发门面，但依赖的是本地 trait/stub 边界，不能据此推断已经具备真实 TiKV/Domain 的全部运行能力。

## 核心职责

文件把三种输入方式收敛为同一条“发现当前表 → 构造每表 checksum executor → 在 GC service safepoint 保护下并发执行 → 输出 JSON”的流水线：

- `RunChecksumTable` 从备份存储的 `backupmeta`/schema 中取得旧表 ID，用旧表与当前表的同名映射生成 rewrite checksum 请求。
- `RunUpstreamChecksumTable` 不读取备份元数据，直接用调用方提供的 `RestoreTS` 校验当前集群表。
- `RunPitrChecksumTable` 从外部文件或 `mysql.tidb_pitr_id_map` 加载上下游 ID 映射，伪造带上游表/分区 ID 的旧表元数据后生成 rewrite 请求；`ChecksumTS == 0` 时向 PD 获取当前 TSO。

三个入口都使用配置中的 `TableFilter` 限定库表，用 `ChecksumConcurrency` 控制单表内部 executor 并发，用 `TableConcurrency` 控制同时运行的表数。成功结果逐表写入 stderr 摘要，并把 `Vec<ChecksumResult>` 序列化为 stdout JSON，便于脚本消费。

## 主要符号

- `checksumTableCtx`：一次执行的共享上下文，持有 `ChecksumWithRewriteRulesConfig`、初始化后的 `ConnMgr`/`Domain` 以及最终 `checksumTS`。`mgr` 和 `dom` 在 `init` 成功前为 `None`，后续私有方法直接 `unwrap`，因此公开入口规定了必须先初始化的调用顺序。
- `tableInDB`：当前 infoschema 表 `TableInfo` 与其小写库名的组合。它只在本文件内部流转。
- `RunChecksumTable`、`RunUpstreamChecksumTable`、`RunPitrChecksumTable`：三类公开业务入口；RustCodeGraph 显示它们都调用 `init`、`getTables` 和 `runChecksum`，并分别调用 `genRequests`、`genUpstreamRequests`、`genRequestsWithIDMap`。
- `init`：通过 `DIAL_HOOKS.new_mgr` 测试钩子或 `NewMgr` 创建连接管理器，再由 `Glue::GetDomain` 取得 Domain。
- `getTables`：遍历 `InfoSchema::AllSchemas`，依次应用 `MatchSchema`、`MatchTable`，调用 `SchemaTableInfos` 收集当前表。
- `loadOldTableIDs`：从配置的存储读取 `MetaFile`，反序列化 `BackupMeta`，经 `MetaReader::ReadSchemasFiles` 加载并过滤旧表。
- `loadPitrIdMap`：优先读取外部 `PitrIDMapsFilename(clusterID, restoredTS)`；存储 URI 为空时查询系统表，并验证分片连续且非空。
- `genRequests`：取得 PD TSO，以 `HashMap<db, HashMap<table, MetaTable>>` 匹配旧表，调用 `ExecutorBuilder::SetOldTable` 开启 ID rewrite。
- `genRequestsWithIDMap`：构造 `router[db][table][downstream_id] = upstream_id`，克隆当前 `TableInfo` 并替换表 ID、全部分区 ID，再作为 fake old table 交给 builder。
- `genUpstreamRequests`：不设置 old table，仅以指定时间戳为当前表构建 executor。
- `runChecksum`：安装 service safepoint、限制表级并发、启动线程、汇总结果/首个错误，并尝试删除 safepoint。
- `request`：绑定一个已构建的 `ChecksumExecutor` 及日志/结果所需库表名。
- `ChecksumResult`：公开结果 DTO；serde 字段固定为 `db_name`、`table_name`、`checksum`、`total_bytes`、`total_kvs`。
- `ScopeGuard`：线程退出时在 `Drop` 中归还一个表并发槽。
- `gen_requests_with_id_map_for_test`、`test_table`：公开测试辅助接口，仅绕过真实集群初始化来观察 rewrite 后的 table ID 和构造带可选分区的 `TableInfo`，不是生产命令入口。

## 执行流程

1. 上层 CLI 解析对应配置后调用三个 `Run*ChecksumTable` 入口之一。入口创建 `checksumTableCtx`，执行 `init` 建立 `ConnMgr` 与 Domain，再由 `getTables` 从当前 infoschema 中筛选目标表。
2. rewrite-rules 路径执行 `loadOldTableIDs`，读取备份 `MetaFile` 和 schema 文件；`genRequests` 向 PD 取 TSO，将备份表按小写库名/表名建双层索引。备份中缺库或缺表时记录日志并跳过当前表，而非终止整批。
3. upstream 路径把 `RestoreConfig.RestoreTS` 直接交给 `genUpstreamRequests`。这里没有 old table，executor 按当前表 ID 读取并聚合。
4. PiTR 路径先执行 `loadPitrIdMap`。外部存储模式直接反序列化 `BackupMeta.DbMaps`；系统表模式根据是否存在 `restore_id` 列选择新旧 SQL，把按 `restore_id, segment_id` 排序的字节分片拼接成完整 `BackupMeta`。随后 `genRequestsWithIDMap` 同时 rewrite 表 ID 和各分区物理 ID。未指定 `ChecksumTS` 时由 `GetTSWithRetry` 补当前 PD TSO。
5. 各生成函数都设置 executor 的 `ChecksumConcurrency` 和 request source type `"br"`，并把选定时间戳写回 `checksumTableCtx.checksumTS`。
6. `runChecksum` 用该时间戳创建 `BRServiceSafePoint` 并调用 `StartServiceSafePointKeeper`。之后按 `TableConcurrency.max(1)` 建立一个由 `Mutex<usize>` 表示的槽位计数；主线程每毫秒轮询空槽，为每张表启动一个 OS 线程。
7. worker 调用 `ChecksumExecutor::Execute`；回调用局部 `AtomicI64` 记录该表已完成的 cop 子请求数。成功响应被转换成 `ChecksumResult` 并追加到共享 `Mutex<Vec<_>>`。主线程 join 所有 worker，保留观察到的第一个执行错误或 panic 错误。
8. 所有线程结束后将 safepoint `TTL` 置零并调用 `DeleteServiceSafePoint`。删除失败只告警；若 worker 有错则返回首错，否则返回结果。入口打印 stderr 摘要和 stdout JSON。

## 数据与状态

`checksumTS` 是本文件最重要的运行状态：rewrite-rules 使用新取的 PD TSO，upstream 使用 `RestoreTS`，PiTR 使用显式 `ChecksumTS` 或重试取得的当前 TSO；同一值也作为 safepoint 的 `BackupTS`，保证执行期间目标版本未被 GC 越过。

表名存在两种语义。常规筛选和结果展示主要使用 `CIStr.L` 的小写名；PiTR map 按 Go 行为用数据库映射名和 `TableInfo.Name.O` 原始表名索引。因此新增映射逻辑时不能把 `O`/`L` 随意互换。PiTR router 同时保存逻辑表与分区的 downstream→upstream ID；fake old table 从当前表深克隆后原位替换这些 ID，当前 infoschema 对象不被修改。

并发共享状态包括结果向量 `Arc<Mutex<Vec<ChecksumResult>>>` 和槽位 `Arc<Mutex<usize>>`。每个 worker 的进度计数器是独立 `AtomicI64`，只用于日志，不参与全局调度。结果按线程完成顺序追加，输出顺序不保证与请求或 infoschema 顺序一致。配置、存储句柄和请求被克隆或移动进线程，worker 完成并 join 后释放。

系统表 ID map 使用 `lastRestoreID`、`nextSegmentID` 和 `metaData` 维护分组解析状态。每次 restore ID 切换先反序列化上一组；每组 segment 必须从 0 连续递增且数据非空。无任何行时返回空 map，后续 request 生成会对首张当前表报“no db map found”。

## 依赖与调用关系

上游生产调用来自 [`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs)：该文件导入 crate 根平铺导出的三个入口，并在约第 236、264、291 行把 CLI 配置和 `Glue` 适配器传入。Go 对照调用位于 [`br/cmd/br/operator.go`](../../../cmd/br/operator.go) 的三个同类命令分支。

下游依赖集中在 [`stubs.rs`](stubs.rs)：

- 连接与元数据：`NewMgr`/`ConnMgr`、`Glue`、`Domain`、infoschema、`TableInfo`。
- 存储与备份元数据：`GetStorage`、`BackupMeta`、`MetaReader`、`MetaTable`、`PitrDBMap`。
- 请求构建与执行：`ExecutorBuilder`、`ChecksumExecutor`，最终使用 `mgr.GetStorage().GetClient()`。
- 时间戳与 GC：`ComposeTS`、`GetTSWithRetry`、`BRServiceSafePoint`、`StartServiceSafePointKeeper`、GC manager。

RustCodeGraph 的文件级查询识别出 20 个符号，并确认内部关键调用边：三个公开入口均流向 `runChecksum`；`genRequestsWithIDMap` 由 PiTR 入口及测试 helper 调用；测试 helper 和 `test_table` 的调用者是 [`parity_test.rs`](parity_test.rs) 中的 `contract_normal_config_and_helpers`、`contract_error_paths`。Cargo 直接依赖中的 `serde_json` 承担最终输出序列化；其余数据库依赖经本 crate 的 stub/trait 层间接接入。

## 错误处理与边界

源文件使用统一 `Result<T, Error>`，在存储创建、文件读取、反序列化、infoschema 查询、session SQL、TSO 获取和 executor build 处用 `Annotate`/`Annotatef` 补上下文。明确的硬错误包括 PiTR 缺数据库/表/表 ID/分区 ID 映射，系统表分片丢失或为空，以及 worker panic。rewrite-rules 中备份缺同名库表则是有意的软跳过。

`runChecksum` 会等待所有已启动线程，即使先有 worker 失败也不会提前返回；它仅保留 join 顺序遇到的第一个错误。safepoint 删除失败只写 stderr，不覆盖 checksum 业务错误。`TableConcurrency` 为 0 时被提升为 1，避免永远拿不到槽；但槽位实现是 1 ms sleep 的忙等，不是条件变量或异步 semaphore。

与 Go 版本相比，Rust 公开 API 没有 `context.Context`，worker 之间也没有 `errgroup.WithContext` 的取消传播：一张表失败不会取消其他表，调用方取消亦无通道传入。因此文档不能把 Rust 当前实现描述为具备 Go 的取消语义。另有两个清理边界：Rust 在 `StartServiceSafePointKeeper` 成功后才进入最终删除代码，启动失败时不会像 Go 的预注册 `defer` 那样尝试清理；系统表 SQL 执行失败时 `?` 会在显式 `se.Close()` 前返回，而 Go 用 `defer se.Close()`。这些是当前可见的迁移差异，扩展错误路径时应优先补独立回归测试。

内部方法依赖入口维持初始化不变量，直接对 `mgr`/`dom` 使用 `unwrap`；测试 helper 只能调用不需要连接的 request 生成逻辑。锁中毒也通过 `unwrap` 表现为 panic，而非业务 `Error`。

## 并发与资源生命周期

连接生命周期从 `init` 开始：先取得 `ConnMgr`，再取得 Domain，二者以 `Arc` 保存在上下文并覆盖整个命令执行。文件中没有显式关闭 `ConnMgr`/Domain 的代码，其释放行为取决于 stub 实现和最后一个 `Arc` 被丢弃。系统表路径创建的 session 仅在 SQL 成功后显式 `Close`；如上所述，错误路径目前缺少 RAII guard。

GC 生命周期是“安装 keeper → 执行并 join 全部 worker → TTL=0 删除 service safepoint”。正常返回和 worker 错误都会走删除步骤；删除错误仅告警。由于本实现没有一个覆盖整个 `runChecksum` 的 Drop/defer guard，主线程 panic、keeper 启动失败或锁中毒时不保证执行删除。

每个 `request` 对应一个 OS 线程，但启动受到表级槽位限制。`ScopeGuard` 在线程闭包正常返回、executor 返回错误或线程展开 panic 时归还槽位；若 panic 导致 mutex poisoning，后续 `lock().unwrap()` 仍可能令调度线程崩溃。所有 handle 最终被 join，避免正常错误路径遗留游离线程。单表内部并发由 `ChecksumExecutor` 自己根据 `ChecksumConcurrency` 管理，本文件仅观察其 `Len` 和完成回调。

## 与 Go 版本的对应关系

Rust 三个入口、上下文、请求/结果结构以及 `getTables`、两种 ID map 来源、三类 request 生成和 GC 保护流程，均能在 [`checksum_table.go`](checksum_table.go) 找到一一对应的 Go 符号。JSON tag、备份缺表跳过、PiTR 新旧系统表 schema、segment 连续性检查、表/分区 ID rewrite、request source `BR` 和 checksumTS 选择规则保持一致。

实现机制并非完全相同：

- Go `loadOldTableIDs` 用 goroutine、table/error channel 和 `ctx.Done()` 流式读取；Rust stub API 一次返回 `Vec<MetaTable>`，没有取消分支。
- Go 用 `util.WorkerPool` 与 `errgroup.WithContext`；Rust用 `Mutex<usize>`、sleep 和 `std::thread::spawn`，不传播取消，结果同样不承诺稳定排序。
- Go 的 safepoint 清理通过 `defer` 注册并先 cancel keeper；Rust stub 的 `StartServiceSafePointKeeper` 没有返回 cancel handle，结束时仅以 TTL=0 删除。
- Go session 用 `defer Close`；Rust仅在查询成功后显式关闭。
- Rust额外公开 `gen_requests_with_id_map_for_test` 与 `test_table`，使测试逻辑保留在独立 [`parity_test.rs`](parity_test.rs)，符合生产源与测试分文件的仓库约束。

因此，“业务路由与数据变换基本对齐”有源码证据，但取消、清理和真实后端接线仍受当前 stub 边界限制，不能仅凭 parity helper 判定端到端等价。

## 扩展指南

新增一种 checksum 输入来源时，优先复用 `init`、`getTables`、`runChecksum`，把差异限制在新的配置解析与 request 生成函数；必须明确它选择哪个时间戳、是否设置 old table、如何处理缺映射，以及 `TableFilter` 的名称大小写语义。若修改 PiTR 映射，需同步处理逻辑表和全部分区 ID，并保持按原始表名 `Name.O` 查 map、按小写名 `Name.L` 过滤/展示的约定。

改变并发或错误策略时，修改中心是 `runChecksum` 与 `ScopeGuard`。安全改进应优先引入可取消的执行上下文、阻塞式 semaphore/线程池，以及覆盖整个 safepoint 生命周期的 RAII guard；同时确认首错规则、等待所有 worker 和删除错误不覆盖业务错误这些现有契约是否保持。涉及 session 时也应使用析构守卫，覆盖查询失败路径。

测试应继续放在独立 [`parity_test.rs`](parity_test.rs) 或新建独立 `*_test.rs`，不要内嵌到生产文件。当前测试已覆盖结果 JSON tag、正常表/分区 ID map 以及缺 DB map；扩展时至少补缺 table/table ID/partition ID、segment 缺失/空数据、新旧 `restore_id` schema、备份缺表跳过、`TableConcurrency == 0`、worker 首错与 panic、safepoint 启停/删除失败、session 查询失败清理和结果顺序不应作为契约等用例。若从 stub 升级到真实依赖，还需在独立上游仓库完成依赖移植并使用已发布 tag，不能在本仓库增加本地 patch/vendor 覆盖。

兼容风险主要在 JSON 字段、错误文案关键片段、CIStr 大小写选择、PiTR segment 规则和 ID rewrite；正确性风险集中在 checksumTS/GC 保护与资源清理；性能风险集中在每表 OS 线程、忙等槽位、结果 mutex 和 executor 的双层并发配置。

## 验证依据

- Rust 源码：[`checksum_table.rs`](checksum_table.rs) 全部 730 行；关键符号包括三类公开入口、`loadPitrIdMap`、三种 request 生成函数、`runChecksum`、`ChecksumResult` 和两个测试 helper。
- crate 边界：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)；确认 package 名、Go package 元数据、依赖、模块声明、平铺导出和独立测试模块。
- 生产入口：[`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 第 236、264、291 行附近；对应 Go 入口为 [`br/cmd/br/operator.go`](../../../cmd/br/operator.go) 第 145、167、188 行附近。
- Go 语义：[`checksum_table.go`](checksum_table.go)；逐段核对表发现、备份/PiTR 元数据加载、ID rewrite、executor 构建、worker pool、GC safepoint、JSON 输出和清理方式。
- 独立 Rust 测试：[`parity_test.rs`](parity_test.rs) 的 `checksum_result_json_matches_go_tags`、`service_safe_point_keeper_rejects_invalid_config_before_write`、`contract_normal_config_and_helpers`、`contract_error_paths`。前者验证 JSON tag，正常 helper 用例验证 downstream table ID 100 rewrite 为 upstream ID 200，错误用例验证空 map 返回 `no db map found`；当前没有三类公开入口的真实集群端到端测试。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/task/operator/checksum_table.rs` 确认目标文件和 20 个符号；文件 `node` 读取 1–730 行；`explore` 确认三个入口到 `runChecksum` 的调用流，以及 `genRequestsWithIDMap`/测试 helper 的调用者关系。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的固定 11 章节结构校验、Markdown 路径/事实复核和 diff 检查；运行结果记录在提交交接中。
