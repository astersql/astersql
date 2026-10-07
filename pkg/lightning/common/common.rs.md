# `pkg/lightning/common/common.rs`

源文件：[`common.rs`](common.rs)

## 文件定位

本文件属于 `astersql-lightning-common` crate，是 Lightning/IMPORT INTO 导入链路共享的表级 AutoID 边界。crate 入口 `pkg/lightning/common/lib.rs` 通过 `mod common; pub use common::*;` 将这里的常量、精简表元数据、分配器 trait 和辅助函数公开；`pkg/lightning/common/Cargo.toml` 声明该 crate 的 Go 对照包为 `pkg/lightning/common`。它不负责保存真实的 TiDB 元数据，也不直接连接 KV/etcd，而是把这些能力抽象为 `AutoIDRequirement` 和 `Allocator`，供宿主实现注入。

当前 Rust 生产链中，`pkg/executor/importer/production_regions.rs` 的 `rebaseTableAllocatorsWithStore` 将实际表元数据映射成这里的 `TableInfo`，再调用 `RebaseTableAllocators`。根 facade 还在 `pkg/lib.rs` 中再导出整个 `astersql-lightning-common` crate。常量和默认变量虽然是公开 API，但精确检索没有发现当前 Rust 生产代码直接引用本文件的 `IndexEngineID`、`AllTables`、`DefaultImportantVariables` 或 `DefaultImportVariablesTiDB`；仓库其他 Lightning 迁移层存在同名定义，不能视为本文件的调用边。

## 核心职责

1. 用 `IndexEngineID = -1` 标识索引引擎，用 `AllTables = "all"` 表示面向全部表的操作范围。
2. 以惰性只读映射 `DefaultImportantVariables` 和 `DefaultImportVariablesTiDB` 提供影响导入编码/行为的系统变量默认值。
3. 用 `AllocatorType`、`Allocator`、`AutoIDRequirement` 将 RowID、AUTO_INCREMENT、AUTO_RANDOM 的实际存储分配器隔离在 crate 边界之外。
4. 用 `TableInfo` 保存选择分配器所需的最小表结构事实，而不依赖完整 TiDB `model.TableInfo`。
5. `GetGlobalAutoIDAlloc` 根据表结构和版本语义选择分配器；`GetMaxAutoIDBase` 读取所有候选的最大已用 base；`RebaseTableAllocators` 仅对调用者显式提供 base 的类型执行 rebase。

## 主要符号

- `IndexEngineID: i32`：值为 `-1` 的特殊引擎编号。它与普通数据引擎编号分域；本文件只定义约定，不操作引擎。
- `AllTables: &str`：值为 `"all"` 的全表哨兵。本文件不解析或校验该字符串。
- `DefaultImportantVariables: LazyLock<HashMap<&'static str, &'static str>>`：八个重要系统变量的缺省值，包括包大小、时区、本地化、周格式、加密模式、聚合长度和退避权重。
- `DefaultImportVariablesTiDB`：导入后端附加的 `tidb_row_format_version = 1`。
- `AllocatorType::{RowID, AutoIncrement, AutoRandom}`：也是 rebase `bases` 映射的键；派生 `Copy/Eq/Hash` 使其可稳定查表。
- `Allocator: Send + Sync`：真实分配器协议。`NextGlobalAutoID` 返回下一个可用 ID，`GetType` 标识类型，`Rebase(ctx, base, alloc_ids)` 提升游标。
- `AutoIDRequirement: Send + Sync`：宿主存储协议。`StoreAvailable` 先验证存储可用性，`NewAllocator` 接收库/表 ID、有符号性、类型、缓存步长和表版本并返回共享分配器。
- `TableInfo`：选择逻辑的最小输入，包含 ID/名称/版本、三类 AutoID 标志、是否拆分 AUTO_INCREMENT，以及两类无符号标志。
- `TableHasAutoRowID(&TableInfo) -> bool`：当前只是读取 `HasAutoRowID` 的公共兼容入口。
- `GetGlobalAutoIDAlloc(...)`：选择并构造分配器的核心函数。
- `GetMaxAutoIDBase(...)`：取各分配器 `NextGlobalAutoID` 最大值再减一。
- `RebaseTableAllocators(...)`：按分配器类型选择性 rebase，固定传入 `alloc_ids = false`。

