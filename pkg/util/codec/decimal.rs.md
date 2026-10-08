# `pkg/util/codec/decimal.rs`

## 文件定位

本文件属于 Cargo crate `astersql-util-codec`（`pkg/util/codec/Cargo.toml`），由 `pkg/util/codec/lib.rs` 的私有模块 `decimal` 通过 `include!("decimal.rs")` 纳入 crate，并通过 `pub use decimal::*` 暴露公开函数。它位于通用 Datum 编解码层：上游把 `types::MyDecimal` 连同精度信息交给这里，本文件负责生成或解析 TiDB/MySQL DECIMAL 的可比较二进制载荷；外围 `codec.rs` 再添加 `decimalFlag` 等 Datum 类型标记。

文件只实现 DECIMAL 载荷，不定义十进制定点算法。二进制体的实际布局和数值转换由 `astersql-types-decimal` 提供的 `MyDecimal::{WriteBin, FromBin}` 与 `DecimalBinSize` 决定；本文件增加两字节 `precision`/`frac` 头、输入检查和错误适配。源码中没有类型、trait、常量或条件编译项，只有三个函数。

## 核心职责

1. `EncodeDecimal` 把 DECIMAL 追加到已有 `Vec<u8>`：先写一字节总有效位数和一字节小数位数，再调用 `MyDecimal::WriteBin` 写定长、可按字典序比较的数值体（`decimal.rs:27-50`）。
2. `valueSizeOfDecimal` 在不实际编码的情况下计算同一载荷的长度，即 `DecimalBinSize(precision, frac) + 2`，供通用 Datum 尺寸估算使用（`decimal.rs:53-66`；`codec.rs:278`）。
3. `DecodeDecimal` 校验输入、读取两字节头、确定数值体长度、调用 `MyDecimal::FromBin`，并同时返回未消费的后缀和头部元数据（`decimal.rs:72-91`）。返回后缀使该格式可以嵌入连续编码的 Datum 流。

## 主要符号

- `pub fn EncodeDecimal(mut b: Vec<u8>, dec: &types::MyDecimal, mut precision: i32, mut frac: i32) -> Result<Vec<u8>, errors::SharedError>`：公开编码入口。`precision == 0` 表示调用方未指定元数据，此时同时用 `dec.PrecisionAndFrac()` 覆盖 `precision` 和 `frac`；随后把过大的 `frac` 限制到 `mysql::MaxDecimalScale`，写头并委托 `WriteBin`。已有前缀保留在返回向量中。
- `pub(crate) fn valueSizeOfDecimal(dec: &types::MyDecimal, mut precision: i32, mut frac: i32) -> Result<usize, errors::SharedError>`：crate 内尺寸入口，与编码入口共享“`precision == 0` 时从值推导”的规则。它不改变输入 decimal，也不分配编码缓冲区。
- `pub fn DecodeDecimal(b: &[u8]) -> Result<(&[u8], types::MyDecimal, i32, i32), errors::SharedError>`：公开解码入口。成功元组依次为剩余切片、拥有所有权的 decimal、precision、frac；剩余切片借用原输入。
- `errorInDecodeDecimal`：`DecodeDecimal` 开头的 `fail::fail_point!` 名称，不是普通 Rust 符号。启用时立即返回消息为 `gofail error` 的错误（`decimal.rs:73`）。

## 执行流程

编码流程如下：调用方提供已有缓冲区、decimal 和元数据；若 `precision` 为零，函数从 decimal 同时推导精度与小数位；若 `frac` 超过 MySQL 上限则截到 `MaxDecimalScale`；函数依次追加 `precision as u8`、`frac as u8`，再把整个缓冲区交给 `WriteBin`；底层状态成功时返回新缓冲区，失败时转成 `SharedError`。`codec.rs::encode` 在进入该函数前写入 `decimalFlag`，而直接用于 PB 值时可从空缓冲区开始（`codec.rs:104-106`；`pkg/expression/expr_to_pb.rs:171-177`）。

尺寸流程复用精度推导规则，用 `DecimalBinSize` 得到数值体长度并加两字节头。通用 `EstimateValueSize` 还会再加一字节 Datum 类型标记，因此测试比较的是 `EncodeDecimal` 结果长度加一（`codec.rs:277-279`；`codec_test.rs:723-734`）。

