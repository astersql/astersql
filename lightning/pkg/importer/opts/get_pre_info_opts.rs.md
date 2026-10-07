# `lightning/pkg/importer/opts/get_pre_info_opts.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer-opts` library crate。crate 根在 [`lightning/pkg/importer/opts/lib.rs`](./lib.rs)，通过 `mod get_pre_info_opts` 加载本文件并用 `pub use get_pre_info_opts::*` 将其公开 API 重导出；[`lightning/pkg/importer/opts/Cargo.toml`](./Cargo.toml) 的 `package.metadata.porting.go-package` 将该 crate 对应到 Go 包 `lightning/pkg/importer/opts`。文件本身不采集预导入信息，而是定义传给 importer getter 的配置值及函数式选项。

直接消费方是 [`lightning/pkg/importer/get_pre_info.rs`](../get_pre_info.rs) 中的 `PreImportInfoGetter` 接口与 `PreImportInfoGetterImpl`：`GetAllTableStructures` 和 `EstimateSourceDataSize` 接收 `&[ropts::GetPreInfoOption]`，并调用 `ropts::ApplyGetPreInfoOptions(None, opts)`。相邻的 [`precheck_opts.rs`](./precheck_opts.rs) 还会将一组 `GetPreInfoOption` 克隆进 `PrecheckItemBuilderConfig::PreInfoGetterOptions`，因此本文件也是 precheck 组装链的底层选项定义。

## 核心职责

本文件承担三项职责：

1. 用 `GetPreInfoConfig` 汇总“数据库不存在时是否继续”和“是否绕过缓存”两个布尔开关。
2. 用 `GetPreInfoOption` 表示可组合的配置变换，并由 `WithIgnoreDBNotExist`、`ForceReloadCache` 构造只写一个字段的变换。
3. 用 `Clone`、`NewDefaultGetPreInfoConfig` 和 Rust 额外提供的 `ApplyGetPreInfoOptions` 建立“先复制基线、再按传入顺序叠加选项”的统一流程。

该文件不负责解释数据库错误、访问缓存或执行 I/O；这些行为由上层 getter 根据最终配置决定。当前 Rust 上层只完整接入了 `ForceReloadCache` 的缓存判断，`IgnoreDBNotExist` 与构造期选项仍存在迁移差异，详见“与 Go 版本的对应关系”。

## 主要符号

