# `pkg/structure/hash.rs`

## 文件定位

本文件实现 `astersql-structure` crate 中 `TxStructure` 的 Hash 视图：一个业务 `key` 下的每个 `field` 都编码成独立 KV 数据键，值保持为任意字节。它不是独立 Rust 模块文件，而是由 [`pkg/structure/lib.rs`](lib.rs) 的私有 `hash_impl` 模块通过 `include!("hash.rs")` 编入；`HashPair`、`ReverseHashIterator` 和两个反向迭代器构造函数随后被 crate 根重导出，`TxStructure` 上的公开方法则天然成为该类型的 API。

crate 边界由 [`pkg/structure/Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-structure`。本文件使用 crate 根注入的 `errors`、`kv` 以及 `TxStructure` 等符号；键编解码来自同 crate 的 [`pkg/structure/type.rs`](type.rs)，读写能力与命名空间前缀来自 [`pkg/structure/structure.rs`](structure.rs)。当前仓库的 RustCodeGraph 和 `rg` 结果只发现 crate 重导出及 `pkg/structure` 独立测试直接调用这些 Rust API，没有发现其他生产 Rust 文件直接调用本实现，因此不应把 Go 侧已有的全部元数据调用链描述成当前 Rust 已接线的生产主链。

## 核心职责

- `HSet`、`HGet`、`HDel`、`HClear` 提供字段级增删改查；缺失字段用 `None` 表示。
- `HInc`、`HGetInt64` 和 `EncodeHashAutoIDKeyValue` 约定整数以十进制 UTF-8 字节存储，供 AutoID 一类计数值使用。
- `HKeys`、`HGetAll`、`HGetIter`、`HGetLen` 基于正向前缀扫描派生集合操作；`HGetLastN` 基于反向扫描取编码序尾部的若干项。
- `IterateHash` 扫描一个业务 key，`IterateHashWithBoundedKey` 扫描业务 key 的半开区间 `[hashStartKey, hashEndKey)`。
- `ReverseHashIterator` 暴露可逐步驱动的反向游标，并在显式关闭或析构时关闭底层 KV 迭代器。

这些职责都建立在 `type.rs` 的键布局之上：`prefix + EncodeBytes(key) + HashData + EncodeBytes(field)`。因此这里所谓“最近”或“最后”不是写入时间，而是编码后的 field 字典序末尾。

## 主要符号

- `HashPair { Field, Value }`：拥有字段和值字节的公开传输对象，派生 `Clone`、`Debug`、`Eq`、`PartialEq`。字段名保留 Go API 的大写命名。
- `TxStructure::HSet` / `HGet` / `HDel` / `HClear`：字段写入、读取、批量删除和整张 Hash 清除。`HDel` 的 Rust 参数是 `&[Vec<u8>]`，对应 Go 的可变参数 `...[]byte`。
- `TxStructure::HInc` / `HGetInt64` / `EncodeHashAutoIDKeyValue`：整数读改写、整数读取及预编码键值。`hashFieldIntegerVal` 是私有十进制编码辅助函数。
- `TxStructure::updateHash`：`HSet` 与 `HInc` 共用的私有读改写模板；先读取旧值，再调用闭包生成新值，相等时避免写放大。
- `TxStructure::IterateHash` / `IterateHashWithBoundedKey`：公开的正向迭代原语；前者回调 `(field, value)`，后者回调 `(key, field, value)`。
- `TxStructure::iterReverseHash`：供 `HGetLastN` 使用的私有反向扫描原语，回调的布尔返回值控制是否继续。
- `ReverseHashIterator`：持有 `&TxStructure`、底层 `kv::Iterator`、Hash 前缀、结束标记和当前解码 field；公开 `Next`、`Valid`、`Key`、`Value`、`Close`。
- `NewHashReverseIter` / `NewHashReverseIterBeginWithField`：公开构造入口；私有 `newHashReverseIter` 负责计算反向扫描起点。

## 执行流程

1. 单字段读取由 `HGet` 调用 `encodeHashDataKey` 构造完整键，再用 `kv::GetValue(Context::todo(), reader, key)` 读取；仅“键不存在”被转换为 `Ok(None)`。
2. 单字段写入由 `HSet` 先拒绝只读快照，再进入 `updateHash`。后者读取旧值、执行更新闭包、比较字节；新旧相同则直接成功，否则通过 `writer()?.Set` 写入。
3. `HInc` 复用同一模板：缺失值以 `0` 起步，已有值先校验 UTF-8 再解析为 `i64`，以 `wrapping_add` 加上步长，最后写回十进制字节并返回新值。
4. `HDel` 对字段逐个编码和读取，只删除存在项；任一步失败就停止，前面已完成的删除不会在本层回滚，原子性由调用者所用的底层事务承担。
5. `IterateHash` 用 `hashDataKeyPrefix(key)` 形成 `[prefix, prefix.PrefixNext())` 扫描区间。每轮再次检查前缀，解码 field，调用用户闭包并推进游标。无论循环正常结束还是闭包/解码/推进报错，退出闭包后都会调用 `iterator.Close()`。
6. `IterateHashWithBoundedKey` 分别编码起止业务 key，扫描半开区间。能解码成 Hash 数据键的条目交给回调；区间内不能按 Hash 数据键解码的条目被跳过，迭代继续。
7. `HGetLastN` 从当前 Hash 前缀上界反向扫描，每取得一项就复制成 `HashPair`；数量达到 `num` 后回调返回 `false` 终止。按当前实现，`num == 0` 仍会在非空 Hash 中先加入一项再停止，这与 Go 对照实现一致，调用方不能把它理解为空结果保证。
8. 独立 `ReverseHashIterator` 从前缀上界开始，或从指定 field 完整键的 `PrefixNext()` 开始反向迭代。构造时不会预填 `field`，因此首次 `Next()` 前 `Key()` 仍为空；底层游标若已有效，`Value()` 可读取当前位置。`Next()` 才推进并解码新的当前 field。

## 数据与状态

`TxStructure` 的 `prefix` 隔离命名空间，`reader` 提供读取和迭代，`readWriter` 是否为 `Some` 决定写能力。Hash 不维护单独的长度或元数据：`HGetLen` 每次完整扫描计数，`HClear` 先收集所有完整数据键再逐个删除。字段和值在公开集合 API 中都复制到拥有所有权的 `Vec<u8>`，避免把底层迭代器缓冲区借出到下一次推进之后。

缺失值与空新值存在有意的 Go 兼容边界。`updateHash` 使用 `oldValue.as_deref().unwrap_or_default() == newValue`，所以“缺失字段 + 空字节新值”被判断为无变化而不写入；一旦字段已有非空值，把它改为空值会继续调用底层 `Set`，测试后端按 KV 契约返回 `ErrCannotSetNilValue`。`HGet` 本身只把真正的 not-found 转成 `None`。

排序完全由编码键顺序决定。正向迭代按业务 key/field 编码序输出；反向迭代相反。`HGetLen` 使用 `u64::wrapping_add`，`HInc` 使用 `i64::wrapping_add`，极端溢出时保持固定宽度整数环绕语义。

## 依赖与调用关系

内部主要调用边经 RustCodeGraph 核对为：

- `HSet -> updateHash -> loadHashValue / writer.Set`；`HInc -> updateHash`；`HGetInt64 -> HGet`。
- `HKeys`、`HGetAll`、`HGetIter`、`HGetLen`、`HClear -> IterateHash`。
- `HGetLastN -> iterReverseHash`。
- `NewHashReverseIter`、`NewHashReverseIterBeginWithField -> newHashReverseIter`；`ReverseHashIterator::Next -> kv::Iterator::Next / decodeHashDataKey`。
- `IterateHash`、`iterReverseHash` 和范围扫描都依赖 `kv::Retriever` 创建的迭代器，并依赖 `type.rs` 的 `hashDataKeyPrefix`、`encodeHashDataKey`、`decodeHashDataKey`。

上游直接证据位于 [`pkg/structure/migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和 [`pkg/structure/structure_test.rs`](structure_test.rs)：前者调用几乎全部 Hash API 验证 Go 迁移语义，后者直接驱动两个反向迭代器并检查只读快照错误。`lib.rs` 只重导出独立类型/函数；`TxStructure` 方法无需逐项重导出。Cargo 依赖中，`kv-crate` 提供 Retriever/Mutator/Iterator 与键类型，`codec-dependency` 支撑相邻 `type.rs` 的有序键编码，`dbterror-dependency` 支撑相邻 `structure.rs` 的结构化错误。

## 错误处理与边界

- `HSet`、`HInc`、`HDel` 显式检查 `readWriter`，只读快照返回 `ErrWriteOnSnapshot`。`HClear` 没有入口处检查：空 Hash 会直接成功，非空 Hash 在第一次 `writer()` 时返回该错误。
- not-found 只有在 `HGet` 和 `loadHashValue` 中被降级为缺失；其他 KV、迭代器、闭包和编解码错误原样通过 `errors::SharedError` 传播。
- 整数值必须同时是 UTF-8 和合法十进制 `i64`；任一步失败，`HInc`/`HGetInt64` 返回新构造的共享错误且不执行本次写入。
- `IterateHash` 遇到本前缀内无法解码的键会失败；`IterateHashWithBoundedKey` 则有意忽略无法解码的条目。扩展键类型时必须保留这一区别。
- 所有回调错误都会终止扫描。正向/反向内部扫描在返回前显式关闭迭代器；独立反向迭代器还由 `Drop` 兜底关闭。
- `newHashReverseIter` 将空 field 与未指定 field 同等处理，从整张 Hash 的末尾开始；不能用空 field 请求一个不同的专用边界。
- `ReverseHashIterator::Key` 在构造后、第一次成功 `Next` 前为空，这是现有 API 状态机的事实；使用者应先按既有约定读取初始 `Value`，推进后再读取已解码 `Key`，或在扩展 API 时同步修正实现与测试。

## 并发与资源生命周期

本文件没有锁、线程、异步任务或通道，也没有在 API 内建立事务。可变写 API 通过 `&mut self` 防止同一个 Rust `TxStructure` 值被安全代码同时写入，但跨克隆后端或多个结构实例的事务隔离与原子性完全由 `kv::RetrieverMutator` 实现保证；`updateHash` 的“读—计算—写”不是本层额外加锁的原子操作。

短生命周期扫描 (`IterateHash`、`IterateHashWithBoundedKey`、`iterReverseHash`) 在函数内创建迭代器并在所有捕获到的成功/失败路径上调用 `Close`。`ReverseHashIterator` 把资源生命周期交给调用者，允许显式 `Close`，并用 `Drop` 再次调用关闭作为兜底；底层实现应允许重复关闭。它借用 `TxStructure`，所以 Rust 类型系统保证结构体不会早于迭代器析构，但底层存储的一致性视图仍由 Retriever 契约决定。

`HClear` 为避免扫描过程中修改同一范围，先把键复制到内存，关闭扫描后才删除；代价是与字段数线性增长的临时内存。`HKeys`、`HGetAll`、`HGetLastN` 同样按结果规模分配内存，`HGetLen` 虽不保留结果但仍是线性扫描。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/structure/hash.go`](hash.go)。Rust 保留了 Go 的公开符号命名、键布局、缺失字段语义、字节相等短路、十进制整数、正反向扫描、范围扫描跳过非法键，以及 `num == 0` 时 `HGetLastN` 仍可能返回一项的现有行为。`migration_aster_unit_test.rs` 的测试名 `hash_crud_integer_and_reverse_iteration_match_go` 与 `key_encoding_and_bounded_hash_iteration_match_go` 明确记录了这类迁移目标。

语言适配差异包括：Go 的 `nil` 用 Rust `Option<Vec<u8>>` 表示；Go 错误的 `errors.Trace` 包装在 Rust 中主要变为 `?` 传播；Go 可变参数删除接口变为切片；Go 整数自然的固定宽度环绕在 Rust 中显式写成 `wrapping_add`。Rust 还加强了资源管理：内部迭代显式关闭，公开反向迭代器实现 `Drop`，而当前 Go `ReverseHashIterator.Close` 是空实现。

反向迭代状态机基本照搬 Go：构造函数定位底层游标但不解码初始 field，故现有 Rust 测试先读取初始 `Value`，调用 `Next` 后才断言 `Key`。Rust 的 `Next` 在读取 `Key` 前额外检查底层 `Valid`，避免游标耗尽时访问无效键；这是安全边界上的语言实现差异，不改变正常序列。

## 扩展指南

- 新增字段级写操作时优先复用 `updateHash`，这样可保留缺失/空值等价、无变化跳过写入和统一错误传播；若新操作需要删除或保留 `None`，应扩展更新协议而不是用空 `Vec` 暗示删除。
- 改动排序、范围或反向起点时，同时检查 `type.rs` 的 `encodeHashDataKey`、`hashDataKeyPrefix` 与 `decodeHashDataKey`。编码格式是持久化兼容边界，不能只修改本文件的扫描逻辑。
- 新增聚合 API 时评估是否可基于 `IterateHash`/`iterReverseHash` 流式完成，避免不必要的全量复制；需要删除时沿用 `HClear` 的“先扫描、后修改”模式。
- 调整整数语义时必须明确 UTF-8、十进制、负数、越界和溢出策略，并同步 Go 对照。不要把 `wrapping_add` 静默改为检查溢出，除非兼容契约也随之改变。
- 改动公开行为应把 Rust 测试放在独立文件，而不是嵌入 `hash.rs`。CRUD/整数/范围扫描优先扩展 `migration_aster_unit_test.rs`；快照错误、反向迭代器状态机和错误码优先扩展 `structure_test.rs`。至少覆盖缺失值、空值、非法整数、回调错误、`num == 0`、空 Hash、field 边界和迭代器关闭。
- 性能风险集中在全量扫描与复制、`HClear` 的两阶段内存、每字段一次 KV 删除，以及 `HDel` 的“先读后删”。若批量化，应先确认底层事务和错误时部分进度的兼容语义。

## 验证依据

- 源码：[`pkg/structure/hash.rs`](hash.rs) 全文；模块装配 [`pkg/structure/lib.rs`](lib.rs)；键格式 [`pkg/structure/type.rs`](type.rs)；`TxStructure` 和写错误 [`pkg/structure/structure.rs`](structure.rs)；crate 清单 [`pkg/structure/Cargo.toml`](Cargo.toml)。
- Go 对照：[`pkg/structure/hash.go`](hash.go) 全文，逐项核对公开 API、内部更新流程、迭代边界和错误分支。
- Rust 独立测试：[`pkg/structure/migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 中 `hash_crud_integer_and_reverse_iteration_match_go`、`key_encoding_and_bounded_hash_iteration_match_go`；[`pkg/structure/structure_test.rs`](structure_test.rs) 中 `TestListAndHashSnapshotWritesFail`、`TestHashNilEquivalentAndReverseIterator`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`explore "pkg/structure/hash.rs TxStructure HSet HGet HInc IterateHash ReverseHashIterator"` 核对了 `HSet/HInc -> updateHash`、集合 API `-> IterateHash`、`HGetLastN -> iterReverseHash`、构造函数 `-> newHashReverseIter` 以及上述测试调用者。图查询未显示本 crate 之外的生产 Rust 调用者，另以 `rg` 对全仓 `.rs` 引用复核。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节、文件存在性、链接路径及事实与源码的一致性。
