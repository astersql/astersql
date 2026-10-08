# `pkg/store/mockstore/unistore/cophandler/topn.rs`

源文件：[topn.rs](./topn.rs)

## 文件定位

本文件属于 `astersql-store-mockstore-unistore-cophandler` crate；crate 入口 `pkg/store/mockstore/unistore/cophandler/lib.rs` 以 `pub mod topn` 导出它，`Cargo.toml` 的 `package.metadata.porting.go-package` 指向同目录 Go 包。它为 mock UniStore coprocessor 的下推 `TopN` 算子提供排序键行、有界堆和最终排序，不负责扫描、计划解析或响应编码。

应用内有两条直接接线：`closure_exec.rs::TopNProcessor` 逐行调用 `TopNHeap::add_data_row`，在 `finish` 中消费堆；`mpp_exec.rs::execute_executor` 的 `Executor::TopN` 分支先执行子算子，再将其全部输出送入堆并调用 `into_sorted_rows`。因此本文件位于“子执行器产出行 → 计算 ORDER BY 键 → 有界筛选 → 有序行结果”的中段。

## 核心职责

- `SortRow` 把原始 `Row` 与预先求值的 ORDER BY 键分开保存，避免堆比较时重复执行表达式。
- `TopNSorter` 按 `ByItem` 顺序进行多键比较，处理每一键的升序/降序和 enum 无符号比较，并为最终输出排序。
- `TopNHeap` 维护最多 `total_count` 行的最大堆；根节点始终是当前已接纳集合中最差的一行，新候选只有更优时才替换根。
- `add_data_row` 是从普通执行行进入 TopN 的公开入口；`into_sorted_rows` 消费堆并返回按 ORDER BY 从优到劣排列的原始行。

在 N 固定时，筛选阶段每行至多执行一次键求值并做 `O(log N)` 堆调整，最终对最多 N 行做排序；空间为排序键和完整原始行的 `O(N)` 缓冲。

## 主要符号

- `pub struct SortRow { key: Vec<Datum>, data: Row }`：堆元素。`key[position]` 对应 `order_by_items[position]`；`data` 是最终返回值。
- `pub struct TopNSorter`：持有 `order_by_items`、`rows` 和 `error`。`new` 建立空缓冲，`compare` 比较两行，`sort` 对缓冲执行最终稳定排序；`len`、`is_empty`、`swap` 是容器操作。
- `compare_with(order_by, left, right)`：内部字典序比较函数。逐项比较，遇到首个非相等键即返回；`descending` 反转该项结果；所有项相等则返回 `Ordering::Equal`。
- `enum_unsigned_value`：当 `ByItem::enum_unsigned` 为真时，将简化 `Datum` 映射为 `u64`。`Uint` 保持值，`Int` 用 Rust `as` 保留二进制补码语义，`Real` 使用 bit pattern，`Null`/`Bytes` 映射为 0。
- `pub struct TopNHeap`：组合 `TopNSorter`，并用 `total_count` 表示容量、`heap_size` 表示有效堆长度。
- `TopNHeap::{new, try_to_add_row, add_data_row, into_sorted_rows}`：分别负责构造、接纳已算键行、对普通行求键后接纳、消费并排序输出。
- `TopNHeap::{worse, sift_up, sift_down}`：私有二叉堆操作；“worse”即 ORDER BY 意义上更靠后，因而应更靠近最大堆根。

文件没有 trait、模块级常量或条件编译项；公开结构字段目前可由 crate 外直接访问，调用者需要自行维护 `rows.len() == heap_size`、键数量匹配等不变量。

## 执行流程

1. 调用者用 limit 和 `Vec<ByItem>` 创建 `TopNHeap::new`；内部同时创建空 `TopNSorter`，`heap_size` 为 0。
2. 每个输入 `Row` 进入 `add_data_row`。函数依次执行所有 `ByItem::expr.eval(&row)`，任一表达式失败即返回 `CopError`，该行不会进入堆；全部成功后构造 `SortRow`。
3. `try_to_add_row` 先处理 `total_count == 0`，直接拒绝候选。堆未满时把候选追加到 `rows`、增加 `heap_size`，再由 `sift_up` 恢复“最差行在根”的性质。
4. 堆已满时，将候选与根比较。只有候选的排序结果为 `Ordering::Less`（更优）才覆盖根并 `sift_down`；更差或完全相等的候选被丢弃。堆容量始终不超过 N。
5. 完成输入后，`into_sorted_rows` 消费整个堆，先检查 `sorter.error`，再调用 `TopNSorter::sort`，最后丢弃计算键并抽出原始 `Row`。
6. `TopNProcessor::finish` 因持有 `&mut self`，先用相同容量和排序项的新空堆替换旧堆，再消费旧堆；MPP 路径则直接消费局部堆。

