# `pkg/parser/charset/encoding_bin.rs`

## 文件定位

本文件属于 `astersql-parser-charset` crate；`pkg/parser/charset/Cargo.toml` 将 `lib.rs` 声明为 crate 根，`lib.rs` 再在 `encoding_bin` 模块中以 `include!("encoding_bin.rs")` 纳入本文件，并把该模块的公开项重新导出。它实现的是按原始字节处理的轻量 Binary 编码对象，主要供 `EncodingBase` 的回指派发和直接调用使用。

需要区分同 crate 中的另一条实现路径：`pkg/parser/charset/encoding.rs` 内还有一个私有 `EncodingBin` 和私有静态 `ENCODING_BIN_IMPL`，用于统一 `Encoding` trait、`FindEncoding` 与 `FindEncodingTakeUTF8AsNoop`。两处同名符号位于不同 Rust 模块，当前不是同一个实例；本文只描述 `encoding_bin.rs` 中由 `parser_charset::encoding_bin` 导出的实现。

## 核心职责

- 用 `ENCODING_BIN_IMPL` 保存全局只读的 `EncodingBin`，并由 `init_encoding_bin` 建立 `EncodingBase` 到具体 Binary 实现的回指。
- 将 Binary 语义固定为“一个字节就是一个处理单元”：`peek` 最多返回首字节，`foreach` 逐字节回调。
- 接受任意字节序列：`is_valid` 恒为 `true`，不进行 UTF-8 或其他字符集合法性校验。
- 提供零转换、零拷贝路径：`transform` 忽略操作位和目标缓冲，返回借用原输入的 `TransformResult::Borrowed`。

这些职责与 `pkg/parser/charset/encoding_bin.go` 的 `encodingBin` 相同，适用于 BLOB、binary 字符集及只需字节透传而不应引入字符语义的路径。

## 主要符号

- `pub static ENCODING_BIN_IMPL: OnceLock<EncodingBin>`：延迟写入一次的全局实例。读取者必须先确保初始化完成；`lib.rs::encoding_by_ref(EncodingRef::Bin)` 对未初始化状态使用 `expect("binary initialized")`。
- `pub fn init_encoding_bin()`：构造 `EncodingBin { encoding_base: EncodingBase::new(Encoding::Nop) }`，调用 `set_self(EncodingRef::Bin)` 建立回指，再尝试写入 `OnceLock`。重复调用不会替换已有值，因为 `set` 的返回值被忽略。
- `pub struct EncodingBin`：仅含公开字段 `encoding_base: EncodingBase`；底层编码器为 `Encoding::Nop`。
- `name()`：返回 crate 根常量 `CHARSET_BIN`，即 `"binary"`。
- `tp()`：返回 crate 根的轻量枚举值 `EncodingTp::Bin`。
- `peek(src)`：空切片原样返回，非空切片返回 `&src[..1]`。
- `is_valid(src)`：无条件返回 `true`。
- `foreach(src, op, callback)`：忽略 `op`，按索引逐字节调用 `callback(byte, byte, true)`；回调首次返回 `false` 时提前结束。
- `transform(dest, src, op)`：忽略 `dest` 和 `op`，返回 `Ok(TransformResult::Borrowed(src))`。

此外，`lib.rs` 为 `EncodingBin` 实现 `EncodingView`，把 `name`、`peek`、`foreach` 转发回上述固有方法，使 `EncodingBase` 能通过 `encoding_by_ref` 做动态派发。

## 执行流程

1. 使用轻量 Binary 路径前，调用方先执行 `init_encoding_bin`。函数创建采用 `Encoding::Nop` 的 `EncodingBase`，把其 `self_encoding` 设为 `Some(EncodingRef::Bin)`，然后发布到 `ENCODING_BIN_IMPL`。
2. 直接读取时，调用方通过 `ENCODING_BIN_IMPL.get()` 取得共享引用；`lib.rs::encoding_by_ref` 也以这一方式解析 `EncodingRef::Bin`。
3. 单元探测由 `peek` 完成：空输入不产生处理单元，非空输入固定取一个字节。
4. 遍历由 `foreach` 完成：每轮把同一个单字节切片同时作为 `from` 和 `to`，并把 `ok` 固定为 `true`。回调可用 `false` 控制提前停止。
5. 变换由 `transform` 完成：不创建输出缓冲、不复制数据，也不解释任何 `Op` 标志，结果切片与 `src` 指向同一段内存。

