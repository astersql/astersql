# `pkg/structure/type.rs`

源文件：[`pkg/structure/type.rs`](./type.rs)

## 文件定位

`type.rs` 是 `astersql-structure` crate 的键编码层，不直接读写 KV，而是为 `TxStructure` 提供 String、Hash 和 List 的内部键空间格式。`pkg/structure/lib.rs` 在私有 `type_impl` 模块中通过 `include!("type.rs")` 编入本文件，并以 `pub use type_impl::*` 重导出其公开项。所属 crate 由 `pkg/structure/Cargo.toml` 定义，依赖 `astersql-kv`、`astersql-util-codec` 和 `astersql-util-dbterror`。

`TxStructure` 本体在 `pkg/structure/structure.rs` 中定义：`prefix: Vec<u8>` 是本文件所有编码的命名空间前缀，`reader`/`readWriter` 则由上层 String、Hash、List 方法用编码后的 `kv::Key` 访问。

## 核心职责

- 定义结构类型标志 `TypeFlag = u8` 以及六个稳定字节值：`StringMeta = 'S'`、`StringData = 's'`、`HashMeta = 'H'`、`HashData = 'h'`、`ListMeta = 'L'`、`ListData = 'l'`（`type.rs:24-37`）。大写表示 meta，小写表示 data；这些值是持久化键格式的一部分，不是可随意改名的普通常量。
- 将业务键编码为 `prefix + EncodeBytes(key) + EncodeUint(flag)`；Hash 数据键再追加 `EncodeBytes(field)`，List 数据键再追加 `EncodeInt(index)`（`type.rs:42-47,81-92,133-147`）。
- 对 String/Hash 扫描路径提供逆向解码，使上层迭代器能把底层 KV 键还原为业务 key/field（`decodeStringDataKey`、`decodeHashDataKey`）。
- 为 Hash 单 key 扫描构造不含 field 的 `hashDataKeyPrefix`，让 `HashData` 类型键在编码序上形成可界定范围（`type.rs:124-130`）。

`StringMeta` 在本 crate 当前 Rust 生产路径中没有被消费；`HashMeta` 仅由兼容/测试用的 `EncodeHashMetaKey` 产生。这两点是当前接线事实，不应推导为缺失的运行时能力。

## 主要符号

| 符号 | 可见性 | 语义 |
| --- | --- | --- |
| `TypeFlag` | `pub` | 类型标志的 `u8` 别名。 |
| `StringMeta`, `StringData`, `HashMeta`, `HashData`, `ListMeta`, `ListData` | `pub const` | 持久化键格式中区分结构种类和 meta/data 角色的 ASCII 标志。 |
| `TxStructure::EncodeStringDataKey(&self, key)` | `pub` | 编码 String 数据键；也被 String 范围扫描用作上下界。 |
| `TxStructure::decodeStringDataKey(&self, encoded)` | `pub` | 校验 `prefix` 与 `StringData` 后返回业务键。虽为 `pub`，名称保留 Go 式小写开头。 |
| `TxStructure::EncodeHashMetaKey(&self, key)` | `pub` | 生成历史 Hash meta 键；源码注释明确说明供测试并用于 v5.1 及更早版本。 |
| `TxStructure::encodeHashDataKey(&self, key, field)` | `pub(crate)` | Hash 运行时主编码器。 |
| `TxStructure::EncodeHashDataKey(&self, key, field)` | `pub` | 面向测试/外部的公开包装，直接调用 `encodeHashDataKey`。 |
| `TxStructure::decodeHashDataKey(&self, encoded)` | `pub` | 校验并解码 Hash 数据键，返回 `(key, field)`。 |
| `TxStructure::hashDataKeyPrefix(&self, key)` | `pub(crate)` | 构造 Hash field 集合的公共前缀，用于正向、反向和有界扫描。 |
| `TxStructure::encodeListMetaKey(&self, key)` | `pub(crate)` | 编码 List 的 16 字节索引元数据键。 |
| `TxStructure::encodeListDataKey(&self, key, index)` | `pub(crate)` | 编码 List 元素键，以有符号 `i64` 物理下标作后缀。 |

本文件没有 trait、struct、enum、条件编译项或模块级可变状态；全部行为都是 `TxStructure` 的无副作用键变换方法。

## 执行流程

1. 编码时，方法按预估长度创建 `Vec<u8>`，先复制 `self.prefix`（例如 `EncodeStringDataKey` 的 `type.rs:43-46`）。
2. `codec::EncodeBytes` 对任意二进制业务 key 做可排序、可自我定界的编码，避免 key 内容与后续 flag/field 边界混淆。
3. `codec::EncodeUint` 追加结构标志。String 到此结束；Hash field 键继续 `EncodeBytes(field)`；List 元素键继续 `EncodeInt(index)`。
4. String/Hash 解码时，先以 `starts_with(&self.prefix)` 拒绝其他命名空间的键，然后按与编码完全相反的顺序调用 `DecodeBytes`/`DecodeUint`（`type.rs:50-67,101-121`）。
5. Hash 解码在 flag 校验后继续解出 field。解码方法不要求解码器返回的最终 remainder 为空，因此其保证的是所需部分格式有效，不是“整个输入无额外尾随字节”。

