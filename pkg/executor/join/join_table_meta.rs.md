# `pkg/executor/join/join_table_meta.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate，由同目录 `lib.rs` 以 `pub mod join_table_meta` 暴露，是 Hash Join v2 的“构建侧行布局契约”。它不执行完整连接算法，而是把字段类型、连接键、附加条件列、输出列和 used 标志需求归约成 `JoinTableMeta`，并定义行表实际保存的 `EncodedRow`。生产主链由 `hash_join_v2.rs::HashJoinV2::fetch_and_build_hash_table` 和 `restore_and_probe` 创建元数据，再由 `HashTableContext::build` 交给 `row_table_builder.rs` 编码成 `RowTable`。

`pkg/executor/join/Cargo.toml` 声明 crate 名为 `astersql-executor-join`、入口为 `lib.rs`。当前文件本身只使用标准库的 `BTreeSet` 与原子类型；Cargo 中的大量 join 依赖属于整个 crate，而不是此文件的直接依赖。仓库中未找到 `pkg/executor` 或 `pkg/executor/join` 下的 `doc.go`，因此包边界以 `lib.rs`、Cargo 清单和实际调用点为准。

## 核心职责

1. 用 `FieldKind`、`FieldType` 和 `key_property` 把运行时字段粗分成整数、浮点、十进制、字节、带排序规则的文本、日期时间和 JSON，并给出“是否定长、是否必须序列化、能否内联、整数符号性”等键属性。
2. `new_table_meta` 校验构建键/探测键形状与列下标，选择 `KeyMode`，计算定长键字节数，决定可内联键以及最终保存列的稳定顺序，并计算 null map 大小。
3. `EncodedRow` 保存编码字节、独立 null map、键和行数据偏移以及原子 `used` 状态；`JoinTableMeta` 的访问方法统一解释这些字段。
4. 为需要在 probe 后扫描构建表的 outer/semi 类路径提供 used 标志的原子写入与读取。`hash_join_v2.rs::HashTableContext::mark_used` 置位，`unmatched_rows` 用原子读取筛出未命中行。

该文件描述的是当前 Rust 实现，而不是文件前半部被注释掉的移植草稿。第 16 至 364 行的大段注释记录了更接近 Go 裸指针布局的设计，但不会参与编译，不能作为当前已支持行为。

## 主要符号

- `FieldKind`：字段粗分类。`Text` 保留 `collation` 字符串，但当前 `key_property` 对所有 `Text` 一律判为需要序列化，尚未像 Go 那样调用 collator 判断原始内存能否充当 key。
- `FieldType { kind, fixed_length, nullable }`：布局计算输入。当前文件使用 `kind` 和 `fixed_length`；`nullable` 被保存供上层表达类型，但不改变 null map 是否分配。
- `KeyMode::{OneInt64, FixedSerialized, VariableSerialized}`：分别表示单一兼容整数快路径、全定长序列化键和含变长成分的序列化键。
- `KeyProperty`：`key_property` 的归纳结果，包括 `fixed_length`、`requires_serialization`、`can_be_inlined`、`is_integer` 和 `is_unsigned`。
- `EncodedRow`：拥有 `bytes` 和 `null_map`，以 `key_offset/key_length/row_data_offset` 定位数据，以 `AtomicBool` 保存命中状态。自定义 `Clone` 只复制 clone 时刻的 used 快照，之后两个原子变量互不共享。
- `JoinTableMeta`：构建键下标与类型、键模式和定长长度、保存列顺序及大小、null map 长度、used 标志需求和保存列数量的只读布局描述。
- `JoinTableMeta::{serialized_key_length,key_bytes,is_column_null,advance_to_row_data}`：从 `EncodedRow` 读取键、null bit 和行数据偏移。
- `JoinTableMeta::{set_used_flag,is_current_row_used,is_current_row_used_atomic}`：used 状态接口；无需 used 标志时两个读取方法都直接返回 `true`，写方法不做任何事。
- `key_property`：按 `FieldKind` 生成键属性。整数默认 8 字节（可由 `fixed_length` 覆盖）；浮点和日期时间定长但必须序列化；Decimal/Text/Json 为非定长且必须序列化；Bytes 为非定长但允许内联。
- `new_table_meta`：本文件唯一的元数据构造入口，返回 `Result<JoinTableMeta, String>`，把 Go 中部分 panic/隐式前置条件显式化为错误。

