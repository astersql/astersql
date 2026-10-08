# `pkg/table/tables/mutation_checker.rs`

## 文件定位

本文件属于 `astersql-table-tables` crate；crate 由 `pkg/table/tables/Cargo.toml` 定义，入口 `pkg/table/tables/lib.rs` 通过 `pub mod mutation_checker` 对外公开模块。它提供一组针对“单行变更及其二级索引变更”的内存一致性检查类型和纯函数，设计来源是同目录 Go 实现 `pkg/table/tables/mutation_checker.go`。

当前 Rust 接线需要特别区分两层事实：`Datum` 已被 `index.rs`、`partition.rs` 和 `tables.rs` 作为 crate 内通用值类型使用；但搜索仓库内 Rust 调用点后，`check_data_consistency`、`check_row_insertion_consistency`、`check_handle_consistency`、`check_index_keys`、`compare_index_and_value` 只由独立测试 `mutation_checker_test.rs` 调用，尚未接入 Rust 表写入主链。相对地，Go 的 `CheckDataConsistency` 已由 `tables.go` 的更新、插入和删除路径调用（`tables.go:547,957,1226`）。因此本文件是可执行且有测试的简化校验层，不应描述成已经覆盖 Rust 的每次表写入。

## 核心职责

- `check_data_consistency` 是聚合入口：先执行与 Go 对齐的三个快退条件，再检查行与索引 handle，最后检查索引列值与源行的对应关系。
- `check_handle_consistency` 维护“非删除的索引 PUT 应与行 PUT 使用同一个 handle”的不变量，并识别临时索引 ID 和不会提交的 untouched 值。
- `check_index_keys` 维护“索引 PUT 对应新行、索引 DELETE 对应旧行，索引列顺序按布局映射到行偏移”的不变量。
- `check_row_insertion_consistency` 可单独检查行 value 中已解码出的列值，但与 Go 一样没有从顶层入口主动调用；源码注释说明原因是该检查 CPU 成本较高、对行索引一致性的防护贡献最低。
- `compare_index_and_value` 提供本文件所有值比较的集中语义，包括有符号/无符号整数的数值比较，以及一个简化的多值索引成员比较。

这些职责只验证调用者已经整理好的 `Mutation`、`IndexLayout` 和 `Datum`，本文件不读取事务、MemBuffer、表元数据，也不负责从真实 TiDB key/value 编码中解码 mutation。

## 主要符号

- `Datum::{Null, Int(i64), Uint(u64), Bytes(Vec<u8>)}`：简化的单元格值集合。它实现值相等、哈希和顺序 trait，并被本 crate 的索引、分区和表逻辑共同复用；它不是 Go `types.Datum` 全类型集合。
- `MutationFlags`：记录 `presume_key_not_exists` 与 `untouched`。当前检查逻辑只读取 `untouched`；`presume_key_not_exists` 是数据模型的一部分但在本文件中未参与判断。
- `Mutation`：调用者预处理后的一条 KV 变更，保存原始 `key`/`value`、`index_id`、可选整数 `handle`、已解码的 `indexed_values`，以及行 value 中实际存在的 `(列偏移, Datum)`。其中 `value.is_empty()` 被检查器当作索引删除。
- `IndexLayout`：索引 ID 及其每个索引位置所对应的行列偏移。查找依赖外层 `HashMap` 的 key；结构体字段 `id` 当前不被检查函数读取。
- `ConsistencyError`：区分行值不一致、handle 不一致、索引列值不一致和索引布局缺失。handle 错误携带行/索引 handle 与 index ID；索引值错误携带 index ID 与行偏移。
- `check_data_consistency(...) -> Result<(), ConsistencyError>`：公开顶层入口。
- `check_row_insertion_consistency(...)`、`check_handle_consistency(...)`、`check_index_keys(...)`：公开的三个分项检查入口。
- `compare_index_and_value(...) -> Ordering`：公开比较器；`datum_bytes` 是唯一私有辅助函数，为跨变体回退比较生成字节序列。

本文件没有 trait、条件编译项、全局可变状态或模块级常量；唯一引用的常量是 `index.rs::INDEX_ID_MASK`。

