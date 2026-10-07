# `pkg/lightning/backend/kv/allocator.rs` 逻辑说明

## 文件定位

`allocator.rs` 属于 `astersql-lightning-backend-kv` crate。该 crate 由同目录 `Cargo.toml` 定义，以 `lib.rs` 为入口；`lib.rs` 通过 `mod allocator` 装入本模块，再以 `pub use allocator::*` 向 crate 使用者公开这里的类型和构造函数。crate 的移植元数据把 Go 对照包标为 `pkg/lightning/backend/kv`。

本文件处于 Lightning/IMPORT INTO 的“行数据编码为 KV”路径内，但不负责从元数据服务申请新 ID。它保存编码过程中已经使用过的 AUTO_RANDOM、AUTO_INCREMENT 和隐式 RowID 的最大值，供上层在导入后处理阶段重设表的 ID 水位。直接消费者包括 `pkg/lightning/backend/kv/base.rs`、`pkg/lightning/backend/kv/sql2kv.rs` 和 `pkg/executor/importer/kv_encode.rs`。

## 核心职责

- 用 `AllocatorType` 区分 `AutoRandomType`、`AutoIncrementType`、`RowIDAllocType` 三类水位。
- 用 `panickingAllocator` 中的 `AtomicI64` 保存单类 ID 的当前最大值，并通过 `Rebase` 保证并发更新只增不减。
- 用 `Allocators` 聚合三个共享分配器，通过 `Arc` 让编码器及其克隆观察同一份原子状态。
- 用 `NewPanickingAllocators` 和 `NewPanickingAllocatorsWithBase` 提供零水位或指定水位的构造入口。

这里的“allocator”实际是最大值收集器，不会生成或预留 ID。该事实由 `panickingAllocator` 仅提供 `Rebase`、`Base`、`GetType` 三个操作，以及调用方只在编码既有/派生 ID 后执行 `Rebase` 共同证明。

## 主要符号

- `AllocatorType`：公开枚举，三种变体同时作为 `Allocators::Get` 的查找键；派生 `Eq`、`Hash`，因此上层也可将其用作映射键（例如 `pkg/executor/importer/table_import.rs` 的最大值映射）。
- `panickingAllocator`：模块公开、字段私有的单类水位容器。`base: AtomicI64` 是可变状态，`ty: AllocatorType` 是构造后不变的类型标签。名称沿袭 Go 的 `panickingAllocator`；当前 Rust 类型没有实现一个“其他操作会 panic”的通用 allocator trait，而是只暴露已实现的方法。
- `panickingAllocator::Rebase(newBase, _allocIDs)`：以 CAS 循环尝试将 `base` 提升到 `newBase`；`_allocIDs` 为 Go 签名兼容参数，当前不参与行为。
- `panickingAllocator::Base()`：以 `SeqCst` 读取当前水位。
- `panickingAllocator::GetType()`：返回不可变类型标签。
- `Allocators`：公开聚合容器；公开字段 `SepAutoInc` 保存构造策略，私有 `values` 固定包含三个 `Arc<panickingAllocator>`。派生 `Clone` 时只克隆 `Arc`，不会复制原子水位。
- `Allocators::Get(ty)`：线性扫描固定长度数组并克隆匹配的 `Arc`。内部不变量是三种类型必须全部存在；若该不变量被未来改动破坏，会在 `expect("all allocator types are installed")` 处 panic。
- `NewPanickingAllocators(sepAutoInc)`：以三个零水位调用 `NewPanickingAllocatorsWithBase`。
- `NewPanickingAllocatorsWithBase(sepAutoInc, autoRandBase, autoIncrBase, autoRowIDBase)`：按 AUTO_RANDOM、AUTO_INCREMENT、RowID 的固定顺序创建三个分配器。

## 执行流程

1. 上层建立表编码定义时构造 `Allocators`。默认路径见 `TableDefinition::default`（`base.rs`）和 `tableFromMeta`（`pkg/executor/importer/kv_encode.rs`），两者均调用 `NewPanickingAllocators(false)`。
2. 构造函数为每种 `AllocatorType` 建立独立 `AtomicI64`，指定初始 base，并将其放入固定的三元素数组。
3. 编码每列时，`BaseKVEncoder::ProcessColDatum`（`base.rs`）对 AUTO_RANDOM 或 AUTO_INCREMENT 的最终值调用对应分配器的 `Rebase`。
4. 编码非聚簇/隐式 RowID 时，`tableKVEncoder::Encode`（`sql2kv.rs`）或 `TableKVEncoder::fillRow`（`pkg/executor/importer/kv_encode.rs`）对 `RowIDAllocType` 调用 `Rebase`。
5. `Rebase` 先读取旧值；若新值不大于旧值则直接结束。否则用 `compare_exchange` 写入；竞争失败时以实际当前值重试，直到成功或发现已有线程写入了不小于 `newBase` 的值。
6. 上层通过 `TableAllocators` 克隆聚合容器，或通过 `Get(...).Base()` 读取最终水位；由于克隆共享同一 `Arc`，读取到的是累计状态而不是快照副本。

