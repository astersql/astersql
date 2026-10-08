# `pkg/util/codec/bytes.rs`

## 文件定位

本文件实现 `astersql-util-codec` crate 的字节序列编码基础层，源码由 [`pkg/util/codec/lib.rs`](./lib.rs) 中的私有模块 `bytes_codec` 通过 `include!("bytes.rs")` 纳入，再以 `pub use bytes_codec::*` 对外导出。因此调用方通常使用 `astersql_util_codec::EncodeBytes`，或在本 crate 内直接使用这些符号，而不会引用 `bytes_codec` 模块名。

它位于通用 Datum 编解码的下一层：[`pkg/util/codec/codec.rs`](./codec.rs) 的 `encodeBytes` 根据 `comparable1` 选择本文件的 `EncodeBytes` 或 `EncodeCompactBytes`，`DecodeOne` 和 `Decoder::DecodeOne` 再按 `bytesFlag` / `compactBytesFlag` 调用相应解码函数。由此，本文件同时服务于需要保持键顺序的 key 编码，以及只要求紧凑存储的 value 编码。

crate 边界由 [`pkg/util/codec/Cargo.toml`](./Cargo.toml) 定义，包名为 `astersql-util-codec`，库入口是 `lib.rs`。本文件直接使用的 `errors`、`binary` 和 `EncodeVarint` / `DecodeVarint` 均由 `lib.rs` 的模块作用域注入；没有本文件专属 feature 或可选依赖。

## 核心职责

本文件承担三组职责：

1. 提供升序和降序的 memcomparable 字节编码。`EncodeBytes` 把输入按 8 字节分组，每组追加一个 marker，使编码结果的字典序保持原字节串的升序；`EncodeBytesDesc` 对新追加的编码区间逐字节取反，使字典序变为降序。`DecodeBytes` / `DecodeBytesDesc` 完成逆过程并校验结构。
2. 提供非 memcomparable 的紧凑编码。`EncodeCompactBytes` 写入有符号 varint 长度后追加原数据，`DecodeCompactBytes` 解析长度并借用 payload；它更省空间，但不能用编码后的字节序代表原值顺序。
3. 提供共享的容量规划和按位取反工具。`EncodedBytesLength` 计算精确的 memcomparable 长度，`reallocBytes` 预留追加容量，`reverseBytes` 按目标架构选择实现。

文件本身不添加 `bytesFlag` / `compactBytesFlag`；类型标签由 `codec.rs::encodeBytes` 写入。因此直接调用这些 API 时，调用方必须知道当前字节切片采用哪一种格式。

## 主要符号

- `encGroupSize: usize = 8`、`encMarker: u8 = 0xFF`、`encPad: u8 = 0`：memcomparable 格式常量。`pads` 是一组 8 字节零填充。
- `EncodeBytes(Vec<u8>, &[u8]) -> Vec<u8>`：把升序编码追加到已有 `Vec`。循环条件为 `idx <= data.len()`，所以输入长度恰为 8 的倍数时仍会写入一个全 padding 终止组；空输入也编码为一个 9 字节终止组。
- `EncodeBytesExt(Vec<u8>, &[u8], bool) -> Vec<u8>`：Raw KV 分支直接追加原始数据，否则委托给 `EncodeBytes`。此分支改变的不只是性能，也改变了线格式与可比较性质。
- `EncodedBytesLength(usize) -> usize`：返回 `dataLen + padCount + 1 + dataLen / 8`，与 `EncodeBytes` 的额外终止组规则一致。
- `decodeBytes(&[u8], Option<Vec<u8>>, bool)`：正序/降序解码的共享实现。可接收并复用调用方缓冲区；入口先 `clear`，返回 leftover 和拥有所有权的解码缓冲区。
- `DecodeBytes`、`DecodeBytesDesc`：分别以 `reverse=false` / `true` 调用 `decodeBytes`。
- `EncodeBytesDesc`：只反转调用前 `b.len()` 之后的新编码区间，不破坏已有前缀。
- `EncodeCompactBytes`、`DecodeCompactBytes`：长度前缀编码对。解码结果 payload 和 leftover 都借用输入切片，不复制 payload。
- `wordSize`、`supportsUnaligned`：机器字节数与目标架构选择。`supportsUnaligned` 在 `x86` / `x86_64` 为真，其余架构为假。
- `fastReverseBytes`、`safeReverseBytes`、`reverseBytes`：原地逐字节取反。当前 Rust 快路径通过 `chunks_exact_mut(wordSize)` 分块，但块内仍逐字节处理；它避免了 Go 实现中的 `unsafe` machine-word slice 转换。
- `reallocBytes`：如果剩余容量不足，创建容量恰为 `len + n` 的新 `Vec` 并复制已有前缀；否则原样返回原 `Vec`。

