# `pkg/types/vector.rs`

## 文件定位

本文件实现 AsterSQL Rust 类型系统中的 `VECTOR(FLOAT32)` 基础值对象：构造、维度校验、元素视图、文本转换和小端线格式编解码。它并不是由顶层 `pkg/types/lib.rs` 直接声明；真实装配路径是 `pkg/types/internal/vector/lib.rs` 以 `#[path = "../../vector.rs"] mod vector` 纳入 `astersql-types-vector` 子 crate，再由 `pkg/types/lib.rs` 以 `pub use types_vector as vector` 暴露为 `astersql_types::vector`。该子 crate 的边界和依赖由 `pkg/types/internal/vector/Cargo.toml` 定义，顶层 `pkg/types/Cargo.toml` 通过路径依赖 `types-vector` 接入。

它处于 SQL 向量值的公共表示层：上游包括会话 DDL/DML、表达式求值、Datum、chunk、row codec、通用 codec、序列化和聚合 spill；相邻的 `pkg/types/vector_functions.rs` 在同一个 `VectorFloat32` 上补充距离、算术和比较方法。本文件本身不实现向量距离或索引算法。

## 核心职责

- 用 `VectorFloat32 { data: Vec<u32> }` 保存一个维度头和若干 `f32` 位型，使内存布局与 Go 的小端线格式一致，同时保证 Rust 中 `f32` 视图所需的对齐。
- 通过 `CreateVectorFloat32`、`MustCreateVectorFloat32`、`InitVectorFloat32` 和 `ZeroVectorFloat32` 提供受校验或直接分配的构造路径。
- 通过 `CheckVectorDimValid` 与 `VectorFloat32::CheckDimsFitColumn` 分别校验类型允许的全局维度范围和列声明的精确维度。
- 通过 `String`、`TruncatedString` 与 `ParseVectorFloat32` 完成 SQL/日志使用的 JSON 数组文本互转。
- 通过 `ZeroCopySerialize`、`SerializeTo`、`PeekBytesAsVectorFloat32` 和 `ZeroCopyDeserializeVectorFloat32` 实现“维度 `u32` + N 个 `f32`”的线格式。
- 提供 `SerializedSize`、`EstimatedMemUsage`、`Clone` 和 `IsZeroValue` 等值对象辅助能力。

## 主要符号

- `init()`: 断言本机原生字节序符合小端假设；编译期另有 `#[cfg(not(target_endian = "little"))] compile_error!`。Rust 不会按函数名自动调用 `init`，因此真正的硬保证是条件编译错误。
- `VectorFloat32`: 唯一生产类型，内部 `Vec<u32>` 的第 0 个 word 是维度，其余 word 保存各 `f32` 的原始位型。未实现标准 `Clone` trait，而是保留 Go 风格的固有方法 `Clone()`。
- `CreateVectorFloat32(&[f32]) -> Result<VectorFloat32, SharedError>`: 拒绝 NaN 和正负无穷后复制输入。
- `MustCreateVectorFloat32(&[f32]) -> VectorFloat32`: 对上述构造错误 panic，适合已知常量或测试，不适合不可信输入。
- `InitVectorFloat32(i32) -> VectorFloat32`: 分配 `dims + 1` 个 word、写入维度并把元素清零；负维度和分配长度溢出会 panic。
- `CheckVectorDimValid(i32)`: 接受 `0..=16_383`。注意 `InitVectorFloat32` 自身只检查非负，不执行 16,383 上限校验，调用者在类型定义或外部输入边界应显式调用本函数。
- `Len`、`Elements`、`ElementsMut`: 分别读取头部维度，以及把后续 `u32` 存储零拷贝重解释成不可变/可变 `f32` 切片。
- `String`、`TruncatedString`: 前者用 Go `strconv.FormatFloat(..., 'f', -1, 32)` 语义输出可再次解析的完整数组；后者最多显示五项，用 `'g'`、精度 2 输出，并附加剩余项数。
- `ZeroCopySerialize`: 返回底层 word 缓冲的字节视图；`SerializeTo` 把该视图追加到已有 `Vec<u8>`。
- `PeekBytesAsVectorFloat32`: 仅根据 4 字节维度头计算当前值应占用的前缀长度；长度运算提升到 `u64`，避免 Go 回归输入 `0x4000_0000` 发生 32 位回绕。
- `ZeroCopyDeserializeVectorFloat32`: 校验可用前缀后逐 word 小端解码到新的对齐 `Vec<u32>`，并返回未消费后缀。
- `ParseVectorFloat32`: 仅接受 JSON 数值数组，拒绝 `null`、非数组/残缺 JSON、NaN、无穷、超出 `f32` 范围和超过维度上限的输入。