## 执行流程

`check_data_consistency` 的流程如下：

1. 若 `partitioned`、`pipelined` 为真，或 `staging_handle == 0`，立即返回 `Ok(())`。这些条件表示当前入口无法可靠检查相应写入；测试 `top_level_checker_has_go_fast_exit_conditions` 固化了三条分支。
2. 若 `row_insertion` 存在且其 `key` 非空，调用 `check_handle_consistency`。该函数没有直接使用 key 内容，只把非空 key 当作“存在行写入”的顶层门槛。
3. 无论是否存在行写入，都调用 `check_index_keys`；任一分项错误通过 `?` 立即返回。

`check_handle_consistency` 先从行 mutation 读取可选整数 handle；缺失时不检查。随后顺序遍历索引 mutation：空 value（删除）跳过；使用 `mutation.index_id & INDEX_ID_MASK` 去掉临时索引标志；先验证规范化后的 ID 存在于布局表；仅当原始 ID 含临时标志且 `untouched` 为真时跳过；最后在索引 handle 存在且与行 handle 不同时返回 `InconsistentHandle`。布局存在性检查先于 untouched 快退，防止未知临时索引静默绕过元数据校验。

`check_index_keys` 对每条索引 mutation 同样先规范化 ID、查找布局、再处理临时 untouched。它按 value 是否为空选择源行：空 value 对照 `row_to_remove`，非空对照 `row_to_insert`；对应源行缺失时跳过这一 mutation。随后按 `IndexLayout.column_offsets` 枚举索引位置，从源行按行偏移取期望值，从 `indexed_values` 按索引位置取实际值；任一侧越界都用 `Datum::Null` 代替。首次不相等即返回包含 index ID 和行偏移的错误。

`check_row_insertion_consistency` 只遍历 `row_insertion.row_values` 中实际解码出的列，允许被编码层省略的列不出现；若偏移超出输入行或比较不相等，返回 `InconsistentRowValue`。`compare_index_and_value` 对同类整数/字节按值比较，对 Int/Uint 做符号安全的数值比较，对 Null 建立最低顺序；其余跨类型组合退回 `datum_bytes` 的字典序。`compare_multi_value_index` 为真且索引值为 Bytes 时，把索引字节按 NUL 分隔，只要任一成员等于行值字节便返回相等。

## 数据与状态

所有输入均以共享引用或 slice 传入，函数不修改调用者数据。唯一临时分配发生在 `datum_bytes`：整数转换成十进制字节，Bytes 被克隆，Null 产生空 Vec。`check_handle_consistency` 与 `check_index_keys` 都是对 mutation 数量和索引列数量的线性扫描；HashMap 布局查找按通常情形为常数时间。文件不缓存列布局，也没有 Go `getColumnMaps` 那种事务选项缓存。

需要调用者维护的关键表示约定是：

- `index_id` 的低位是真实索引 ID，高位可能含临时索引标志；规范化掩码来自 `index.rs::INDEX_ID_MASK`（`0x0000_ffff_ffff_ffff`）。
- 空 `Mutation.value` 表示删除，非空表示写入；本文件不会进一步解码 value。
- `indexed_values` 必须已经按索引列顺序排列；`column_offsets` 把该顺序映射回源行。
- `row_values` 只列出行 value 中实际编码的列；省略列不是错误。
- 当前 handle 只能表示 `i64`，不能表达 Go `kv.Handle` 的 common handle。

`MutationFlags.presume_key_not_exists` 与 `IndexLayout.id` 目前是保留状态，扩展代码不应假定它们已参与校验。

## 依赖与调用关系

直接代码依赖很小：标准库 `Ordering`、`HashMap`，以及同 crate 的 `index::INDEX_ID_MASK`。`Cargo.toml` 没有为本文件引入专属外部 crate；`mutation_checker` 模块无 feature gate，默认 feature `expression-runtime` 也不改变此文件的编译内容。

