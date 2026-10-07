# `pkg/lightning/common/key_adapter.rs`

## 文件定位

本文件属于 Cargo crate `astersql-lightning-common`（见 `pkg/lightning/common/Cargo.toml`），实现 Lightning 公共层的键编码适配器。模块由 `pkg/lightning/common/lib.rs` 以 `mod key_adapter` 装配，并通过 `pub use key_adapter::*` 将公开符号提升到 crate 根，因此同 crate 的 `dupdetect.rs` 可以直接以 `crate::KeyAdapter` 使用，外部依赖此 crate 的代码也可从 crate 根导入。

它位于“原始业务键”与“供重复检测排序、存储的键”之间：普通路径可以原样透传键，重复检测路径则把业务键编码成保持字典序的 memcomparable 前缀，并附加 RowID，避免同一业务键的不同记录在键值存储中互相覆盖。文件本身只负责纯内存编解码，不访问磁盘、网络或数据库。

需要区分一个相邻但独立的实现：`pkg/ingestor/ingestctrl/iterator.rs` 自己定义了同名 `KeyAdapter`、`NoopKeyAdapter` 和 `DupDetectKeyAdapter`，签名也不同（RowID 为 `i64`，错误为该 crate 的 `Result`）。它不是本文件类型的调用者；RustCodeGraph 的同名搜索会同时返回两组符号。

## 核心职责

1. 以 `KeyAdapter` trait 统一三项契约：把键追加到调用方提供的缓冲区、从存储键恢复业务键、精确预估编码后长度。trait 继承 `Send + Sync`，允许适配器对象安全地放入 `Arc<dyn KeyAdapter>` 并跨线程共享。
2. `NoopKeyAdapter` 为不需要区分重复行的路径提供零变换实现：忽略 RowID，只追加输入键或数据。
3. `DupDetectKeyAdapter` 生成 `encode_bytes(key) || row_id || row_id_len(u16, big-endian)`。memcomparable 前缀维持业务键顺序，RowID 使相同业务键的记录仍有不同的完整编码键，末尾长度字段使解码器能定位并剥离可变长 RowID。
4. `decode_bytes` 验证 8 字节分组、marker 与零填充，拒绝截断或非法编码；`MinRowID` 提供九个零字节的最小 RowID 占位，供构造重复检测范围边界。
5. `reallocBytes` 对应 Go 辅助函数的容量预留语义。当前 Rust `DupDetectKeyAdapter::Encode` 依靠 `Vec::extend_from_slice` 自动扩容，并未调用它；它仍是经 crate 根公开的兼容 API。

## 主要符号

- `pub const MinRowID: [u8; 9]`：全零的九字节 RowID。`key_adapter_test.rs::test_min_row_id` 验证以它构造的编码结果不大于所覆盖的整数 handle、Lightning comparable-varint 和 common-handle 风格样本。
- `pub trait KeyAdapter: Send + Sync`：公共抽象。`Encode(dst, key, row_id)` 和 `Decode(dst, data)` 都保留 `dst` 已有前缀并在其后追加；`EncodedLen` 只计算本次编码数据的长度，不包含传入 `dst` 的已有长度。
- `pub fn reallocBytes(Vec<u8>, usize) -> Vec<u8>`：在保留长度和内容的同时调用 `Vec::reserve(additional)`，保证至少能再追加 `additional` 字节。与 Go 版本相比，Rust `reserve` 只承诺容量下限，不承诺精确容量。
- `pub struct NoopKeyAdapter`：无状态、`Clone + Copy + Default` 的零大小类型。编码长度为 `key.len()`，编码/解码不会验证输入格式。
- `fn encode_bytes(Vec<u8>, &[u8]) -> Vec<u8>`：私有 memcomparable 编码器。每组写入八个数据/零填充字节和一个 `0xff - pad` marker；当输入长度恰为八的倍数（包括空键）时，额外写入全零终止组和 marker `0xf7`。
- `fn decode_bytes(&[u8]) -> Result<Vec<u8>, CommonError>`：私有解码器。逐个读取九字节组，验证 marker 推导出的 pad 不超过 8、pad 区域全零，并在首个 `pad > 0` 的终止组返回。它允许输入后方仍有未读字节，但调用它的公开解码路径会预先切掉 RowID 尾部。
- `pub struct DupDetectKeyAdapter`：无状态、`Clone + Copy + Default` 的重复检测实现。`EncodedLen` 为 `(key.len() / 8 + 1) * 9 + row_id.len() + 2`，与 `encode_bytes` 的终止组规则一致。

