# `pkg/planner/util/column.rs`

## 文件定位

`indexInfo2ColsImpl` 一次遍历索引列，根据标志生成“连续可用前缀”和/或“与索引定义等长的完整结果”。

## 核心职责

- `indexCol2Col` 以大小写归一化名称 `IndexColumn.Name.L` 在 `ColumnInfo.Name.L` 中定位同一列，并返回对应表达式列的克隆。
- 若索引声明了正前缀长度，且该长度小于元数据中的完整字段长度，克隆列会设置 `IsPrefix = true`；源 `columns` 切片中的对象不会被修改。
- `indexInfo2ColsImpl` 一次遍历索引列，根据标志生成“连续可用前缀”和/或“与索引定义等长的完整结果”。前缀结果在第一次缺列后永久停止追加，完整结果则在缺列位置保留 `None`。
- 长度值会归一化：索引长度等于表达式列字段完整长度时改为 `-1`，使 `-1` 统一表示“不使用前缀长度”。
- 三个公开函数只是同一实现的视图：`IndexInfo2PrefixCols` 返回连续前缀，`IndexInfo2FullCols` 返回完整槽位，`IndexInfo2Cols` 同时返回两者。

## 主要符号

- `fn indexCol2Col(column_infos, columns, index_column) -> Option<expression::Column>`：内部名称匹配器。命中时按相同下标从 `columns` 取列并克隆；未命中返回 `None`。它隐含要求 `column_infos` 与 `columns` 按位置一一对应且后者不少于前者，否则命中超出 `columns` 长度的元信息时会发生越界 panic。
- `struct IndexInfo2ColsFlags(u8)`：内部位标志包装，不对 crate 外公开。`extractPrefixCols` 为位 1，`extractFullCols` 为位 2；当前没有实现通用位运算 trait，而是在实现内部直接检查 `.0`。
- `type IndexColumns`：四元组别名，顺序固定为“前缀列、前缀长度、完整可选列、完整长度”。该别名是私有的，但作为 `IndexInfo2Cols` 的返回类型出现在公开函数签名中时，其展开类型仍是公开标准容器与公开列类型。
- `fn indexInfo2ColsImpl(...) -> IndexColumns`：核心单遍算法。零标志通过 `assert_ne!` 拒绝；公开包装函数均传入非零标志。
- `pub fn IndexInfo2PrefixCols(...) -> (Vec<Column>, Vec<isize>)`：只请求连续前缀；第一次缺列时可提前结束遍历。
- `pub fn IndexInfo2FullCols(...) -> (Vec<Option<Column>>, Vec<isize>)`：只请求完整视图；缺列记为 `None`，对应长度记为 `-1`，后续列仍继续解析。
- `pub fn IndexInfo2Cols(...) -> IndexColumns`：同时设置两个标志，以一次遍历产生两套严格对齐的结果。

本文件没有 trait、枚举、模块级可变状态或条件编译项。

## 执行流程

1. 公开包装函数选择需要的结果形态，并调用 `indexInfo2ColsImpl`。
2. 实现先断言至少启用一个提取标志，再按需求以 `index.Columns.len()` 预分配向量容量；未请求的结果保持空向量。
3. 对每个 `IndexColumn`，`indexCol2Col` 线性扫描 `column_infos`，按规范化名称找位置，并克隆同位置的表达式列。
4. 若没有匹配列，设置 `prefix_complete = true`。只请求前缀时立即结束；请求完整视图时追加 `None` 与 `-1` 后继续，从而仍能保留缺口后的索引列。
5. 若匹配成功，先取得索引声明长度，再从 `Column.RetType` 读取完整字段长度；`RetType` 缺失时把完整长度视为 `-1`。声明长度与完整长度相等且不是 `-1` 时，将声明长度归一化为 `-1`。
6. 前缀视图只在 `prefix_complete` 尚未置位时追加列和长度；完整视图始终追加 `Some(column)` 和对应长度。
7. 返回四个向量；包装函数丢弃未请求的部分。