## 执行流程

`GetGlobalAutoIDAlloc` 先验证 `requirement` 存在且 `StoreAvailable()` 为真，然后拒绝 `db_id == 0`。通过校验后按互斥优先级选择：

1. 若存在隐式 RowID 或 AUTO_INCREMENT，进入 RowID 主分支。
2. 只有 `SeparateAutoIncrement && HasAutoIncrement` 同时成立时，先创建独立 `AutoIncrement` 分配器，缓存步长为 `1`。
3. 随后总是创建 `RowID` 分配器，缓存步长为 `2`。因此该分支返回顺序固定为可选的 `AutoIncrement`、再 `RowID`。
4. 只有在第一分支不成立且 `HasAutoRandom` 为真时，才返回单个 `AutoRandom` 分配器，缓存步长为 `2`。
5. 三类条件均不成立时，以表名生成“has no auto ID”内部错误。

`GetMaxAutoIDBase` 复用上述选择结果，以 `1` 初始化 `max_next_id`，依次调用 `NextGlobalAutoID` 并保留最大值，最终返回 `max_next_id - 1`。这个初值保证所有分配器均报告初始 next=1 时 base 为 0，也使空列表在理论上仍为 0；正常调用不会得到空列表，因为选择函数会返回至少一个分配器或错误。

`RebaseTableAllocators` 同样先取得完整分配器列表。对每个分配器，用 `GetType` 查找 `bases`；没有对应键就跳过，有键才调用 `Rebase(ctx, base, false)`。因此空映射是成功的无操作，部分映射只更新对应类型。

## 数据与状态

两个默认变量表由 `LazyLock` 首次访问时构造，之后作为进程内共享静态只读映射使用；键和值都是 `'static` 字符串。它们没有热更新、配置覆盖或持久化逻辑。

`TableInfo` 是按值拥有的快照式描述，字符串名称用于错误文本；字段不会被本文件修改。`GetGlobalAutoIDAlloc` 每次调用都会请求新的 `Arc<dyn Allocator>`，但是否映射到相同底层游标由 `AutoIDRequirement` 实现决定。测试中的 `MockRequirement` 按 `(table_id, AllocatorType)` 复用 `AtomicI64`，验证多次构造仍观察同一全局 next；生产实现则由 `pkg/executor/importer/table_import.rs` 中 `AllocatorRebaseBindings.Requirement` 注入。

base 的语义是“已经使用的最大 ID”，next 的语义是“下一个可用 ID”。所以读取时执行 `next - 1`，测试分配器 rebase 时把 next 写成 `base + 1`。无符号标志和 `Version` 仅原样传给分配器工厂，本文件不自行解释范围或版本。

## 依赖与调用关系

- crate 内依赖：`CommonError` 和 `Context` 由 `lib.rs` 中其他模块再导出后通过 `crate::{...}` 使用；标准库提供 `HashMap`、`Arc` 和 `LazyLock`。
- crate 清单：`pkg/lightning/common/Cargo.toml` 的直接运行时依赖只有 `astersql-lightning-log` 与 `libc`。本文件未直接引用二者；真实存储/元数据依赖通过 trait 反转给调用者。
- 直接上游：`pkg/executor/importer/production_regions.rs::rebaseTableAllocatorsWithStore` 构造 `TableInfo` 和按类型映射的 bases，调用 `RebaseTableAllocators`，之后无论业务结果如何都尝试关闭 etcd client，并在成功构造 bindings 时重置连接。
- 接线边界：`pkg/executor/importer/table_import.rs::AllocatorRebaseBindings` 持有 `Arc<dyn astersql_lightning_common::AutoIDRequirement>`，说明分配器实现由 importer 宿主提供。
- 内部调用：`GetMaxAutoIDBase -> GetGlobalAutoIDAlloc -> AutoIDRequirement::NewAllocator/Allocator::NextGlobalAutoID`；`RebaseTableAllocators -> GetGlobalAutoIDAlloc -> Allocator::GetType/Allocator::Rebase`；选择函数内部调用 `TableHasAutoRowID`。
- Go 生产链补充：`lightning/pkg/importer/meta_manager.go` 使用 `GetMaxAutoIDBase` 决定首次分配起点，`lightning/pkg/importer/table_import.go` 在本地后端导入完成后用 `RebaseTableAllocators` 同步 RowID/AUTO_INCREMENT。这些是移植意图证据，不等同于 Rust 调用边。