- `pub struct GetPreInfoConfig { pub IgnoreDBNotExist: bool, pub ForceReloadCache: bool }`：可复制的最终配置。派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`；派生的 `Default` 与显式默认构造函数目前都得到两个 `false`。
- `GetPreInfoConfig::Clone(c: Option<&GetPreInfoConfig>) -> Box<GetPreInfoConfig>`：兼容 Go 可为 `nil` 的接收者。`Some` 时复制所有字段，`None` 时返回默认配置；返回 `Box` 对应 Go 返回指针的使用形态，但副本与输入值独立。
- `NewDefaultGetPreInfoConfig() -> Box<GetPreInfoConfig>`：显式创建两个开关均关闭的默认配置。新增字段时必须同时审查此函数和派生 `Default` 是否继续一致。
- `type GetPreInfoOption = Arc<dyn Fn(&mut GetPreInfoConfig) + Send + Sync>`：线程安全、可克隆共享所有权的配置闭包。`Arc` 使 option 可放入 `Vec` 并由 `precheck_opts.rs` 克隆；`Send + Sync` 允许其随上层并发对象跨线程传递或共享。
- `WithIgnoreDBNotExist(bool) -> GetPreInfoOption`：捕获布尔值，应用时只覆写 `IgnoreDBNotExist`。
- `ForceReloadCache(bool) -> GetPreInfoOption`：捕获布尔值，应用时只覆写 `ForceReloadCache`。
- `ApplyGetPreInfoOptions(base: Option<&GetPreInfoConfig>, opts: &[GetPreInfoOption]) -> GetPreInfoConfig`：先通过 `Clone` 取得独立配置，再按切片顺序同步执行所有闭包并按值返回结果；同一字段重复设置时最后一个 option 生效。

所有上述符号都是 crate 根重导出的公开 API。文件没有常量、trait、条件编译项或私有辅助函数。

## 执行流程

典型调用流程如下：

1. 调用方构造零个或多个 option，例如 `ForceReloadCache(true)`；构造函数只捕获参数，不立即修改配置。
2. `PreImportInfoGetterImpl::GetAllTableStructures` 或 `EstimateSourceDataSize` 将 option 切片交给 `ApplyGetPreInfoOptions`。
3. `ApplyGetPreInfoOptions` 调用 `GetPreInfoConfig::Clone(base)`。`base` 为 `None` 时从 `NewDefaultGetPreInfoConfig` 开始，为 `Some` 时得到输入的独立副本。
4. 函数依次调用每个 `Arc<dyn Fn>`，所有闭包都在当前线程上同步修改局部 `cfg`。顺序是契约的一部分，因此后写覆盖前写。
5. 上层读取最终字段。当前 [`get_pre_info.rs`](../get_pre_info.rs) 的两个直接调用点用 `ForceReloadCache` 决定是否返回 `tableStructs` 或 `estimated` 中的缓存；为 `true` 时跳过缓存命中路径、重新计算并更新缓存。

`WithIgnoreDBNotExist` 的意图是在 schema 获取时允许数据库不存在，但当前 Rust 上层没有完整复现 Go 的错误筛选流程：`getTableStructuresByFileMeta` 在远端获取失败时先用 `?` 返回错误，而其后针对缺少单表的 `if/else` 两支目前生成相同占位 `TableInfo`。因此不能把该开关描述为当前 Rust 已经能吞掉远端“数据库不存在”错误。

## 数据与状态

`GetPreInfoConfig` 是小型纯值对象，没有内部引用、锁或外部资源。`Clone(Some(...))` 复制两个布尔值，随后修改返回的 `Box<GetPreInfoConfig>` 不会影响源对象；`parity_test.rs::contract_boundary` 用修改副本后检查原值的方式锁定这一点。

每个 option 捕获一个 `bool`，其共享状态只存在于 `Arc` 的引用计数中；闭包本身不维护可变状态。配置修改发生在调用方提供的 `&mut GetPreInfoConfig` 上。`ApplyGetPreInfoOptions` 创建局部副本，因而不会回写 `base`。空 option 切片保持基线不变；`None` 基线加空切片等价于显式默认配置。

字段的业务含义由上层体现：

- `ForceReloadCache = false` 允许 getter 返回已有缓存，`true` 则要求重新采集/计算。
- `IgnoreDBNotExist` 在 Go 中只针对匹配到“数据库不存在”的远端 schema 错误放宽处理，不是忽略任意错误；当前 Rust 选项值可被设置，但该精确错误语义尚未接通。

## 依赖与调用关系

本文件唯一直接导入的是标准库 `std::sync::Arc`；opts crate 的 Cargo manifest 当前没有外部依赖。模块由 [`lib.rs`](./lib.rs) 装配并重导出。

RustCodeGraph 与源码交叉核对得到的主要边为：

- `ApplyGetPreInfoOptions -> GetPreInfoConfig::Clone -> NewDefaultGetPreInfoConfig`。
- `PreImportInfoGetterImpl::GetAllTableStructures -> ApplyGetPreInfoOptions`。
- `PreImportInfoGetterImpl::EstimateSourceDataSize -> ApplyGetPreInfoOptions`。
- `precheck_opts.rs::WithPreInfoGetterOptions` 接收并克隆 `GetPreInfoOption`，使 option 能经 `PrecheckItemBuilderConfig` 间接传递。
- `get_pre_info_test.rs` 的 getter 构造辅助函数使用 `WithIgnoreDBNotExist(true)`；缓存相关测试使用 `ForceReloadCache(true)`。

RustCodeGraph 的 `explore` 还报告 `ForceReloadCache` 被 `test_get_pre_info_estimate_source_size` 和 `test_get_pre_info_get_all_table_structures` 调用，`WithIgnoreDBNotExist` 被 `make_pre_import_getter_with_storage` 调用。精确 `callees` 查询确认 `ApplyGetPreInfoOptions` 调用 `Clone`；个别精确 `callers` 查询在本次执行时超时，所以上述上游边另由 `rg` 和直接读取源码确认。

## 错误处理与边界

本文件 API 都不返回 `Result`，option 闭包也没有错误通道。可观测边界包括：

- `Clone(None)` 合法且回退到默认值，用于保留 Go nil receiver 的行为。
- `ApplyGetPreInfoOptions(..., &[])` 是无操作，不会 panic 或改变基线。
- option 按切片顺序执行；同一字段被多次设置时最后写入生效，`parity_test.rs::contract_error` 以 `true` 后接 `false` 验证此规则。
- 类型别名没有阻止调用者构造会 panic 或改写多个字段的自定义闭包；本文件提供的两个构造器不会 panic，且各自只改一个字段。扩展时不应假定任意第三方 option 都具备这一性质。
- `Arc<dyn Fn + Send + Sync>` 要求捕获数据满足线程安全约束，但并不捕获闭包执行中的 panic，也不提供事务回滚；若某个 option panic，之前已执行的配置修改不会被本函数恢复。
- `IgnoreDBNotExist` 按 Go 语义应只放宽特定 schema 错误。若补齐 Rust 接线，应保留错误分类边界，不能扩成吞掉所有 `FetchRemoteTableModels` 错误。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务、文件句柄或网络连接。option 被 `Arc` 管理，最后一个强引用释放时闭包及其捕获值自动销毁；这里只有 `bool` 捕获，因此没有显式清理动作。`parity_test.rs::contract_resource_cleanup` 验证 option 可经 builder 克隆、重复应用并安全 drop。

`Send + Sync` 只表示 option 对象可以跨线程移动和共享。实际配置变更需要独占的 `&mut GetPreInfoConfig`，`ApplyGetPreInfoOptions` 又只修改栈上的局部副本，所以文件内部没有共享可变配置，也不需要锁。上层 getter 的缓存使用 `Mutex`，但锁及缓存生命周期属于 [`get_pre_info.rs`](../get_pre_info.rs)，不由本文件管理。

## 与 Go 版本的对应关系

直接对照文件是 [`get_pre_info_opts.go`](./get_pre_info_opts.go)。字段名、两个默认值、nil/`None` 克隆语义，以及两个 option 对单字段的覆写均与 Go 对齐。Rust 使用 `Box<GetPreInfoConfig>` 表达独立堆分配配置，使用 `Arc<dyn Fn + Send + Sync>` 代替 Go 的 `func(*GetPreInfoConfig)`；`Arc` 是为了支持 option 切片克隆和 Rust 并发 trait 约束。

`ApplyGetPreInfoOptions` 是 Rust 为收敛 Go 调用点中“克隆配置并循环应用 option”的重复代码而增加的辅助函数，Go 同路径文件没有同名函数。它的 `Some(base)` 路径可表达 Go 的 `p.getPreInfoCfg.Clone()` 后叠加调用期 options。

当前调用接线尚未完全对齐：

- Go `NewPreImportInfoGetter` 把构造期 options 应用到 `getPreInfoCfg` 并存入 getter；每次方法调用先克隆该基线，再叠加调用期 options。
- Rust `NewPreImportInfoGetter` 当前把参数命名为 `_opts` 且没有在 `PreImportInfoGetterImpl` 保存基线；`GetAllTableStructures` 和 `EstimateSourceDataSize` 调用 `ApplyGetPreInfoOptions(None, opts)`。因此构造期 option 不会成为后续方法的基线。
- Go `getTableStructuresByFileMeta` 仅在错误文本匹配数据库不存在时、且 `IgnoreDBNotExist` 为真时转入源端 schema 流程；Rust 当前远端调用错误会直接传播，后面的 `IgnoreDBNotExist` 分支与 `else` 分支又产生相同占位表。

这些是上层迁移/接线现状，不是本文件 option 原语自身的错误；后续若对齐行为，应同时修改上层实现及其独立测试，不能只改本文件注释或默认值。

## 扩展指南

新增配置项时，至少应同步：

1. 在 `GetPreInfoConfig` 增加字段，并同时决定派生 `Default` 与 `NewDefaultGetPreInfoConfig` 的明确默认值，避免两条默认路径漂移。
2. 增加只负责该字段的 option 构造器；若捕获非 `Copy` 数据，要确认它能满足 `Send + Sync`，并明确多次应用和 clone 的所有权语义。
3. 确认 `GetPreInfoConfig::Clone` 的派生克隆足以复制新字段；若新字段持有共享句柄，要区分“配置值独立”与“底层资源共享”。
4. 在真实消费点读取新字段，而不是仅增加可设置但无行为的 API。若行为需要继承构造期基线，应优先补齐 getter 保存基线并调用 `ApplyGetPreInfoOptions(Some(&base), opts)` 的接线。
5. 在独立测试文件 [`parity_test.rs`](./parity_test.rs) 增加默认值、覆盖顺序、`Clone(None)`、非空基线和资源释放契约；涉及缓存/schema 行为时，还应同步 [`get_pre_info_test.rs`](../get_pre_info_test.rs) 及对应 Go 测试意图。不要把 Rust 单元测试内嵌到本生产文件。

兼容风险主要是更改默认值或 option 顺序导致历史调用行为改变；正确性风险主要是 option 已暴露但消费点未接线，或把 `IgnoreDBNotExist` 扩成忽略无关错误；性能风险主要来自错误地默认启用 `ForceReloadCache`，使 schema/大小估算反复执行。该配置目前极小，`Box`/`Arc` 的成本相对 I/O 很低，但高频新增大型捕获值时应重新评估克隆成本。

## 验证依据

本说明基于以下证据（均为本次只读检查，未运行 Cargo）：

- 目标源码 [`get_pre_info_opts.rs`](./get_pre_info_opts.rs)：7 个主要符号、默认值、闭包类型和 option 应用顺序。
- crate 声明 [`Cargo.toml`](./Cargo.toml) 与模块入口 [`lib.rs`](./lib.rs)：crate 名、Go 包映射、无外部依赖以及公开重导出关系。
- Go 对照 [`get_pre_info_opts.go`](./get_pre_info_opts.go) 和调用实现 [`get_pre_info.go`](../get_pre_info.go)：原始函数式 option 契约、构造期基线保存、缓存与数据库不存在错误的消费语义。
- Rust 调用实现 [`get_pre_info.rs`](../get_pre_info.rs)：接口参数、`ApplyGetPreInfoOptions(None, opts)` 调用、`ForceReloadCache` 缓存分支，以及当前 `IgnoreDBNotExist` 接线状态。
- 独立 Rust 测试 [`parity_test.rs`](./parity_test.rs) 与 [`get_pre_info_test.rs`](../get_pre_info_test.rs)：默认值、独立克隆、空 options、最后写入生效、重复应用/drop，以及两个上层缓存重载场景。Go 同目录没有 `get_pre_info_opts_test.go`；相关 Go 行为覆盖位于 [`get_pre_info_test.go`](../get_pre_info_test.go) 和 `table_import_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录 7 个文件；`explore "lightning/pkg/importer/opts/get_pre_info_opts.rs GetPreInfoOptions get_pre_info_opts"` 返回目标源、Go 对照和 blast radius；`query ApplyGetPreInfoOptions --kind function` 定位到本文件第 83 行；`callees ApplyGetPreInfoOptions --file ...` 返回 `Clone`。上游边随后以 `rg` 和源码读取复核。
- 仓库中未找到 importer package 的 `doc.go`，因此没有额外包级契约可读。结构验收使用任务规定的命令，验证本文件存在且恰有 11 个固定二级标题。
