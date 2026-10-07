# `br/pkg/stream/decode_kv.rs`

## 文件定位

`decode_kv.rs` 是 `astersql-br-pkg-stream` crate 中的流备份 KV 事件编解码模块。crate 根 `br/pkg/stream/lib.rs` 通过 `#[path = "decode_kv.rs"] pub mod decode_kv` 装载它，并以 `pub use decode_kv::*` 扁平再导出公开符号，因此同 crate 代码既可写 `crate::decode_kv::NewEventIterator`，外部依赖也可从 crate 根使用这些 API。

该文件处理的是流备份数据文件内部的单条记录边界，而不是 TiDB 行编码或 MVCC 键本身。每条记录的物理布局固定为 `u32 little-endian key_len | key bytes | u32 little-endian value_len | value bytes`；上层 `br/pkg/stream/search.rs::StreamBackupSearch::searchFromDataFile` 在校验文件 SHA-256 后，用这里的迭代器切分记录，再继续解析键尾时间戳、MemComparable 键和 WriteCF 值。

`br/pkg/stream/Cargo.toml` 将该目录声明为库 crate（`[lib] path = "lib.rs"`，porting 元数据的 Go 包为 `br/pkg/stream`）。本文件自身只使用 Rust 标准库，不直接使用 Cargo 中列出的外部依赖。

## 核心职责

1. `EncodeKVEntry` 把一对任意字节 key/value 编成一条带双长度前缀的记录，供流备份数据文件或测试数据拼接。
2. `DecodeKVEntry` 从给定切片开头解出一条记录，并返回 key、value 以及本条记录消耗的字节数；它允许输入切片后面继续存在下一条记录。
3. `EventIterator` 持有整块缓冲区和当前位置，将连续记录逐条暴露为 `Key`/`Value`，并把第一次解码错误保存在迭代器中。
4. `Iterator` trait 固化与 Go `stream.Iterator` 对应的调用协议：调用方以 `Valid → Next → GetError → Key/Value` 循环消费。

该模块不负责文件读取、校验和验证、CF 区分、键比较或业务值解释；这些职责位于 `br/pkg/stream/search.rs` 等上层模块。

## 主要符号

- `pub trait Iterator`：公开的顺序迭代接口。`Next(&mut self)` 推进；`Valid(&mut self) -> bool` 同时承担可继续判断和超大缓冲错误写入，因此需要可变借用；`Key`、`Value` 返回最近一次成功解码结果的借用；`GetError` 返回可选错误字符串借用。
- `pub struct EventIterator`：trait 的当前实现。`buff: Vec<u8>` 拥有输入缓冲，`pos: u32` 是下一条记录的起始偏移，`k`/`v` 保存最近成功记录的独立副本，`err` 保存终止性错误。字段均不公开，状态只能通过 trait 方法观察。
- `pub fn NewEventIterator(buff: Vec<u8>) -> EventIterator`：取得整块缓冲所有权并创建初始迭代器；初始 `pos` 为 0，key/value 为空，错误为空。它返回具体类型而不是 `Box<dyn Iterator>`。
- `impl Iterator for EventIterator`：实现推进、有效性、当前记录访问和错误访问。`Next` 首先再次调用 `Valid`，所以对已结束或已出错迭代器重复调用是无操作。
- `pub fn EncodeKVEntry(k: &[u8], v: &[u8]) -> Vec<u8>`：一次分配目标容量，依次写入两个小端 `u32` 长度和两段原始字节。
- `pub fn DecodeKVEntry(buff: &[u8]) -> Result<(Vec<u8>, Vec<u8>, u32), String>`：从缓冲开头解出一条记录；成功值第三项是 `8 + key_len + value_len`，供 `EventIterator.pos` 累加。

文件没有模块级常量、宏、条件编译项、异步函数或泛型实现。

## 执行流程

编码流程如下：

