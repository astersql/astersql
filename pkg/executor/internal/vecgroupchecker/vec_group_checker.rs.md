# `pkg/executor/internal/vecgroupchecker/vec_group_checker.rs`

## 文件定位

本文件实现 Rust 版 `VecGroupChecker`：它接收一个已经按分组表达式结果有序的 `chunk::Chunk`，把相邻且分组键相等的行切成若干半开区间 `[begin, end)`。这种“只比较相邻行”的前提来自调用方，而不是由检查器排序或验证；若输入中相同键不连续，同一个逻辑分组会被拆成多个区间。

crate 边界由同目录 `Cargo.toml` 定义，包名为 `astersql-executor-internal-vecgroupchecker`，真实实现由 `lib.rs` 的 `mod vec_group_checker; pub use vec_group_checker::*;` 导出。其直接依赖是 Chunk、codec、expression、MySQL 类型常量和 Datum 类型 crate。workspace 根 `Cargo.toml` 纳入该成员，`pkg/lib.rs` 又通过 `pkg::executor::internal::vecgroupchecker` facade 重导出。`pkg/executor`、`aggregate`、`join`、`windows` 的 Cargo 清单声明了依赖，但仓库搜索未发现这些 Rust 生产文件中未注释的 `vecgroupchecker::` 调用；因此当前可确认的是 API、测试与依赖接线已经存在，不能据此宣称 Rust 执行器主链已经实际调用它。

Go 同路径实现明确说明其通常服务于 Stream Aggregation，并且实际调用者还包括窗口分区、Merge Join 和有序 Shuffle。Rust 文件是这些语义的移植实现，但当前应用主链位置应区分“Go 已接线”与“Rust 尚未找到活跃调用”这两个事实。

## 核心职责

1. `SplitIntoGroups` 对当前 Chunk 建立分组边界，并返回当前首组是否延续上一 Chunk 的末组。
2. `getFirstAndLastRowDatum` 对每个 GROUP BY 表达式只求值当前批首行和末行，再编码成可跨 Chunk 比较的组合键。
3. `evalGroupItemsAndResolveGroups` 对每个表达式整列求值，并把任一列发生变化的位置标记为新组起点。
4. `GetNextGroup`、`IsExhausted` 和 `GroupCount` 向消费方提供顺序迭代与计数接口。
5. `Reset` 清理本批状态，同时刻意保留 `lastGroupKeyOfPrevChk`，使下一批仍能判断跨批连续性。

检查器不负责聚合、Join、窗口计算或任务分发，只产出行区间及跨批连续标志。它也不拥有输入 Chunk；首尾 `Datum`、编码键、布尔标记和 offset 都保存在自身状态中。

## 主要符号

- `AllocateBuffer` / `ReleaseBuffer`：临时表达式结果列的函数指针类型。默认分别使用 `expression::GetColumn` 和 `expression::PutColumn`，公开字段允许独立测试注入跟踪器或失败分配器。
- `VecGroupChecker<'ctx>`：核心有状态类型。`ctx` 借用表达式求值上下文；`GroupByItems` 持有表达式；`sameGroup`、`groupOffset` 和游标描述当前批；三个 group-key 字节缓冲与首尾 Datum 支持跨批判断。
- `NewVecGroupChecker(ctx, vecEnabled, items)`：构造并装箱检查器，预留 `sameGroup` 容量 1024，其余缓存为空。它不验证上下文、表达式类型或排序前提，验证延迟到切分时。
- `SplitIntoGroups(&mut self, chk)`：唯一的切分入口，返回 `Result<bool, expression::Error>`；布尔值表示当前首组与上一批末组相同，而不是表示切分成功。
- `getFirstAndLastRowDatum`：按 `EvalType` 调用相应标量求值接口，支持 Int、Real、Decimal、Datetime/Timestamp、Duration、JSON、VectorFloat32 和 String，并把 NULL 显式写为 `Datum::Null`。
- `evalGroupItemsAndResolveGroups`：借出临时列，通过 `expression::EvalExpr(ctx, vecEnabled, ...)` 求值，再按类型比较相邻行；无论求值或比较成功与否，已借出的列都会在返回前交还。
- `GetNextGroup`：根据 `groupOffset` 和 `nextGroupID` 返回下一段并推进游标。
- `IsExhausted`：判断 `nextGroupID >= groupCount`。
- `Reset`：清空当前批缓存和 `groupCount`，但不清空跨批末键，也不修改 `nextGroupID`；`SplitIntoGroups` 会在调用它后把游标置零。
- `GroupCount`：返回当前 Chunk 内区间数量，未扣除与上一 Chunk 连续的首组。

