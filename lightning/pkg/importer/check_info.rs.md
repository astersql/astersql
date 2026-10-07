# `lightning/pkg/importer/check_info.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` crate。crate 入口 `lightning/pkg/importer/lib.rs` 以 `mod check_info` 装载它并通过 `pub use check_info::*` 再导出其公开常量和 `Controller` 方法；Cargo 边界见 `lightning/pkg/importer/Cargo.toml`，其中本地依赖 `astersql-lightning-pkg-precheck` 提供统一的检查 ID、`Checker` trait 和 `CheckResult`。

它位于 Lightning 正式导入前的预检查编排层：上游 `Controller::preCheckRequirements` 和 `Controller::DataCheck`（`lightning/pkg/importer/import.rs`）按阶段调用这里的包装方法；本文件再把语义化方法映射为 `precheck::CheckItemID`，交给 `PrecheckItemBuilder::BuildPrecheckItem`（`lightning/pkg/importer/precheck.rs`）构造具体 checker。真正的容量、Region、文件、checkpoint、schema 等规则主要位于 `lightning/pkg/importer/precheck_impl.rs`，因此本文件是“调度与跳过策略”而不是规则实现本体。

## 核心职责

1. 集中声明与 Go `check_info.go` 对齐的 CSV、采样和 Region 判定阈值，并供 `precheck_impl.rs` 等模块复用。
2. 通过 `toPrecheckContext` 跨 crate 转换上下文，保留字符串值映射和取消状态。
3. 通过 `Controller::doPreCheckOnItem` 统一执行“准备 builder → 构造 checker → 执行 → 收集非空结果”的公共骨架。
4. 为每类检查提供表达业务意图的 `Controller` 方法，并在调用 checker 前实施 backend、并行导入、checkpoint 开关、源存储类型和任务状态等短路策略。
5. 为 Region 组合检查规定顺序：先空 Region，再 Region 分布；若已有任务越过初始状态则全部跳过。

本文件不决定某个失败是 `critical` 还是 `performance`，也不计算具体检查结果；这些由 `precheck_impl.rs` 中实现 `precheck::Checker` 的具体类型返回，随后由 `checkTemplate.Collect` 汇总。

## 主要符号

- `DEFAULT_CSV_SIZE = 10 GiB`：大 CSV 判定阈值；当前由 `largeFileCheckItem::Check` 使用。
- `MAX_SAMPLE_DATA_SIZE = 10 MiB`、`MAX_SAMPLE_ROW_COUNT = 10 * 1024`：与 Go 采样上限保持数值对齐。仓库搜索表明 Rust 当前除导出/一致性测试外尚未消费这两个常量；Go 版在 `get_pre_info.go` 的采样流程使用它们，扩展时不能把“已声明”误当成“Rust 采样已接线”。
- `WARN_EMPTY_REGION_CNT_PER_STORE = 500`、`ERROR_EMPTY_REGION_CNT_PER_STORE = 1000`：空 Region 的告警/失败基准。
- `WARN_REGION_CNT_MIN_MAX_RATIO = 0.75`、`ERROR_REGION_CNT_MIN_MAX_RATIO = 0.5`、`CHECK_REGION_CNT_RATIO_THRESHOLD = 1000`：Region 分布检查的比率和启用门槛；具体比较发生在 `precheck_impl.rs`。
- `toPrecheckContext(ctx) -> precheck::context::Context`（crate 内可见）：移动 `str_values` 和 `cancelled` 字段，连接 importer 上下文与 precheck crate 上下文。
- `Controller::isSourceInLocal(&self) -> bool`：以 `store.URI().starts_with(objstore::LocalURIPrefix)` 判定源存储是否为本地 URI。
- `Controller::doPreCheckOnItem(&mut self, ctx, checkItemID) -> Result<()>`：核心分发入口。它要求 `precheckItemBuilder` 存在，将 `Controller.keyspaceName` 同步到 builder，构造并运行 checker；只有 `Check` 返回 `Some(CheckResult)` 时才写入 `checkTemplate`。
- `clusterResource`：可把 `taskMgr` 以 `taskManagerKey` 注入检查上下文，然后分发 `CheckTargetClusterSize`。
- `ClusterIsAvailable`、`StoragePermission`、`HasLargeCSV`、`checkCSVHeader`：分别直接分发目标集群版本、源存储权限、大文件和 CSV 表头检查。
- `checkEmptyRegion`、`checkRegionDistribution`：分别分发两个 Region 子检查；`checkClusterRegion` 负责状态门控和顺序组合。
- `localResource`：本地源额外执行 `CheckLocalDiskPlacement`，所有源类型都继续执行 `CheckLocalTempKVDir`；后者自身可根据 backend 返回 `None`。
- `checkTableEmpty`：TiDB backend 或 `ParallelImport` 时跳过，否则分发 `CheckTargetTableEmpty`。
- `checkCheckpoints`：checkpoint 未启用时跳过，否则分发 `CheckCheckpoints`。
- `checkSourceSchema`、`checkCDCPiTR`、`checkPDTiDBFromSameCluster`：TiDB backend 时跳过；其他 backend 分别检查源 schema、CDC/PiTR 冲突、PD 与 TiDB 集群一致性。

