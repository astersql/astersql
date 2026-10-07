# `lightning/pkg/importer/precheck_impl.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` library crate；crate 边界由 [`Cargo.toml`](./Cargo.toml) 的 `[lib] path = "lib.rs"` 定义，`lib.rs` 以 `#[path = "precheck_impl.rs"] mod precheck_impl` 装入并 `pub use precheck_impl::*`。它是 Lightning 导入前检查的具体规则层：相邻的 [`precheck.rs`](./precheck.rs) 负责准备配置、源数据元信息、目标端访问器、checkpoint DB、PD 地址获取器和目标 DB，再由 `PrecheckItemBuilder::BuildPrecheckItem` 按 `CheckItemID` 分发到本文件的构造函数。

本文件不是完整 Go 实现的等价移植，而是源码模块注释所称的 slim port。它公开 14 类 `precheck::Checker` 实现及辅助函数，返回 `Box<dyn precheck::Checker>`，供 builder 统一调度。直接运行时行为应以本文件为准；同路径 [`precheck_impl.go`](./precheck_impl.go) 是语义目标和差异依据，不能把 Go 中尚未移植的检查能力视为 Rust 当前能力。

## 核心职责

- 把共享输入（`Config`、`Arc<dyn PreImportInfoGetter>`、`Vec<MDDatabaseMeta>`、可选 checkpoint DB、可选目标 DB 和 PD 地址闭包）冻结到各检查器实例中。
- 为每个检查器绑定稳定的 `CheckItemID`、严重级别、通过状态和人类可读消息；`ok_result` 统一构造 `Some(CheckResult)`，跳过项直接返回 `Ok(None)`。
- 检查四类风险：目标集群容量/版本/region 状态，源文件与本地磁盘配置，checkpoint/schema/CSV 元数据，以及 CDC/PiTR、目标表和 PD/TiDB 目标端状态。
- 将 importer 内部 `crate::Error` 经 `map_err` 转成 precheck crate 的错误类型，使基础设施错误与“检查已执行但未通过”的 `Passed = false` 保持区分。
- 通过 `dialEtcdWithCfg` 把 Lightning TLS、keyspace、PD 地址、可取消上下文和可注入 PD factory 转换成 `astersql_metaservice::DialEtcdClient` 所需参数。

## 主要符号