文件没有 trait、枚举、模块级常量或条件编译项。

## 执行流程

`SplitIntoGroups` 的流程如下：

1. 读取 `chk.NumRows()`，调用 `Reset` 清理当前批状态，并将 `nextGroupID` 置零。
2. 若 `GroupByItems` 为空，直接把 `numRows` 作为唯一组的结束 offset，令 `groupCount = 1` 并返回 `true`。这表示无 GROUP BY 时所有批次属于同一逻辑组；即使 Chunk 为空也走该分支。
3. 有分组项但 Chunk 为空时，返回 `VecGroupChecker requires a non-empty chunk`，避免后续读取第 0 行或末行。
4. 对每个表达式调用 `getFirstAndLastRowDatum`，分别收集当前批首行、末行的多列表达式结果。随后必须取得 `ctx`，以其 `Location()` 调用 `codec::EncodeKey` 编码组合键。
5. 若已有 `lastGroupKeyOfPrevChk`，将它与 `firstGroupKey` 比较得到 `firstSameAsPrevious`；然后把当前 `lastGroupKey` 克隆到跨批缓存。这个更新发生在逐行扫描前。
6. 若当前首尾组合键相等，利用“输入已按键连续排列”的前提直接认定整批只有一组，记录末尾 offset 后返回。该快路径避免整列求值。
7. 否则把 `sameGroup` 初始化为全 `true`，再将第 0 行标为 `false`（第一行总是当前批的新组起点）。对每个分组表达式整列求值；只有此前所有列仍认为相同的位置才比较本列，任一列变化都会把该位置永久置为 `false`。
8. 扫描 `sameGroup[1..]`，把所有新组起点作为前一组的结束 offset，最后追加 `numRows`。`groupOffset.len()` 即当前批组数。

消费方应在成功切分后循环执行 `while !IsExhausted() { GetNextGroup() }`。跨 Chunk 合并由上层完成：若返回值为 `true`，当前批第一个区间应接到上一批最后一个逻辑组，而不是重复计为新组。Go 的 `StreamAggExec.consumeOneGroup`、`WindowExec`/`PipelinedWindowExec` 和 `MergeJoinTable` 展示了这种消费模式；Go `partitionRangeSplitter` 则只在单批内轮询区间分配 worker。

## 数据与状态

- `ctx: Option<&dyn EvalContext>` 是借用，不由检查器释放。只要存在分组项，首尾求值和编码都需要它；无分组项路径允许 `None`。
- `GroupByItems: Vec<ExprBox>` 拥有表达式对象。源码为绕开同时可变借用而在两轮循环中克隆这个 Vec；表达式本身通过 trait object 执行。
- `firstRowDatums` / `lastRowDatums` 的顺序与 `GroupByItems` 一致，形成组合键的列顺序。测试 `TestVecGroupCheckerDATARACE` 验证 String、Decimal、JSON 在输入被重置并改写后仍保留原值，说明这些缓存不能借用 Chunk 的可变底层存储。
- `firstGroupKey` / `lastGroupKey` 是当前批的编码键，每次 `Reset` 清空并在编码时复用分配；`lastGroupKeyOfPrevChk` 跨 `Reset` 保留并通过 `clone_from` 独立持有当前末键。
- `sameGroup[i]` 表示第 `i` 行是否与第 `i-1` 行同组。它是各分组列比较结果的逻辑与：某列一旦把位置改成 `false`，后续列不再做该位置的值比较。
- `groupOffset` 保存每个组的排他结束下标，所以第一组为 `[0, groupOffset[0])`，后续组为 `[groupOffset[n-1], groupOffset[n])`；最后一个 offset 恒为 `numRows`。
- `nextGroupID` 是消费游标，`groupCount` 是本批区间数。`GroupCount` 不自动处理跨批去重，测试中的累计公式是 `GroupCount() - usize::from(same_as_previous)`。
- `vecEnabled` 只传给 `expression::EvalExpr` 选择表达式求值路径，不改变分组比较规则。