因此，对索引列 `[a, b, c]` 和当前 schema 中的 `[a, c]`，前缀结果只有 `[a]`，完整结果为 `[Some(a), None, Some(c)]`；`pkg/planner/util/column_test.rs::test_index_info2_cols` 直接验证了这一不变量。

## 数据与状态

输入均为借用切片或借用元数据，函数不取得所有权，也不写回 `IndexInfo`、`ColumnInfo` 或源表达式列。输出中的列是 `expression::Column::clone()` 产生的值；`IsPrefix` 的修改只落在克隆上。

四个输出向量存在两组位置不变量：每个列向量与同组长度向量等长；完整组每个位置对应原 `index.Columns` 的同一位置，除非调用者根本未请求完整组。前缀组只覆盖从第一个索引列开始、直到首个无法映射列之前的最长连续段。

长度使用 `isize`，并沿用 Go `types.UnspecifiedLength` 的数值约定 `-1`。缺列的完整槽位长度也是 `-1`；调用者必须结合 `Option` 区分“列缺失”与“列存在但无前缀长度”，不能只看长度。时间复杂度为 `O(I × C)`（每个索引列线性搜索所有列元信息），输出空间为 `O(I)`；通常索引列数较小，当前实现没有额外名称映射表。

## 依赖与调用关系

下游依赖只有两类：`model::{ColumnInfo, IndexColumn, IndexInfo}` 提供表/索引元数据，`expression::Column` 提供规划器列及其 `RetType`、`IsPrefix`。名称匹配依赖 `CIStr.L` 的规范化形式；字段长度分别从 `ColumnInfo.FieldType`（决定 `IsPrefix`）与 `Column.RetType`（归一化返回长度）读取。

已核实的 Rust 生产调用边如下：

- `fallback_index_scan_from_table_plan -> planner_util::IndexInfo2PrefixCols`（`base_physical_plan.rs:2898-2928`）：把表扫描回退为索引扫描时填充 `IdxCols` 与 `IdxColLens`。
- `build_lookup_scan_from_logical` 内的回退路径 `-> IndexInfo2PrefixCols`（`base_physical_plan.rs:3307-3347`）：没有现成访问路径时生成 `AccessPath`。
- `source_probe -> IndexInfo2PrefixCols`（`index_join_probe.rs:173-209`）：IndexJoin 探测候选只接受从索引首列开始连续可用的列；空结果会跳过候选。
- 逻辑数据源 common-handle 分支 `-> IndexInfo2FullCols`（`logical_plan_builder_runtime.rs:4806-4837`）：把每个完整槽位转成 `(Column, length)`；遇到 `None` 明确返回 `common-handle column is absent` 错误。

仓库生产 Rust 搜索未发现 `IndexInfo2Cols` 的直接调用；它当前由 `column_test.rs` 用来验证组合结果与两个单独视图一致。RustCodeGraph 的文件关系将 `column.rs` 直接关联到两个 physicalop 文件，但对 `logical_plan_builder_runtime.rs` 中通过依赖别名调用的边未列入文件摘要，因此该边另由精确源码搜索和调用点读取核实。

## 错误处理与边界

该模块不返回 `Result`，预期的“schema 中不存在索引列”通过 `Option` 和前缀截断表达，而不是错误。`IndexInfo2FullCols` 的调用者可自行决定缺列是否可接受；common-handle 调用者选择将其提升为规划错误。

需要注意的硬边界：

- `flags == 0` 会触发 `assert_ne!` panic，但三个公开入口不会构造零标志。
- `column_infos` 和 `columns` 的位置契约未在运行时验证；名称在较长的 `column_infos` 中命中而 `columns[index]` 不存在时会越界 panic。
- 若规范化名称重复，`position` 只取第一个匹配项；正常表元数据应保证列名唯一。
- `RetType == None` 时完整字段长度退化为 `-1`，可能使索引声明 `-1` 保持不变；该分支不会报错。
- `index_column.Length > 0` 且小于 `ColumnInfo.FieldType.GetFlen()` 才设置 `IsPrefix`。零、负值、等于或大于完整长度都不会设置该标志；其中等于完整长度的返回长度还会归一化为 `-1`。
- 本层不校验字符集、字节/字符长度语义或索引长度合法性，这些约束属于元数据建立和更上游的 DDL/规划逻辑。