文件没有自定义 struct、enum、trait、宏或条件编译项；所有业务入口均是既有 `Controller` 的固有方法。

## 执行流程

应用主链分成两组：

1. `Controller::preCheckRequirements` 先调用 `DataCheck`；若 `cfg.App.CheckRequirements`，检查集群版本，并在 `ownStore` 时检查源存储权限。初始化 meta manager 后，仅 local backend 且启用 requirements 时依次执行本地资源、集群容量、Region、CDC/PiTR 和 PD/TiDB 一致性检查。该上游目前对 `checkClusterRegion(...).ok()` 丢弃运行错误，但 checker 若成功返回失败结果，仍会进入模板；最后 `checkTemplate.Success()` 决定是否把聚合失败信息返回。
2. `Controller::DataCheck` 在启用 requirements 时检查大 CSV；无条件经过 checkpoint 门控；启用 requirements 时检查源 schema；最后执行目标表空检查和 CSV header 检查。

单项的公共流程由 `doPreCheckOnItem` 实现：

1. 取得可变 builder；缺失时返回 `precheckItemBuilder is nil`。
2. 把控制器当前 `keyspaceName` 写入 builder，使 CDC/PiTR 等 checker 获取最新 keyspace。
3. 再以共享引用取得 builder，按检查 ID 调用 `BuildPrecheckItem`；未知 ID 或构造失败直接向上返回。
4. 将 importer `Context` 转为 precheck `Context` 并调用 `Checker::Check`。
5. checker 错误被转换为 importer `errors::New(e.to_string())`；`Ok(None)` 表示跳过且不产生模板行，`Ok(Some(result))` 则收集 `Severity`、`Passed`、`Message`。

`checkClusterRegion` 是唯一的组合流程：要求 `taskMgr` 存在，在 `CheckTasksExclusively` 回调中扫描任务；任一 `task.status > taskMetaStatusInitial` 就视为恢复已开始并返回成功。否则先运行空 Region 检查，成功后再运行分布检查，前一步失败会阻止后一步。

## 数据与状态

本文件自身没有全局可变状态。阈值均为编译期常量；每次调用的可观察副作用落在 `Controller`：

- `precheckItemBuilder.keyspaceName` 在每次分发前被覆盖为 `Controller.keyspaceName`。
- `checkTemplate` 在 checker 返回 `Some(CheckResult)` 时追加一条结果；一个“执行成功但 `Passed = false`”的检查仍由本方法返回 `Ok(())`，失败状态留给模板聚合阶段处理。
- `clusterResource` 构造派生上下文并注入克隆后的任务管理器句柄，不修改传入上下文。
- `checkClusterRegion` 读取任务元数据状态来决定是否继续，不在本文件修改任务记录。

`Context` 转换只保留当前两个结构共有且被显式搬运的 `str_values` 与 `cancelled`。`check_info_test.rs::test_precheck_context_preserves_cancellation_and_values` 验证取消位和字符串键值在 crate 边界后仍可观察。

## 依赖与调用关系

上游真实调用边由源码搜索确认：

- `import.rs::preCheckRequirements` → `ClusterIsAvailable`、`StoragePermission`、`localResource`、`clusterResource`、`checkClusterRegion`、`checkCDCPiTR`、`checkPDTiDBFromSameCluster`。
- `import.rs::DataCheck` → `HasLargeCSV`、`checkCheckpoints`、`checkSourceSchema`、`checkTableEmpty`、`checkCSVHeader`。
- 独立 Rust 测试还会直接调用这些方法，以锁定跳过条件和结果汇总行为。