NULL 规则是：连续 NULL 属于同一组；NULL 与非 NULL 之间产生边界。非 NULL 值按类型比较；Decimal、Time、JSON、Vector 使用各自的 `Compare` 语义，String 先经 `codec::ConvertByCollationStr` 规范化，因此大小写和尾部空格是否等价由字段 collation/长度语义决定。

## 依赖与调用关系

下游依赖可从具体符号核对：

- `chunk::Chunk`/`Row`/`Column`：提供行数、按行标量求值输入以及整列表达式结果。
- `expression::Expression`、`EvalContext`、`EvalExpr`：决定表达式类型并执行首尾标量求值和整列求值。
- `expression::GetColumn` / `PutColumn`：管理临时列池；可注入字段只是测试与资源契约接点。
- `codec::EncodeKey`：把首尾多列 Datum 编成跨批可比较键；`ConvertByCollationStr`：使字符串相邻比较遵守 collation。
- `types::Datum`、`EvalType` 和各种具体值比较函数：保存首尾值并决定行边界。

RustCodeGraph 对 `SplitIntoGroups` 给出的文件内被调边包括 `Reset`、`getFirstAndLastRowDatum` 和 `evalGroupItemsAndResolveGroups`；索引 `node --file` 展示的源码进一步确认 `EncodeKey`、`EvalExpr` 及各类型 getter 的调用。精确 `callers` 命令在本地 SQLite 索引上长时间无结果后被中止，因此上游关系用索引搜索与 `rg` 交叉核验。

Rust 上游当前能确认的只有：`lib.rs` 公共导出、`pkg/lib.rs` facade 重导出、多个 Cargo 依赖声明，以及同 crate 独立测试的直接调用。`pkg/executor/aggregate/agg_stream_executor.rs` 和 `pkg/executor/join/merge_join.rs` 内出现的 `vecgroupchecker` 代码位于注释化的 Go 移植说明中，不是活跃 Rust 调用。`pkg/executor/shuffle.rs` 的活跃 `GroupChecker` 是另一套本地 trait，不能当成本类型调用者。

Go 生产上游已接线且可作为设计意图证据：`aggregate/agg_stream_executor.go` 延续跨 Chunk 聚合组，`windows/window.go` 与 `pipelined_window.go` 切分窗口 partition，`join/merge_join.go` 切分有序 join key，`shuffle.go` 的 range splitter 让同组行进入同一 worker；构造入口还可见于 `pkg/executor/builder.go` 和 `pkg/executor/windows/builder.go`。

## 错误处理与边界

- 有分组项的空 Chunk 返回显式错误；Go 版依赖调用方不传空批并会继续索引首行，Rust 在这里增加了防御性边界。
- 有分组项但 `ctx == None` 时返回 `VecGroupChecker requires an evaluation context`。无分组项路径不访问上下文。
- 不支持的 `EvalType` 在首尾求值阶段或整列比较阶段返回 `unsupported type ... during evaluation`。当前显式支持九类 EvalType，其中 Datetime 与 Timestamp 共用时间比较。
- 表达式标量求值、`EncodeKey`、临时列分配和 `EvalExpr` 的错误均通过 `?` 传播。Rust 对 codec 错误直接转换为 `expression::Error`；Go 版还通过 `ctx.ErrCtx().HandleError` 处理编码错误，两者是否在所有 SQL mode 下完全等价没有由本文件测试验证，扩展错误策略时必须专门核对。
- 临时列成功分配后，主体逻辑放在闭包中，随后无条件调用 `release(column)`，最后再返回闭包结果；因此 `EvalExpr` 或比较错误不会漏还列。若分配本身失败，则没有资源需要释放，错误原样返回。独立测试覆盖了成功借还一次和分配错误传播。
- `GetNextGroup` 不做边界检查；未先成功切分、已经耗尽后继续调用，或状态被外部公开字段破坏时，会因索引 `groupOffset` 越界而 panic。调用者必须先检查 `IsExhausted`。
- 首尾键相同快路径依赖有序且同键连续的不变量。对未排序输入，例如首尾相同而中间不同，它会错误地合并整批；本类型不检测此类违规输入。
- `SplitIntoGroups` 在更新 `lastGroupKeyOfPrevChk` 后才可能进入逐行求值并报错。因此失败后跨批键状态可能已推进到失败批的末键；调用方不应在忽略错误后继续复用检查器。