## 错误处理与边界

所有可失败入口返回 `Result<_, CommonError>` 并使用 `?` 立即向上传播首个错误，不做重试或补偿。缺少 requirement 和 `StoreAvailable() == false` 共用 `internal error: kv store should not be nil`；零数据库 ID 使用 `internal error: dbID should not be 0`；没有任何 AutoID 类型时错误包含表名。`NewAllocator` 本身不返回 `Result`，所以工厂失败策略属于实现方契约；若其 panic，本文件不会捕获。

`GetMaxAutoIDBase` 在任一分配器读取失败时丢弃已计算的部分最大值并返回错误。`RebaseTableAllocators` 可能在前面的分配器已成功 rebase 后，因后续分配器失败而返回错误；函数没有事务封装和回滚，因此调用方必须把多分配器部分成功视为可能状态。未知或多余的 `bases` 键不会报错，只因没有匹配分配器而被忽略。

选择优先级很重要：若 RowID/AUTO_INCREMENT 分支成立，即使 `HasAutoRandom` 也为真，AutoRandom 分支不会执行。这与 Go 文件针对 TiDB AutoID 约束的假设一致；若未来放宽“一表一种主 AutoID 路径”的模型，必须显式修改该优先级并增加组合测试。

## 并发与资源生命周期

`Allocator` 与 `AutoIDRequirement` 都要求 `Send + Sync`，返回值包在 `Arc` 中，允许调用者跨线程共享实现。静态默认映射由 `LazyLock` 保证线程安全的一次初始化。除这些约束外，本文件不创建线程、任务、锁、channel、事务或网络连接。

资源生命周期属于上游：`RebaseTableAllocators` 只借用 `Context`、requirement、bases 和表信息，临时持有分配器 `Arc`，函数返回后释放本地引用。生产调用点负责 etcd client 的关闭和 AutoID 连接重置。取消语义也由 `Context` 及具体 `Allocator::Rebase` 实现解释；本文件本身不轮询取消状态。