解码流程先执行故障注入点；输入少于三字节时立即报错，因为格式至少需要两字节头和一个数值体字节。函数取出头部，以 `DecimalBinSize` 计算预期体长并再次检查剩余长度；然后在默认 `MyDecimal` 上调用 `FromBin`。只有底层状态成功才返回 `&body[consumed..]`，因此串联数据不会被误吞（`decimal.rs:73-90`）。`codec.rs::DecodeOne` 把解出的值、precision 和 frac 写回 Datum；面向 Chunk 的解码路径还会按目标字段小数位做后续舍入（`codec.rs:1371-1376`、`1662-1667`）。

## 数据与状态

线格式是 `[precision: u8][frac: u8][MyDecimal binary body]`。头部长度恒为两字节，数值体长度由 `DecimalBinSize(precision, frac)` 唯一决定；`WriteBin`/`FromBin` 承担符号、分组和可比较序编码等细节。`precision` 表示总有效位数，`frac` 表示小数位数。调用方以零 precision 请求自动推导时，传入的 frac 也会被 decimal 自身值覆盖。

`EncodeDecimal` 消费并返回 `Vec<u8>`，可在已有前缀后原地继续追加；`DecodeDecimal` 不复制输入后缀，只返回借用切片，但构造一个新的 `MyDecimal`。除 failpoint 的进程级测试配置外，本文件没有全局可变状态、缓存或持久化状态。`valueSizeOfDecimal` 是纯尺寸计算，前提是底层 `DecimalBinSize` 接受给定元数据。

## 依赖与调用关系

crate 边界由 `pkg/util/codec/Cargo.toml` 定义：本文件直接使用 `astersql-errors` 的兼容命名空间、`astersql-parser-mysql` 的 `MaxDecimalScale`、`astersql-types-decimal` 再导出的 decimal API，以及启用 `failpoints` feature 的 `fail` crate；这些名称由 `lib.rs` 的 `errors`、`mysql`、`types` 模块引入 `decimal` 子模块。

RustCodeGraph 显示主要上游边为：`codec.rs::encode`、`codec.rs::HashGroupKey` 和测试调用 `EncodeDecimal`；两个 `codec.rs::DecodeOne` 路径调用 `DecodeDecimal`；`codec.rs::EstimateValueSize` 调用 `valueSizeOfDecimal`。仓库中的进一步直接使用包括 `pkg/expression/expr_to_pb.rs` 编码 MySQL decimal、`pkg/expression/pb_to_expr_runtime.rs` 解码 PB 表达式，以及 `pkg/expression/distsql_builtin.rs::codec` 的编码再导出和解码适配。`pkg/tablecodec/tablecodec_test.rs` 证明通用 `DecodeOne` 会把本文件的 failpoint 错误继续传出。

下游边虽然未被图索引解析成调用边，但源码直接可见且由类型 crate 提供：`PrecisionAndFrac` 推导元数据，`WriteBin` 编码，`DecimalBinSize` 校验并计算长度，`FromBin` 解码，`errors::New` 把底层错误文本适配为共享错误。

## 错误处理与边界

- 编码时，`WriteBin` 的截断、溢出或非法精度错误被转换为 `errors::SharedError`。`codec_test.rs:738-744` 分别以 `precision=20, frac=5` 和 `precision=12, frac=10` 验证不适配数值会失败，并验证错误会穿过 `EncodeValue`。
- `EncodeDecimal` 只把超过 `MaxDecimalScale` 的 frac 向下限制；它没有显式检查负数、precision 与 frac 的关系或 `i32 -> u8/isize` 转换范围，这些组合是否合法由调用约定和 `WriteBin` 决定。扩展时不能把当前强制转换误写成完整的输入验证。
- `valueSizeOfDecimal` 传播 `DecimalBinSize` 错误，但不像 `EncodeDecimal` 那样先限制超大 frac；因此调用方应提供与实际编码相同的合法元数据，修改任一入口的归一化规则时必须检查两者长度一致性。
- 解码在读取头前要求 `b.len() >= 3`，再按计算出的 `bin_size` 检查 body，避免切片越界。非法头会由 `DecimalBinSize` 报错；非法数值体会由 `FromBin` 报错。
- Rust 版本在调用 `FromBin` 前检查完整 body，并只在成功后计算剩余切片。Go 版本直接使用 `FromBin` 返回的消费长度再切片；这是更显式的 Rust 边界保护，但成功格式和错误传播目标一致。

## 并发与资源生命周期

三个函数都没有锁、线程、异步任务、通道、事务或 I/O，输入 decimal 仅以不可变引用访问，因此普通调用之间没有共享可变数据竞争。编码缓冲区所有权移入 `EncodeDecimal` 并在成功时移出；如果 `WriteBin` 返回错误，局部缓冲区随错误路径释放，调用者拿不到部分编码结果。解码结果中的剩余切片生命周期绑定到输入字节，decimal 则独立拥有。