下游关系为：包装方法 → `doPreCheckOnItem` → `PrecheckItemBuilder::BuildPrecheckItem` → `Box<dyn precheck::Checker>::Check` → `checkTemplate.Collect`。builder 的 match 覆盖本文件使用的全部检查 ID，并向具体构造器传递 `cfg`、源元数据、pre-info getter、checkpoint DB、PD 地址 getter或目标 DB。

主要 crate/模块依赖：

- `crate::context`：调用上下文；`crate::errors`：importer 的统一错误类型与 `Trace`。
- `crate::import::Controller`：被扩展的核心控制器。
- `crate::meta_manager::taskMetaStatusInitial`：恢复是否开始的状态边界。
- `crate::objstore`：本地 URI 前缀。
- `crate::precheck`：任务管理器上下文键及 `WithPrecheckKey`。
- `astersql_lightning_pkg_precheck`：检查 ID、checker 接口、检查结果和独立上下文类型；由 `Cargo.toml` 以本地 path 依赖引入。

RustCodeGraph 显示目标文件被 `lightning/pkg/importer/precheck_impl.rs` 使用（阈值导入），但针对目标 Rust 方法执行 `callers`/`callees` 查询返回空数组；因此以上调用边以精确源码搜索和已索引文件内容补证，而不是把空图结果解释为无调用者。

## 错误处理与边界

- builder 或 task manager 缺失分别是显式错误：`precheckItemBuilder is nil`、`taskMgr is nil`，避免空指针式失败。
- builder 不支持某个检查 ID 时返回 `unsupported check item: ...`；本文件不吞掉该错误。
- checker 的独立错误会丢失原错误类型，仅以字符串重新包装；需要依赖错误类别的扩展应评估这一兼容风险。
- `CheckResult.Passed == false` 不是 `doPreCheckOnItem` 的 Rust `Err`；调用者必须依靠 `checkTemplate.Success()` / `FailedMsg()` 作最终门控。
- `Ok(None)` 是 checker 约定的“跳过”，区别于通过结果。包装层的提前 `Ok(())` 同样不写模板，因此新增调用方不能用“存在输出行”判断方法是否被调用。
- `isSourceInLocal` 是 URI 前缀判断，不做路径规范化或文件存在性检查。
- TiDB backend 会跳过目标表空、源 schema、CDC/PiTR、PD/TiDB 集群一致性检查；并行导入只额外跳过目标表空检查；关闭 checkpoint 只跳过 checkpoint 检查。
- Region 状态判断使用严格大于 `taskMetaStatusInitial`；恰好处于初始状态仍会运行两项 Region 检查。
- 空 Region 与 Region 分布阈值的当前 Rust 算法比 Go 具体规则更精简。例如 Go 会结合表数量动态提高阈值，Rust `precheck_impl.rs` 当前直接使用常量；文档不能据 Go 的完整行为宣称 Rust 已实现动态阈值。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、文件或网络连接。checker 为调用内创建的 `Box<dyn Checker>`，执行完即释放；上下文与必要句柄通过所有权移动、`clone` 或 `Arc` 传递。

`checkClusterRegion` 使用 task manager 的 `CheckTasksExclusively` 获取任务集合的一致视图，但 Rust 实现只在排他回调中扫描状态，随后释放排他区间，再调用两个 checker。Go `check_info.go` 则在 `CheckTasksExclusively` 回调内执行 Region 检查。Rust 源码注释说明这是为了在可变借用 `self` 的约束下把检查移到闭包外；因此当前 Rust 不能声称在“状态扫描 + 两项检查”全过程持有同一排他保护，若并发任务状态转换对此敏感，应优先补设计与回归测试。

`clusterResource` 把 `taskMgr.clone()` 包在新的 `Arc` 中放入派生上下文。其生命周期至少覆盖本次 checker 调用；本文件不负责关闭或回收 task manager。连续检查通过 `ctx.clone()` 复用逻辑上下文，取消状态会经 `toPrecheckContext` 传给 checker，但具体 checker 是否及时检查取消由其实现决定。

## 与 Go 版本的对应关系

Rust API 基本逐项对应 `lightning/pkg/importer/check_info.go`：常量值、方法名称、检查 ID 映射、local source 分支、TiDB/parallel/checkpoint 短路，以及“checker 返回 nil/None 时不收集结果”的外部语义一致。`check_info_test.rs`、`table_import_test.rs` 和 `parity_test.rs` 明确以 Go 测试/常量为对照。

已确认的差异或迁移边界：