| 符号 | 输入/持有状态 | 当前 Rust 行为 |
| --- | --- | --- |
| `map_err` / `ok_result` | importer error；item/severity/pass/message | 统一错误形状与结果构造；所有非跳过 checker 都返回 `Some`。 |
| `clusterResourceCheckItem` / `NewClusterResourceCheckItem` | `preInfoGetter` | 用 `EstimateSourceDataSize().SizeWithIndex × GetMaxReplica()` 计算需求，累加所有 store 的 `Available`，容量不足时以 `Warn` 失败。使用饱和加法/乘法，负估值按 0 处理。 |
| `clusterVersionCheckItem` | getter、`dbMetas` | 调 `CheckVersionRequirements`；业务校验错误被折叠为 `Critical` 且 `Passed=false` 的结果，而非传播。当前 `dbMetas` 未被 `Check` 使用。 |
| `emptyRegionCheckItem` | getter、`dbMetas` | 取所有 store 中最大的 `EmptyRegionCount`，按 `WARN_...` / `ERROR_...` 常量分级；只有超过错误阈值才失败。当前 `dbMetas` 未使用。 |
| `regionDistributionCheckItem` | getter、`dbMetas` | 计算所有 store 的 region 最小/最大比；store 不超过 1 个或最大值不超过 `CHECK_REGION_CNT_RATIO_THRESHOLD` 时跳过但返回通过结果，低于错误比率才失败。当前 `dbMetas` 未参与动态阈值。 |
| `storagePermissionCheckItem` | `Config` 快照 | 仅验证 `Mydumper.SourceDir` 非空，空值为 `Critical` 失败；不实际访问对象存储。 |
| `largeFileCheckItem` | 配置、元数据快照 | `StrictFormat` 时跳过并通过；否则遍历所有 data file，仅对 CSV 的 `FileSize > DEFAULT_CSV_SIZE` 报 `Warn` 失败并列出路径。 |
| `localDiskPlacementCheckItem` | 配置快照 | 仅当源路径非空且与 `SortedKVDir` 字符串完全相等时失败；不解析 URI，也不探测文件系统设备。 |
| `localTempKVDirCheckItem` | 配置、getter、元数据 | 非 local backend 返回 `None`；local backend 只检查 `SortedKVDir` 非空，消息中通过 `hasCompressedFiles` 标记是否有压缩文件。当前 getter 不参与容量估算，严重级别固定为 `Critical`。 |
| `checkpointCheckItem` | 配置、getter、元数据、可选 `checkpoints::DB` | checkpoint 未启用时返回 `None`；启用时只检查 DB 句柄是否存在。getter、元数据及 checkpoint 内容当前均未读取。 |
| `CDCPITRCheckItem` | keyspace、配置、`Send + Sync` PD 地址闭包 | TiDB backend 返回 `None`；其他 backend 建立 namespaced etcd client，调用 `streamhelper::GetCDCPiTRStatus`，显式 `Close`，有活动任务则 `Critical` 失败。`NewCDCPITRCheckItemWithKeyspaceName` 允许 builder 显式传 keyspace。 |
| `schemaCheckItem` | 配置、getter、元数据、可选 checkpoint DB | 调 `GetAllTableStructures`；只要结构非空，或源 `dbMetas` 为空即通过。配置与 checkpoint DB 当前未参与逐表 schema 兼容检查。 |
| `csvHeaderCheckItem` | 配置、getter、元数据 | `CSV.Header=true` 返回 `None`；否则固定返回 `Critical` 通过结果，不读取文件首行。getter、元数据当前未使用。 |
| `checkFieldCompatibility` | 两个字符串 | 仅做 ASCII 大小写不敏感相等比较；目前生产路径无调用者。 |
| `tableEmptyCheckItem` | 配置、getter、元数据、可选 checkpoint DB | TiDB backend 或 `ParallelImport` 时返回 `None`；否则顺序调用 `IsTableEmpty`，只把 `Some(false)` 收集为非空，`Some(true)` 与 `None` 均不报错。checkpoint DB 当前未参与跳过逻辑。 |
| `hasDefault` | `ColumnInfo` | 默认值、auto increment、可空且具名、生成列或 `_tidb_rowid` 任一成立即为真；目前生产路径无调用者。 |
| `pdTiDBFromSameClusterCheckItem` | 可选 `DB`、PD 地址闭包 | 只验证 DB 句柄存在且地址列表非空；不比较实际集群身份。 |
| `dialEtcdWithCfg` | importer context、配置、地址、keyspace | 从三项 cluster TLS 配置生成 `PdSecurity`/`EtcdDialConfig`，以 `ctx.Err()` 提供取消检查，并把 `MetadataRuntime.pd_factory` 交给 metaservice dialer。 |

所有 checker 均实现 `precheck::Checker::{GetCheckItemID, Check}`。构造函数会克隆配置和元数据，因此后续修改原始 `Config` 或切片不会改变已创建 checker 的快照；getter、DB 和闭包则通过 `Arc` 共享。

## 执行流程

1. `NewPrecheckItemBuilderFromConfig`（`precheck.rs`）创建 `PreImportInfoGetter`、checkpoint DB、目标 DB 和 PD 地址回退闭包；`NewPrecheckItemBuilderWithKeyspaceName` 保存这些共享依赖。
2. 调用方把某个 `CheckItemID` 交给 `PrecheckItemBuilder::BuildPrecheckItem`。该 match 覆盖本文件 14 个 checker ID；未知 ID 返回 `unsupported check item`，不会进入本文件。
3. 对应 `New*CheckItem` 克隆值类型输入、克隆 `Arc` 或保存可选句柄，返回 trait object。`NewCDCPITRCheckItem` 还会从配置取 keyspace，并转发到带显式 keyspace 的构造函数。
4. runner 调用 `GetCheckItemID` 与 `Check`。每个 `Check` 先处理适用性分支：例如非 local 的临时目录检查、禁用的 checkpoint、启用 CSV header、TiDB/parallel 模式的空表检查会返回 `None`。
5. 适用的 checker 读取配置快照、遍历元数据，或同步调用 `PreImportInfoGetter`/PD 闭包/etcd client。可判定的业务风险写入 `CheckResult.Passed`；访问 getter、TLS 或 etcd 失败则返回 `Err`，但版本不兼容是特例，会被转换为失败结果。
6. 对 CDC/PiTR，流程为取 PD 地址 → `dialEtcdWithCfg` → 包装成 `etcd::Client` → 读取活动状态 → `Close` → 将活动状态反转为 `Passed`。