## 执行流程

`new_table_meta` 的流程如下：

1. 要求 `build_key_indices`、`build_key_types`、`probe_key_types` 长度相同，并拒绝越界的构建键下标；否则返回字符串错误。
2. 对 build/probe 键分别调用 `key_property`。若构建键是整数而对应探测键不是整数，立即返回错误。
3. 比较整数符号性。符号不同的键需要一个额外 sign flag，因此增加 `fixed_key_length`，也禁止原始内联。
4. 仅当恰有一个整数键且不需要 sign flag 时选择 `OneInt64`；否则所有键都有定长就选择 `FixedSerialized`，任一键变长则选择 `VariableSerialized`。
5. 用 `BTreeSet` 判定构建键下标是否重复。只有键下标唯一、所有键都可内联且没有 sign flag 时，键列才被加入 row data 的最前方。
6. 依次接上 `columns_used_by_other_condition` 和 `output_columns`，借助另一个 `BTreeSet` 去重但保持“第一次出现”的输入优先级；每个下标在加入前都做范围校验。
7. 按 `row_columns_order` 投影 `column_sizes`。普通模式按每 8 个保存列一字节计算 null map；需要 used 标志时额外预留一位，并按 32 位原子访问单位向上对齐到 4 字节。
8. 返回完整元数据。`row_table_builder.rs::encode_row` 随后按保存列位置写 null bit、序列化 key 与列数据并做 8 字节对齐；Hash Join v2 将这些行装入 `HashTableV2`。

主执行链为：`HashJoinV2::fetch_and_build_hash_table`（或 spill 后的 `restore_and_probe`）→ `new_table_meta` → `HashTableContext::build` → `build_row_table`/`RowTableBuilder` → `EncodedRow` → probe 阶段的 `mark_used` 与 `unmatched_rows`。

## 数据与状态

`JoinTableMeta` 是创建后按值传递/克隆的布局快照，没有内部可变性。`build_key_indices` 和 `build_types` 保留输入副本；`row_columns_order[i]` 与 `column_sizes[i]` 必须一一对应，后者的 `None` 表示变长列。`saved_column_count` 等于最终去重后的保存列数量；它而非 build schema 总列数决定 null map 大小。

`EncodedRow.bytes` 的当前生产布局由 `row_table_builder.rs::encode_row` 决定：可选的 used 占位字节、null map、key、row data，最后补齐到 8 字节。与此同时，`EncodedRow.null_map` 又单独保存一份 null 位图，`is_column_null` 读取的是这份独立位图，且参数是“保存列位置”而不是原 schema 列号；`row_table_builder_test.rs::sparse_saved_columns_use_compact_null_map_positions` 明确验证了这一约束。

重要不变量包括：键类型数组长度相等；所有保存列下标有效；`key_offset + key_length` 必须落在 `bytes` 内；`column_sizes.len() == row_columns_order.len()`；需要 used 标志时 `null_map_length` 是 4 的倍数。后两个切片/索引不变量由构造路径维护，但 `EncodedRow` 字段公开，因此外部手工构造错误数据仍可能在 `key_bytes` 中触发越界 panic。

## 依赖与调用关系

上游生产调用只有 `hash_join_v2.rs` 中两处 `new_table_meta`：首次 build 与 spill 分区恢复。两处都从实际 build 行推断 `FieldType`，复用 build 键类型作为 probe 键类型，保存所有 build 列，并根据 `need_scan_row_table_after_probe_done` 决定是否启用 used 标志。

下游直接关系如下：