RustCodeGraph 将本文件识别为已索引文件（23 个符号），`node --file` 展示了完整 259 行源码。精确 `query` 能唯一定位 `check_data_consistency` 和 `check_row_insertion_consistency`；但 `callers` 命令在本次验证中未返回即持续运行，随后以仓库精确文本搜索补证：除 `mutation_checker_test.rs` 外没有检查函数的 Rust 调用点。已确认的内部调用边为：

- `check_data_consistency` → `check_handle_consistency` → `compare_index_and_value`；
- `check_data_consistency` → `check_index_keys` → `compare_index_and_value`；
- `check_row_insertion_consistency` → `compare_index_and_value`；
- `compare_index_and_value` → `datum_bytes`（多值成员或跨类型回退路径）。

模块级消费者目前主要是 `index.rs`、`partition.rs`、`tables.rs` 对 `Datum` 的复用。Go 对照的应用主链则是 `tables.go` 的 `updateRecord`、`addRecord`、`removeRecord` → `CheckDataConsistency`/`checkDataConsistency`。

## 错误处理与边界

四种 `ConsistencyError` 都是确定性的业务校验结果，没有包装底层解码错误的能力。顶层与分项函数使用 `Result` 和 `?` 在首次错误处短路。`MissingIndex` 在 untouched 临时索引快退之前产生，这是测试明确覆盖的不变量；普通索引即使错误设置 `untouched` 也不能跳过检查。

以下边界会有意返回成功：分区表、流水线 DML、无 staging handle、没有行 handle、索引删除的 handle 检查、缺失对应源行，以及未出现在 `row_values` 中的行列。它们表示“不在这里检查”，不等价于已证明数据一致。

以下行为是当前 Rust 实现特有、扩展时应谨慎复核的边界：源行或索引值越界被当作 Null，而 Go 依赖经过元数据构造的合法布局并直接索引；跨类型比较会使用自定义字节回退，不等价于 Go 的类型上下文与 collation；多值 Bytes 用 NUL 分隔，无法表示成员自身含 NUL 的无歧义编码。由于当前 Rust 写主链尚未调用这些函数，这些限制不会自动被真实事务输入验证。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、staging 区或其他外部资源。函数只在调用栈内借用 slices/HashMap，并在返回时释放短生命周期的 Vec。`staging_handle` 只是 `u64` 有效性标志，检查器既不拥有也不提交/回滚对应 staging 数据。

因此并发安全主要由数据不可变借用保证；同一数据可被多个读者并行检查，只要调用者没有通过其他同步机制外的方式并发修改底层数据。事务隔离、MemBuffer 快照一致性以及 mutation 收集时机均是未来接线层的责任。

## 与 Go 版本的对应关系

Rust 顶层保留了 Go 的主要意图和顺序：分区、pipelined、无 staging 快退；顶层暂不启用高成本行 value 检查；先核对 handle，再核对索引列；临时索引 ID 用掩码恢复；删除索引对照旧行，写入索引对照新行；untouched 临时值不提交所以跳过。Rust 独立测试分别覆盖这些契约，Go 测试 `TestCheckRowInsertionConsistency`、`TestCheckIndexKeysAndCheckHandleConsistency` 和 `TestCompareIndexData` 提供原始行为背景。

Rust 并非 Go 实现的完整一比一移植：

- Go 从 `kv.MemBuffer` staging 区收集并解码真实 mutation，Rust 要求调用者预先构造 `Mutation`；Rust 文件没有 `collectTableMutationsFromBufferStage`、列映射缓存和 failpoint corruption 工具。
- Go 接受 `TableCommon`、事务、类型上下文、真实表/索引元数据，支持 int/common handle、临时索引 value 解码、restored data、新旧 collation、前缀索引截断及额外索引布局；Rust 只接受整数 handle、简化 Datum 和偏移布局。
- Go `CompareIndexAndVal` 的多值索引语义是遍历 JSON 数组并用二进制 collator 比较；Rust 当前使用 NUL 分隔的 Bytes 成员表，只是测试夹具所需的简化表示。
- Go 错误使用带表名、索引名、列名和值的 TiDB 错误码并写日志；Rust 枚举错误只保留最小定位字段，也没有日志副作用。
- Go 顶层已在表写路径运行；Rust 校验函数当前只有测试调用。Rust 的“功能存在”不能作为“主链已启用”的证据。