## 数据与状态

- `Config` 和 `Vec<MDDatabaseMeta>` 在构造时复制，checker 是独立快照；其中嵌套的数据库、表和文件元数据供大文件、压缩文件、schema 与空表检查遍历。
- `Arc<dyn PreImportInfoGetter>` 是多数 checker 的共享查询边界，提供容量估算、store 信息、最大副本数、版本检查、结构读取和空表探测。checker 不缓存这些查询结果。
- `Option<Arc<dyn checkpoints::DB>>` 表示 checkpoint 存储可能不可用；当前仅 `checkpointCheckItem` 判断 `is_some()`，`schemaCheckItem` 与 `tableEmptyCheckItem` 虽保存该字段但未读取。
- `Arc<dyn Fn(Context) -> Vec<String> + Send + Sync>` 允许跨线程安全共享 PD 地址源，但本文件的 `Check` 本身同步调用它一次。
- region 与容量聚合使用局部变量。容量用 `u64::saturating_add`/`saturating_mul` 防止溢出；region 比率以 `f64` 计算，并在 `max_c == 0` 时显式取 `1.0`。
- 多个结构字段当前仅为 API/Go 形状兼容而保留（例如若干 `dbMetas`、`preInfoGetter`、`cpdb`），不能据字段存在推断相应完整检查已实现。

## 依赖与调用关系

上游主链是 `precheck.rs::PrecheckItemBuilder::BuildPrecheckItem` → 本文件 `New*CheckItem` → `precheck::Checker::Check`。`lib.rs` 同时公开 builder 与实现符号；独立 Rust 测试通过 crate re-export 直接构造 checker。RustCodeGraph 将目标文件标为被 `lightning/pkg/importer/import.rs` 和 `br/pkg/restore/log_client/stubs.rs` 使用；其中明确可见的直接复用是 `import.rs` 调用 crate re-export 的 `dialEtcdWithCfg` 建立 keyspace-aware 元数据客户端。

下游依赖按职责分组：

- `crate::check_info` 提供 region 阈值和默认 CSV 大小；`crate::config` 决定 backend、路径、checkpoint、TLS 与 keyspace。
- `crate::get_pre_info::PreImportInfoGetter` 是集群/源数据查询抽象；`crate::mydump`、`crate::model` 提供源文件和列模型。
- `astersql-lightning-pkg-precheck` 定义 `Checker`、ID、严重级别、结果和错误；`astersql-lightning-pkg-checkpoints` 定义可选 checkpoint DB。
- `crate::etcd` 与 `crate::streamhelper` 完成 CDC/PiTR 状态查询；`astersql-metaservice` 完成带 TLS、keyspace 和可注入 PD factory 的 etcd 建连。
- `Cargo.toml` 直接声明 `astersql-metaservice`、checkpoints、precheck、mydump 等本地 crate；本文件没有 feature gate 或条件编译项。

RustCodeGraph 的精确查询确认 Rust `NewClusterResourceCheckItem` 位于第 67 行、签名返回 `Box<dyn precheck::Checker>`，并与 Go 同名函数同时存在于索引；`dialEtcdWithCfg` 名称在仓库有多个定义，因此导航时必须以 `lightning/pkg/importer/precheck_impl.rs` 文件限定，避免误取 BR 版本。

## 错误处理与边界