- `row_table_builder.rs` 接收 `JoinTableMeta`，按 `key_mode` 序列化 key，按 `row_columns_order` 写列数据，使用 `fixed_key_length` 做最大元素估算，并生成 `EncodedRow`。
- `join_row_table.rs`/`hash_table_v2.rs` 容纳和索引编码行；RustCodeGraph 的文件级结果显示目标文件被 24 个 join 源文件或测试文件引用。
- `hash_join_v2.rs::HashTableContext` 持有 `JoinTableMeta`；probe 命中时调用 `set_used_flag`，probe 完成后的未匹配扫描调用 `is_current_row_used_atomic`。
- `join_table_meta_test.rs` 直接覆盖键模式、列顺序、下标错误、null map、偏移和原子 used 状态；`row_table_builder_test.rs` 覆盖元数据被真实编码流程消费时的行为。

当前生产代码未直接调用 `serialized_key_length`、`key_bytes`、`is_column_null`、`advance_to_row_data` 或非 Acquire 的 `is_current_row_used`；其中部分是行布局访问 API 和测试观察面。函数级 `callers/callees` 图查询没有返回边，以上精确调用点由 `rg` 与源码读取核验，文件级依赖由 RustCodeGraph 核验。

## 错误处理与边界

`new_table_meta` 有三类显式错误：键元数据长度不一致、构建键越界、整数 build 键对应非整数 probe 键；保存列（内联键、other condition 或输出列）越界也返回错误。错误类型目前是静态语义的 `String`，调用者使用 `?` 向上传播到 Hash Join v2 的字符串错误通道。

需要注意的边界：空键列表不会命中 `needs_sign_flag[0]`，因为短路条件先检查 `properties.len() == 1`，最终得到定长序列化模式和零长度键；重复键不内联，但仍参与属性与定长长度累计；非内联且未被 other/output 请求的键列不会保存到 row data；空 `output_columns` 在 Rust API 中表示“不保存输出列”，而 Go 的 `nil` 才表示“保存全部列”，Rust 当前没有用同一个切片参数表达 Go 的 nil/非 nil 区别。

`key_bytes` 依赖公开偏移字段的一致性，错误数据会切片越界。`is_column_null` 对超出 null map 的位置返回 `false`，因为使用 `get`；这与 Go 的裸指针直接读取不同。当前 Rust 没有 Go 的 `isReadNullMapThreadSafe`/`isColumnNullThreadSafe` 两套 null map 读取路径，因为 used 状态已从 null map 拆为独立 `AtomicBool`。

## 并发与资源生命周期

该文件不创建线程、任务、锁、通道或事务。并发共享点只有 `EncodedRow.used: AtomicBool`。`set_used_flag` 使用 `Relaxed` store；`is_current_row_used_atomic` 使用 `Acquire` load；`is_current_row_used` 使用 `Relaxed` load。这里的原子值只是单向 false→true 的命中标记，不保护 `bytes` 或 `null_map` 的发布；调用方必须在并发 probe 前完成行构建并通过外部生命周期保证行仍然存在。

