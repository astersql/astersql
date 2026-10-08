# `pkg/util/codec/number.rs`

## 文件定位

`number.rs` 是 `astersql-util-codec` crate 的整数编解码实现。模块入口 `pkg/util/codec/lib.rs` 以私有 `mod number` 装载它，再通过 `pub use number::*` 把公开函数暴露到 crate 根；同一入口还把这些函数导入 `bytes.rs` 和 `codec.rs` 的 `include!` 模块，因此它既可被 `astersql_util_codec::EncodeInt` 这类外部路径调用，也服务于更高层的通用 datum/字节编码。

该文件位于 SQL 层与持久化字节布局之间：例如 `pkg/tablecodec/tablecodec.rs` 用 `EncodeInt`/`DecodeInt` 组成和解析表、索引、分区键，`pkg/structure/type.rs` 用定长整数编码结构化元数据键，`pkg/store/mockstore/unistore/tikv/mvcc/tikv.rs` 用 uvarint 和定长整数编码 MVCC 值。它不解释数值的 SQL 类型，也不负责 I/O；职责是把调用方已经确定的 `i64`/`u64` 与确定性字节表示互换。

## 核心职责

文件提供三组协议，不能相互替换：

1. `EncodeInt`/`EncodeUint` 及其 `Desc`、解码变体使用固定 8 字节大端表示，目标是字节字典序可比较。`EncodeIntToCmpUint` 先翻转有符号整数的最高位，使 `i64` 数值序映射到 `u64` 字节序；降序编码再对全部位取反。
2. `EncodeVarint`/`EncodeUvarint` 及解码函数使用 Go `encoding/binary` 兼容的 7-bit continuation 编码，目标是紧凑和流式拼接，不保证 memcomparable。带符号值先做 ZigZag 变换。
3. `EncodeComparableVarint`/`EncodeComparableUvarint` 及解码函数以标签表示载荷长度，使变长结果仍保持数值序与字节字典序一致。负数标签占 `[0, 7]`，内联非负数占 `[8, 247]`，较大正数标签占 `[248, 255]`。

所有编码函数接收并返回 `Vec<u8>`，在既有前缀后追加结果；所有解码函数读取切片前缀，并在成功时返回未消费的后缀和数值。该后缀契约允许上层顺序解析复合键或值。

## 主要符号

- `signMask: u64`：值为 `0x8000_0000_0000_0000`，供 `EncodeIntToCmpUint` 与 `DecodeCmpUintToInt` 以异或执行可逆的符号域映射。
- `EncodeInt`、`EncodeIntDesc`、`DecodeInt`、`DecodeIntDesc`：有符号固定宽度协议。编码总是追加 8 字节；升序和降序版本分别写入映射值与其按位反值。
- `EncodeUint`、`EncodeUintDesc`、`DecodeUint`、`DecodeUintDesc`：无符号固定宽度协议；大端表示天然保持升序，按位取反得到降序。
- `maxVarintLen64`：64 位 uvarint 的最大字节数 10。`decodeUvarintPrefix` 使用它执行终止和第十字节溢出检查。
- `VarintDecodeError::{Insufficient, Overflow}`：仅供文件内部区分输入未终止与超过 64 位；公开函数再将其映射为共享错误。
- `EncodeVarint`、`DecodeVarint`：带符号紧凑编码。编码把 `v` 映射为 `2*v` 或负数对应的奇数编码，解码以最低位恢复符号。
- `EncodeUvarint`、`DecodeUvarint`：逐个写入/读取 7 位载荷，最高位表示后续仍有字节。
- `negativeTagEnd`（8）与 `positiveTagStart`（247）：memcomparable 变长格式的两个标签边界。
- `EncodeComparableVarint`：负数按二进制补码所需的 1 至 8 个大端字节编码，标签为 `8 - length`；非负数委托给 `EncodeComparableUvarint`。
- `EncodeComparableUvarint`：`0..=239` 编成单字节 `value + 8`；更大值写入 `247 + length` 和 1 至 8 个大端载荷字节。
- `DecodeComparableUvarint`、`DecodeComparableVarint`：解析标签、检查长度和符号半区，再返回值与后缀。
- `errDecodeInsufficient`、`errDecodeInvalid`：通过 `LazyLock<SharedError>` 保持与 Go 包级 sentinel 相同的错误身份语义；`traceError` 模拟 `errors.Trace` 包装且保留 cause。