## 执行流程

### 升序 memcomparable 编码

`EncodeBytes` 先按 `(data.len() / 8 + 1) * 9` 为追加区间预留容量，然后从偏移 0 开始逐组处理：完整组原样复制并写 marker `0xFF`；最后一个不完整组复制剩余字节、补零到 8 字节，再写 `0xFF - padCount`。完整组不会终止解码，所以长度为 8 的倍数时必须额外写入 marker 为 `0xF7` 的全零终止组。

这一布局使前缀关系可区分。例如 `b"a"` 的终止 marker 早于 `b"a\0"` 的终止 marker；原始数据相同的前缀不会因补零而合并成同一编码。`EncodedBytesLength` 使用同一分组规则计算结果长度。

### memcomparable 解码与降序变体

`decodeBytes` 每次至少读取 9 字节。正序时 `padCount = 0xFF - marker`，降序时由于整个编码已取反，marker 本身就是原 padding 数。若 padding 数大于 8，则拒绝 marker；随后只把组内真实数据追加到输出缓冲区，并推进输入。

`padCount == 0` 表示还有后续组；非零才是终止组。终止组剩余位置必须全部是正序的 `0x00` 或降序的 `0xFF`，否则返回错误。降序解码在读完全部组后对已收集的数据再次取反，恢复原字节。成功结果保留终止组后的输入为 leftover，支持连续值解码。

`EncodeBytesDesc` 先调用升序编码，再仅对新追加部分执行 `reverseBytes`。因此调用方传入的已有前缀保持不变，编码 payload 的字典序则反向。

### compact 编解码

`EncodeCompactBytes` 先为最大 10 字节 varint 长度和 payload 预留空间，调用 [`pkg/util/codec/number.rs`](./number.rs) 的 `EncodeVarint` 写入非负长度，再追加原数据。`DecodeCompactBytes` 先调用 `DecodeVarint`，拒绝负长度或超过剩余输入的长度，然后按长度切出 payload 与 leftover。该路径不做 payload 拷贝，也不提供排序保证。

在高层路径中，`codec.rs::encodeBytes` 在 comparable 模式前写 `bytesFlag`，否则写 `compactBytesFlag`；`codec.rs::DecodeOne` 根据 flag 分派后构造 Datum，`codec.rs::Decoder::DecodeOne` 则把自身的 `buf` 交给 `DecodeBytes` 并在返回后取回，以减少重复分配。

## 数据与状态

本文件没有可变全局状态。`pads` 是编译期固定的全零数组，常量描述稳定线格式；编码和解码的所有工作状态都局限于参数、局部变量或返回值。

memcomparable 编码的核心不变量是：每个组固定为 8 字节数据加 1 字节 marker；只有 padding 数非零的组终止一个值；marker 精确记录 padding 数；终止 padding 必须取固定值。结果长度恒为 `(floor(dataLen / 8) + 1) * 9`，这也是 `EncodedBytesLength` 与 `codec.rs::sizeBytes` 所依赖的关系。

所有编码函数都保留传入 `Vec<u8>` 的现有前缀并在末尾追加。`DecodeBytes` 返回拥有所有权的 `Vec<u8>`，可通过 `Option<Vec<u8>>` 复用容量；传入缓冲区的旧内容会被清空。`DecodeCompactBytes` 的两个返回切片都与输入同生命周期，调用方不能在它们仍被使用时修改或释放输入。

## 依赖与调用关系

直接下游依赖如下：

- `EncodeCompactBytes` / `DecodeCompactBytes` 调用 `number.rs::EncodeVarint` / `DecodeVarint`；后者负责 ZigZag 风格有符号 varint 及“不足/溢出”错误。
- 错误通过 `lib.rs::errors` 暴露的 `astersql-errors::SharedError`、`New` 和 `Errorf` 构造。
- `binary::MaxVarintLen64` 由 `lib.rs` 中的兼容模块提供，用于保守容量预留。