若经 `EncodingBase` 间接调用，其 `is_valid`、`foreach` 或 `transform` 会先依据 `self_encoding` 调用 `lib.rs::encoding_by_ref`；这解释了初始化时必须设置 `EncodingRef::Bin`。目标文件自身的 `is_valid` 和 `transform` 则不需要走基座。

## 数据与状态

全局可变状态仅为 `OnceLock` 的“未初始化/已初始化”状态；初始化后保存一个不可变 `EncodingBin`。对象内部的 `EncodingBase` 保存 `Encoding::Nop` 和 `Some(EncodingRef::Bin)`，没有每次请求的缓存、计数器或可变缓冲。

输入和输出都以借用切片表达。`peek`、`foreach` 的片段均指向原输入；`transform` 的 `Borrowed` 结果生命周期与输入绑定。调用方不能把这些借用延长到输入生命周期之外，也应注意 Go 接口所说的别名风险：若底层数据通过其他所有者被改写，观察到的返回内容也会变化；Rust 的普通安全借用会阻止同一作用域内的冲突可变借用。

## 依赖与调用关系

- 装配入口：`pkg/parser/charset/lib.rs` 的 `pub mod encoding_bin` 使用 `use crate::*` 后 `include!` 本文件，因此 `EncodingBase`、`Encoding`、`EncodingRef`、`EncodingTp`、`Op`、`ByteBuffer`、`TransformResult`、`EncodingError` 和 `CHARSET_BIN` 都来自 crate 根再导出或定义。
- 上游直接证据：`pkg/parser/charset/charset_1_aster_unit_test.rs::binary_encoding_is_a_zero_copy_byte_passthrough` 调用 `init_encoding_bin`、读取 `ENCODING_BIN_IMPL`，并调用 `is_valid` 与 `transform`。
- 上游间接入口：`lib.rs::encoding_by_ref(EncodingRef::Bin)` 返回此文件的全局实例；`lib.rs` 中的 `impl EncodingView for encoding_bin::EncodingBin` 把基座所需操作转发给它。
- 下游依赖：`init_encoding_bin` 调用 `EncodingBase::new` 和 `EncodingBase::set_self`；核心方法只使用切片、回调和 crate 根的枚举/别名，不调用外部编码库。
- crate 边界：`Cargo.toml` 声明了 `encoding`、`encoding_rs` 等依赖，但本文件的 Binary 快路径只使用 crate 根封装的 `Encoding::Nop`，没有直接引用这些外部 crate。

RustCodeGraph 对 `init_encoding_bin` 识别出的直接调用者是上述 Rust 单元测试；源码搜索没有发现生产代码显式调用初始化函数。这意味着当前轻量实例的生产初始化接线不能仅凭本文件证明，文档不把它描述为已在应用启动阶段自动初始化。

## 错误处理与边界

- `peek` 显式处理空输入，避免对 `[..1]` 越界；非空时固定返回一个字节。
- `foreach` 对空输入不调用回调；对非空输入不会产生非法分支，`ok` 永远为 `true`。回调返回 `false` 是正常的短路控制，不是错误。
- `is_valid` 接受包括 `0x00`、`0xff` 和无效 UTF-8 在内的所有字节。
- `transform` 的返回类型允许错误，但本实现始终返回 `Ok`；`dest`、`op`、替换、截断与错误收集标志都不影响结果。
- 未调用 `init_encoding_bin` 就经 `encoding_by_ref(EncodingRef::Bin)` 读取会触发 `expect("binary initialized")` panic；直接 `.get()` 则得到 `None`。
- `init_encoding_bin` 忽略重复 `OnceLock::set` 的失败，因此它是幂等式的“首次写入生效”，但也不会报告已有实例或替换状态。

## 并发与资源生命周期

`OnceLock<EncodingBin>` 提供线程安全的一次发布；成功初始化后，各线程只共享不可变引用。重复并发初始化时只有一个 `set` 成功，其余失败结果被丢弃；各候选对象内容相同，所以目标文件没有可见的后写覆盖，但调用方也拿不到谁完成初始化的反馈。