1. `EncodeKVEntry` 按 `4 + k.len() + 4 + v.len()` 预分配 `Vec<u8>`。
2. 将 `k.len()` 转为 `u32` 并按小端写入，再追加 key 原始字节。
3. 同样写入 value 长度和 value 原始字节，返回完整记录。多条返回值可以直接顺序拼接。

单条解码流程如下：

1. `DecodeKVEntry` 先要求输入至少有 8 字节，以容纳两个长度头。
2. 读取前 4 字节得到 `kLen`，检查输入是否至少包含 `4 + kLen + 4` 字节。
3. 复制 key 字节，随后读取 value 的 4 字节长度头。
4. 检查 value 体没有越过输入结尾，复制 value，并返回 `(key, value, consumed)`。输入中 `consumed` 之后的尾部字节留给下一次解码。

迭代流程如下：

1. 调用方用 `NewEventIterator` 交入整块文件内容；构造时不会预解码第一条记录。
2. `Valid` 在无错误时确认缓冲长度可由 `u32` 表示，并判断 `pos < buff.len()`。
3. `Next` 对 `buff[pos..]` 调用 `DecodeKVEntry`。成功时替换 `k`/`v` 并按返回长度增加 `pos`；失败时仅设置 `err`，不推进位置，也不清空上一条成功结果。
4. 上层应在 `Next` 后立即检查 `GetError`，只有无错误时才消费 `Key`/`Value`。`searchFromDataFile` 正是按这一顺序执行，并将迭代错误转换成其统一 `Error`。
5. 最后一条完整记录使 `pos == buff.len()`；下一次 `Valid` 返回 false，表示正常结束且 `GetError` 仍为 `None`。

## 数据与状态

记录格式只描述字节边界，不解释 key/value 内容。key 和 value 可以为空：空 key 或空 value 的长度头为 0，只要两个长度头完整，解码仍成功。解码成功时返回的 `Vec<u8>` 是输入切片的副本；`EventIterator` 又将这些向量保存为当前记录状态，所以 `Key`/`Value` 的借用不指向 `buff` 的子切片。

`pos` 和记录占用长度使用 `u32`，对应落盘格式的 32 位长度及 Go 实现。正常不变量是：无错误时 `pos` 指向下一条记录开头，且每次成功推进量恰为 `8 + kLen + vLen`。`Valid` 会拒绝总长度超过 `u32::MAX` 的 `EventIterator.buff`，避免把 `usize` 静默截断后与 `pos` 比较。

错误状态是粘性的：一旦 `err` 为 `Some`，`Valid` 永远返回 false，之后 `Next` 不再解码。失败不会回滚或覆盖最近一次成功的 `k`/`v`，因此错误后的 `Key`/`Value` 可能仍显示旧记录，调用方不能把它们视为本次失败的输出。

## 依赖与调用关系

直接下游关系：

- `EventIterator::Next → DecodeKVEntry`：每次推进只解一条记录。
- `DecodeKVEntry` 和 `EncodeKVEntry` 只依赖 `Vec`、切片、`u32::{to_le_bytes,from_le_bytes}`、`TryInto` 和字符串格式化等标准库能力。
- `br/pkg/stream/lib.rs` 负责模块装载与公开再导出；本文件没有访问 `stubs.rs`，也不依赖 `Cargo.toml` 中的 serde、sha2、zstd 等库。

经 RustCodeGraph 核实的主要上游边：

- 生产路径 `br/pkg/stream/search.rs::searchFromDataFile → NewEventIterator`，随后循环调用 `Valid`、`Next`、`GetError`、`Key` 和 `Value`。这里解出的 key/value 会继续参与前缀过滤、时间戳解码和 CF 合并。
- 测试路径 `br/pkg/stream/search_test.rs::fake_data_file → EncodeKVEntry`，用相同布局构造 DefaultCF/WriteCF 文件内容。
- `br/pkg/stream/parity_test.rs::go_rust_public_contract_matches` 直接覆盖编解码往返、消费长度、短输入错误和迭代器超大缓冲拒绝。
- `br/pkg/stream/decode_kv_test.rs` 是最接近的独立 Rust 测试；此外 `rewrite_meta_rawkv_test.rs`、`search_test.rs` 等测试通过迭代器接口或编码器间接依赖该模块。