## 并发与资源生命周期

本文件是无共享状态的同步纯计算辅助：不创建线程、异步任务、锁、通道、事务、文件或网络资源。所有借用只持续到调用返回，局部向量与克隆列由 Rust 所有权自动释放。由于没有全局缓存或内部可变共享状态，同一输入可被多个线程并行调用；实际可发送/共享性仍由输入类型及调用者上下文决定。

性能生命周期集中在单次调用内：按请求结果预分配容量，只有实际输出元素产生克隆；只请求前缀时遇到首个缺列会提前退出。若未来索引列数量或 schema 宽度显著扩大，名称线性查找是首要优化点，但任何缓存或映射都必须保持“首个规范化名称匹配”和位置对齐语义。

## 与 Go 版本的对应关系

关键语义一致：按 `Name.L` 匹配；前缀列用克隆避免修改原列；首个缺列截断连续前缀；完整结果以空槽占位并继续；等于字段完整长度的索引长度归一化为未指定长度。

表示差异主要来自语言：Go 的缺列是 `nil *expression.Column`，Rust 是 `Option<expression::Column>`；Go 的指针切片变成借用切片和拥有值的输出；Go `intest.Assert` 对应 Rust `assert_ne!`；Go `types.UnspecifiedLength` 在 Rust 中直接写为 `-1`。Rust 的 `Column.RetType` 是可选值，因此用 `map_or(-1, ...)` 安全处理缺失类型；Go 版本直接调用 `col.RetType.GetFlen()`，依赖非空指针契约。

`pkg/planner/util/column_test.go::TestIndexInfo2Cols` 是原始行为基线；`pkg/planner/util/column_test.rs::test_index_info2_cols` 保留相同的列缺失场景，并额外显式比较完整槽位的 `Some`/`None` 形状与列等价性。当前两个测试都重点覆盖截断和占位，未单独覆盖正前缀长度、等长归一化、`IsPrefix` 克隆隔离或错位输入 panic。

## 扩展指南

- 若增加新的返回视图，优先扩展 `IndexInfo2ColsFlags` 与 `indexInfo2ColsImpl` 的单遍逻辑，避免三个公开入口出现语义漂移；同时评估四元组是否应升级为具名结构，降低位置误用风险。
- 若改变列匹配规则，应从 `indexCol2Col` 切入，并保持 `ColumnInfo`/`Column` 的位置契约、规范化名称语义和“只修改克隆”的不变量。引入 ID 匹配或哈希映射前，应确认 Go 对照行为及重复名称处理。
- 若改变长度语义，应同时检查 `IsPrefix` 判定与返回长度归一化，因为它们当前分别读取 `ColumnInfo.FieldType` 和 `Column.RetType`；还要审查 `AccessPath.IdxColLens`、`PhysicalIndexScan.IdxColLens`、ranger 对 `-1` 的解释以及 common-handle 长度消费方。
- 测试应继续放在独立文件 `pkg/planner/util/column_test.rs`，不要内嵌到生产源文件。建议补充：短前缀设置 `IsPrefix` 且源列不变、长度等于完整字段长度时返回 `-1`、首列/中间列缺失、空索引、`RetType` 缺失，以及输入切片错位的预期契约。Go 行为发生变化时同步核对 `column.go` 与 `column_test.go`。
- 兼容风险主要是索引访问路径可用列集合或长度哨兵变化，可能改变 range 构造、IndexJoin 候选和 common-handle 解码；性能风险主要来自将当前一次遍历/按需克隆改成重复转换，或为小索引引入高成本映射。