直接上游调用以 `codec.rs` 为核心：`encodeBytes` 调用两种编码；顶层 `DecodeOne` 调用两种解码；带 Chunk 的 `Decoder::DecodeOne` 复用 `DecodeBytes` 缓冲区。RustCodeGraph 还把 `bytes.rs` 标记为被 34 个文件使用。局部调用搜索可见代表性生产路径包括：

- [`pkg/meta/reader.rs`](../../meta/reader.rs) 用 `EncodeBytes` / `DecodeBytes` 组装和解析元数据键。
- [`pkg/session/runtime/crossks_session_pool.rs`](../../session/runtime/crossks_session_pool.rs) 与 `system_session.rs` 用它编码系统元数据 key。
- [`br/pkg/restore/split/client.rs`](../../../br/pkg/restore/split/client.rs) 和 `split.rs` 通过 `EncodeBytesExt` 在事务 KV 与 Raw KV 边界间选择格式。
- [`br/pkg/restore/snap_client/import.rs`](../../../br/pkg/restore/snap_client/import.rs)、`br/pkg/restore/utils/rewrite_rule.rs` 使用正序编码处理 region 边界与 rewrite key。

这些调用说明本文件不是独立工具函数集合：memcomparable 线格式参与元数据、region 范围、恢复与分裂键的跨组件协议。修改格式会影响 Rust/Go 互操作、持久化键以及字典序范围判断。

## 错误处理与边界

`decodeBytes` 显式拒绝三类畸形输入：剩余长度不足一个 9 字节组；marker 推导出的 padding 数大于 8；终止组 padding 字节不是规定值。连续完整组若始终没有合法终止组，最终也会因输入不足而失败。错误消息分别包含“不足”、无效 marker 或无效 padding 的上下文。

`DecodeCompactBytes` 传播 `DecodeVarint` 的长度前缀不足或超过 64 位错误，并额外拒绝负长度和声明长度大于剩余输入的情况。长度转换到 `usize` 只发生在这些检查之后。成功解码允许存在 leftover，这是连续字段协议所需行为，不应被当作错误。

空输入是合法值：memcomparable 形式为 8 个零加 marker `0xF7`，compact 形式为长度零的 varint。恰好 8 字节的输入会多一个终止组。`EncodeBytesExt(..., true)` 对空输入不追加任何字节，这与非 Raw KV 分支的 9 字节结果有意不同。

本 API 不限制输入规模；极端长度下容量加法和乘法仍受 `usize` / `Vec` 分配限制，分配失败由 Rust 运行时处理而非返回 `SharedError`。调用方还必须避免把 Raw KV 原字节交给 `DecodeBytes`，或把 compact 编码用于依赖字典序的键范围。

## 并发与资源生命周期

全部函数都是同步、无锁、无任务和无通道的纯局部计算；不可变静态数据可安全被多线程共享。每次调用只修改其独占的 `Vec<u8>` 或 `&mut [u8]`，Rust 借用规则阻止同一缓冲区的无同步并发写入。

主要资源成本是线性扫描和内存。`EncodeBytes` 每 8 字节增加 1 字节 marker，并总是增加终止组；`EncodeCompactBytes` 增加 1 至 10 字节长度前缀。`reallocBytes` 尽量复用传入容量，容量不足时复制一次已有前缀。`Decoder::DecodeOne` 通过把 `self.buf` 暂时移出再传给 `DecodeBytes` 来跨多次解码复用分配。