## 扩展指南

若要把检查器接入 Rust 写主链，最可能修改 `check_data_consistency` 的调用层而不是直接扩大本文件职责：先在独立生产模块中从事务 staging/MemBuffer 收集单表 mutation、解码真实 key/value 和构造布局，再调用本文件。必须同步补充独立测试文件，不能把测试嵌入 `mutation_checker.rs`。

安全扩展建议如下：

- 扩充 `Datum` 前，先检查 `index.rs`、`partition.rs`、`tables.rs` 的穷尽匹配，并同步这些消费者及其独立测试；该枚举并非本文件私有。
- 支持 common handle 时，应替换或抽象 `Option<i64>`，并保留 Go 的规则：只有 handle 类型可比时才比较，不能把编码字节偶然相同当作完整语义。
- 支持真实临时索引时，应在检查前解析当前 value、删除标志和 untouched 状态；仍须保持“先验证索引元数据存在，再允许 untouched 跳过”。
- 对齐前缀索引、collation、时间/JSON 等类型时，应移植 Go 的截断和类型上下文语义，不能继续依赖 `datum_bytes` 字典序作为数据库比较规则。
- 若改变缺失源行或越界列的处理，应新增回归用例明确“无法检查”与“数据损坏”的边界，并评估是否会在热写路径制造误报。
- 性能风险集中在每次写入的线性扫描和比较分配；接入主链前应避免重复构造布局，并评估 `datum_bytes` 的 Vec 分配。Go 注释给出的完整实现复杂度是 `O(M*C + I)`，当前 Rust 已预解码版本更接近 mutation 与索引列总数的线性扫描，但输入准备成本在本文件之外。

应同步维护 `pkg/table/tables/mutation_checker_test.rs`；若涉及真实编码或主链接线，还需扩展对应表写路径的独立 Rust 测试，并继续用 `pkg/table/tables/mutation_checker_test.go` 核对 Go 边界意图。

## 验证依据

- Rust 源码：`pkg/table/tables/mutation_checker.rs`，RustCodeGraph `node --file ... --offset 1 --limit 260` 完整读取 259 行；`query check_data_consistency --kind function` 与 `query check_row_insertion_consistency --kind function` 均唯一命中。
- 模块与 crate：`pkg/table/tables/lib.rs:35,69` 分别声明生产模块和独立测试模块；`pkg/table/tables/Cargo.toml` 确认 crate 名、入口、feature 与 Go package 元数据；该包没有 `doc.go`。
- Rust 调用证据：精确搜索五个公开检查函数的 `(...` 调用，仅命中 `pkg/table/tables/mutation_checker_test.rs`；生产文件 `index.rs`、`partition.rs`、`tables.rs` 只复用 `Datum`。RustCodeGraph 的通用 `explore` 结果因同名符号噪声过多未作为调用结论，精确 `callers` 本次持续运行无结果后中止，故调用结论由精确仓库搜索交叉验证。
- Go 对照：`pkg/table/tables/mutation_checker.go` 的 `CheckDataConsistency`、`checkDataConsistency`、`checkHandleConsistency`、`checkIndexKeys`、`checkRowInsertionConsistency`、`compareIndexData`、`CompareIndexAndVal`、mutation 收集与列映射缓存；`pkg/table/tables/tables.go:547,957,1226` 的真实写路径调用。
- 测试依据：Rust `pkg/table/tables/mutation_checker_test.rs` 覆盖数值比较、部分行列、多值成员、临时索引掩码、handle/布局错误、删除与 untouched、PUT/DELETE 源行选择、缺失源行和顶层快退；Go `pkg/table/tables/mutation_checker_test.go` 覆盖真实编码、时区、common/int handle、collation、前缀截断及行解码错误。
- 本任务是纯文档分析，按总计划不运行 Cargo；完成前另行执行任务规定的 11 章节结构检查并人工复核上述“存在但未接线”边界。
