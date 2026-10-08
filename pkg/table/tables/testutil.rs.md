# [`pkg/table/tables/testutil.rs`](testutil.rs)

## 文件定位

本文件属于 `astersql-table-tables` crate，是 `pkg/table/tables` 中专门服务于分区重组测试的辅助模块。crate 根模块在 `pkg/table/tables/lib.rs` 中以 `pub mod testutil` 无条件公开它，因此函数可以被 crate 内测试和外部依赖者引用；当前 Rust 仓库中唯一实际引用位于 `pkg/table/tables/tables_test.rs`。

它不参与 SQL 请求、分区路由或 DDL 的生产执行链，也不创建表。它的作用是在测试中模拟“当前 schema 视图”和“上一 schema 视图”之间的分区重组状态切换，以便验证过渡期读写行为。该定位与同路径 Go 文件 `pkg/table/tables/testutil.go` 的注释“used in tests”一致。

## 核心职责

文件只承担一项职责：`swap_reorg_part_fields` 接收两个类型擦除后的可变对象，确认二者都是 `PartitionedTable` 后，将影响分区重组观察结果的全部 Rust 字段成对交换。

它同时维护三个关键契约：

1. 动态类型不匹配时返回 `false`，不以 panic 表示测试前置条件失败。
2. 两次成功调用会恢复原状态，因为每一步都使用对称的 `std::mem::swap`。
3. 成功返回前交换 `PartitionedTable` 当前定义的全部五个字段，避免产生“定义来自一个 schema、路由状态来自另一个 schema”的混合对象。

## 主要符号

- `pub fn swap_reorg_part_fields(src: &mut dyn Any, dst: &mut dyn Any) -> bool`：本文件唯一的模块级函数和公开 API。`src`、`dst` 通过 `Any::downcast_mut::<PartitionedTable>()` 恢复具体类型；返回值只表示两侧类型检查及交换是否成功，不携带错误详情。
- `PartitionedTable`：定义在 `pkg/table/tables/partition.rs`。被交换的字段为 `definitions: Vec<PartitionDefinition>`、`expression: PartitionExpr`、`partitions: HashMap<i64, Partition>`、`reorganize_partitions: HashMap<i64, Partition>` 和 `double_write_partitions: HashMap<i64, Partition>`。
- `std::any::Any`：提供与 Go `table.Table` 类型断言相近的动态边界。调用者不需要在函数签名处暴露具体表类型，但只有 `PartitionedTable` 能成功。
- `std::mem::swap`：逐字段原地交换所有权，不克隆字段内容，也不分配一套新的表状态。

本文件没有常量、类型、trait、`impl`、条件编译项或内部辅助函数。

## 执行流程

1. 对 `src` 执行 `downcast_mut::<PartitionedTable>()`。若失败，函数立即返回 `false`；此时尚未修改任何输入。
2. 对 `dst` 执行相同下转型。若失败，同样返回 `false`；第一步只取得过可变引用，没有写入，因此两侧仍保持原状。
3. 按固定顺序交换 `definitions`、`expression`、`partitions`、`reorganize_partitions`、`double_write_partitions`。
4. 全部交换完成后返回 `true`。

`pkg/table/tables/tables_test.rs` 中的 `reorganization_swap_exchanges_all_fields_and_checks_dynamic_type` 构造两个字段内容不同的分区表，在一侧加入重组分区、另一侧加入双写分区。调用后，它把两个完整对象分别与对方调用前的克隆比较，从而证明不是只交换常态分区；该测试还用 `i64` 作为错误类型，验证失败返回值。

## 数据与状态

函数不持有全局状态，所有变更均局限于调用者提供的两个 `PartitionedTable`：

- `definitions` 描述逻辑分区集合。
- `expression` 决定值到分区的路由规则。
- `partitions` 保存正常可见物理分区。
- `reorganize_partitions` 保存 DDL 重组期间当前参与重组的分区映射。
- `double_write_partitions` 保存过渡期需要额外写入、但尚未作为正常分区可见的目标映射。

成功后的强不变量是：`src` 的五个字段整体等于调用前 `dst` 的五个字段，反之亦然。函数依赖 `PartitionedTable` 当前只有这五个字段这一结构事实；若将来结构新增会影响 schema 视图的字段，而本函数没有同步交换，完整对象交换不变量就会失效。

## 依赖与调用关系

- crate 边界：`pkg/table/tables/Cargo.toml` 声明包名 `astersql-table-tables`，库入口是 `lib.rs`；默认启用 `expression-runtime`。本函数本身只使用 crate 内 `partition::PartitionedTable` 和 Rust 标准库，不直接依赖 Cargo 中列出的外部 crate。
- 模块接线：`pkg/table/tables/lib.rs` 的 `pub mod testutil` 导出本模块；该导出没有 `#[cfg(test)]`，所以 API 会进入正常库构建，尽管当前用途仅为测试。
- 上游调用者：RustCodeGraph 将 `pkg/table/tables/tables_test.rs` 标为本文件唯一使用者；其测试通过 `use crate::testutil::swap_reorg_part_fields` 调用。仓库搜索未发现其他 Rust 调用点。
- 下游依赖：函数直接调用两次 `Any::downcast_mut` 和五次 `mem::swap`，操作 `pkg/table/tables/partition.rs::PartitionedTable` 的字段；无 I/O、存储、SQL、表达式求值或后台任务调用。
- 应用主链位置：它位于分区表测试支线，不在“SQL -> planner -> executor -> table/storage”线上；Go 集成测试会在 DDL failpoint 回调中反复切换新旧 schema 表状态，而当前 Rust 直接测试仅验证交换原语本身。

## 错误处理与边界

函数没有 `Result` 或错误类型。唯一显式失败条件是任一参数无法下转为 `PartitionedTable`，此时返回 `false`。

