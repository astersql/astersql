# `pkg/util/codec/float.rs`

[源文件](./float.rs) · [crate 清单](./Cargo.toml) · [Go 对照实现](./float.go)

## 文件定位

本文件属于 `astersql-util-codec` crate（`pkg/util/codec/Cargo.toml`）。crate 入口 `pkg/util/codec/lib.rs` 以私有 `mod float` 声明模块，再用 `pub use float::*` 将四个公开函数导出到 crate 根：`EncodeFloat`、`DecodeFloat`、`EncodeFloatDesc` 和 `DecodeFloatDesc`。因此调用方通常写成 `codec::EncodeFloat`，不直接依赖 `float` 模块路径。

它位于 SQL 值与稳定字节表示的边界：把 `f64` 转换为固定 8 字节、可按字节字典序比较的形式，供 Datum 编解码、表达式 protobuf 常量、rowcodec 行格式和规划属性指纹复用。文件不负责类型标签；例如 `pkg/util/codec/codec.rs` 的 `Encoder::encode` 先写入 `floatFlag`，再调用这里的编码函数。

## 核心职责

1. `encodeFloatToCmpUint64` 把 IEEE 754 `f64` 位模式映射到可比较的 `u64`：对满足 `f >= 0.0` 的值设置最高位，对其余值按位取反。
2. `decodeCmpUintToFloat` 根据最高位撤销上述变换，再以 `f64::from_bits` 恢复浮点值。
3. `EncodeFloat` / `DecodeFloat` 借助大端无符号整数编解码，提供升序 mem-comparable 表示。
4. `EncodeFloatDesc` / `DecodeFloatDesc` 借助无符号整数的降序编解码，在相同数值范围上反转字节序关系。

对普通数值（包括正负无穷）而言，升序编码的字节字典序与数值顺序一致，降序编码相反；`pkg/util/codec/float_2_aster_unit_test.rs::floats_round_trip_and_encode_in_go_memcomparable_order` 覆盖这两个不变量。NaN 和带符号零有专门边界，不能套用“所有 `f64` 位模式均可逆”的结论。

## 主要符号

- `fn encodeFloatToCmpUint64(f: f64) -> u64`：内部正向位变换。`signMask` 来自 `pkg/util/codec/number.rs`，值为最高位掩码。非负分支保留其余 63 位并设置最高位；负分支翻转全部 64 位，使更负的普通数值产生更小的无符号值。
- `fn decodeCmpUintToFloat(mut u: u64) -> f64`：内部逆变换。若最高位已设置则清除该位，否则翻转全部位，最后用 `f64::from_bits` 解释结果。
- `pub fn EncodeFloat(b: Vec<u8>, v: f64) -> Vec<u8>`：在已有缓冲区末尾追加 `encodeFloatToCmpUint64(v)` 的 8 字节大端表示；由 `EncodeUint` 完成追加。
- `pub fn DecodeFloat(b: &[u8]) -> Result<(&[u8], f64), errors::SharedError>`：从输入开头消费 8 字节，返回未消费后缀和解码值；由 `DecodeUint` 检查长度并读大端整数。
- `pub fn EncodeFloatDesc(b: Vec<u8>, v: f64) -> Vec<u8>`：先执行相同浮点位变换，再由 `EncodeUintDesc` 对可比较整数取反并写成 8 字节大端形式。
- `pub fn DecodeFloatDesc(b: &[u8]) -> Result<(&[u8], f64), errors::SharedError>`：由 `DecodeUintDesc` 读取并撤销降序整数取反，再恢复浮点值。

本文件没有类型、trait、`impl`、模块级常量或条件编译项；两个位变换函数为私有实现，四个编解码函数是公开 API。

## 执行流程

升序编码流程为：调用者传入可继续追加的 `Vec<u8>` 和 `f64`；`encodeFloatToCmpUint64` 取得 `to_bits()`；普通非负值设置符号位，普通负值全位取反；`EncodeUint` 将所得 `u64::to_be_bytes()` 追加到缓冲区。映射把负数放在最高位未设置的半区，把非负数放在最高位已设置的半区，并在各自半区保持普通数值顺序。

升序解码流程为：`DecodeUint` 要求至少 8 字节，读取前 8 字节为大端 `u64` 并返回 `&b[8..]`；`decodeCmpUintToFloat` 检查最高位，分别执行清最高位或全位取反；结果通过 `f64::from_bits` 恢复。输入存在额外字节时不会被吞掉，测试以 `0xaa` 后缀验证这一点。