## 执行流程

文本入口的主流程是：`ParseVectorFloat32` 先特判去除首尾空白后的 `null`，再由 `serde_json` 解析为 `Vec<f64>`；逐项检查非有限值与 `f32` 范围并收窄为 `f32`；把长度转换为 `i32` 并调用 `CheckVectorDimValid`；最后使用 `InitVectorFloat32` 分配，再通过 `ElementsMut().copy_from_slice` 填充。`pkg/expression/builtin_vec.rs`、`pkg/session/runtime/dml.rs` 等调用者会在解析后继续用 `CheckDimsFitColumn` 核对目标 SQL 类型的 `flen`。

程序内构造入口 `CreateVectorFloat32` 直接扫描 `&[f32]`，拒绝 NaN/Inf，然后分配并复制。它不调用 `CheckVectorDimValid`，所以负责处理任意规模外部集合的调用者必须先校验；`pkg/session/runtime/dml.rs` 和 `pkg/expression/builtin_inference.rs` 展示了“先 `CheckVectorDimValid`，再 `CreateVectorFloat32`”的接线。

线格式读取先由 `PeekBytesAsVectorFloat32` 要求至少 4 字节，按小端读取维度，以 `u64(dim) * 4 + 4` 计算本值长度，并确认输入足够。`ZeroCopyDeserializeVectorFloat32` 随后只消费这个前缀，把每个 4 字节块转换为 `u32`，把剩余切片原样返回；因此 codec 或连续值流可以精确推进一个值。写入方向由 `ZeroCopySerialize` 暴露字节视图，`SerializeTo` 将其追加到调用者缓冲区。

## 数据与状态

有效实例的不变量是 `data.len() >= 1`，且 `data[0] as usize == data.len() - 1`。由 `InitVectorFloat32`、解析和反序列化产生的实例满足该不变量。每个 word 固定 4 字节，故线格式大小为 `4 + Len() * 4`；零维向量仍占 4 字节，内容为全零维度头。

`Elements`/`ElementsMut` 的 `unsafe` 转换依赖 `u32` 与 `f32` 尺寸、对齐相同，并利用“任意 32 位位型均可作为 `f32`”这一性质。可变视图会直接改变后续序列化结果。`Clone()` 深拷贝底层缓冲，测试 `vector_clone_is_independent` 证明修改副本不会回写原值。

`Default` 生成空 `Vec<u32>`，它适合占位和 `SerializedSize()` 等不读取头部的场景，但不满足上述有效值不变量；对默认值调用 `Len`、`Elements`、`String` 或 `IsZeroValue` 会因索引 `data[0]` panic。语义上的零值应使用 `ZeroVectorFloat32()`，而不是 `VectorFloat32::default()`。

## 依赖与调用关系

直接依赖为：`crate::errors` 提供 `SharedError`/`errors::New`，`crate::UnspecifiedLength` 支持不限制列维度的哨兵值，`goish::strconv::FormatFloat` 保持 Go 格式化语义，`serde_json` 解析文本。它们分别由 `pkg/types/internal/vector/lib.rs` 和该子 crate 的 `Cargo.toml` 接入。

主要上游调用链包括：