`fastReverseBytes` 与 `safeReverseBytes` 都是原地操作，不分配内存。架构分支在编译期由 `cfg(target_arch)` 决定；当前 Rust 快路径没有 Go 的 `unsafe` 非对齐 machine-word 访问，因此不存在相应裸指针生命周期，但也不能假定它具有与 Go word-wise 取反相同的性能特征。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/codec/bytes.go`](./bytes.go)，独立测试是 [`pkg/util/codec/bytes_test.go`](./bytes_test.go)。Rust 保留了 Go 的公开函数集合、8+1 分组线格式、额外终止组、Raw KV 旁路、正/降序互逆、compact 长度前缀、错误判定和已有前缀保留行为。Rust 测试 [`pkg/util/codec/bytes_test.rs`](./bytes_test.rs) 迁移了 Go 的正序/降序向量、畸形输入和 Raw KV 分支；[`pkg/util/codec/bytes_1_aster_unit_test.rs`](./bytes_1_aster_unit_test.rs) 额外覆盖 compact payload/leftover 与高层 Datum 有序性。

实现层差异主要来自语言模型：Go 使用 `[]byte` 和可选的 `nil` buffer，Rust 使用拥有所有权的 `Vec<u8>`、`Option<Vec<u8>>` 与借用切片；Go compact 解码对下游错误调用 `errors.Trace`，Rust 直接用 `?` 传播 `SharedError`，可观察的成功数据与错误分类意图不变。Go `fastReverseBytes` 通过 `unsafe` 把字节切片解释为 `[]uintptr` 后按机器字取反；Rust 的同名函数用安全的 chunk 遍历并逐字节取反，结果一致，但优化机制并不等价。

Go 的 `supportsUnaligned` 在运行时由 `runtime.GOARCH` 常量表达，Rust 用条件编译为 x86/x86_64 生成常量。两边都在其他架构回退到安全逐字节路径。

## 扩展指南

若新增或修改编码格式，先判断是否需要字典序保证。需要排序键时应围绕 `EncodeBytes` / `decodeBytes` 保持分组、marker、终止组和降序取反不变量；只用于 value 且关注空间时才使用 compact 路径。任何线格式变化都必须同步 Go `bytes.go`、Rust `bytes.rs`、对应独立测试以及依赖 flag 分派的 `codec.rs`，并评估已持久化键和 BR/region 边界兼容性。

新增解码校验应集中在 `decodeBytes`，以同时覆盖正序和降序；新增公共包装函数应在 `bytes.rs` 实现，由 `lib.rs` 的现有整体再导出暴露。若改变容量估算，必须同步 `EncodedBytesLength`、`EncodeBytes` 的预留公式和 `codec.rs::sizeBytes`，否则可能出现长度预估错误或额外分配。

测试逻辑必须继续放在独立文件中。优先扩展 `bytes_test.rs` 的 Go 对齐向量和畸形输入表；Rust 特有或跨层回归可放入 `bytes_1_aster_unit_test.rs`。至少覆盖空值、7/8/9 字节边界、内含零字节、已有前缀、leftover、正序/降序往返、Raw KV 两分支、错误 marker/padding、compact 负数/超长/截断长度。性能优化必须同时验证不同架构路径结果一致，并关注 `Decoder` 缓冲区复用是否退化。

## 验证依据

- RustCodeGraph `status`：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标源码和两份 Rust 测试均可按文件读取。
- RustCodeGraph `node --file pkg/util/codec/bytes.rs`：核对了 259 行完整源码、全部常量/函数/条件编译项，并得到“被 34 个文件使用”的文件级关系。
- RustCodeGraph `query`：确认 Rust 与 Go 的 `EncodeBytes`、`decodeBytes`、`EncodeCompactBytes`、`reverseBytes`、`reallocBytes` 对应符号及位置。
- RustCodeGraph `explore`：确认 Go 侧 `EncodeBytes -> encodeBytes`、`DecodeBytes -> DecodeOne` 等调用关系，以及 tablecodec/BR 等下游使用面；精确限定文件的 `callers/callees` 子命令未返回可用结果，因此以局部 `rg` 补查 Rust 直接调用点。
- 已读源码与配置：`pkg/util/codec/bytes.rs`、`lib.rs`、`codec.rs`、`number.rs`、`Cargo.toml`；Go 对照 `bytes.go`；测试 `bytes_test.rs`、`bytes_1_aster_unit_test.rs`、`bytes_test.go`。
- 局部调用搜索确认了 `codec.rs::encodeBytes`、两个 `DecodeOne` 路径，以及 `pkg/meta/reader.rs`、`pkg/session/runtime/*`、`br/pkg/restore/*` 的代表性生产调用。
- 本任务只新增说明文档，不修改运行时代码；按计划不运行 Cargo。结构验收使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题。