- Rust `doPreCheckOnItem` 会先检查 builder 是否为 `Some`，并在每次调用前同步 `keyspaceName`；Go 方法直接使用已存在的 builder。
- Rust 在 importer/precheck 两个 crate 之间显式转换上下文；Go 使用同一个 `context.Context`。
- Go 的 `checkClusterRegion` 在排他闭包内检查，Rust仅在闭包内扫描任务、在闭包外检查，排他范围不同。
- Rust checker 错误通过文本重建 importer 错误；Go 用 `errors.Trace` 保留错误链。
- `MAX_SAMPLE_DATA_SIZE` 和 `MAX_SAMPLE_ROW_COUNT` 在 Go `get_pre_info.go` 被使用，Rust 当前只有声明和部分常量测试证据，尚无采样流程消费证据。
- 相关 Rust 测试注释明确称若干 checker 为 “slim” 实现，例如 CSV header 当前始终产生通过结果、表空检查不按 checkpoint 过滤；这些是具体 checker 的迁移状态，不应由本调度文件掩饰为完整 Go 等价。

## 扩展指南

新增预检查项时，最小安全接线通常包括：

1. 在 `lightning/pkg/precheck/precheck.rs` 定义稳定的 `CheckItemID` 和展示名，并实现/扩展相应 checker 契约。
2. 在 `lightning/pkg/importer/precheck_impl.rs` 实现规则，在 `PrecheckItemBuilder::BuildPrecheckItem` 增加构造分支和所需依赖。
3. 在本文件增加语义化 `Controller` 方法，并明确 backend、配置开关、恢复状态及 `None`/失败结果的处理；再在 `import.rs::preCheckRequirements` 或 `DataCheck` 的正确阶段接线。
4. 将规则级测试放在独立测试文件（优先 `precheck_impl_test.rs`），将调度、跳过和结果收集测试放在 `check_info_test.rs`；不要把 Rust 单元测试内嵌回生产源文件。若影响顶层顺序，再同步 `import_test.rs` 或相关集成测试。
5. 对照 Go 的 `check_info.go`、`precheck_impl.go` 与相关测试，逐项说明有意差异，尤其避免把现有 slim checker 的行为无意扩展或收缩。

修改现有路径时的重点风险：改变检查顺序可能影响昂贵检查和首个错误；改变 `Ok(None)` 与 `Some(Passed=true)` 会影响模板输出；扩大 Region 排他区间可能引入借用/死锁风险，缩小则可能出现状态竞态；阈值变化影响性能告警与导入阻断。常量若迁移到其他模块，必须同步 `precheck_impl.rs` 引用和 `parity_test.rs` 的 Go 数值对齐断言。

## 验证依据

- 目标源码：`lightning/pkg/importer/check_info.rs`，RustCodeGraph `node --file` 确认共 214 行、31 个索引符号，并显示 `precheck_impl.rs` 使用该文件。
- 调用入口：`lightning/pkg/importer/import.rs` 的 `Controller::preCheckRequirements`、`Controller::DataCheck`；模块装配：`lightning/pkg/importer/lib.rs`。
- 分发与规则：`lightning/pkg/importer/precheck.rs::PrecheckItemBuilder::BuildPrecheckItem`、`lightning/pkg/importer/precheck_impl.rs`、`lightning/pkg/precheck/precheck.rs::{Checker, CheckResult, CheckItemID}`。
- crate 边界：`lightning/pkg/importer/Cargo.toml` 的 `[lib] path = "lib.rs"`、porting metadata 和 `astersql-lightning-pkg-precheck` path 依赖。
- Go 对照：`lightning/pkg/importer/check_info.go`；补充搜索确认采样常量在 `get_pre_info.go` 使用，Region/CSV 阈值在 `precheck_impl.go` 使用。
- Rust 独立测试：`lightning/pkg/importer/check_info_test.rs`（CSV header、目标表空、本地资源、上下文转换）、`lightning/pkg/importer/table_import_test.rs`（集群资源、Region、大 CSV）、`lightning/pkg/importer/parity_test.rs`（常量与跳过契约）；规则细节另由 `precheck_impl_test.rs` 覆盖。
- RustCodeGraph 精确 `query` 找到 Rust/Go 同名符号；对 `doPreCheckOnItem`、`checkClusterRegion`、`localResource` 的 Rust 节点执行 `callers`/`callees` 均返回空数组。调用边随后由 `rg` 精确匹配补证，此索引限制已在“依赖与调用关系”中保留。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以固定 11 章节命令做结构验证，并人工复核没有把未接线或 slim 行为写成完整支持。