- `pkg/session/runtime/ddl.rs`、`dml.rs`：DDL 维度合法性以及写入/转换时的解析与列维度匹配。
- `pkg/expression/builtin_vec.rs`、`builtin_vec_vec.rs`、`builtin_inference.rs`：标量/向量表达式把文本或 embedding 转换为 `VectorFloat32`。
- `pkg/types/datum.rs` 与 `pkg/types/internal/datum/lib.rs`：Datum 的向量存取、转换、展示和内存估算；后者直接再导出核心向量符号。
- `pkg/util/chunk/column.rs`、`mutrow.rs`，`pkg/util/rowcodec/{encoder,decoder}.rs`，`pkg/util/codec/codec.rs`：行列缓冲和存储编码按该线格式写入、窥探与读取。
- `pkg/expression/pb_to_expr_runtime.rs`、`expr_to_pb.rs`：protobuf 表达式值与线格式互转。
- `pkg/executor/aggfuncs/spill_serialize_helper.rs`：聚合中间结果 spill 的大小核算、序列化和恢复。
- `pkg/types/vector_functions.rs`：直接调用 `Len`、`Elements`、`ElementsMut` 与 `InitVectorFloat32`，为同一类型补充比较、距离和逐元素算术。

RustCodeGraph 的文件节点报告目标文件被 13 个已索引文件直接使用；由于 Rust 与 Go 符号同名导致通用 `callers/callees` 查询不能可靠消歧，以上调用点进一步由精确符号搜索核验。

## 错误处理与边界

可恢复错误统一返回 `errors::SharedError`。文本语法/类型错误归一为 `Invalid vector text: ...`；数值错误区分 NaN、infinite 和超出 `float32` 范围；维度错误区分负数、超过 16,383 以及与 `VECTOR(flen)` 不匹配；线格式错误区分不足 4 字节的头和短于声明长度的主体。

`PeekBytesAsVectorFloat32` 和反序列化只验证结构长度，不调用 `CheckVectorDimValid`，也不拒绝元素中的 NaN/Inf。因此它们能够载入维度大于 SQL 类型上限或含非有限位型的线数据；边界上的类型校验必须由更高层完成，后续 `vector_functions.rs` 的算术会拒绝产生 Inf/NaN 的结果。反序列化错误通过 `Result::Err` 返回，不暴露部分向量；输入借用没有被修改。

若头部声明的长度合法且输入还有额外字节，额外部分不是错误，而作为 remainder 返回。`SerializeTo` 同样保留并追加到既有输出。`EstimatedMemUsage` 是“结构体静态大小 + 已用线格式字节”的估算，不包含 `Vec` 多余 capacity 或分配器开销。

平台边界由编译期小端限制保证。`InitVectorFloat32`、`MustCreateVectorFloat32` 以及从无效 `Default` 值读取属于可 panic 路径；面向不可信输入应使用返回 `Result` 的构造/解析函数并先做维度校验。

## 并发与资源生命周期

该实现没有锁、原子、任务、通道或后台资源。`VectorFloat32` 独占 `Vec<u32>`；只读方法借用 `&self`，修改元素必须持有 `&mut self`，Rust 借用规则阻止同一实例上的并发可变访问。`ZeroCopySerialize` 返回的字节切片生命周期绑定于 `&self`，在借用结束前不能可变借用或释放该向量。

