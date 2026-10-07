# `pkg/executor/union_scan.rs`

## 文件定位

本文件属于 `astersql-executor` crate；对应源码为 [`union_scan.rs`](./union_scan.rs)。`pkg/executor/Cargo.toml` 将 crate 根设为 `lib.rs`，`pkg/executor/lib.rs` 通过 `pub mod union_scan;` 公开本模块，并在测试配置中用独立文件 `union_scan_test.rs` 注册单元测试。它提供 UnionScan 的运行时无关核心：把事务内存脏表中的新增行与底层 MVCC 快照扫描结果按键序归并，以实现“读己之写”。直接的数据容器依赖只有 `astersql_util_chunk::Chunk`。

当前 Rust 接线仍需区分“核心已实现”和“生产运行时已接入”：仓库搜索仅发现 `pkg/executor/union_scan_test.rs` 为 `UnionScanRuntime` 提供实现，未发现生产实现或生产代码直接构造 `UnionScanExec<R>`。生产构建入口 `pkg/executor/builder.rs::buildUnionScanExec` 会构建子 reader，再委托 `ExecutorBuilderDependencies::build_union_scan_from_reader`，但该依赖接口与本文件的泛型类型之间没有可见的直接接线。因此，本文件是可执行、已测试的归并核心与适配协议，不能据现有证据宣称它已被 Rust 生产链直接实例化。

## 核心职责

- `UnionScanRuntime` 把具体事务、子执行器、行类型、过滤/虚拟列计算和资源关闭动作隔离在适配层；本文件只控制生命周期和归并状态机。
- `UnionScanExec` 在两个已排序输入之间逐行归并：一侧是事务脏表新增行，另一侧是快照扫描行；被当前事务修改过的快照行会先被遮蔽。
- `Next` 在归并之后计算虚拟列并应用条件，再把通过条件的行写入输出 `Chunk`。条件不匹配只丢弃当前行，不会提前结束批次。
- `compareExec` 实现“索引列优先、handle 破同键、降序整体翻转”的比较策略；`need_extra_sorting` 仅保存与 Go 对齐的配置，本文件内不读取它。

## 主要符号

- `pub trait UnionScanRuntime`（`union_scan.rs:29`）：定义三个关联类型 `Context`、可克隆的 `Row`、`Error`，以及打开子执行器、创建脏行迭代器、拉取两侧数据、判断快照遮蔽、比较、求值、输出和关闭资源等 17 个边界方法。所有可失败操作用关联错误类型返回，核心不绑定具体事务/表达式实现。
- `pub struct UnionScanExec<R>`（`union_scan.rs:83`）：持有 `runtime`、脏行预取槽 `added_row`、快照批次 `snapshot_rows`、游标 `snapshot_cursor`、复用的 `snapshot_chunk` 和可选物理表 ID 列位置。
- `Open` / `open`（`union_scan.rs:100,106`）：前者先调用 `runtime.open_base`，后者记录物理表 ID 列、构建脏行迭代器并分配快照 `Chunk`。
- `Next`（`union_scan.rs:114`）：重置输出批次，循环调用 `getOneRow`，再执行虚拟列和条件求值并追加结果。
- `Close`（`union_scan.rs:131`）：清理两侧缓存，关闭新增行迭代器，然后关闭子执行器。
- `getOneRow`（`union_scan.rs:140`）：执行单步双路归并；比较相等时选择新增行。
- `getSnapshotRow`（`union_scan.rs:167`）：复用或补充快照批次，过滤已被事务修改的快照行；缓存表模式直接返回空快照侧。
- `getAddedRow`（`union_scan.rs:204`）：以单槽预取方式读取新增行，只有归并选中它后才清空槽位。
- `pub struct compareExec<C>` 与 `compare`（`union_scan.rs:213,226`）：按 `used_index` 逐列调用注入的比较函数，全部相等时调用 handle 比较函数；`descending` 对最终非相等结果取反。

## 执行流程

1. 构建层应先创建带具体 `UnionScanRuntime` 的 `UnionScanExec`。调用 `Open` 时先打开底层 reader，再由 `open` 获取分区物理表 ID 列、建立事务新增行迭代器并创建快照缓冲区。
2. 每次 `Next` 读取运行时的最大批大小，重置请求 `Chunk`，并以重置后的 `output_capacity` 作为本轮目标行数。
3. `getOneRow` 分别通过 `getSnapshotRow` 和 `getAddedRow` 查看两侧当前行。仅一侧有行时消费该侧；两侧均有行时调用 `compare_rows`，快照键严格更小时消费快照，否则消费新增行。相等时新增行优先，使事务内版本覆盖快照版本。
4. `getSnapshotRow` 先处理缓存表短路，再尝试当前 `snapshot_rows`。批次耗尽后清空缓存，通过 `next_snapshot_rows` 拉新批次，并逐行调用 `snapshot_row_was_modified`；已修改行不进入候选缓存。若某批全部被遮蔽，会继续拉取，直至得到候选或子扫描结束。
5. 合并得到的行再交给 `evaluate_virtual_columns_and_conditions`。返回 `Some` 才追加到输出；返回 `None` 时继续归并，直到输出达到容量或两侧耗尽。
6. `Close` 清除本地预取和快照状态，先同步关闭新增行迭代器，再把子执行器关闭结果返回给调用方。