上层执行链为：String API（`string.rs`）→ `EncodeStringDataKey`/扫描时 `decodeStringDataKey`；Hash API 与反向迭代器（`hash.rs`）→ `encodeHashDataKey`/`hashDataKeyPrefix`/`decodeHashDataKey`；List API（`list.rs`）→ `encodeListMetaKey`/`encodeListDataKey`→ `kv::Retriever` 或 `kv::RetrieverMutator`。

## 数据与状态

键格式可概括为：

| 结构 | 持久化键 |
| --- | --- |
| String data | `prefix + EncodeBytes(key) + EncodeUint('s')` |
| Hash meta | `prefix + EncodeBytes(key) + EncodeUint('H')` |
| Hash data | `prefix + EncodeBytes(key) + EncodeUint('h') + EncodeBytes(field)` |
| List meta | `prefix + EncodeBytes(key) + EncodeUint('L')` |
| List data | `prefix + EncodeBytes(key) + EncodeUint('l') + EncodeInt(index)` |

`prefix` 来自 `NewStructure(..., prefix)`，由 `TxStructure` 持有；每次编码都把它复制到新建缓冲区，不会修改 `TxStructure`。`key`、`field` 以字节切片输入并复制进返回键；解码结果是拥有所有权的 `Vec<u8>`。

List 的 meta 值不在本文件编码：`pkg/structure/list.rs::listMeta::Value` 把 `LIndex` 和 `RIndex` 分别写成 8 字节大端数，本文件只负责它的 key。List data key 的 `index` 来自这两个逻辑边界，上层用 wrapping 加减保留 Go `int64` 溢出语义（`list.rs:73-91,107-128`）。

## 依赖与调用关系

- `codec::EncodeBytes`/`DecodeBytes`、`EncodeUint`/`DecodeUint`、`EncodeInt` 由 `astersql-util-codec` 通过 `lib.rs::codec` 重导出，决定键的二进制表示和排序特性。
- `kv::Key` 由 `astersql-kv` 通过 `lib.rs::kv` 重导出，是所有编码方法的返回边界。`hash.rs` 还依赖 `kv::Key::PrefixNext`/`HasPrefix` 把 `hashDataKeyPrefix` 转成扫描范围。
- `errors::New` 包装 codec 解码错误和前缀错误；`ErrInvalidHashKeyFlag` 定义在 `structure.rs`，用于 String/Hash flag 不匹配。
- 直接上游调用者由源码引用确认：`string.rs` 调用 String 编/解码；`hash.rs` 在 CRUD、正反向迭代和有界扫描中调用 Hash 方法；`list.rs` 在 push/pop/index/set/clear 中调用 List 方法。
- crate 外可用 API 是经 `lib.rs` 重导出的六个常量、`TypeFlag` 和 `pub` 方法。全仓 Rust 引用搜索中，`EncodeHashDataKey`、`EncodeHashMetaKey` 的直接外部使用只在本 crate 的 `migration_aster_unit_test.rs`；运行时 Hash/List 路径使用 `pub(crate)` 内部方法。

RustCodeGraph 对 `pkg/structure/type.rs` 识别出 10 个符号，文件节点标示它被 `pkg/structure/migration_aster_unit_test.rs` 使用。精确 `callers/callees` 查询未在当前索引输出边，因此上述具体调用关系另由 `rg` 定位并阅读直接源码确认，没有把空图结果解读为“无调用者”。

## 错误处理与边界

- 编码方法均返回 `kv::Key` 而非 `Result`；内存分配失败之外没有可恢复分支，也不访问底层 KV。
- `decodeStringDataKey` 和 `decodeHashDataKey` 先校验命名空间前缀。两者前缀错误文本都是 `invalid encoded hash data key prefix`（`type.rs:51-53,105-107`）；String 路径的“hash”是与 Go 对照文件一致的历史文本，不应在纯文档任务中擅自修正。
- `DecodeBytes`/`DecodeUint` 的任何失败通过 `errors::New(error.to_string())` 转成 `errors::SharedError`。flag 能解码但不匹配时，使用 `ErrInvalidHashKeyFlag.GenWithStack` 产生数据库错误，消息中包含实际标志字符。
- 编码器允许空 `prefix`、空 key 和空 field；这些都作为普通二进制输入编码。`migration_aster_unit_test.rs::key_encoding_and_bounded_hash_iteration_match_go` 实际使用空 prefix 和空 Hash field，证明这些是支持边界。
- 解码方法接收拥有所有权的 `kv::Key`；校验前缀后才切片，不会因输入比 prefix 短而越界。
- 本文件不校验键是否真实存在、List index 是否处于 meta 边界内，也不处理只读快照写入错误；这些分别属于 `string.rs`/`hash.rs`/`list.rs` 的 KV 访问和 `structure.rs::writer`。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务或迭代器。所有编码方法只借用 `&self` 和输入切片，为每次调用分配独立 `Vec<u8>`，返回的 `kv::Key` 不借用输入生命周期；解码结果同样拥有数据。因此本层没有共享可变状态或额外同步协议。