方法不加锁、不启动任务、不使用通道、不持有文件或网络资源。`foreach` 同步执行回调，回调结束后单字节借用不被本实现保存。`transform` 不分配输出，其结果只在源切片生命周期内有效。`EncodingBase` 的回指是枚举值而非自引用指针，避免了自引用对象的生命周期问题。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/parser/charset/encoding_bin.go`：

- Go 的包级 `EncodingBinImpl = &encodingBin{encodingBase{enc: encoding.Nop}}` 对应 Rust 的 `OnceLock` 加 `init_encoding_bin` 构造；Go `init()` 自动设置 `self`，Rust 当前需要显式调用初始化函数。
- Go 的嵌入字段 `encodingBase` 对应 Rust 的命名字段 `encoding_base`；Go 保存接口回指，Rust 保存 `EncodingRef::Bin`。
- `Name`、`Tp`、`Peek`、`IsValid` 和 `Foreach` 的返回值、逐字节边界以及回调短路语义一致。
- Go `Transform` 返回原 `src` 与 `nil`，Rust 返回 `TransformResult::Borrowed(src)` 与 `Ok`，明确表达同一零拷贝和别名语义。
- Go 类型通过嵌入 `encodingBase` 获得 `MbLen`、大小写等接口方法；本文件没有实现 crate 中 `encoding.rs::Encoding` trait，而是只在 `lib.rs` 中实现较窄的 `EncodingView`。统一 trait 查找当前由 `encoding.rs` 内另一份私有 Binary 实现承担，这是迁移结构差异，不应误认为完全等价接线。

Go 的 `encoding_test.go` 广泛验证统一编码接口，但没有为 `encoding_bin.go` 提供同名独立测试；目标 Rust 文件的直接回归证据是 `charset_1_aster_unit_test.rs` 中的 Binary 专项测试。

## 扩展指南

- 若改变 Binary 的分块规则，应同步修改 `peek` 与 `foreach`，保持“探测宽度”和“遍历步长”一致，并扩展独立测试覆盖空输入、多个任意高位字节和回调提前停止。
- 若改变合法性或变换规则，应同时评估 `is_valid`、`transform`、`EncodingBase` 的回指派发以及 Go `encoding_bin.go`；Binary 当前的关键兼容契约是任意字节合法、忽略 `Op`、输出与输入别名。
- 若希望把轻量实现接入生产初始化，应在 crate 的明确初始化入口完成，而不是依赖测试调用；需要验证并发首次初始化、重复初始化和初始化前读取行为。
- 若要统一 `encoding_bin.rs` 与 `encoding.rs` 的两个 Binary 实现，应先核对两套公开类型（crate 根轻量类型与 `encoding.rs` trait 类型）、返回所有权差异（`Borrowed` 与 `Vec` 拷贝）和所有调用者，不能只删除其中一个同名结构。
- 测试逻辑应继续放在独立文件，例如扩展 `pkg/parser/charset/charset_1_aster_unit_test.rs` 或新增同目录 `encoding_bin_test.rs` 并在 `lib.rs` 的 `#[cfg(test)]` 区接线，不应内嵌到生产源文件。
- 性能风险主要是误把 `Borrowed` 改为复制，或在 `foreach` 中引入字符解码；兼容风险主要是开始拒绝高位字节、解释 `Op`，或改变空输入及短路行为。

## 验证依据

- 目标源码：`pkg/parser/charset/encoding_bin.rs`，核对了全局实例、初始化函数、结构体及六个固有方法。
- crate 装配与适配：`pkg/parser/charset/lib.rs`，核对了 `include!`、公开再导出、`EncodingView`、`encoding_by_ref`、`TransformResult`、轻量枚举和测试模块声明。
- 基座实现：`pkg/parser/charset/encoding_base.rs`，核对了 `set_self`、基于 `encoding_by_ref` 的校验/遍历/变换派发和未初始化 panic 边界。
- 统一编码路径：`pkg/parser/charset/encoding.rs`，核对了私有 Binary 实现、`Encoding` trait、`FindEncoding` 与 `FindEncodingTakeUTF8AsNoop`，确认它与目标模块是两套符号。
- crate 清单：`pkg/parser/charset/Cargo.toml`，核对 crate 名、`lib.rs` 入口及依赖边界。
- Go 对照：`pkg/parser/charset/encoding_bin.go`、`pkg/parser/charset/encoding.go`；测试参考：`pkg/parser/charset/encoding_test.go`。
- Rust 独立测试：`pkg/parser/charset/charset_1_aster_unit_test.rs::binary_encoding_is_a_zero_copy_byte_passthrough`，验证任意字节合法、变换成功、返回 `Borrowed` 且指针与输入相同。
- RustCodeGraph：`status` 显示索引包含 `pkg/parser/charset/encoding_bin.rs`；文件节点列出 83 行源码；`query init_encoding_bin --kind function` 定位到第 24 行；`explore` 识别其直接调用者为上述 Binary 专项测试。常见方法名存在大量同名项，因此调用关系再用限定目录的源码搜索消歧。
- 结构验证按任务命令执行；本任务为纯文档分析，依照计划不运行 Cargo。