降序路径仅在整数层多一次互补：`EncodeUintDesc` 写出 `!u` 的大端字节，`DecodeUintDesc` 读回后再取反。因此它复用完全相同的浮点映射，却把字节比较顺序反转；测试以 `0xbb` 后缀验证降序解码同样只消费 8 字节。

## 数据与状态

编码结果始终追加 8 字节，不带长度或类型标签。函数不维护全局状态；所有中间状态仅为栈上的 `u64`。编码接管并返回传入的 `Vec<u8>`，可在同一缓冲区连续追加；解码借用输入切片并返回其后缀，不复制后缀。

关键边界由 `pkg/util/codec/float_2_aster_unit_test.rs::float_bit_edges_follow_go_sign_transform` 固化：Rust 与 Go 的 `f >= 0` 对 `-0.0` 均为真，所以 `-0.0` 与 `+0.0` 编码相同，解码统一得到正零。该条件对所有 NaN 为假；负号 NaN 按当前测试向量可逐位往返，而正号 NaN 经负数分支编码后，解码为“原位模式取反并清最高位”的值，不保持 NaN 位模式。上层 `pkg/expression/expr_to_pb.rs` 因此显式拒绝把负零下推为 protobuf 浮点常量。

## 依赖与调用关系

直接下游依赖全部来自同一 crate 根再导出：`EncodeUint`、`DecodeUint`、`EncodeUintDesc`、`DecodeUintDesc`、`signMask` 和 `errors`。`pkg/util/codec/number.rs` 表明整数编码采用大端固定宽度；两种解码器在长度不足 8 时返回错误，成功时只消费前 8 字节。`pkg/util/codec/Cargo.toml` 没有为本文件设置 feature 开关；错误类型经 crate 的 `errors` 模块从 `astersql-errors` 再导出。

直接生产调用证据包括：

- `pkg/util/codec/codec.rs`：`Encoder::encode`、`Encoder::HashCode` 等 Datum 路径写入浮点，`DecodeOne`、`DecodeAsFloat32` 等路径恢复浮点。
- `pkg/expression/expr_to_pb.rs` 与 `pkg/expression/pb_to_expr_runtime.rs`：序列化和反序列化 Float32/Float64 protobuf 常量；`pkg/expression/distsql_builtin.rs` 还将解码错误转换为 `ExpressionError`。
- `pkg/util/rowcodec/encoder.rs` 与 `pkg/util/rowcodec/decoder.rs`：行格式中的 FLOAT/DOUBLE 字段编码和解码。
- `pkg/planner/property/physical_property.rs::buildHashCode`：把 `ExpectedCnt`、`AvgInnerRowCnt` 编入物理属性指纹。

RustCodeGraph 将 `float.rs` 标为被 9 个文件使用，但对经 crate 根再导出的四个公开函数未生成跨模块 caller 边；上述调用点由仓库文本引用复核。图内 callee 边确认两个公开编码函数调用 `encodeFloatToCmpUint64`，两个公开解码函数调用 `decodeCmpUintToFloat`；整数 API 的调用因再导出同样未完整出现在这些边中，源码签名提供直接证据。

## 错误处理与边界

编码函数不返回错误。解码函数唯一的可恢复失败来自底层固定宽度整数解码：输入少于 8 字节时返回 `SharedError("insufficient bytes to decode value")`。本文件用 `errors::Trace(Some(error)).expect(...)` 保留该错误；传入值明确是 `Some(error)`，所以此处的 `expect` 不依赖外部输入形成 `None`。

长度达到 8 字节后，任意 64 位模式都可被解释为某个 `f64`，因此没有“非法浮点字节”分支。多余输入作为剩余切片返回。调用者必须使编码和解码方向成对；用 `DecodeFloat` 读取降序编码（或反之）不会触发格式错误，却会得到错误数值。

排序保证应限定为遵守通常数值顺序的非 NaN 值；NaN 没有普通全序，当前映射也不是所有 NaN 位模式上的双射。`-0.0` 的符号位会丢失。若上层业务要求逐位身份、NaN payload 保真或区分带符号零，应使用其他表示或在进入本 API 前单独处理。

## 并发与资源生命周期

所有函数均为无共享状态的同步纯计算（除拥有的输出缓冲区增长外），没有锁、原子变量、任务、通道、事务、I/O 或缓存，因此可由多个线程独立调用。编码沿用传入 `Vec<u8>` 的容量，空间不足时可能由 `Vec` 重新分配；函数返回新的所有权，不保留别名。