RustCodeGraph 的文件反向引用还列出 `br/pkg/stream/crr/internal/checkpoint/storage.rs`，但该文件中的 `Iterator` 主要是标准库迭代 trait；不能仅凭同名符号把它当成 `decode_kv::Iterator` 的生产调用者。本文生产主链以明确 import 和调用点的 `search.rs` 为准。

## 错误处理与边界

`DecodeKVEntry` 对三类截断输入返回 `Err("invalid buff")`：总长度少于两个长度头、key 声明长度连同 value 长度头越界、value 声明长度越界。它不会验证尾部是否恰好结束，因为尾部可以是后续记录。`EventIterator::Next` 把该字符串存入 `err`；`searchFromDataFile` 再转成包级 `Error` 返回。

`EventIterator::Valid` 对超过 `u32::MAX` 的总缓冲写入包含实际长度和上限的错误字符串并返回 false。这项检查只存在于迭代器入口；公开的 `DecodeKVEntry` 自身会把 `buff.len()` 转为 `u32`，因此不应直接向它传入超过 `u32::MAX` 的切片。类似地，`EncodeKVEntry` 将 `k.len()`/`v.len()` 直接转换为 `u32`，调用契约要求单段长度可由 `u32` 表示，否则长度前缀会截断。

长度表达式 `8 + kLen` 与 `8 + kLen + vLen` 使用 `u32` 算术。当前代码没有显式的 checked arithmetic；面对恶意构造、接近 `u32::MAX` 的声明长度时，debug/release 下的溢出表现可能不同。扩展解析器时必须把这一点视为输入校验边界，而不能仅依赖切片索引最终失败。

空缓冲使 `Valid` 正常返回 false；仅含完整记录后再附加 1 至 7 个字节时，已完成的记录仍可读取，下一次 `Next` 将记录 `invalid buff`。`decode_kv_test.rs::test_decode_kv_entry_error` 明确覆盖了“合法记录 + 单个脏字节”的行为。

## 并发与资源生命周期

本模块没有线程、任务、锁、通道、文件句柄或事务。`EventIterator` 独占 `Vec<u8>`，状态通过 `&mut self` 串行推进；代码没有显式提供跨线程共享协议。若调用方需要并发消费，应在更高层做数据分片或同步，而不是并发修改同一迭代器。

构造时输入缓冲的所有权转入迭代器，并持续到迭代器析构。每次成功解码都会为 key 和 value 分配并复制数据，下一次成功推进会释放/替换旧向量；通过 `Key`/`Value` 获得的切片借用不能越过对迭代器的下一次可变操作。正常结束和错误结束都不需要显式清理。