## 执行流程

`NoopKeyAdapter` 的流程只有一次追加：`Encode` 把 `key` 追加到 `dst`，`Decode` 把 `data` 追加到 `dst` 并返回 `Ok`；传入 RowID 不影响结果。

`DupDetectKeyAdapter::Encode` 的流程如下：

1. `encode_bytes` 从 `dst` 当前末尾开始处理业务键。每八字节形成一个九字节组；不足八字节时补零，并通过 marker 记录补零数量。
2. 若业务键长度是八的倍数，追加一个 pad 为 8 的终止组，从而避免一个键是另一个键前缀时破坏字典序和解码边界。
3. 直接追加调用方提供的 `row_id`。本文件不解释或重新编码 RowID；调用方必须提供已经满足比较需求的字节表示，例如测试使用 `EncodeIntRowID`。
4. 把 `row_id.len()` 截为 `u16` 后按大端序追加两个字节。正常支持范围因此隐含为 RowID 长度不超过 `u16::MAX`。

`DupDetectKeyAdapter::Decode` 反向处理：先要求总长度至少为 2，从最后两字节读取 RowID 长度；再验证输入足以容纳 `row_id + 长度字段`；随后把这段尾部排除，只将前缀交给 `decode_bytes`；解出的业务键追加到原 `dst` 后返回。它有意丢弃 RowID，因为 `KeyAdapter::Decode` 的契约是恢复业务键而不是完整拆包。

应用内的直接消费流程见 `pkg/lightning/common/dupdetect.rs`：`NewDupDetector` 保存 `Arc<dyn KeyAdapter>`；`DupDetector::Init` 和 `DupDetector::Next` 对迭代器的原始编码键调用 `Decode`，再比较相邻业务键。两个完整编码键可因 RowID 不同而共存和排序，但解码后相等时会被判定为重复，并记录到写批或按选项返回重复键错误。

## 数据与状态

两个适配器都是无字段类型，不保存跨调用状态。所有状态都在参数和返回的 `Vec<u8>` 中流动，因此同一个实例可同时服务多个调用者。`dst` 是可复用的所有权缓冲：已有内容是调用方前缀，本次结果追加在后面。测试 `test_encode_key_to_pre_allocated_buf`、`test_decode_key_to_pre_allocated_buf` 和 `test_decode_key_dst_is_insufficient` 分别验证容量足够时底层指针可保持、容量不足时允许重分配且旧前缀不丢失。

重复检测编码的结构不保存独立偏移量：末尾固定两个字节保存 RowID 长度，解码时据此计算 `data.len() - row_len - 2`。业务键前缀由九字节组自描述；最后一个带 pad 的组是终止标志。业务键的排序由 memcomparable 前缀决定；仅当业务键相同时，后接的 RowID 字节参与完整编码键的次序和唯一性。

`MinRowID` 是值常量而非可变全局状态。当前代码没有缓存、锁、原子变量或隐式生命周期。

## 依赖与调用关系

- 上游装配：`pkg/lightning/common/lib.rs` 声明并再导出本模块，同时以独立文件 `key_adapter_test.rs` 挂载单元测试。
- 直接生产消费者：`pkg/lightning/common/dupdetect.rs` 导入 `KeyAdapter`，在 `DupDetector.keyAdapter` 中保存 `Arc<dyn KeyAdapter>`，并从 `Init`/`Next` 调用 `Decode`。`dupdetect_test.rs` 以 `NoopKeyAdapter` 验证这一消费链。
- 下游内部依赖：仅使用标准库的 `Vec`、切片、迭代器、`repeat_n` 和整数大端转换；错误通过同 crate 的 `crate::CommonError` 构造。
- crate 边界：`pkg/lightning/common/Cargo.toml` 的运行依赖只有 `astersql-lightning-log` 和 `libc`，本文件没有直接使用二者，也没有 feature 或条件编译分支。
- 图证据：RustCodeGraph 将本文件索引为 119 行，并识别 `KeyAdapter` trait、两种适配器、三个方法、`reallocBytes`、`encode_bytes`、`decode_bytes` 与 `MinRowID`。名称查询还显示 ingestor 中存在另一套同名符号；逐文件核验确认那套实现是独立定义，不能视为本文件的类型边。

