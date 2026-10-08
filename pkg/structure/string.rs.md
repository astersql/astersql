# `pkg/structure/string.rs`

对应源码：[pkg/structure/string.rs](string.rs)。

## 文件定位

本文件为 `astersql-structure` crate 中 `TxStructure` 的 String 数据结构实现。`pkg/structure/lib.rs` 通过私有模块 `string_impl` 的 `include!("string.rs")` 装配它；方法实现附着在公开的 `TxStructure` 上，所以调用者使用 `TxStructure::{Set, Get, GetInt64, Inc, Iterate, Clear}`，无需也不能直接引用 `string_impl`。

crate 边界由 `pkg/structure/Cargo.toml` 定义：它直接依赖 `astersql-kv`、`astersql-util-codec` 和 `astersql-util-dbterror`。根 workspace 及 `pkg/lib.rs` 的 `facade_structure` 会导出该 crate，`pkg/tablecodec/Cargo.toml` 也依赖并在 `pkg/tablecodec/lib.rs::structure` 中重导出它。不过，对 `NewStructure` 和这些方法的 Rust 调用搜索目前只确认了本 crate 内测试；没有确认 crate 外生产 Rust 路径实际构造 `TxStructure` 并调用本文件 API。因此，本文件是已实现且已测试的迁移基础能力，但不能据此声称它已经接入完整 Rust SQL 运行主链。

## 核心职责

- 将任意字节业务键交给 `TxStructure::EncodeStringDataKey` 编码，在共享 KV 空间中用 `StringData` 类型标志和 `TxStructure::prefix` 隔离 String 数据（`pkg/structure/type.rs`）。
- 提供字节值的写入、读取和幂等删除：`Set`、`Get`、`Clear`。
- 提供十进制 `i64` 读取和原地增减：`GetInt64`、`Inc`。
- 在编码后的半开区间内顺序扫描 String 键，并把解码后的业务键和值交给调用者回调：`Iterate`。
- 统一保留底层 `kv` 错误；仅将“键不存在”转换为 String API 的正常缺失语义。

本文件不保存独立元数据，也不实现过期时间、比较并交换或跨键事务规则；读写能力、事务性和持久化均由构造 `TxStructure` 时注入的 `kv::Retriever`/`kv::RetrieverMutator` 决定。

## 主要符号

- `TxStructure::Set(&mut self, key: &[u8], value: &[u8]) -> Result<(), errors::SharedError>`：编码键，经 `writer()` 取得可写后端，再复制 `value` 为 `Vec<u8>` 写入。空值是否允许由后端 `Mutator::Set` 决定；测试内存后端会拒绝空值。
- `TxStructure::Get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, errors::SharedError>`：经 `kv::GetValue` 读取；`kv::IsErrNotFound` 对应 `Ok(None)`，存在时为 `Ok(Some(value))`，其他错误原样返回。
- `TxStructure::GetInt64(&self, key: &[u8]) -> Result<i64, errors::SharedError>`：复用 `Get`；缺失返回 `0`，存在值必须是有效 UTF-8 且能完整解析为十进制 `i64`。
- `TxStructure::Inc(&mut self, key: &[u8], step: i64) -> Result<i64, errors::SharedError>`：编码键后调用 `kv::IncInt64`。下游在键缺失时以 `step` 初始化；已有值必须是十进制 `i64`，加法使用 `wrapping_add`，写回十进制字节并返回新值（`pkg/kv/utils.rs::IncInt64`）。
- `TxStructure::Iterate<F>(&self, key: &[u8], upperBound: &[u8], function: F) -> Result<(), errors::SharedError>`：扫描编码后的 `[key, upperBound)` 区间，逐项解码键、读取值并调用 `FnMut` 回调。
- `TxStructure::Clear(&mut self, key: &[u8]) -> Result<(), errors::SharedError>`：删除编码键；底层若以 not-found 报错则转为成功，因此对缺失键幂等。

键格式并非在本文件定义。直接依赖的 `pkg/structure/type.rs::{EncodeStringDataKey, decodeStringDataKey}` 使用 `prefix + EncodeBytes(key) + EncodeUint(StringData)`；`StringData` 为字节 `b's'`。`pkg/structure/structure.rs::TxStructure` 持有 `reader`、可选 `readWriter` 和 `prefix`，其 `writer()` 是所有写方法的只读快照保护入口。

## 执行流程

1. 构造方调用 `NewStructure(reader, readWriter, prefix)`。`readWriter = None` 表示只读快照，读取和迭代仍可用。
2. 单键操作先调用 `EncodeStringDataKey`，把命名空间前缀、可排序字节编码和 String 数据类型标志组合成底层 `kv::Key`。
3. `Set`/`Clear` 先通过 `writer()` 验证存在写后端；`Get` 只使用 `reader`。`GetInt64` 是 `Get` 之上的解析层，`Inc` 则把读改写流程委托给 `kv::IncInt64`。
4. `Iterate` 分别编码起止业务键，再调用 `reader.Iter(lower, Some(upper))` 建立正向迭代器。循环仅在 `Valid()` 时执行：`Key()` 取得编码键，`decodeStringDataKey` 恢复业务键，`Value()` 取得值，回调成功后才调用 `Next()`。
5. `Iterate` 将循环包在内部闭包中保存成功或错误结果，随后无条件调用 `iterator.Close()`，最后返回原结果。因此解码、回调或 `Next` 失败也不会跳过显式关闭。