性能上，整文件缓冲、每条记录两次复制和 `Vec` 分配是主要资源成本。修改为零拷贝视图会改变 `Iterator` 的生命周期设计及调用方持有语义，属于需要同时调整 trait、实现和测试的接口变更。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/stream/decode_kv.go`，独立测试是 `br/pkg/stream/decode_kv_test.go` 与 `br/pkg/stream/decode_kv_test.rs`。两版共同保持以下语义：

- 相同的双 `u32` 小端长度前缀布局；
- 相同的 `pos: uint32/u32` 推进模型；
- `Next` 先检查有效性，失败后保存错误并停止继续迭代；
- `Valid` 拒绝总缓冲超过 `MaxUint32`；
- 截断记录使用 `invalid buff` 核心文案；
- 调用方先 `Valid`、再 `Next`，并在推进后检查错误。

已验证的实现差异如下：

- Go `NewEventIterator` 返回 `Iterator` 接口并持有调用者切片；Rust 返回具体 `EventIterator` 并取得 `Vec<u8>` 所有权。
- Go 解码出的 key/value 是原缓冲的切片；Rust `DecodeKVEntry` 返回新 `Vec<u8>`，因此每条记录发生复制。
- Go 错误用 `berrors.ErrInvalidArgument` 包装，可参与错误分类；Rust 仅返回或保存 `String`，保留文案但不保留类型身份。
- Go 的 `Valid() bool` 可在内部修改错误是因为接收者为指针；Rust 将这一副作用显式体现在 `Valid(&mut self)`。
- Rust trait 的方法沿用 Go 导出命名，并由 crate 根的 lint allow 接纳；这是迁移期 API 对齐，不是惯用 Rust 命名。

Go/Rust 两份独立测试都覆盖多条拼接记录往返和尾部脏字节错误；Rust 的 `parity_test.rs` 额外覆盖消费长度、短输入与超大缓冲错误文案。

## 扩展指南

- 若改变记录布局、长度宽度或字节序，必须同步修改 `EncodeKVEntry`、`DecodeKVEntry`、`EventIterator::Next` 的位置类型，并确认 TiKV/BR 已有落盘文件兼容；同时更新 Go 对照实现或明确版本化策略。
- 若新增校验（例如 checked addition、单条大小上限或要求无尾部），优先集中在 `DecodeKVEntry`，并在 `decode_kv_test.rs` 增加独立回归：空 key/value、截断 key、缺失 value 长度头、截断 value、极端声明长度和多记录尾部。测试逻辑应继续与 `decode_kv_test.go` 保持一致，Rust 专属安全边界可在 parity 测试补充。
- 若改变迭代协议，需同步审查 `search.rs::searchFromDataFile` 中 `Valid → Next → GetError` 的顺序。尤其不要在失败时让旧 `Key`/`Value` 被误当成新记录。
- 若优化为零拷贝，应评估让 `EventIterator` 保存范围而非 `Vec` 副本，并用借用生命周期表达当前记录；这会影响 trait 对象安全性、上层在推进后复制数据的时机和公开 API 兼容性。
- 若引入结构化错误，应保留上层可添加文件路径上下文的能力，并考虑与 Go `ErrInvalidArgument` 的分类语义对齐，而不只匹配字符串。
- 本仓库要求 Rust 测试与源文件分离；新增回归应放在 `br/pkg/stream/decode_kv_test.rs`，跨模块公开契约才放入 `parity_test.rs`，不要内嵌到 `decode_kv.rs`。

## 验证依据

- RustCodeGraph 状态：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/stream` 确认目标、Go 对照和独立测试均被索引。
- RustCodeGraph 源码节点：`node --file br/pkg/stream/decode_kv.rs` 核对 136 行完整实现及反向引用；`node --file br/pkg/stream/lib.rs` 核对模块装载、测试挂载和公开再导出。
- RustCodeGraph 调用证据：针对 `NewEventIterator`、`EncodeKVEntry`、`DecodeKVEntry`、`EventIterator::{Next,Valid,Key,Value,GetError}` 的查询确认 `searchFromDataFile` 生产调用边，以及 `search_test.rs`、`decode_kv_test.rs`、`parity_test.rs` 的测试调用边。
- crate 边界：`br/pkg/stream/Cargo.toml`；生产入口：`br/pkg/stream/search.rs::searchFromDataFile`；模块入口：`br/pkg/stream/lib.rs`。
- Go 对照：`br/pkg/stream/decode_kv.go`；Go 测试：`br/pkg/stream/decode_kv_test.go`。
- Rust 测试：`br/pkg/stream/decode_kv_test.rs`、`br/pkg/stream/parity_test.rs`，以及使用相同编码布局构造数据文件的 `br/pkg/stream/search_test.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核文档只描述已由上述源码、调用边、Cargo 或对照测试支持的事实。