唯一跨调用状态是 `fail` crate 管理的 `errorInDecodeDecimal` 配置，它只服务故障注入。测试必须成对启用/移除；`pkg/tablecodec/tablecodec_test.rs:410-412` 使用 `fail::cfg` 后立即 `fail::remove`，表明该状态不能遗留给其他测试。

## 与 Go 版本的对应关系

`pkg/util/codec/decimal.go` 同样包含 `EncodeDecimal`、`valueSizeOfDecimal` 和 `DecodeDecimal`，Rust 版本逐项保留了：precision 为零时从值推导 precision/frac、编码时限制最大 frac、两字节头、`WriteBin`/`DecimalBinSize`/`FromBin` 委托、解码 failpoint 和剩余字节返回。

类型层面的差异是 Go 使用 `[]byte`、`*types.MyDecimal`、`int` 和多返回值，Rust 使用拥有所有权的 `Vec<u8>`、借用/拥有的 `MyDecimal`、`i32` 元数据和 `Result`。Go 用 `errors.Trace` 保留错误链，Rust 当前以 `error.to_string()` 重建 `SharedError`，因此错误分类身份未在此层显式保留。Go 解码错误时可能返回已推进的切片、nil decimal 及已读元数据；Rust 的 `Result::Err` 不携带这些部分结果。

`pkg/util/codec/decimal_test.rs` 对齐 `decimal_test.go` 的两组测试：多种正负数和零值的编码/解码比较，以及整数 3、decimal 0.03 的字符串小数位保持。Rust 额外直接证据包括 `bytes_1_aster_unit_test.rs:103-118` 的既有前缀、自动推导和短输入测试，`codec_test.rs:719-744` 的字典序、估算长度及截断/溢出测试，`tablecodec_test.rs:410-412` 的故障注入传播测试。

## 扩展指南

若修改线格式或元数据规则，应同时修改 `EncodeDecimal`、`DecodeDecimal` 和 `valueSizeOfDecimal`，并确认 `codec.rs` 的类型标记、Datum 元数据回填及 Chunk 舍入仍兼容；线格式已被表达式 PB、table codec 和排序键路径使用，改变头部或 body 会有跨版本兼容风险。若只新增校验，先明确 Go 行为并避免在尺寸函数、编码函数和解码函数之间产生不一致。

测试逻辑必须继续放在独立文件，优先扩展 `pkg/util/codec/decimal_test.rs`；排序、尺寸和通用 Datum 接线应同步 `pkg/util/codec/codec_test.rs`，故障注入传播应同步 `pkg/tablecodec/tablecodec_test.rs`。Go 对齐修改还应核对对应的 `decimal_test.go`、`codec_test.go` 和 `tablecodec_test.go`。应重点加入：最大 precision/frac、非法或截断 body、串联载荷的剩余切片、已有缓冲区前缀、负数排序，以及 `valueSizeOfDecimal` 与真实编码长度的一致性。

性能上，避免把当前追加式 `Vec` 编码改成额外复制；解码应继续借用剩余输入。正确性上，不要自行重写 `MyDecimal` 二进制算法，应在 `astersql-types-decimal` 中处理其内部格式并由本层只维护封装契约。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`files --filter pkg/util/codec` 确认目标、Go 对照和测试均已索引；`node --file pkg/util/codec/decimal.rs` 确认文件共 91 行及三个函数；`node/callers/callees` 查询 `EncodeDecimal`、`DecodeDecimal`、`valueSizeOfDecimal` 确认直接上游边。图未识别 decimal 类型方法的下游边，改由函数体与 Cargo 再导出核对。
- 源码与装配：`pkg/util/codec/decimal.rs`、`pkg/util/codec/lib.rs:157-162,193-195`、`pkg/util/codec/Cargo.toml`、`pkg/util/codec/codec.rs:104-106,277-279,1371-1376,1662-1667,1814-1823`。
- Go 对照：`pkg/util/codec/decimal.go`、`pkg/util/codec/codec.go`、`pkg/expression/expr_to_pb.go:164-172`、`pkg/expression/distsql_builtin.go:1364-1370`。
- 独立测试：`pkg/util/codec/decimal_test.rs` 与 `decimal_test.go`；补充边界证据来自 `pkg/util/codec/bytes_1_aster_unit_test.rs:103-118`、`codec_test.rs:719-744` 和 `pkg/tablecodec/tablecodec_test.rs:410-412`，对应 Go 测试亦已核对。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本文所有关键结论均可回到上述符号、调用点、Cargo 声明或测试。