半开区间的顺序由编码保持性和后端迭代器共同保证。调用者必须提供语义上有序的业务边界；本文件不会交换逆序边界，也不会单独验证 `key < upperBound`。

## 数据与状态

本文件没有模块级常量、独立结构体或静态可变状态。所有状态来自 `TxStructure`：

- `prefix: Vec<u8>` 是逻辑命名空间的一部分；相同业务键在不同前缀下映射到不同 KV 键。
- `reader: Box<dyn kv::Retriever>` 服务 `Get` 和 `Iterate`。
- `readWriter: Option<Box<dyn kv::RetrieverMutator>>` 服务 `Set`、`Inc` 和 `Clear`；缺失时写操作失败。
- String 值是未经结构层解释的 `Vec<u8>`。只有 `GetInt64`/`Inc` 施加十进制 `i64` 格式约束。

String 没有像 List/Hash 那样的独立元数据键；一个业务键对应一个带 `StringData` 标志的数据键。`Get` 返回拥有所有权的值，`Iterate` 则把本次迭代取得的解码键和值以切片借给回调，回调若需跨调用保存必须自行复制（现有测试正是这样做的）。

## 依赖与调用关系

上游装配链为 `pkg/structure/lib.rs::string_impl -> include!("string.rs") -> impl TxStructure`；`TxStructure` 和 `NewStructure` 定义在 `pkg/structure/structure.rs`。RustCodeGraph 对目标文件识别出 7 个符号，并确认 `GetInt64 -> Get` 调用边；按目标路径检查其余直接边如下：

- `Set -> EncodeStringDataKey -> writer -> kv::Mutator::Set`
- `Get -> EncodeStringDataKey -> kv::GetValue/kv::IsErrNotFound`
- `GetInt64 -> Get -> str::from_utf8 -> str::parse::<i64>`
- `Inc -> EncodeStringDataKey -> writer -> kv::IncInt64`
- `Iterate -> EncodeStringDataKey -> reader.Iter -> decodeStringDataKey -> callback -> Iterator::Next/Close`
- `Clear -> EncodeStringDataKey -> writer -> kv::Mutator::Delete/kv::IsErrNotFound`

已确认的 Rust 上游主要是独立测试：`pkg/structure/main_test.rs` 验证存储隔离；`pkg/structure/migration_aster_unit_test.rs` 验证 String 往返、迭代、整数更新、清理、快照写错误及键编解码；`pkg/structure/structure_test.rs` 补充缺失整数值与负步长。`migration_aster_unit_test.rs` 还把 `Set` 当作原始 String 写入口，用编码后的 Hash 元数据键构造有界 Hash 扫描夹杂数据。搜索未发现 crate 外生产 Rust 调用者；这项迁移接线限制是当前事实，而非 API 设计限制。

## 错误处理与边界

- `Set`、`Inc`、`Clear` 在只读快照上由 `writer()` 返回注册错误 `ErrWriteOnSnapshot`；相关 Rust 与 Go 测试都覆盖此行为。
- `Get` 把唯一的 not-found 类错误转换为 `None`，不会掩盖其他 reader 错误。`Clear` 对 Delete 的 not-found 做相同归一化。
- `GetInt64` 对缺失键返回 `0`；非 UTF-8、非十进制、超出 `i64` 范围的值会转换为 `errors::New`。当前 structure 测试未直接覆盖这些解析失败分支。
- `Inc` 的非整数值会报错；缺失键以 `step` 初始化。`pkg/kv/utils_test.rs` 覆盖初次写入、累加、非整数失败和超过 `u32::MAX` 的正常增长，但未覆盖跨越 `i64` 边界；实现对 `i64` 溢出显式回绕。
- `Set` 不在结构层拒绝空值；实际结果取决于后端契约。当前测试 `MemoryStore::Set` 返回 `ErrCannotSetNilValue`，因此不能把空字节写入视为跨后端保证。
- `Iterate` 会传播建立迭代器、键解码、用户回调和前进操作中的首个错误，并在返回前关闭迭代器。无效前缀或非 `StringData` 标志由 `decodeStringDataKey` 拒绝；当前直接测试覆盖合法往返，未覆盖 String 解码错误和回调失败。
- API 接受空键、空前缀和任意字节键；编码器负责消除边界歧义。没有本文件级长度限制或 UTF-8 键限制。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务，也不声明 `TxStructure` 的并发共享能力。`&mut self` 使单个安全 Rust 调用点上的写操作互斥借用，但跨对象并发、事务冲突和 `Inc` 的原子性仍取决于注入的 `RetrieverMutator` 实现；不可仅凭本文件推断生产后端的锁语义。