边界行为如下：

- `src` 类型错误：在读取 `dst` 之前立即失败。
- `src` 正确而 `dst` 类型错误：已经取得的 `src` 可变引用没有被写入，函数仍保持原子式的“全不交换”。
- 两侧都是 `PartitionedTable`：五次 `mem::swap` 对这些容器和枚举不会产生可恢复错误，因此函数没有部分成功的错误分支。
- 两个参数不能安全地指向同一个可变对象；Rust 的可变借用规则要求调用者提供互斥的 `&mut` 引用。使用安全 Rust 的正常调用无法构造重叠参数。
- 本函数没有检查分区定义、表达式和映射之间的语义一致性；一致性来自“整组字段一起交换”以及调用者传入的两个表原本有效。

## 并发与资源生命周期

本文件不创建锁、线程、异步任务、通道、事务、文件句柄或网络资源。两个 `&mut dyn Any` 在调用期间提供独占访问，函数本身没有并发同步责任。

`mem::swap` 只交换字段值的所有权；`Vec` 和 `HashMap` 的底层分配随字段一起转移，不逐元素复制，也不会在交换过程中释放其内容。函数返回后，可变借用结束，资源继续由交换后的两个 `PartitionedTable` 管理，最终按各自正常生命周期释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/table/tables/testutil.go` 的 `SwapReorgPartFields(src, dst table.Table) bool`。两版共同语义是：先对两侧执行具体分区表类型检查，任一失败则返回 `false`；成功时原地交换分区重组所需状态并返回 `true`。

字段不是机械的一一对应。Go 的 `partitionedTable` 嵌入 `TableCommon`，对照函数交换 `meta`、`partitionExpr`、`reorgPartitionExpr`、`partitions`、`reorganizePartitions`、`doubleWritePartitions` 六组状态；Rust 的轻量 `PartitionedTable` 将当前模型表达为 `definitions`、单一 `expression` 和三个分区映射，共五个字段。因此 Rust 实现保持的是“交换 Rust 表对象的全部分区定义与过渡态”的行为意图，而不是复制 Go 的内部布局。

Go 的真实使用证据位于 `pkg/table/tables/test/partition/partition_test.go`：`beforeRunOneJobStep` failpoint 回调在分区重组的多个 schema 状态中反复调用该函数，在当前/上一 schema 视图之间交替执行插入、更新、删除并检查两张表的数据一致性。Rust 的 `pkg/table/tables/test/partition/partition_test.rs` 目前只以场景清单和 Go 源映射记录这一行为，直接可执行回归由 `pkg/table/tables/tables_test.rs` 的单元测试覆盖；这说明 Rust 尚未复刻该 Go 端到端 failpoint 场景。

## 扩展指南

- 若 `PartitionedTable` 新增会影响重组期 schema 视图、分区定位或写入目标的字段，必须同步在 `swap_reorg_part_fields` 中交换它，并扩展 `reorganization_swap_exchanges_all_fields_and_checks_dynamic_type`，让两个调用前对象在新字段上也不同。否则现有“完整对象与对方旧值相等”的断言通常会直接暴露遗漏。
- 若要支持其他表实现，不应悄悄把错误类型视作成功。应先明确 API 是继续模拟 Go 的具体类型断言，还是改为 trait 形式；后者会改变公开边界和兼容语义，需要单独设计。
- 若 Rust 模型以后拆分常态与重组专用表达式，应按 Go `partitionExpr`/`reorgPartitionExpr` 的语义同时交换两者，并增加路由行为断言，而不只是字段相等断言。
- 测试逻辑应继续放在独立的 `pkg/table/tables/tables_test.rs`，不要内嵌到本源文件。需要补齐 Go 的 DDL 过渡场景时，应扩展独立的分区测试文件，并保留 failpoint 各 schema 状态下的插入、更新、删除和一致性检查意图。
- 性能风险很低：现实现为常数次所有权交换。扩展时应避免深拷贝大型分区映射；兼容风险主要来自遗漏新字段或改变 `false` 的动态类型失败契约。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 `pkg/table/tables/testutil.rs`；文件节点显示 50 行、2 个符号节点，并标识 `pkg/table/tables/tables_test.rs` 为唯一使用文件。
- RustCodeGraph `node --file pkg/table/tables/testutil.rs` 与 `node pkg/table/tables/testutil.rs::swap_reorg_part_fields`：确认函数签名、两次动态下转型、五次字段交换和成功返回路径。
- RustCodeGraph `node partition.rs::PartitionedTable`：确认结构定义及五个被交换字段，位置为 `pkg/table/tables/partition.rs`。
- `pkg/table/tables/lib.rs`：确认 `testutil` 的公开模块接线以及 crate 内测试模块的独立文件组织。
- `pkg/table/tables/Cargo.toml`：确认 crate 名称、库入口、默认 feature 和依赖边界。
- `pkg/table/tables/tables_test.rs`：确认直接回归测试 `reorganization_swap_exchanges_all_fields_and_checks_dynamic_type` 同时覆盖完整状态交换与动态类型失败。
- `pkg/table/tables/testutil.go`、`pkg/table/tables/partition.go`：确认 Go 类型断言、交换字段及 Go/Rust 数据模型差异。
- `pkg/table/tables/test/partition/partition_test.go` 与 `pkg/table/tables/test/partition/partition_test.rs`：确认 Go 端 DDL 重组中的真实使用场景，以及 Rust 当前对该场景的映射程度。
- 人工复核结论：本文件是测试专用的分区重组状态交换原语；安全扩展的关键是让交换字段与 `PartitionedTable` 的重组相关状态持续保持完整同步，并在独立测试中同时覆盖成功后的完整对象状态和类型失败的不修改行为。