文件没有自定义 struct、公开 trait、impl 或条件编译项。除公开常量 `signMask` 和上述公开函数外，长度常量、标签常量、错误枚举及辅助函数均为模块私有。

## 执行流程

固定宽度有符号升序编码的流程是：`EncodeInt` 调用 `EncodeIntToCmpUint` 翻转符号位，使用 `to_be_bytes` 产生 8 字节大端序，然后追加到传入缓冲区。`DecodeInt` 先要求至少 8 字节，读取首 8 字节并调用 `DecodeCmpUintToInt`，最后返回 `&b[8..]`。`Desc` 路径在写入前或读出后额外按位取反，因此较大数值的编码反而按字典序更小。

普通 varint 编码由 `EncodeVarint` 先完成带符号到无符号的 ZigZag 映射，再委托 `EncodeUvarint`。后者循环输出低 7 位，对非末字节设置 `0x80`，直到剩余值小于 `0x80`。两个解码入口共用 `decodeUvarintPrefix`：逐字节累积 `(byte & 0x7f) << shift`，遇到最高位未设置的字节即返回消费长度；第十字节大于 1 判定为溢出，遍历完仍无终止字节判定为不足。`DecodeVarint` 再以最低位是否为 1 决定是否对右移结果取反。

memcomparable 编码先按值域选择标签。负 `i64` 的标签越小，表示载荷越长、值也越负；载荷保留补码高位语义。非负/无符号值 `0..=239` 直接压入一个偏移后的字节，其他值用“长度标签 + 最短大端载荷”。解码时先读取标签：无符号路径拒绝负数标签；带符号路径对负标签以 `u64::MAX` 初始化累积值，从而在左移拼接较短补码时保留符号扩展。载荷完成后，正标签结果必须不超过 `i64::MAX`，负标签结果必须落在负半区。

`DecodeComparableVarint` 对内联的 `0..=239` 有一个必须保留的 Go 历史契约：它返回的是原始输入切片 `b`，而不是消费标签后的切片。`pkg/util/codec/float_2_aster_unit_test.rs` 明确断言了值、切片长度和指针均遵循此行为；调用方若要连续解析，不能假设该分支会推进输入。

## 数据与状态

编码状态只存在于函数局部的 `Vec<u8>`、累积值、位移量和消费长度中。函数取得 `Vec<u8>` 所有权后追加数据，是否扩容由 `Vec` 容量决定；现有前缀保持不变。解码借用输入，不复制后缀，返回切片的生命周期受原输入约束。

固定宽度格式的长度恒为 8。普通 varint 长度为 1 至 10。memcomparable 无符号/非负格式的单字节区为 `0..=239`；其余正数以及所有负数占一个标签加 1 至 8 个载荷字节。标签和大端载荷共同构成排序不变量，调整任一边界都会改变持久化键格式。

唯一的进程级状态是两个 `LazyLock<SharedError>`。它们首次使用时各创建一个 `SharedError`，后续返回 clone；clone 共享底层错误身份，使 `errors::Cause(...).ptr_eq(...)` 能区分“不足”和“非法”两个 sentinel。除此之外没有可变全局状态。

## 依赖与调用关系

直接代码依赖只有 `crate::errors` 以及 Rust 标准库的整数大端转换、切片转换和 `std::sync::LazyLock`。`crate::errors` 在 `pkg/util/codec/lib.rs` 中再导出 `astersql-errors`；`pkg/util/codec/Cargo.toml` 以路径依赖 `../../errors` 声明该边界。此文件本身没有 feature gate，也不直接依赖网络、磁盘、时钟或第三方序列化库。

内部调用边为：`EncodeInt -> EncodeIntToCmpUint`，`DecodeInt -> DecodeCmpUintToInt`，`EncodeVarint -> EncodeUvarint`，`DecodeVarint/DecodeUvarint -> decodeUvarintPrefix`，`EncodeComparableVarint -> EncodeComparableUvarint`（仅非负分支），两个 comparable 解码器调用 sentinel 构造函数并在部分错误路径调用 `traceError`。