多键比较要求 `SortRow::key` 至少覆盖所有 `ByItem`。正常的 `add_data_row` 保证一项表达式产生一项键；直接调用公开的 `try_to_add_row` 则不受此构造约束。缺键时 `compare_with` 会比较 `Option<&Datum>`，`None < Some`，这只是当前防越界行为，不应视为 SQL 排序契约。

## 数据与状态

`TopNHeap` 的可变状态只有 `sorter.rows`、`sorter.error` 和 `heap_size`；`total_count` 与排序项在正常处理期间保持不变。未满阶段满足 `rows.len() == heap_size <= total_count`，满后只原位替换根，因此仍保持该关系。堆使用零基数组：父节点 `(child - 1) / 2`，子节点 `2 * parent + 1/2`。

`Datum` 与 `ByItem` 定义在 `cop_handler.rs`。常规比较委托给 `Datum::cmp`：NULL 最小，数值类型可交叉比较，字节串按字节字典序，其他跨族按粗粒度类型标签比较。降序仅反转比较结果。相等候选在堆满后不会替换已有行；最终 `slice::sort_by` 是稳定排序，但堆调整已经可能改变早期相等行的相对次序，所以调用者不能依赖 tie 的全局输入顺序。

`SortRow` 同时拥有键和原始行；`TopNProcessor` 还会先克隆 `KvPair::value`，因此宽行或大字节值会增加复制和峰值内存成本。文件不保存扫描游标、事务、时间戳或响应 chunk 状态。

## 依赖与调用关系

上游调用边（由 RustCodeGraph 符号查询和局部 `rg` 交叉核对）：

- `closure_exec.rs::TopNProcessor::process → TopNHeap::add_data_row`；`TopNProcessor::finish → TopNHeap::into_sorted_rows`。
- `mpp_exec.rs::execute_executor(Executor::TopN) → TopNHeap::{new, add_data_row, into_sorted_rows}`。
- 测试调用者为 `topn_test.rs` 和 `cop_handler_test.rs::TestMppExecutor`。

下游依赖：

- `cop_handler.rs::{ByItem, Expr, Datum, Row, CopError}`：表达式求值、值比较、行表示及错误类型。
- 标准库 `std::cmp::Ordering` 与 `Vec` 排序/存储。
- `TopNSorter::sort → compare_with`；`try_to_add_row → TopNSorter::compare → compare_with`；`add_data_row → ByItem::expr.eval`。

目标 crate 的 Cargo 清单未为本文件单列 feature；其内部 `cop_handler` 模块提供上述类型。清单的大量工作区依赖为 optional，不能据此断言 TopN 直接调用它们；本文件源码的直接 import 只有本 crate 类型和标准库。

## 错误处理与边界

- 表达式错误：`Expr::eval` 可能返回列偏移越界或类型错误，`add_data_row` 使用 `?` 原样传播；已经接纳的旧行仍留在堆中，但调用者通常立即中止整个算子。
- 比较错误：Rust 的 `Datum::cmp` 是不可失败的，因此当前 `compare_with` 不会设置 `TopNSorter::error`；`into_sorted_rows` 对该字段的检查是保留的 Go 对齐接口，目前没有本文件内写入路径。
- `limit == 0`：任何行都返回 `Ok(false)`，最终结果为空；`topn_test.rs` 有直接回归。
- 空输入：直接得到空结果。排序项为空时所有行键相等；容量未满可保留前 N 行，满后新行均被拒绝。
- tie：满堆时严格要求新行更优，相同键不替换根；不存在额外 row handle 作为确定性 tie-breaker。
- 浮点、NULL、字节串和混合类型遵循简化 `Datum::Ord`，不是完整 TiDB 类型上下文/排序规则语义。尤其 enum 分支对非预期 `Real`、`Null`、`Bytes` 仍给出数值映射，调用链应保证 `enum_unsigned` 只用于 enum 结果。
- 公共字段和 `try_to_add_row` 允许外部构造键数不匹配或手工改动 `heap_size`；正常生产入口不会这样做，但扩展时不应绕开 `add_data_row`，除非同时维护全部不变量。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、异步任务或 unsafe 代码。所有修改都要求 `&mut self`，因此单个堆在一个执行器调用栈内串行使用；是否跨线程取决于外层所有权，但这里没有共享并发协议。