## 并发与资源生命周期

`VecGroupChecker` 是每条消费流的可变状态机：切分和迭代都需要 `&mut self`，没有锁、原子计数、线程、异步任务或通道。它不设计为多个执行器并发共享；外部即使通过同步包装共享，也必须保证一次 `SplitIntoGroups` 到对应区间消费完成之间不被另一批重置。

生命周期由三层组成：

1. 构造期绑定借用的 `EvalContext` 和拥有的表达式列表。
2. 每批调用 `SplitIntoGroups`，重用 Vec 容量，生成当前批的 Datum、键、标记和 offsets；逐列表达式求值期间从列池借出一个 Column 并在该列比较完成或报错后归还。
3. 跨批仅保留 `lastGroupKeyOfPrevChk`；显式 `Reset` 清空本批数据并令 `groupCount = 0`，但不会断开跨批连续性。若需要开启完全独立的数据流，应新建检查器，或未来增加明确清空跨批键的 API，而不能假设 `Reset` 会完成这一点。

`Reset` 不重置 `nextGroupID`，但由于它把 `groupCount` 置零，`IsExhausted` 仍返回 true；`TestIssue53867` 固化了这一契约。下一次 `SplitIntoGroups` 会显式把 `nextGroupID` 归零。

## 与 Go 版本的对应关系

Rust 的结构字段、构造器、切分两阶段、首尾快路径、NULL 比较、collation 转换、offset 迭代和跨批末键，均直接对应 `vec_group_checker.go` 的同名实现。Rust 独立测试复刻了 Go 测试的 DATARACE、跨 Chunk 组数、字符串 collation/padding 和 Issue 53867 场景，并补充了任意 `Expression`、临时列借还及分配失败传播测试。

需要注意的实现差异：

- Go 假定非空 Chunk；Rust 在有分组项时显式拒绝空 Chunk。无分组项时二者都把整个批次视为一组并返回 `true`。
- Go 为 Decimal、JSON、Vector 和 String 显式执行复制以规避底层 Chunk 复用；Rust 的求值结果被写入拥有型 `Datum`，对应独立性由 `TestVecGroupCheckerDATARACE` 实测覆盖到 Decimal、JSON 和 String，VectorFloat32 尚无同类独立性用例。
- Go 用 `ErrCtx.HandleError` 包装 `EncodeKey` 错误，Rust 直接 `map_err(expression::Error::from)`；错误上下文策略可能存在差异。
- Go 的 Duration 整列路径比较 `GoDurations()`；Rust 通过字段 decimal 调用 `GetDuration` 后比较。两者意图相同，但不同小数精度的完整等价性没有专门测试。
- Go 延迟释放临时列；Rust 用“闭包结果 + 闭包后释放”实现等价的所有返回路径清理。
- Go 的生产调用已经覆盖 stream aggregation、window、merge join 和 shuffle；Rust 当前只确认 Cargo/facade 接线与单元测试，不能把注释化移植代码当作运行时接线。

## 扩展指南