- getter/etcd/TLS 错误通常用 `map_err(map_err)?` 或显式映射传播；这类错误表示无法完成检查，不应降格成通过结果。
- `clusterVersionCheckItem::Check` 将 `CheckVersionRequirements` 的错误内容写入消息并返回 `Passed=false`，因此调用方收到的是成功执行的 `CheckResult`，不是 `Err`。
- `None` 的含义是“不适用/跳过”，而不是通过：local temp（非 local）、checkpoint（未启用）、CDC/PiTR（TiDB backend）、CSV header（配置已启用）、table empty（TiDB backend 或 parallel import）使用此约定。region distribution 的“skipped”则仍返回 `Some(Passed=true)`，两种跳过形状并不统一。
- 阈值均采用严格大于/小于：CSV 恰好等于 `DEFAULT_CSV_SIZE` 不算大文件；empty region 恰好等于警告/错误阈值不会进入更严重分支；region 比率恰好等于阈值不会进入对应告警分支。
- 空 store 列表会让容量需求为 0 时通过、empty-region 最大值保持 0 并通过、region distribution 返回 skipped pass。`IsTableEmpty` 返回 `None` 也不会判定非空，这是当前边界而非已验证的远端空表结论。
- 多处 slim 实现只检查形状而非真实环境：非空存储 URI不等于权限可用，路径字符串不同不等于不同磁盘，存在 DB/PD 地址不等于同一集群，存在 checkpoint DB 不等于 checkpoint 内容有效。

## 并发与资源生命周期

本文件没有 `async`、线程创建、channel、锁或并行迭代；数据库/表/文件和 store 都同步、顺序遍历。因此大量表的 `tableEmptyCheckItem` 会逐表串行查询，不能套用 Go 版本的 worker/errgroup 并发假设。

共享对象以 `Arc` 持有，checker 销毁时自动递减引用计数；配置与元数据为自有克隆，不借用 builder 生命周期。PD 地址闭包要求 `Send + Sync`，为在外层并发 runner 中共享保留类型安全，但本文件内部不调度并发。

`CDCPITRCheckItem::Check` 每次适用检查都新建 etcd client，状态查询后立即显式 `Close`。若 `GetCDCPiTRStatus` 返回错误，源码先调用 `Close` 再传播错误；若 dial 失败则不存在待关闭 client。`dialEtcdWithCfg` 的 metaservice context 通过闭包观察 importer `Context::Err()`，资源取消语义由下游 client/dialer实现。

## 与 Go 版本的对应关系

Rust 保留了 Go 文件的 checker 名称、构造入口、`CheckItemID`、大体严重级别和核心跳过方向，独立测试 [`precheck_impl_test.rs`](./precheck_impl_test.rs) 也按 Go suite 的检查项组织。但当前是明确的 slim port，关键差异包括：

- 集群容量未区分 TiKV/TiFlash、未读取 task manager 汇总状态；empty-region/region-distribution 未过滤非 Up/TiFlash store，也未按源表数动态调阈值。
- 存储权限仅判空；本地磁盘仅比较路径字符串；临时 KV 目录不读取磁盘容量、磁盘 quota 或估算数据量，压缩文件也不会把严重级别降为 `Warn`。
- checkpoint 仅判断句柄存在；schema 不做逐表列、默认值、extend/ignore column、首行与 checkpoint 跳过检查；CSV header 不采样源文件。因此 Rust 的 `checkFieldCompatibility` 和 `hasDefault` 只是极简兼容辅助函数，尚未接入 Go 的完整算法。
- 空表检查是串行的，不读取 checkpoint 来跳过已有进度；`Option<bool>::None` 被忽略。Go 使用按 `RegionConcurrency` 限制的 errgroup/channel，并排序失败表名。
- CDC/PiTR Rust 使用 `streamhelper::GetCDCPiTRStatus` 的单一布尔结果，并在 TiDB backend 返回 `None`；Go 分别查询 PiTR tasks 与 CDC changefeeds，非 local 返回显式通过结果且支持 instruction 文案/测试注入 client。
- PD/TiDB 同集群 Rust 只检查两类输入是否存在；Go 调 `util.CheckIfSameCluster` 比较 PD 与 TiDB 暴露的实际地址集合。
- Rust 的 `dbMetas`、`cpdb` 等字段保留了构造形状但部分未消费。扩展时应移植缺失行为，而不是删除这些字段来“简化”。