## 数据与状态

唯一运行时可变状态是三个 `AtomicI64`。构造后，`AllocatorType`、数组成员和成员顺序均不变化。每个类型的 base 可从任意 `i64` 初始值开始；算法只比较有符号整数，因此负初始值或负新值也遵循普通的有符号大小关系。

关键不变量如下：

- 对单个 `panickingAllocator`，成功执行序列中的 `Base()` 单调不减。
- 三种类型各有一个数组成员，`Get` 的成功依赖此完整性。
- `Allocators::clone`、`Allocators::Get` 都共享内部实例；它们不是值快照。
- 三种 base 彼此独立。特别是当前 Rust 实现即使 `SepAutoInc == false`，也不会把 AUTO_INCREMENT 查询重定向到 RowID；`SepAutoInc` 目前只是被保存，没有参与 `Get` 分支。

## 依赖与调用关系

本文件只使用 Rust 标准库：`Arc` 负责共享所有权，`AtomicI64`、`Ordering` 负责无锁水位更新。`Cargo.toml` 中的 `encode`、`tablecodec`、`verification` 是整个 `astersql-lightning-backend-kv` crate 的依赖，本文件没有直接引用它们，也没有 feature 条件或条件编译项。

主要上游调用边：

- `NewPanickingAllocators -> NewPanickingAllocatorsWithBase`：本文件内唯一显式下游函数调用边。
- `TableDefinition::default -> NewPanickingAllocators(false)`：`pkg/lightning/backend/kv/base.rs`。
- `tableFromMeta -> NewPanickingAllocators(false)`：`pkg/executor/importer/kv_encode.rs`。
- `BaseKVEncoder::ProcessColDatum -> Allocators::Get -> panickingAllocator::Rebase`：AUTO_RANDOM 与 AUTO_INCREMENT 水位收集。
- `tableKVEncoder::Encode -> Get(RowIDAllocType) -> Rebase`：`pkg/lightning/backend/kv/sql2kv.rs` 的隐式 RowID 路径。
- `TableKVEncoder::fillRow -> Get(RowIDAllocType) -> Rebase`：`pkg/executor/importer/kv_encode.rs` 的导入编码路径。

`lib.rs` 的公开再导出使外部 crate 可以直接以 `astersql_lightning_backend_kv::AllocatorType`、`NewPanickingAllocators` 等名称使用本模块。RustCodeGraph 对目标文件报告了来自编码器、导入器及测试的使用关系；因 Go/Rust 同名符号会影响精确消歧，具体调用边又通过上述文件中的路径限定搜索核对。

## 错误处理与边界

`Rebase`、`Base`、`GetType` 和两个构造函数均不返回 `Result`；正常执行没有业务错误通道。`newBase <= old`（包括相等或回退请求）是合法的无操作，不是错误。并发 CAS 失败也不是业务失败，而是更新 `old` 后重试。

显式 panic 边界只有 `Allocators::Get` 找不到类型时的 `expect`。当前构造函数总是安装三个枚举变体，因此通过公开构造入口无法触发；若未来新增枚举变体、改变 `values` 构造或提供过滤能力，必须同步处理这一不变量。

本实现不会检查 ID 是否超出表列类型、AUTO_RANDOM 增量掩码或业务允许范围；这些转换与校验属于编码调用方。尤其 Go 的 `BaseKVEncoder.ProcessColDatum` 会对 AUTO_RANDOM 值取 `IncrementalMask`，而当前 Rust `base.rs` 直接用整数值 rebase；这是调用层迁移语义，不应在没有整体设计与回归测试时仅在本文件内猜测修正。

## 并发与资源生命周期

`AtomicI64` 让同一分配器可被多个线程并发 rebase，无互斥锁和阻塞等待。所有原子读、比较交换均使用 `Ordering::SeqCst`，提供全局最强排序；本算法实际只维护单变量最大值，未来若为性能改用更弱内存序，需用并发测试证明不会破坏发布/观察约束。

`Arc` 管理分配器生命周期：`Allocators` 持有三个强引用，`Get` 返回新的强引用，聚合容器被丢弃后，已取出的分配器仍可继续工作。没有后台线程、通道、文件句柄、事务或显式关闭动作。线程退出或 `Arc` 最后一个强引用释放时，分配器自然销毁。

独立 Rust 测试 `allocator_test.rs::TestAllocator` 将同一个 RowID 分配器克隆给 16 个线程，分别 rebase 到 `0..15`，join 后断言 base 为 15，并验证较小值 4 不会回退水位。这同时覆盖了共享生命周期和 CAS 最大值不变量。

## 与 Go 版本的对应关系