## 数据与状态

`added_row` 是新增流的窥视缓存：`getAddedRow` 可被重复调用而不推进迭代器，只有该行被选择输出时 `getOneRow` 才设为 `None`。`snapshot_rows` 与 `snapshot_cursor` 形成批内游标；读取当前快照行只克隆行，不推进游标，只有快照行被选择后才递增。因此比较失败时两侧位置均保持不变，允许调用者观察到未消费状态。

`snapshot_chunk` 在 `open` 中创建并跨批复用。`getSnapshotRow` 用 `expect` 维护“必须先 Open 再 Next”的生命周期不变量。`physical_table_id_column` 在打开时固定，并随每次 `snapshot_row_was_modified` 传给运行时，使分区表可以用物理表 ID 构造正确的记录键。

两个输入必须采用与 `compare_rows` 一致的有序关系，否则逐行归并不能保证全局顺序。相同键时只清除 `added_row`，快照候选本身不在该分支推进；正确的运行时应已通过 `snapshot_row_was_modified` 遮蔽事务修改的快照键。`compareExec.collators` 必须覆盖每个 `used_index`，否则直接索引会越界；这是构建适配层必须保证的不变量。

## 依赖与调用关系

上游设计链是物理 UnionScan 计划到执行器构建：`pkg/executor/builder.rs::buildUnionScanExec` 暂时设置 `encounterUnionScan`，构建 `plan.child_plan()`，再由 `buildUnionScanFromReader` 调用 `ExecutorBuilderDependencies::build_union_scan_from_reader`。当前索引和源码搜索没有发现该接口对本文件 `UnionScanExec<R>` 的生产实例化；已验证的直接调用者都在 `pkg/executor/union_scan_test.rs`。

内部调用边为：`Open -> open_base + open`，`open -> physical_table_id_column + build_added_rows_iterator + new_snapshot_chunk`，`Next -> getOneRow -> getSnapshotRow/getAddedRow`，而 `getSnapshotRow -> reading_cached_table + next_snapshot_rows + snapshot_row_was_modified`，`getAddedRow -> next_added_row`；归并后 `Next -> evaluate_virtual_columns_and_conditions + append_row`；`Close -> close_added_rows_iterator + close_child`。RustCodeGraph 对这些边均给出了本文件内单一调用关系。

下游 crate 依赖是 `astersql-util-chunk`（`pkg/util/chunk` 路径 crate）。事务缓冲、表达式、collation、handle 和具体 reader 并未直接依赖，而由 `UnionScanRuntime` 的实现注入。相比之下，Go 文件直接依赖 `kv.MemBuffer`、表达式、表/编码、collation、statement context 和多种 reader 类型。

## 错误处理与边界

`Open`、`Next`、`getOneRow`、`getSnapshotRow`、`getAddedRow`、`Close` 通过 `?` 原样传播 `R::Error`，本文件不重写错误上下文。打开流程不是事务式回滚：若 `open_base` 成功而 `build_added_rows_iterator` 失败，本文件不会自动调用 `Close`，资源清理由调用方/运行时契约负责。`Close` 中新增行迭代器的关闭无返回值，子执行器关闭错误则会返回。

比较发生在推进任何一侧游标之前；`union_scan_propagates_merge_comparison_errors_without_consuming_rows` 验证比较失败后 `added_row` 仍为原值且 `snapshot_cursor` 仍为 0。快照拉取、遮蔽判断、虚拟列/条件求值的错误也会立即终止本次 `Next`。条件返回 `None` 是正常过滤，不是错误。

边界包括：两侧同时结束返回 `None`；缓存表模式完全不拉快照侧；空快照批被视为流结束；整批均被遮蔽时继续拉下一批；输出容量为 0 时循环不执行。后一点由运行时容量契约控制，本文件没有额外防护。未调用 `Open` 就进入需要拉取快照的 `Next` 会触发 `snapshot_chunk` 的 `expect` panic，而非返回 `R::Error`。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、锁或 `unsafe`；`&mut self` 串行化一次执行器实例的状态推进。是否需要锁住事务内存缓冲由运行时实现负责。Go 版本在 `open` 获取 `MemBuffer` 读锁以建立快照 getter，并在每次 `Next` 期间持有读锁；Rust trait 当前没有把这一具体锁策略编码为类型或显式方法，因此不能从本文件确认等价的生产并发保护。

资源生命周期是 `Open -> 多次 Next -> Close`。`snapshot_chunk` 在打开时分配并复用，`snapshot_rows` 和新增行槽随调用推进，`Close` 清空逻辑缓存并关闭两侧。独立测试 `union_scan_cached_table_uses_only_added_rows_and_close_releases_both_sides` 验证缓存表不会消费快照批次，且关闭时新增行迭代器与子执行器都被释放。`Close` 没有把 `snapshot_chunk` 设回 `None`，重新打开会以新 chunk 覆盖它。