这些差异由同路径 Go 源码及 Go 测试 [`precheck_impl_test.go`](./precheck_impl_test.go) 直接证明；文档中的“通过”仅描述当前 Rust 判定，不表示完整 Go 风险已被排除。

## 扩展指南

1. 新增检查项时，先在 precheck crate 定义/确认 `CheckItemID` 与 runner 预期，再在本文件新增独立 checker 与构造函数，并在 `PrecheckItemBuilder::BuildPrecheckItem` 增加唯一分支。测试必须放在独立的 [`precheck_impl_test.rs`](./precheck_impl_test.rs)，不要嵌入生产文件。
2. 补齐现有 Go 语义时，以 [`precheck_impl.go`](./precheck_impl.go) 对应 `Check` 为行为基线，逐项迁移动态阈值、过滤规则、checkpoint/schema/CSV 算法和消息语义；不要因当前 slim 测试较窄而删除 Go 分支。
3. 修改结果协议时同时核对三种出口：`Ok(Some(result))`、`Ok(None)`、`Err`。特别注意版本检查当前把验证失败包装为结果，而基础设施错误通常传播。
4. 补齐并发空表检查时，需要保持有界并发、取消传播、checkpoint 跳过、稳定排序和错误注解；共享收集器需明确同步策略，不能直接把当前顺序循环无界并发化。
5. 补齐 CDC/PiTR 或 metaservice 时，保持 keyspace、TLS 三元组、`MetadataRuntime.pd_factory` 注入、取消检查和 client 关闭；新增错误路径必须验证资源也会释放。
6. 更改阈值、backend 适用性或配置路径比较时，同步更新 Rust 独立测试，并对照 Go `TestPrecheckImplSuite` 中同名场景；兼容风险主要是以前被跳过/通过的任务变为阻塞，性能风险主要来自远端逐表请求、源文件采样和集群元数据查询。
7. 若开始使用目前未读字段（`dbMetas`、`cpdb`、getter），应优先复用现有结构而非改公开构造签名；这能减少 builder 和外部调用面的迁移破坏。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、目标文件已索引为 Rust 文件并识别 74 个符号；`files --filter lightning/pkg/importer/precheck_impl.rs` 命中唯一目标。
- RustCodeGraph `node --file lightning/pkg/importer/precheck_impl.rs --offset 1 --limit 500` 与 `--offset 500 --limit 500`：完整读取 898 行生产源码，并确认索引报告的直接使用文件；`query NewClusterResourceCheckItem --kind function --json`、`query NewCDCPITRCheckItem --kind function --json`、`query dialEtcdWithCfg --kind function --json` 用于消除 Rust/Go及仓库同名符号歧义。`callers/callees` 精确查询曾执行，但本地 CLI 在 30 秒窗口内未返回，因此调用关系以已索引文件节点和调用点源码复核。
- 调用链与 crate 边界：[`precheck.rs`](./precheck.rs) 的 `PrecheckItemBuilder::BuildPrecheckItem`、[`lib.rs`](./lib.rs) 的模块声明/再导出、[`Cargo.toml`](./Cargo.toml) 的 library 与依赖声明，以及 [`import.rs`](./import.rs) 对 `dialEtcdWithCfg` 的直接调用。
- 当前 Rust 边界测试：[`precheck_impl_test.rs`](./precheck_impl_test.rs) 覆盖容量、版本、empty region、region 分布、存储路径、大文件、本地目录、checkpoint、schema、CSV header、空表、CDC/PiTR 和 PD/TiDB 输入形状；测试明确标注多项 slim 限制。
- Go 对照：[`precheck_impl.go`](./precheck_impl.go) 的同名构造函数/`Check`/辅助函数，以及 [`precheck_impl_test.go`](./precheck_impl_test.go) 的 `TestPrecheckImplSuite` 和 14 组同名场景。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另以固定标题命令验证本文恰有 11 个规定章节，并人工检查所有“已支持”陈述均限定为目标 Rust 当前源码事实。