直接 Go 对照为 `pkg/lightning/backend/kv/allocator.go`，测试为 `allocator_test.go`。两版共同点是：三种 allocator 类型、可指定初始 base、CAS 单调 rebase、读取 base/类型，以及将它们用于导入期最大 ID 收集。Rust 的 `allocator_test.rs` 还增加了显式多线程竞争验证；Go 测试则分别验证三种类型的目标水位及回退无效。

存在以下已核实差异：

- Go `panickingAllocator` 嵌入 `autoid.Allocator`，未覆盖的接口方法会通过空嵌入值体现“不可用”占位语义；Rust 没有对应 trait，只提供三个可用方法，因此不要把名称理解成 Rust 会对所有其他操作显式 panic。
- Go `Rebase` 接收 `context.Context`、返回 `error`（当前总为 `nil`）；Rust 删除 context 与错误返回，仅保留 `_allocIDs` 兼容位。
- Go 构造结果是 `pkg/meta/autoid.Allocators`。该容器的 `Get` 在 `SepAutoInc == false` 且请求 AUTO_INCREMENT 时会改查 RowID，使二者共享同一 allocator。当前 Rust `Allocators::Get` 忽略 `SepAutoInc`，始终返回独立的 AUTO_INCREMENT 实例；这是实质语义差异，现有 Rust 单测未覆盖该开关。
- Go `Get` 在缺失类型时返回 `nil`；Rust 因固定数组不变量选择 panic。

因此扩展或对齐时应以当前 Rust 行为为现状，同时把 `SepAutoInc` 的别名语义作为优先兼容性检查点，不能仅因字段存在就声称已与 Go 等价。

## 扩展指南

- 新增 allocator 类型：同步修改 `AllocatorType`、`Allocators::values` 的容量/构造、`Get` 的完整性策略，以及 `allocator_test.rs`；还要检查所有穷尽 `match`，尤其 `pkg/executor/importer/production_regions.rs` 和 `pkg/dxf/importinto/encode_and_sort_operator.rs`。
- 对齐 `SepAutoInc`：最可能修改 `Allocators::Get`，并在独立测试 `allocator_test.rs` 中分别覆盖 true/false 时 AUTO_INCREMENT 与 RowID 是否共享同一个 base。必须同时核对 Go `pkg/meta/autoid/autoid.go::Allocators.Get` 和编码调用方，避免改变导入后 rebase 的最大值。
- 改变 rebase 规则：修改 `panickingAllocator::Rebase`，测试至少覆盖相等值、较小值、负值、`i64` 边界及多线程竞争；不得把测试嵌回生产源文件。
- 改变共享方式：`Arc` 或 `Clone` 语义的变化会影响 `BaseKVEncoder::TableAllocators` 返回值是否仍指向实时状态，需要同步 `base.rs`、`sql2kv.rs` 和导入器测试。
- 增加错误返回或 context：这是公开 API 变化，会波及全部 `.Rebase` 调用点；应先确定是否需要与 Go 接口完全一致，再统一迁移，避免只对单一路径做版本简化。

兼容风险主要是 `SepAutoInc` 别名语义和外部公开命名；正确性风险是水位回退或不同克隆不再共享；性能风险集中在高并发下 `SeqCst` CAS 的竞争。任何行为修改都应保留 PingCAP 许可证，并按仓库规则在真正处理的 Rust 源文件顶部保留 AsterSQL 标识。

## 验证依据

- RustCodeGraph：`status` 显示当前仓库索引可用；`node --file pkg/lightning/backend/kv/allocator.rs` 核对了全部 115 行、主要符号及 `NewPanickingAllocators -> NewPanickingAllocatorsWithBase` 调用边；`node --file pkg/lightning/backend/kv/allocator_test.rs` 核对了并发与回退测试。对同名 Go/Rust 符号执行了 `query`、`callers`、`callees`；工具未能稳定消歧调用者，因此未把空调用者输出当作“无调用”。
- Rust 源与模块：`pkg/lightning/backend/kv/allocator.rs`、`lib.rs`、`base.rs`、`sql2kv.rs`，以及 `pkg/executor/importer/kv_encode.rs`。
- crate 边界：`pkg/lightning/backend/kv/Cargo.toml`，确认 crate 名、`lib.rs` 入口、依赖和 Go 包移植元数据；本文件没有直接外部 crate 依赖或 feature 分支。
- Rust 测试：`pkg/lightning/backend/kv/allocator_test.rs::TestAllocator`，验证指定初值、并发取最大值、不回退和类型标签。
- Go 对照：`pkg/lightning/backend/kv/allocator.go`、`allocator_test.go`，并额外核对 `pkg/meta/autoid/autoid.go::{NewAllocators, Allocators.Get}` 以确认 `SepAutoInc` 的重定向语义。
- 精确路径搜索：用 `rg` 核对了 `NewPanickingAllocators`、`AllocatorType`、`Get`、`Rebase`、`Base`、`GetType` 在 Lightning 与导入器 Rust 路径中的直接引用。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求目标文件存在且恰有上述 11 个固定二级章节；交付前另行执行该命令并记录退出码。