- 新增 `EvalType` 时必须同时修改 `getFirstAndLastRowDatum` 与 `evalGroupItemsAndResolveGroups`：前者决定跨批首尾键，后者决定批内边界；只改一处会导致跨批与批内语义分裂。还要核对 `codec::EncodeKey` 和 `Datum` 是否支持该类型。
- 修改相等性语义时，应优先定位类型分支。字符串必须同步考虑 `ConvertByCollationStr`、大小写、Unicode 与定长 padding；JSON/Vector/Decimal/Time 应复用类型自身比较契约，避免退化为字节或显示文本比较。
- 修改跨批行为时，重点审查 `lastGroupKeyOfPrevChk` 的更新时间、`Reset` 的保留语义和失败后的状态。若增加“开始新流”能力，建议提供单独方法并在独立测试中证明它不会误合并首批。
- 修改资源管理时，保持“成功分配必定恰好释放一次、分配失败不释放、错误不回退到直接读输入列”的契约；同步扩展 `evaluated_columns_are_allocated_and_released` 与 `allocation_errors_are_propagated`。
- 修改公开迭代 API 时，保持 `[begin, end)`、offset 单调递增和末 offset 等于 `NumRows()`；若决定把 `GetNextGroup` 改为安全返回值，需要同步所有未来 Rust 调用者和 Go 对齐说明。
- 性能敏感点包括首尾相等快路径、`sameGroup` 的短路比较、Vec 容量复用、每个表达式一次整列求值，以及当前 `GroupByItems.clone()` 的成本。优化必须保留任意 Expression（不仅是列引用）的语义，测试 `constant_expression_is_evaluated_through_the_expression_contract` 专门约束这一点。
- 测试必须继续放在独立文件 `vec_group_checker_test.rs`，不要内嵌到生产 `.rs`。至少同步覆盖：新类型的 NULL/非 NULL 边界、跨 Chunk 连续与不连续、首尾快路径、错误传播、资源归还以及 Go 对照行为。
- 若把该 crate 接入 Rust 执行器主链，应先选择具体消费者（聚合、窗口、Join 或 Shuffle），增加真实生产调用和对应独立执行器测试；Cargo 中已有依赖声明本身不等于接线完成。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 Rust/Go 源与测试均在索引中。
- RustCodeGraph `files --filter pkg/executor/internal/vecgroupchecker`：确认 `lib.rs`、目标文件、Rust 测试及 Go 对照文件均被索引。
- RustCodeGraph `node --file pkg/executor/internal/vecgroupchecker/vec_group_checker.rs --offset 1 --limit 500`：完整读取目标文件 386 行，核对类型、函数、分支和文件内调用。
- RustCodeGraph `query VecGroupChecker`、`query SplitIntoGroups`、`query split_into_groups`：区分 Rust/Go 同名符号、测试符号以及其他模块的同名函数。精确 `callers SplitIntoGroups --file ...` 在本地 SQLite 后端超过 60 秒未返回并被中止；未把缺失结果推断成“无调用者”。
- 读取 `pkg/executor/internal/vecgroupchecker/Cargo.toml`、`lib.rs`、workspace `Cargo.toml`、`pkg/lib.rs` 及 executor/aggregate/join/windows Cargo 清单，核对 crate、依赖与 facade 边界。
- 读取 `vec_group_checker.go`、`vec_group_checker_test.go`、`vec_group_checker_test.rs` 和 `main_test.rs`，核对 Go 移植语义、边界条件、测试初始化及 Rust 增补用例。
- 搜索 Rust `vecgroupchecker::`、公开符号和 Cargo 依赖，并读取 `pkg/executor/shuffle.rs`、`aggregate/agg_stream_executor.rs`、`join/merge_join.rs` 的相关位置，确认活跃调用与注释化代码的边界。
- 搜索并读取 Go 生产调用位置：`pkg/executor/aggregate/agg_stream_executor.go`、`shuffle.go`、`windows/builder.go`、`windows/window.go`、`windows/pipelined_window.go`、`join/merge_join.go` 与 `builder.go`。

本任务是纯文档分析，按计划不运行 Cargo 或代码测试。结构验收只要求目标文档存在并且恰好包含本页的十一个固定二级标题。