`Set` 和 `Clear` 的临时编码键在调用结束后释放；`Get` 返回拥有的值。`Iterate` 在函数内拥有迭代器，并保证正常及错误路径均显式 `Close()`。回调在迭代器存活期间同步执行；该 API 不缓存所有结果，也不把迭代器泄露给调用者。测试用 `MemoryIterator` 会预先物化范围内容，这只是测试后端行为，不代表生产 Retriever 的内存模型。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/structure/string.go`，六个方法名称、业务顺序和缺失键语义一一对应。`pkg/structure/structure_test.go::TestString` 与 Rust 的 `migration_aster_unit_test.rs::string_round_trip_iteration_and_snapshot_errors_match_go` 覆盖相同主流程：Set、区间 Iterate、Get、Inc、GetInt64、Clear，以及只读快照写失败。

已确认的表示层差异如下：

- Go `Get` 以 `nil, nil` 表示缺失；Rust 用 `Ok(None)`，存在值为 `Some(Vec<u8>)`。
- Go 每个写方法显式检查 `readWriter == nil`；Rust 集中复用 `TxStructure::writer()` 生成 `ErrWriteOnSnapshot`。
- Go `GetInt64` 将字节直接转为 string 后交给 `strconv.ParseInt`；Rust 先要求 UTF-8，再用 `parse::<i64>`。两者都拒绝非十进制有效整数，但错误文本和非 UTF-8 的失败来源不同。
- Go `Inc` 调用 `kv.IncInt64` 后再次归一化 not-found；Rust 下游 `kv::IncInt64` 已在缺失时写入 `step`，所以直接返回结果。Rust 下游以 `wrapping_add` 明确溢出回绕语义。
- Rust `Iterate` 显式保证 `Close()` 在循环结果之后执行；当前 Go 实现没有显式关闭调用。两者均为编码后的半开区间扫描，且在回调错误时立即停止。

Rust 的 `pkg/structure/structure_test.rs::TestStringIntegerAndMissingValues` 额外覆盖缺失 `GetInt64 == 0`、缺失键增量初始化及负步长。Go 侧 `pkg/kv/utils_test.go::TestIncInt64` 和 Rust 侧 `pkg/kv/utils_test.rs::test_inc_int64` 为委托的整数逻辑提供更细边界证据。

## 扩展指南

- 新增单键 String 操作时，必须复用 `EncodeStringDataKey`；写路径必须经 `writer()`，读路径使用 `reader`，以维持前缀隔离和快照写保护。
- 新增范围操作时，应沿用编码边界与半开区间规则，并像 `Iterate` 一样保证所有提前返回路径关闭迭代器。若改变回调生命周期或改成批量物化，需评估大范围扫描的内存与延迟。
- 修改整数语义时需同步检查 `pkg/kv/utils.rs::IncInt64`，并与 `pkg/kv/utils.go`、`pkg/structure/string.go` 对齐；特别关注无键初始化、负数、非法字节、`i64` 边界和错误类型。
- 修改键布局时需同时审查 `pkg/structure/type.rs`、Go 的 `pkg/structure/type.go` 以及 List/Hash 与同前缀扫描的排序兼容性；已有数据的键格式属于持久化兼容边界。
- 测试逻辑应放在独立文件，而不是嵌入 `string.rs`。主行为优先扩展 `pkg/structure/migration_aster_unit_test.rs` 或 `pkg/structure/structure_test.rs`；底层整数行为扩展 `pkg/kv/utils_test.rs`，并同步核对相应 Go 测试。
- 若将该 API 接入新的生产 Rust 子系统，应增加该调用链的独立集成测试，并核实所用后端对 Delete-not-found、空值、迭代器关闭和并发 Inc 的真实保证，不能以当前 `MemoryStore` 替代生产契约。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/structure` 确认 Rust/Go 对照文件及独立测试；`node --file pkg/structure/string.rs` 读取目标 95 行并报告 7 个符号；`query EncodeStringDataKey`、`query DecodeStringDataKey` 定位 `pkg/structure/type.rs`；`callees GetInt64` 确认目标定义调用 `string.rs::Get`；`node IncInt64` 核对 Rust/Go 下游实现。
- 生产源码与配置：`pkg/structure/string.rs`、`pkg/structure/type.rs`、`pkg/structure/structure.rs`、`pkg/structure/lib.rs`、`pkg/structure/Cargo.toml`、`pkg/kv/utils.rs`、根 `Cargo.toml`、`pkg/lib.rs`、`pkg/tablecodec/Cargo.toml`、`pkg/tablecodec/lib.rs`。
- Go 对照：`pkg/structure/string.go`、`pkg/structure/type.go`、`pkg/kv/utils.go`。
- 独立测试：`pkg/structure/main_test.rs`、`pkg/structure/migration_aster_unit_test.rs`、`pkg/structure/structure_test.rs`、`pkg/structure/structure_test.go`、`pkg/kv/utils_test.rs`、`pkg/kv/utils_test.go`。
- 调用检索：对 `NewStructure`、`TxStructure` 及六个方法进行路径限定 `rg`，只确认本 crate Rust 测试中的构造与调用；因此文档明确保留“未确认 crate 外生产调用”的限制。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构命令，并人工复核文件定位、运行流程、安全扩展点及未经验证边界。