## 错误处理与边界

`NoopKeyAdapter::Decode` 对任意字节都成功。`DupDetectKeyAdapter::Decode` 和 `decode_bytes` 以 `CommonError::new("decode", message)` 返回可传播错误，主要边界为：

- 少于两个字节，无法读取 RowID 长度：`insufficient bytes to decode value`。
- RowID 长度加两字节超过总输入长度：同样返回 insufficient 错误。
- memcomparable 前缀不足一个完整九字节组，或一直没有合法终止组：返回 insufficient 错误。
- marker 推导出的 pad 大于 8：返回 `invalid marker byte`。
- pad 区域包含非零字节：返回 `invalid padding byte`。

错误发生前，`DupDetectKeyAdapter::Decode` 尚未向 `dst` 追加解码结果，因为 `decode_bytes` 先在独立 `Vec` 中完成验证；因此错误返回时传入 `dst` 被消费但不会暴露部分输出。错误消息包含出错分组的调试字节，调用方若把它写入日志需考虑键材料的敏感性。

编码端有一个需由调用方保证的边界：`row_id.len() as u16` 会在超过 65535 字节时截断，而 `EncodedLen` 仍按实际长度计算，随后解码会依据截断值切错尾部。本文件没有显式拒绝这一输入；扩展或调用时不能假定任意长度 RowID 都能往返。算术还依赖 `usize` 容量计算和 `Vec` 分配，极端输入可能触发分配失败；当前 API 不把分配失败表示为 `CommonError`。

## 并发与资源生命周期

`KeyAdapter: Send + Sync` 是本文件唯一的并发契约。现有实现无内部可变状态，所以共享引用调用不会发生数据竞争；`pkg/lightning/common/dupdetect.rs` 通过 `Arc<dyn KeyAdapter>` 持有它，适配器随最后一个 `Arc` 释放。

每次编码取得 `dst` 的所有权并返回同一个或扩容后的 `Vec`；每次 DupDetect 解码还会创建一个临时解码 `Vec`，再把它追加到 `dst`。没有借用结果指向输入切片，输入可在返回后立即释放。容量足够时 `Vec` 可复用原分配，容量不足时由标准库重新分配；调用方不得跨调用保存旧数据指针并假设其稳定。