解码结果中的剩余切片与输入拥有相同生命周期，不分配浮点载荷，也不持有输入之外的资源。固定工作量是常数时间；每次调用处理恰好 8 个载荷字节。资源生命周期风险主要在调用方：必须在使用返回切片期间保持原输入有效，并使用返回的剩余切片推进复合编码流。

## 与 Go 版本的对应关系

`pkg/util/codec/float.go` 是逐函数对照：Rust 的 `f64::to_bits` / `f64::from_bits` 对应 Go 的 `math.Float64bits` / `math.Float64frombits`，`!`、`signMask` 分支以及升序/降序整数复用逻辑一致。API 形态差异来自语言：Go 使用可追加的 `[]byte` 并以三元组返回解码错误；Rust 获取 `Vec<u8>` 所有权，并用 `Result<(&[u8], f64), SharedError>` 表达成功或失败。

Go 的 `pkg/util/codec/codec_test.go::TestFloatCodec` 验证普通值、最值、最小正数和正负无穷的往返与排序。Rust 的 `pkg/util/codec/codec_test.rs::TestFloatCodec` 移植同类向量；独立的 `pkg/util/codec/float_2_aster_unit_test.rs` 进一步验证后缀保留、短输入错误、更多严格序列，以及 Go `f >= 0` 语义导致的 `±0` 和 NaN 行为。现有 Rust 逻辑没有自行“修正”这些 Go 边界，因此跨语言字节兼容性优先于 IEEE 位模式全面保真。

## 扩展指南

- 修改排序映射时，应同时审查 `encodeFloatToCmpUint64` 和 `decodeCmpUintToFloat`，保持互逆关系，并确认 Go `pkg/util/codec/float.go` 的字节兼容要求。既有持久化键、行编码、哈希或 protobuf 值可能依赖当前 8 字节格式，不能只做局部算法替换。
- 修改升序或降序格式时，应成对修改对应的 encode/decode API，并检查 `pkg/util/codec/number.rs` 的大端和取反约定；降序路径不应另建一套浮点映射。
- 新增行为测试应放在独立测试文件 `pkg/util/codec/float_2_aster_unit_test.rs`，不要嵌入 `float.rs`。至少同步覆盖升序/降序往返、字节序、后缀推进、少于 8 字节的错误、`±0`、两类符号 NaN、无穷与相邻极小值；若要求与 Go 对齐，也应同步评估 `pkg/util/codec/codec_test.go`。
- 调整错误类型或错误消息时，应审查 `pkg/expression/distsql_builtin.rs` 的错误转换、`pkg/util/codec/codec.rs` 的 `?` 传播及 rowcodec 调用方。
- 性能变更应保留固定宽度、常数时间和缓冲区追加模式；避免为单个浮点引入临时堆分配。涉及 planner 指纹时还要验证相同属性产生稳定字节，避免缓存键兼容性变化。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/util/codec` 确认实现、Go 对照和独立测试；`node --file pkg/util/codec/float.rs` 读取完整 80 行实现；对六个符号执行 `query`，对四个公开 API 和两个私有变换执行 `callers` / `callees`；另以 `node` 核验 `number.rs` 的四个整数 API。
- 源码与装配：`pkg/util/codec/float.rs`、`pkg/util/codec/lib.rs`、`pkg/util/codec/number.rs`、`pkg/util/codec/Cargo.toml`。
- Go 对照：`pkg/util/codec/float.go`、`pkg/util/codec/codec_test.go::TestFloatCodec`。
- Rust 测试：`pkg/util/codec/float_2_aster_unit_test.rs::floats_round_trip_and_encode_in_go_memcomparable_order`、`float_bit_edges_follow_go_sign_transform`，以及 `pkg/util/codec/codec_test.rs::TestFloatCodec`。
- 上游调用：`pkg/util/codec/codec.rs`、`pkg/expression/expr_to_pb.rs`、`pkg/expression/pb_to_expr_runtime.rs`、`pkg/expression/distsql_builtin.rs`、`pkg/util/rowcodec/encoder.rs`、`pkg/util/rowcodec/decoder.rs`、`pkg/planner/property/physical_property.rs`。
- 本任务只新增说明文档，不运行 Cargo。交付结构检查要求目标文件存在，并且十一个固定二级标题各出现一次。