事务和迭代器生命周期在上层：`TxStructure` 持有 `Box<dyn kv::Retriever>` 及可选 `RetrieverMutator`；`string.rs::Iterate` 和 `hash.rs` 的扫描方法在返回前显式 `Close`，`ReverseHashIterator` 还以 `Drop` 关闭迭代器。这些资源行为依赖本文件产生的范围键，但不由本文件管理。

## 与 Go 版本的对应关系

`pkg/structure/type.go` 是逐符号的直接对照：六个 flag 的字节值、九个 `TxStructure` 方法、编/解码顺序、容量预留量、错误分支和历史错误文本均保持对齐。对应关系为：Go `byte` ↔ Rust `u8`，Go `[]byte` 输入 ↔ Rust `&[u8]`，Go `kv.Key` ↔ Rust `kv::Key(Vec<u8>)`，Go 多返回值错误 ↔ Rust `Result<..., errors::SharedError>`。

Rust 实现中 `kv::Key(...)` 显式包装 codec 返回的字节向量，并用 `map_err` 代替 Go `errors.Trace`；这是类型/错误模型差异，不改变键字节语义。Rust 内部方法使用 `pub(crate)` 来对应 Go 的 package-private 小写方法；`decodeStringDataKey` 和 `decodeHashDataKey` 在 Rust 中形式上是 `pub`，属于可见性差异，但当前 crate 外 Rust 搜索未见生产调用。

Go 回归 `pkg/structure/structure_test.go::TestIterateHashWithBoundedKey` 使用 `EncodeHashMetaKey` 在范围中插入应被跳过的 meta 键，验证 Hash data flag 对有界扫描的分类作用。Rust 对应回归是 `pkg/structure/migration_aster_unit_test.rs::key_encoding_and_bounded_hash_iteration_match_go`，同时验证 String/Hash 编码后可解码往返。

## 扩展指南

- 新增一种结构或 meta/data 类别时，首先分配不冲突的稳定 flag，并在同一变更中对齐 `pkg/structure/type.go`。修改现有 flag 会使旧持久化键无法被新代码识别，必须视为存储格式兼容性变更，而非普通重构。
- 新增编码器时，保持 `prefix -> EncodeBytes(key) -> EncodeUint(flag) -> 类型特有后缀` 的层次，并根据是否跨 crate 使用选择 `pub` 或 `pub(crate)`。若需扫描，同时提供前缀构造和对称解码，不要在调用方手写字节布局。
- 修改解码时要保留三类边界：错误 prefix、截断/非法 codec 输入、可解码但 flag 错误。如果决定拒绝额外尾随字节，这是行为收紧，需要 Go 对齐与兼容性评估。
- 测试应继续放在独立文件，不嵌入 `type.rs`。首选扩展 `pkg/structure/migration_aster_unit_test.rs` 的键编码回归，覆盖空 key/field、任意二进制字节、错误 prefix、错误 flag、截断键和 List 负/边界 index；并与 `pkg/structure/structure_test.go` 的 Go 意图保持对齐。
- 性能上，编码是每次 KV 操作的热路径。调整容量估算、增加临时缓冲或重复编码都应评估分配次数；但不能为减少分配而返回借用临时内存的键，否则会破坏现有所有权边界。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/structure` 列出 Rust/Go 对照及独立测试；`node --file pkg/structure/type.rs --offset 1 --limit 260` 返回全部 148 行、10 个符号与被 `migration_aster_unit_test.rs` 使用的文件边。`query` 分别确认了九个 Rust 方法及其 Go 同名对照。
- Rust 源码：`pkg/structure/type.rs`、`lib.rs`、`structure.rs`、`string.rs`、`hash.rs`、`list.rs`；Cargo 边界：`pkg/structure/Cargo.toml`。直接引用搜索确认 String/Hash/List 的所有上游调用位于这三个实现文件。
- Rust 测试：`pkg/structure/migration_aster_unit_test.rs::hash_crud_integer_and_reverse_iteration_match_go` 覆盖公开 Hash data 键与实际 CRUD/扫描；`key_encoding_and_bounded_hash_iteration_match_go` 覆盖 String/Hash 往返解码、空 field、Hash meta 与有界迭代分类。
- Go 对照：`pkg/structure/type.go`、`string.go`、`hash.go`、`list.go`；Go 测试：`pkg/structure/structure_test.go::TestIterateHashWithBoundedKey`。
- 结构验证命令：`test -f pkg/structure/type.rs.md && test "$(rg -c '^## (文件定位|核心职责|主要符号|执行流程|数据与状态|依赖与调用关系|错误处理与边界|并发与资源生命周期|与 Go 版本的对应关系|扩展指南|验证依据)$' pkg/structure/type.rs.md)" -eq 11`。
- 人工复核要点：文档已解释文件为何存在（统一类型化 KV 键空间）、如何运行（编码/解码顺序及上层调用链）以及如何安全扩展（flag 兼容、Go 对齐、独立回归和性能风险）。本任务是纯文档分析，按计划不运行 Cargo。