`EncodedRow` 拥有所有缓冲区，没有 Go 版本的裸指针、手工偏移解引用或原地修改 null map。行随 `RowTable`/`HashTableV2` 生命周期释放。自定义 `Clone` 会复制字节和 null map，并把当时的 used 值装入新的 `AtomicBool`，因此 clone 不是共享视图；若新增逻辑期望多个副本同步命中状态，应改用共享所有权而不能依赖当前 clone。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/join/join_table_meta.go`，测试对照是 `join_table_meta_test.go`。概念映射为 `joinTableMeta`→`JoinTableMeta`、`keyProp/getKeyProp`→`KeyProperty/key_property`、`newTableMeta`→`new_table_meta`，三种 `keyMode` 也一一对应。两版都保留以下核心规则：单一兼容整数键走 OneInt64；符号混合增加 sign flag 并禁用内联；全定长键走 fixed serialized；变长键走 variable serialized；重复键不内联；保存列按“内联键、other condition、输出列”排序去重；used 模式的 null map 按 4 字节对齐。

Rust 当前不是 Go 结构的逐字段完整复刻。Go 还维护 `isFixedLength/rowLength`、`isJoinKeysFixedLength/isJoinKeysInlined`、`serializeModes`、`columnCountNeededForOtherCondition`、`totalColumnNumber`、`colOffsetInNullMap`、动态 `rowDataOffset` 和 `fakeKeyByte`；Rust 只保留其当前拥有型编码流程实际消费的子集。Go 把 used flag 放进 null map 首 32 位并用 `atomic.Load/StoreUint32`，Rust 使用独立 `AtomicBool`。Go 根据 MySQL 类型与 collation 细分更多类型，Rust 使用较粗的 `FieldKind`，且 `requires_serialization` 没有作为字段保存在 `JoinTableMeta` 中。

还有两个需要避免误读的差异：Go 的 `outputColumns == nil` 表示保存所有列，而 Rust 的生产调用者显式传入完整下标数组；Go 遇到整数 build/probe 类型不匹配会 panic，Rust 返回 `Err`。因此扩展时应以实际 Rust 调用链为准，同时用 Go 测试矩阵检查是否遗漏类型、序列化模式或布局语义，不能把文件顶部的注释草稿当成已经实现的等价层。

## 扩展指南

- 新增字段类别时，先扩展 `FieldKind` 和 `key_property`，明确固定长度、序列化、内联与符号规则，并在 `join_table_meta_test.rs` 增加三种 key mode、混合符号和变长组合用例；同时核对 `row_table_builder.rs::encode_value` 与 `hash_join_v2.rs::infer_field_types` 是否能实际产生和编码该类型。
- 修改行布局时，应同步审查 `new_table_meta`、`row_table_builder.rs::encode_row/serialize_key/calculate_row_data_length`、`hash_table_v2.rs` 和 `HashTableContext`。偏移或 null 位定义必须由 builder 与访问器共同变更，并扩展独立测试文件，不能把测试写回生产源文件。
- 若补齐 Go 的 `serializeModes`、collation 原始内联、fake key 或动态 row-data offset，需先把相应状态加入 `JoinTableMeta`，再贯通 builder/probe 消费点；仅增加字段或复制顶部注释不能形成可用行为。
- 若改变 used 标志内存序，需说明它是否只表示原子布尔状态，还是承担其他行数据的发布/可见性。需要共享 clone 状态时，应重新设计所有权（例如共享原子），并补并发测试。
- 若要表达 Go 的 `nil outputColumns`，Rust API 应使用 `Option<&[usize]>` 或等价显式状态，避免把空切片同时解释为“全部列”和“无输出列”。这属于兼容性接口变化，应同步所有生产调用点与测试。
- 性能风险集中在额外序列化、变长长度前缀、保存列膨胀、BTreeSet 构造开销和更强原子序；正确性风险集中在 build/probe 类型对应、紧凑 null 位下标、公开偏移字段和 Go nil 语义差异。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件、目标文件已索引；`files --filter pkg/executor/join` 列出目标、Go 对照及测试；`query JoinTableMeta`、`query new_table_meta --kind function --json`、`query key_property --kind function --json` 和 `node --file pkg/executor/join/join_table_meta.rs` 确认符号、签名、源码范围及“被 24 个文件使用”的文件级关系。精确 `callers/callees` 未返回函数边，故未据此声称函数级图关系。
- Rust 源码：`pkg/executor/join/join_table_meta.rs`；直接生产调用与生命周期：`hash_join_v2.rs`；编码消费方：`row_table_builder.rs`；模块入口：`lib.rs`；crate 边界：`Cargo.toml`。
- 独立 Rust 测试：`join_table_meta_test.rs` 验证 key mode、列顺序、越界、null map 对齐、符号混合、非请求列与 used 标志；`row_table_builder_test.rs` 验证分区编码、8 字节对齐、紧凑 null 位、恢复边界与短行错误。
- Go 对照：`join_table_meta.go` 的 `joinTableMeta`、`getKeyProp`、`newTableMeta`、`setupJoinKeys`、`setupColumnOrder`；`join_table_meta_test.go` 的键模式、内联/定长、序列化模式、列顺序、null map 与线程安全测试矩阵。
- 精确源码检索确认生产调用：`hash_join_v2.rs` 两处 `new_table_meta`、一处 `set_used_flag`、一处 `is_current_row_used_atomic`；其余访问器仅在独立 Rust 测试中直接调用。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核只新增本说明文档、未修改 Rust/Go/Cargo/`plan.md`。