RustCodeGraph 的文件节点报告该文件被 41 个文件使用。源码精确检索确认的代表性上游包括：

- `pkg/tablecodec/tablecodec.rs`：表/索引/分区键中大量调用固定宽度有符号编码与解码，是数据库键布局的主要消费者。
- `pkg/structure/type.rs`：把结构类型标签、列表索引等编码进元数据键。
- `pkg/planner/property/physical_property.rs`：用 `EncodeInt` 组成物理属性 hash 输入。
- `pkg/store/mockstore/unistore/tikv/mvcc/{tikv.rs,mvcc.rs}`：编码/解码 MVCC 的时间戳、TTL 和其他整数域。
- `pkg/meta/reader.rs`、`pkg/domain/canonical_domain.rs`、`pkg/infoschema/issyncer/loader.rs`：构造带无符号类型后缀的元数据键。

函数级 RustCodeGraph `callers/callees` 查询在本次分析中未在 30 秒内返回结果，因此上述上游以索引的文件级 used-by 结果和精确源码检索交叉确认；没有把未返回的函数边当作证据。

## 错误处理与边界

四个固定宽度解码器在输入少于 8 字节时返回 `"insufficient bytes to decode value"`；长度足够后切片范围固定，后续 `try_into().unwrap()` 由先验长度检查保证不会失败。它们允许额外后缀，并原样返回。

普通 varint 解码把未出现终止字节映射为 `"insufficient bytes to decode value"`，把第十字节不合法映射为 `"value larger than 64 bits"`。`decodeUvarintPrefix` 的第十字节规则与 Go `encoding/binary.Uvarint` 对齐；新增快捷路径时不能只检查字节数而遗漏该值域约束。

comparable 无符号解码拒绝 `[0, 7]` 的负标签，载荷短于标签声明长度时报不足。comparable 有符号解码还拒绝两类非规范符号表示：正标签承载超过 `i64::MAX` 的值，以及负标签解出非负半区的值。相关错误经过 `traceError` 后仍可通过 cause 找到稳定 sentinel；`pkg/util/codec/number_test.rs` 用指针身份验证这一点。

编码函数覆盖 `i64`/`u64` 全值域且没有可返回错误。它们可能因内存分配失败而由 Rust 运行时终止，但 API 不把分配失败表示为 `Result`。本文件不检查编码是否为最短形式；解码接受的格式边界以标签、长度和符号半区检查为准。

## 并发与资源生命周期

所有转换函数都不持有锁、不启动任务、不使用通道，也不跨调用保存缓冲区；只要调用方对各自的 `Vec`/切片遵循 Rust 所有权规则，函数天然可并发调用。解码返回的后缀只是原输入的借用，不延长底层存储生命周期。

两个错误 sentinel 的 `LazyLock` 使用标准库线程安全的一次初始化。并发首次触发时只构造一个底层错误实例；之后 clone 共享身份而不暴露可变状态。函数结束时局部累积变量立即释放，编码结果所有权交还调用方，没有需要显式关闭或回滚的资源。