资源完全由 Rust 所有权管理。`SortRow` 拥有排序键和行；被拒绝或被根替换的候选立即 drop。`into_sorted_rows(self)` 消费堆，确保排序键随 `SortRow` 一并释放，只把原始行移入结果。`TopNProcessor::finish` 的 replacement 技巧使 processor 留在可再次使用的空状态；MPP 的局部堆在返回或错误传播时自动释放。没有显式关闭、回滚或后台清理动作。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/cophandler/topn.go`：`SortRow`/`TopNSorter`/`TopNHeap` 分别对应 Go 的 `sortRow`/`topNSorter`/`topNHeap`；Rust 的 `sift_up`、`sift_down` 内联替代 `container/heap.Push` 和 `heap.Fix`。两版都用反向 Less 语义让“当前最差行”位于堆顶，都在堆满时仅用严格更优候选替换根，最后再按正常 ORDER BY 排序全部保留行。

当前 Rust 是面向 mock 执行器的简化移植，并非 Go 比较栈的完全等价实现：Go `topNSorter.Less` 调用 `types.Datum.Compare`，带 `StatementContext` 的类型上下文及由 protobuf collation 选择的 collator，并可记录比较错误；Go heap 对 enum 根据字段类型调用 `GetUint64`。Rust 用 `ByItem::enum_unsigned` 显式标志和 `Datum::Ord`，没有 statement context、collation 或可失败比较，`error` 因而目前是闲置状态。扩展字符串排序、SQL coercion 或 enum 类型推导时必须重新核对这些差异。

独立 Rust 测试 `topn_test.rs` 验证 enum 的 unsigned 行为、升降序、重复键及零 limit；`cop_handler_test.rs::TestMppExecutor` 验证 TopN 通过 MPP 执行器的降序集成和堆的升序结果。同目录没有专门针对 `topNHeap` 的 Go 单测；Go 行为依据生产实现本身核对，不能把 Rust 测试误称为 Go 测试移植的一一对应。

## 扩展指南

- 新增 ORDER BY 类型或排序规则时，优先修改 `compare_with` 及 `cop_handler.rs::Datum`/`ByItem` 的语义，并同步独立 `topn_test.rs`；字符串 collation、NULL 顺序、NaN、signed/unsigned 边界应各有用例。若目标是 Go 等价，需引入明确的类型/排序上下文，而不是继续扩大 `enum_unsigned_value` 的兜底映射。
- 调整接纳策略、tie-breaker 或 heap 布局时，修改 `try_to_add_row`、`worse`、`sift_up`、`sift_down`，并用输入排列、重复键、N=0/1、N 大于输入数覆盖不变量。不要把测试写回 `topn.rs`；测试应留在同目录 `topn_test.rs`，集成行为同步 `cop_handler_test.rs`。
- 改变公开字段或消费接口时，同时核对 `closure_exec.rs::TopNProcessor` 的 replacement 流程和 `mpp_exec.rs::Executor::TopN` 分支；两条路径都必须保持错误传播与最终有序输出。
- 性能优化应保留“每行键只求值一次”和 `O(N)` 有界内存。若减少宽行克隆，需确认所有权不会让扫描缓冲失效；若改用不稳定排序或并行化，需要明确相等键确定性和比较上下文是否可安全共享。
- `TopNSorter::error` 若继续无写入者可在独立行为变更任务中评估删除；本分析任务不改变接口。若未来比较可失败，应确保堆调整期间的部分状态和最终错误优先级有测试定义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/store/mockstore/unistore/cophandler` 找到目标、Go 对照和独立测试；`node --file .../topn.rs` 读取 1–226 行；`query` 确认 `SortRow`、`TopNSorter`、`TopNHeap` 以及 `try_to_add_row`、`add_data_row`、`into_sorted_rows` 的符号位置与签名。精确 callers/callees 命令未返回边，故再以局部源码搜索核验调用关系。
- 生产源码：`topn.rs`；直接类型定义 `cop_handler.rs::{Datum, Expr, ByItem, CopError, Row}`；直接调用者 `closure_exec.rs::TopNProcessor`、`mpp_exec.rs::execute_executor`；模块入口 `lib.rs`。
- 边界与 crate：`pkg/store/mockstore/unistore/cophandler/Cargo.toml` 的包名、lib 路径和 Go porting metadata。
- Go 对照：`pkg/store/mockstore/unistore/cophandler/topn.go`，核对 sorter、反向 heap Less、严格替换条件、enum 无符号分支及比较错误保存。
- 测试证据：`pkg/store/mockstore/unistore/cophandler/topn_test.rs`；`pkg/store/mockstore/unistore/cophandler/cop_handler_test.rs::TestMppExecutor`。未运行 Cargo，符合本纯文档任务约束。
- 交付验证应执行任务指定命令，确认文件存在且恰有上述 11 个固定二级标题；人工复核重点为真实调用边、简化比较差异、零 limit、tie、错误传播和资源所有权。