文件不创建线程、任务、锁、通道、文件句柄或事务，也没有显式 `Drop` 清理。批量写入、flush 与关闭属于消费者 `DupDetector`/`WriteBatch` 的生命周期，不属于适配器自身。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/lightning/common/key_adapter.go`，总体契约和线格式一致：Go `KeyAdapter` 的三个方法对应 Rust trait；Go `NoopKeyAdapter` 原样追加；Go `DupDetectKeyAdapter` 使用 `codec.EncodeBytes` 后追加 RowID 与两字节大端长度；两边的 `EncodedLen` 都使用每八字节增加一个九字节组并始终包含终止组的公式；`MinRowID` 都是九个零字节。

Rust 将 Go 的 `codec.EncodeBytes`/`codec.DecodeBytes` 局部复刻为私有 `encode_bytes`/`decode_bytes`，测试覆盖了顺序保持、重复键因 RowID 区分、往返和非法 padding。Rust `CommonError` 承接 Go `errors.New`/codec 解码错误，但错误类型、包装栈和具体文本不保证完全相同。

缓冲实现存在可观察但契约兼容的差异：Go `reallocBytes` 在容量不足时按所需精确容量新建切片，Rust `reserve` 可选择更大容量；Go Decode 尝试让 codec 直接写进 `dst` 的剩余容量并处理别名结果，Rust先分配临时解码结果再 `extend` 到 `dst`。两者都保留 `dst` 前缀，相关 Rust/Go 测试都要求容量充足时最终结果复用原缓冲、容量不足时内容正确。

Rust 测试 `key_adapter_test.rs` 基本对应 `key_adapter_test.go`，但还显式加入非法 marker/padding 拒绝测试。Go 的 `TestMinRowID` 使用真实 `kv.IntHandle`、`codec.EncodeKey` 和 `kv.NewCommonHandle`；Rust 测试使用 `EncodeIntRowID` 和手工构造的 common-handle 风格 payload 来验证相对顺序，因而不能把它描述成对全部 Go handle 编码的穷尽证明。

## 扩展指南

- 新增适配器时实现 `KeyAdapter` 的三个方法，并保持“追加到 `dst`、不覆盖已有前缀”的契约；实现必须是 `Send + Sync`。测试应放在独立的 `pkg/lightning/common/key_adapter_test.rs`，不要内嵌回生产源文件。
- 修改编码格式时必须同步 `Encode`、`Decode` 与 `EncodedLen`，并验证空键、长度 1/7/8/9、包含零字节、最大合法 RowID、截断输入、非法 marker/padding、相同键不同 RowID以及跨键排序。格式用于已排序数据时，兼容性风险高于普通内部重构：旧数据是否仍能读取、范围边界和 Go 端是否产生相同字节都需要明确策略。
- 若要正式支持超长 RowID，应在编码前加入显式上限检查；但当前 `Encode` 不返回 `Result`，改变错误契约会影响所有 trait 实现和调用方。不能只扩大长度字段而不同时处理线格式版本和 Go 兼容。
- 若优化解码分配，可让解码器直接写入 `dst` 的备用容量，但必须谨慎处理扩容、已有前缀、错误时部分写入和别名安全，并保留三个预分配缓冲测试。
- 若修改 `MinRowID`，需同时检查范围扫描边界和所有 RowID 编码类别的排序不变量，至少同步 Rust `test_min_row_id` 与 Go `TestMinRowID` 的意图。
- ingestor 的 `pkg/ingestor/ingestctrl/iterator.rs` 有独立同名实现。若目标是统一两套代码，需要单独设计 trait 签名、RowID 表示和错误类型的迁移；不能只改本文件并假定 ingestor 自动获得变化。

## 验证依据

- 源码与装配：`pkg/lightning/common/key_adapter.rs`、`pkg/lightning/common/lib.rs`、`pkg/lightning/common/Cargo.toml`。
- 直接消费与相邻实现：`pkg/lightning/common/dupdetect.rs`、`pkg/lightning/common/dupdetect_test.rs`、`pkg/ingestor/ingestctrl/iterator.rs`、`pkg/ingestor/ingestctrl/iterator_test.rs`、`pkg/ingestor/ingestctrl/duplicate.rs`。后两处用于确认同名 ingestor trait 的独立边界，而非证明对本文件的调用。
- Go 对照与测试：`pkg/lightning/common/key_adapter.go`、`pkg/lightning/common/key_adapter_test.go`。
- Rust 独立测试：`pkg/lightning/common/key_adapter_test.rs`，覆盖 Noop/DupDetect 往返、`EncodedLen`、业务键顺序、同键不同 RowID、无效 padding/marker/截断、预分配缓冲复用、容量不足重分配和 `MinRowID` 相对次序。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/lightning/common/key_adapter.rs` 返回完整 119 行及引用文件摘要；对 `KeyAdapter`、`NoopKeyAdapter`、`DupDetectKeyAdapter`、`reallocBytes`、`encode_bytes`、`decode_bytes`、`MinRowID` 的 `query` 确认符号位置。精确 `callers/callees` 未为这些 trait/零大小类型返回可用边，因此调用关系以索引的文件引用摘要和直接源码导入/调用交叉核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核本文未把同名 ingestor 实现误作本文件调用者。