与函数名不同，Rust 的 `ZeroCopyDeserializeVectorFloat32` 为建立对齐的自有存储会复制并小端解码输入前缀；返回的向量不借用输入，因而可以独立存活和修改。它返回的 remainder 仍借用原输入。`Clone()` 也分配独立缓冲，而 `Elements` 和序列化视图不分配。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/vector.go`，回归测试是 `pkg/types/vector_test.go`。两版保持相同的 4 字节小端维度头、`4*N` 元素区、16,383 维上限、NaN/Inf/范围错误、字符串格式、截断显示、前缀消费以及主要错误文本。Rust 的 `pkg/types/vector_test.rs` 复刻 Go 的端序、零向量、解析、Datum、比较和序列化场景；`pkg/types/truncate_12_aster_unit_test.rs` 还覆盖维度边界、格式化、独立克隆、溢出长度和含 NaN 线数据。

实现差异如下：

- Go 的 `VectorFloat32.data` 是 `[]byte`，`Elements()` 通过 unsafe 返回可变 `[]float32`；Rust 用对齐的 `Vec<u32>`，并把只读 `Elements` 与独占可变 `ElementsMut` 分开。
- Go 的 `ZeroCopyDeserializeVectorFloat32` 直接令向量引用输入前缀，要求调用者不得修改输入；Rust 版本会复制到自有缓冲，只保留“精确消费一个前缀”的接口语义，不保留反序列化零拷贝性能特征。
- Go 的 `ZeroVectorFloat32` 是包级变量；Rust 是每次构造新值的函数，避免共享可变底层切片。
- Go `init()` 会在包加载时运行；Rust 同名函数不会自动运行，但非小端目标会被 `compile_error!` 拒绝。
- Rust `serde_json::from_str::<Vec<f64>>` 会自然拒绝数组后的非空白尾随内容，对应 Go 显式检查 parser remainder 的效果。
- Rust 对声明长度的算术显式使用 `u64`，并由 `go_merge_10_vector_dimension_overflow_is_rejected` 及 `vector_deserialize_rejects_overflowing_lengths` 固化 Go 溢出回归语义。

## 扩展指南

新增构造入口时，应复用 `CheckVectorDimValid`、非有限值检查和 `InitVectorFloat32`，不要直接拼装不满足“头部维度等于元素数”的 `data`。若入口接收外部集合，应特别避免当前 `CreateVectorFloat32` 不检查 16,383 上限这一陷阱。新增文本语法或格式化策略需同时评估 SQL 可往返性、Go `strconv` 兼容性及错误文本兼容。

修改线格式时必须同步检查 Datum、chunk、rowcodec、codec、protobuf 表达式、spill 和 `pkg/util/serialization` 的所有消费者，并保持 remainder 精确推进；这属于存储/交换兼容性变更，不能只改本文件。若要让反序列化真正零拷贝，需要重新设计所有权与对齐表示（例如区分 borrowed/owned 类型），不能让未对齐 `&[u8]` 直接成为 `&[f32]`。

测试应保持独立文件，不嵌入生产源码。基础行为优先更新 `pkg/types/vector_test.rs`，Go 对照同步核验 `pkg/types/vector_test.go`；子 crate 实际 Cargo test target `pkg/types/truncate_12_aster_unit_test.rs` 应覆盖公开 API、维度边界、畸形线数据、格式化和克隆不变量。距离或逐元素运算属于 `pkg/types/vector_functions.rs` 及其相应独立测试，不应塞入本文件。

兼容性风险集中在线格式、浮点格式化和错误分类；性能风险集中在大向量分配/复制、文本解析中间 `Vec<f64>`，以及名为 ZeroCopy 的反序列化实际逐 word 复制。任何 unsafe 视图修改都应继续维持 `u32/f32` 尺寸和对齐证明。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/types/vector.rs --offset 1 --limit 500` 返回目标文件完整 1–287 行，并报告 13 个直接使用文件；`query` 分别确认 `CreateVectorFloat32`、`ParseVectorFloat32`、`ZeroCopyDeserializeVectorFloat32`、`PeekBytesAsVectorFloat32`、`CheckDimsFitColumn`、`ZeroCopySerialize` 的 Rust/Go 定义候选。精确自然语言 `explore` 与同名 `callers/callees` 未产出可消歧边，因此调用关系以仓库精确符号搜索补证。
- 生产源码与装配：`pkg/types/vector.rs`、`pkg/types/internal/vector/lib.rs`、`pkg/types/internal/vector/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`、`pkg/types/vector_functions.rs`。
- Go 对照：`pkg/types/vector.go`。
- 独立测试：`pkg/types/vector_test.rs`、`pkg/types/vector_test.go`、`pkg/types/truncate_12_aster_unit_test.rs`。
- 直接调用证据：`pkg/session/runtime/{ddl,dml,relational_scan,relational_value,system_query}.rs`、`pkg/expression/{builtin_vec,builtin_vec_vec,builtin_inference,pb_to_expr_runtime,expr_to_pb}.rs`、`pkg/types/datum.rs`、`pkg/util/chunk/{column,mutrow}.rs`、`pkg/util/rowcodec/{encoder,decoder}.rs`、`pkg/util/codec/codec.rs`、`pkg/executor/aggfuncs/spill_serialize_helper.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核没有把 `vector_functions.rs` 的能力误归为本文件。