性能上，固定宽度路径为常数时间并追加 8 字节；varint/comparable 路径最多处理 10 或 9 个字节，同样有严格上界。传入容量不足可能触发 `Vec` 重新分配，因此高频调用方可预留容量，但不能绕过公开编码器自行复制协议而造成格式漂移。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/codec/number.go`。Rust 保留了 Go 的函数名、三组格式、标签常量、错误文本、返回后缀语义和数值排序规则。Go 的 `binary.BigEndian` 对应 Rust 的 `to_be_bytes`/`from_be_bytes`；Go `binary.PutVarint`/`Varint` 的语义由 Rust 的 ZigZag 逻辑和 `decodeUvarintPrefix` 手工实现；Go 对负 comparable 值以 `math.MaxUint64` 开始符号扩展，Rust 对应使用 `u64::MAX`。

API 形态差异来自语言：Go 接受/返回 `[]byte` 并单独返回 `error`，Rust 编码取得 `Vec<u8>` 所有权，解码返回 `Result<(&[u8], T), SharedError>`。Go 错误时通常返回 `nil` 后缀和零值；Rust 错误分支只返回错误。`errDecodeInsufficient`/`errDecodeInvalid` 在 Go 是包级变量，在 Rust 是返回 `LazyLock` clone 的私有函数，以维持相同 cause 身份。

尤其需要注意，Go `DecodeComparableVarint` 的单字节内联分支直接 `return b, ...`，即不消费标签；Rust 明确保留这一历史行为，而 `DecodeComparableUvarint` 会正常消费标签。这不是文档推断，而是两份源码和 Rust 独立测试共同确认的兼容约束。

Go `pkg/util/codec/codec_test.go` 的 `TestNumberCodec` 覆盖极值往返及混合 comparable 流，`TestNumberOrder` 覆盖升降序和 comparable 字典序。Rust 的更细粒度对照覆盖在 `pkg/util/codec/float_2_aster_unit_test.rs`，错误 sentinel 身份及极值序关系另见 `pkg/util/codec/number_test.rs`。

## 扩展指南

若扩展既有格式，先判断需求属于固定宽度、普通 varint 还是 memcomparable，避免仅因“更短”就替换持久化键编码。修改 `signMask`、大端顺序、取反位置、`negativeTagEnd`、`positiveTagStart`、长度阈值或错误文本都会影响 Go 互操作、历史数据读取或键排序，应视为格式兼容变更。

新增或修改公开函数时，应继续在 `number.rs` 实现，由 `pkg/util/codec/lib.rs` 的现有 `pub use number::*` 导出；除非模块边界确实变化，无需增加另一套再导出。错误应沿用 `crate::errors::SharedError`，需要兼容 Go sentinel 的错误必须复用稳定实例并验证 `Cause` 身份，不能每次临时 `New`。

测试逻辑必须保持在独立文件。首选同步更新 `pkg/util/codec/number_test.rs` 或 `pkg/util/codec/float_2_aster_unit_test.rs`，并与 `pkg/util/codec/codec_test.go` 的向量和排序意图核对。至少覆盖：全值域边界附近的往返、前缀追加、成功后的后缀、升序/降序字典序、十字节 varint 溢出、截断载荷、非法标签/符号半区，以及 comparable 单字节分支的特殊不消费契约。

对性能敏感的扩展应保持有界循环并避免中间分配；对协议敏感的扩展应先给出 Go 与 Rust 的相同字节向量。若打算“修正”单字节 comparable signed 解码的后缀推进，必须先审计所有混合流消费者并以显式版本或迁移方案处理，不能在现有函数内静默改变。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/codec/number.rs` 确认目标文件含 31 个符号；`node --file pkg/util/codec/number.rs --offset 1 --limit 500` 返回完整 434 行源码并报告 41 个 used-by 文件；`query EncodeComparableVarint --kind function --json` 区分了 Go、Rust 和 Lightning 的同名实现。函数级 `callers/callees` 查询 30 秒内无结果，调用点改由精确源码检索核验。
- 生产源码：`pkg/util/codec/number.rs`（全部常量、辅助函数和公开 API）、`pkg/util/codec/lib.rs`（模块装载、再导出和 `errors` 边界）、`pkg/tablecodec/tablecodec.rs`、`pkg/structure/type.rs`、`pkg/planner/property/physical_property.rs`、`pkg/store/mockstore/unistore/tikv/mvcc/tikv.rs` 与 `mvcc.rs`（代表性直接调用点）。
- crate 声明：`pkg/util/codec/Cargo.toml`，确认 crate 名为 `astersql-util-codec`、库入口为 `lib.rs`、错误依赖来自 `../../errors`，且该文件没有专属 feature。
- Go 对照：`pkg/util/codec/number.go`；Go 测试证据：`pkg/util/codec/codec_test.go` 的 `TestNumberCodec`、`TestNumberOrder`。
- Rust 独立测试：`pkg/util/codec/number_test.rs` 的错误身份与极值排序测试；`pkg/util/codec/float_2_aster_unit_test.rs` 的固定宽度、普通 varint、comparable 边界向量、后缀、非法输入和排序测试。
- 本任务是只读代码分析加 Markdown 新增，按任务约束未运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核唯一生产物、路径和兼容性陈述。