## 与 Go 版本的对应关系

Rust 的 `UnionScanExec::{Open,open,Next,Close,getOneRow,getSnapshotRow,getAddedRow}` 与 `pkg/executor/union_scan.go` 的同名流程逐项对应；`compareExec::compare` 也保持先索引列、再 handle、最后按降序翻转的顺序。两版都让新增行在键相等时胜出，都跳过事务缓冲中已有键的快照行，并把冲突插入留给提交期一致性检查。

Rust 版本把 Go 具体字段和操作压缩为 `UnionScanRuntime` 边界：Go 的 `memBuf/memBufSnap`、条件和虚拟列、各类 reader、handle 构造及 `chunk.MutRow` 均不在本文件实现。Go `open` 会剥掉特定 `SelectionExec`、从事务取 MemBuffer、根据 Table/Index/IndexLookup/IndexMerge/MPP reader 构造不同脏行迭代器，并报告未知 child；这些生产适配逻辑在本 Rust 文件中尚无对应具体实现证据。

Go `Next` 对 MemBuffer 加读锁，逐个计算虚拟列并做类型转换、非空零值修正和 `EvalBool`；Rust 只调用一个合并后的求值钩子。Go `getSnapshotRow` 从行构造 handle 和物理表记录键后查询 `memBufSnap`；Rust 只调用布尔钩子。Go `compareExec` 自带 `handleCols`，Rust 改为由闭包注入 handle 比较。Rust 的 `need_extra_sorting` 与 Go 字段对齐但当前未参与 `compare`。

Go 回归 `pkg/executor/union_scan_test.go` 覆盖普通表/分区表上的删除、更新、索引读取与降序、唯一索引、复合主键、虚拟列、缓存表、并发写快照及性能基准。Rust 独立测试当前聚焦归并、遮蔽、过滤、缓存表、关闭、比较器和比较错误保序，覆盖面小于 Go 集成回归。

## 扩展指南

- 接入生产路径时，应在具体执行器适配层实现 `UnionScanRuntime`，把事务 MemBuffer 快照、reader 拉取、handle/物理表键构造、collation、虚拟列求值和条件过滤连接起来，并确认 `builder.rs::build_union_scan_from_reader` 最终构造本类型；不要把 Go 的这些语义静默省略。
- 增加 reader 类型或全局索引/分区行为时，优先扩展运行时的 `build_added_rows_iterator`、`snapshot_row_was_modified` 和 `physical_table_id_column` 实现；保持脏行流与快照流使用同一比较顺序。
- 修改排序语义时同步检查 `compareExec::compare`、运行时 `compare_rows`、相等键的脏行优先规则，以及 `need_extra_sorting` 的实际使用位置。collator 与 `used_index` 的形状必须由构建阶段验证，避免运行期越界。
- 修改过滤或生成列顺序时，保持“归并后求值”的 Go 语义，并覆盖类型转换、NULL 约束、过滤掉多行后仍填满批次等场景。
- Rust 回归应继续放在独立的 `pkg/executor/union_scan_test.rs`，不可内嵌到生产源文件；生产接线完成后补充与 `pkg/executor/union_scan_test.go` 对齐的表/索引/分区/缓存表集成测试。性能敏感点是逐行克隆、逐行遮蔽查询、整批全遮蔽时的循环，以及比较闭包和虚拟列求值开销。

## 验证依据

- RustCodeGraph 状态：SQLite 索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；通过 `node --file pkg/executor/union_scan.rs` 核对了完整 256 行源码，通过 `query` 核对 `UnionScanRuntime`、`UnionScanExec`、`compareExec`，并通过 `explore`/调用边结果核对内部调用关系。
- 生产与 crate 证据：`pkg/executor/Cargo.toml`（crate 名、`lib.rs` 根、`astersql-util-chunk` 路径依赖）、`pkg/executor/lib.rs:269,545-546`（模块导出与独立测试注册）、`pkg/executor/builder.rs:3325-3351`（计划构建入口及依赖委托）、`pkg/executor/builder.rs:1936-1941`（构建依赖接口）。
- Rust 行为证据：`pkg/executor/union_scan.rs`；独立测试 `pkg/executor/union_scan_test.rs` 的四个测试分别验证比较顺序/降序、归并遮蔽与过滤、缓存表和关闭生命周期、比较错误不消费行。
- Go 对照证据：`pkg/executor/union_scan.go` 的具体执行器全流程；`pkg/executor/union_scan_test.go` 的 `TestUnionScanForMemBufferReader`、`TestIssue53951`、`TestIssue32422`、`TestSnapshotWithConcurrentWrite` 及四个 UnionScan benchmark。
- 仓库搜索 `impl .*UnionScanRuntime` 和 `UnionScanExec<` 仅命中本文件及 `union_scan_test.rs`；这是“尚无生产适配实现”结论的直接证据。未运行 Cargo，符合本纯文档任务约束。