测试 `pkg/lightning/common/common_test.rs` 使用 `Mutex<HashMap<...>>` 保护分配器注册表、用 `AtomicI64` 表示 next，并采用 `SeqCst`；这是测试替身的同步策略，不是本文件强加给生产实现的算法。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/common/common.go`，Rust 保留了两个常量、两组默认变量、三个主要函数及分配器选择顺序。`pkg/lightning/common/common_test.rs` 的 case 矩阵与 `pkg/lightning/common/common_test.go` 一致：无 AutoID 报错；隐式 RowID 得到 RowID；普通 AUTO_INCREMENT 复用 RowID；`SepAutoInc` 时返回 AutoIncrement 后 RowID；纯 AUTO_RANDOM 返回 AutoRandom。两边也都验证空 bases 不改变状态、部分 rebase 只改变指定类型、最大 base 取所有 next 的最大值减一。

Rust 为解除对完整 TiDB 类型的依赖，引入本地 `TableInfo`，由调用侧从 Go 对应的 `model.TableInfo` 语义字段转换。Go 通过 `autoid.Requirement.Store() != nil` 验证存储，Rust 抽象成 `StoreAvailable()`；Go 直接调用 `autoid.NewAllocator`，Rust 调用 `AutoIDRequirement::NewAllocator`。Go 的 cache option/table version 是具体类型，Rust 用 `u64/u16` 传递等价信息。Rust 的 `AllocatorType` 名称更短，但调用点显式完成 `RowIDAllocType/AutoIncrementType/AutoRandomType` 的映射。

两边都给独立 AUTO_INCREMENT 传步长 `1`、给 RowID/AUTO_RANDOM 传步长 `2`。Go 注释解释步长 2 用于规避步长 1 开启实验特性的同时近似禁用缓存；Rust 源码保留了“近似不缓存”的结论，但未携带 issue 链接。Rust 测试是纯内存替身，Go 测试会创建 mock store、建库建表并解析真实 SQL，因此 Go 测试对完整元数据推导提供更强的集成证据，Rust 测试则覆盖本地抽象本身。

## 扩展指南

- 新增分配器类型时，先扩展 `AllocatorType`，再更新 `TableInfo` 的判定事实、`GetGlobalAutoIDAlloc` 的选择顺序、生产调用处的类型映射，以及 `pkg/lightning/common/common_test.rs` 的组合矩阵；同时核对 Go `autoid.AllocatorType` 和 `pkg/lightning/common/common.go`。
- 修改默认系统变量时同步核对 `pkg/lightning/common/common.go`，并检查真正消费变量的 importer/importsdk 路径；不能仅修改静态表而假设 Rust 生产链已接线。
- 修改 cache step、unsigned 或 table version 传递时，需要在测试 requirement 中记录 `NewAllocator` 参数，新增参数级断言；现有测试主要断言返回类型和 next/base 行为。
- 为 `RebaseTableAllocators` 增加原子性时，必须在 trait/宿主存储层设计事务或补偿机制；在本函数内简单捕获错误无法撤销已完成的前序 rebase。
- 新增行为测试应继续放在独立的 `pkg/lightning/common/common_test.rs`，不要内嵌到生产文件。若改变生产接线，还应同步覆盖 `pkg/executor/importer/production_regions.rs` 附近的 importer 测试。
- 保持 `Send + Sync` 和 `Arc` 契约，避免把不可跨线程实现藏进 trait object；对可能阻塞的远程操作，应由实现方处理超时和 `Context` 取消。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter pkg/lightning/common` 确认 `common.rs` 有 24 个符号。
- RustCodeGraph 源码视图：`node --file pkg/lightning/common/common.rs --offset 1 --limit 500` 覆盖目标文件全部 204 行，并给出公开符号签名。
- RustCodeGraph 精确查询：`query GetGlobalAutoIDAlloc/GetMaxAutoIDBase/RebaseTableAllocators/TableHasAutoRowID --json` 区分了 Go/Rust 同名符号；对四个 Rust node ID 执行 `callers`/`callees` 未返回边，因此调用关系改用精确源码检索核验，未把缺失图边写成“无调用者”。
- 已读 Rust 路径：`pkg/lightning/common/common.rs`、`pkg/lightning/common/lib.rs`、`pkg/lightning/common/Cargo.toml`、`pkg/lightning/common/common_test.rs`、`pkg/executor/importer/production_regions.rs`、`pkg/executor/importer/table_import.rs`。
- 已读 Go 对照：`pkg/lightning/common/common.go`、`pkg/lightning/common/common_test.go`，并抽查 `lightning/pkg/importer/meta_manager.go` 与 `lightning/pkg/importer/table_import.go` 的真实使用场景。
- 直接调用证据：精确检索定位到 `pkg/executor/importer/production_regions.rs` 对 `astersql_lightning_common::RebaseTableAllocators` 的调用；目标函数内部对 `GetGlobalAutoIDAlloc`、`NextGlobalAutoID`、`GetType` 和 `Rebase` 的调用由完整源码确认。
- 结构校验要求：文档必须存在，且固定标题正好为“文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据”共 11 个。
