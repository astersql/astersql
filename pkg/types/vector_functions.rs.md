# `pkg/types/vector_functions.rs`

## 文件定位

本文件为 `VectorFloat32` 补充距离、内积、范数、逐元素算术和全序比较能力。它不定义向量的存储格式；存储、解析和序列化实现在 [`pkg/types/vector.rs`](vector.rs)，其中 `VectorFloat32` 用一个维度字和若干 `f32` 数据字保存内容，并通过 `Len`、`Elements`、`ElementsMut` 暴露本文件所需的视图。

模块由 [`pkg/types/internal/vector/lib.rs`](internal/vector/lib.rs) 通过 `#[path = "../../vector_functions.rs"] mod vector_functions` 挂入 `astersql-types-vector` 子 crate。该子 crate 的公开向量符号再经 [`pkg/types/lib.rs`](lib.rs) 的 `pub use types_vector as vector` 暴露给上层。根 [`pkg/types/Cargo.toml`](Cargo.toml) 以路径依赖 `types-vector = { package = "astersql-types-vector", path = "internal/vector" }` 建立这一边界；具体测试目标则定义在 [`pkg/types/internal/vector/Cargo.toml`](internal/vector/Cargo.toml)。

## 核心职责

- 在所有需要双向量配对计算的入口先验证维度相同，避免 `zip` 静默截短。依据：`checkIdenticalDims` 及其在距离、内积和 `binary_operation` 中的调用。
- 提供 SQL 向量函数的数值内核：`L1Distance`、`L2SquaredDistance`、`L2Distance`、`InnerProduct`、`NegativeInnerProduct`、`CosineDistance` 和 `L2Norm`。
- 通过统一的 `binary_operation` 实现 `Add`、`Sub`、`Mul`，并拒绝产生无穷或 NaN 的结果。
- 通过 `Compare` 提供按分量字典序、公共前缀相同后按维度排序的比较语义，供 Datum 比较和 session 关系运算使用。

本文件只做纯内存数值运算，不负责 SQL NULL 语义。上层 [`pkg/expression/builtin_vec.rs`](../expression/builtin_vec.rs) 和 [`pkg/expression/builtin_vec_vec.rs`](../expression/builtin_vec_vec.rs) 将输入 NULL 或本文件返回的 NaN 转成 SQL NULL。

## 主要符号

- `new_error(message) -> errors::SharedError`：把本地生成的维度、溢出和 NaN 文案包装成共享错误。
- `trace(error) -> errors::SharedError`：用 `errors::Trace(Some(error))` 为下层错误补充追踪信息；传入必为 `Some`，因此用 `expect` 固化该不变量。
- `VectorFloat32::checkIdenticalDims(&self, other)`：公开的维度守卫。长度不同返回 `vectors have different dimensions: A and B`。
- `L2SquaredDistance`：以 `f32` 累加差值平方，最终提升为 `f64`。它也是 `L2Distance` 的下层入口。
- `L2Distance`：对平方距离取 `f64::sqrt`；下层错误经 `trace` 传播。
- `InnerProduct`：以 `f32` 累加分量乘积，最终提升为 `f64`；`NegativeInnerProduct` 复用它并取负。
- `CosineDistance`：单次遍历同时累计内积和两侧平方范数；相似度为 NaN 时原样返回 NaN，否则夹紧到 `[-1, 1]` 后返回 `1 - similarity`。
- `L1Distance`：以 `f32` 累加对应分量差的绝对值。
- `L2Norm`：唯一明确以 `f64` 做乘法和累加的度量，再取平方根；这与 Go 文件中对齐 pgvector 精度的注释一致。
- `Add`、`Sub`、`Mul`：公开薄入口，只选择闭包并委托私有 `binary_operation`。
- `binary_operation`：分配同维结果、逐分量写入，并在第二遍扫描中拒绝 infinity 和 NaN；当前不检查 underflow。
- `Compare`：比较公共前缀的每个分量；首次不等即返回 `-1` 或 `1`，公共前缀相同时用维度比较得到 `-1/0/1`。

文件没有模块级常量、trait 或条件编译项。除 `new_error`、`trace`、`binary_operation` 外，其余方法均为公开 API。

## 执行流程

距离类二元运算的共同流程如下：调用 `checkIdenticalDims`；取得两侧 `Elements`；用 `zip` 单次遍历公共位置并按算法累计；把累计结果转换为 `f64` 返回。维度守卫使 `zip` 的长度必然等于两向量维度。`L2Distance` 和 `NegativeInnerProduct` 不重复遍历，分别复用 `L2SquaredDistance` 和 `InnerProduct`。

`CosineDistance` 同时累计 `product`、`left_norm`、`right_norm`。当空向量或任一零范数向量导致 `0/0` 时返回 NaN；正常值先夹紧以消除浮点舍入造成的轻微越界，再转成距离。SQL 标量路径的 `vectorDistance`（[`pkg/expression/builtin_vec.rs`](../expression/builtin_vec.rs)）和向量化宏路径（[`pkg/expression/builtin_vec_vec.rs`](../expression/builtin_vec_vec.rs)）会把该 NaN 映射为 NULL。

逐元素算术先由 `Add`、`Sub` 或 `Mul` 选择运算闭包。`binary_operation` 校验维度后调用 `InitVectorFloat32(self.Len())` 分配结果，用 `ElementsMut` 写入每个分量，再完整检查结果：infinity 返回 overflow 错误，NaN 返回 NaN 错误；全部有限才交付结果。

`Compare` 不要求维度相同。它只比较两侧公共前缀，发现首个不同分量即返回；若公共前缀相同，则短向量在前。这一行为直接用于 [`pkg/types/datum.rs`](datum.rs) 的 `KindVectorFloat32` 比较，也用于 [`pkg/session/runtime/relational_value.rs`](../session/runtime/relational_value.rs) 的关系值排序/比较。

## 数据与状态

所有方法均只读取接收者和参数；仅 `binary_operation` 创建并填充一个新的 `VectorFloat32`。原向量不会被修改，也没有全局或线程局部状态。

距离、内积、L1 距离和余弦计算按 Go 实现使用 `f32` 中间累计，再以 `f64` 返回；这意味着提升返回类型不会恢复累计期间丢失的精度。`L2Norm` 是例外，它先把每个分量提升为 `f64` 再平方和累加。逐元素结果仍为 `f32`。

零维向量在求和类操作中得到零；两个零维向量的 `CosineDistance` 得到 NaN。不同维度的距离和算术均报错，但 `Compare` 有意允许不同维度并以长度打破公共前缀平局。

## 依赖与调用关系

直接下游依赖只有当前 crate 的 `VectorFloat32`、`InitVectorFloat32` 和 `errors`：`Len` 保证维度判断，`Elements`/`ElementsMut` 提供分量视图，初始化函数负责结果缓冲区，错误模块提供 Go 风格 `New`/`Trace` 包装。算法本身只使用 Rust 标准库浮点和迭代器操作。

已核实的上游生产调用包括：

- [`pkg/expression/builtin_vec.rs`](../expression/builtin_vec.rs)：标量 SQL `VEC_L1_DISTANCE`、`VEC_L2_DISTANCE`、`VEC_NEGATIVE_INNER_PRODUCT`、`VEC_COSINE_DISTANCE` 和 `VEC_L2_NORM`。
- [`pkg/expression/builtin_vec_vec.rs`](../expression/builtin_vec_vec.rs)：同类 SQL 函数的批量向量化求值。
- [`pkg/session/runtime/relational_value.rs`](../session/runtime/relational_value.rs)：兼容执行路径解析文本向量后调用距离/范数方法，并用 `Compare` 参与关系值比较。
- [`pkg/types/datum.rs`](datum.rs)：Datum 的 `KindVectorFloat32` 分支委托 `Compare`。

精确 Rust 搜索未发现 `Add`、`Sub`、`Mul` 的生产调用者；它们当前是公开的类型层能力，并由独立测试验证。`L2SquaredDistance` 和 `InnerProduct` 的直接仓内使用主要分别来自 `L2Distance`、`NegativeInnerProduct` 以及测试。

## 错误处理与边界

- 维度不一致：所有双向量距离、内积和逐元素算术均返回共享错误；派生入口继续用 `trace` 传播。`Compare` 不属于此约束。
- NaN/无穷：`binary_operation` 明确拒绝结果中的 infinity 和 NaN，错误文本分别为 `value out of range: overflow` 和 `value out of range: NaN`。该检查发生在完整结果计算后，因此出错时结果不会暴露给调用者。
- Underflow：当前有意未检查，与 Go `Mul` 中的 TODO 一致；扩展时不能只改单个算术入口，因为三个入口共享 `binary_operation`。
- 余弦零范数：返回成功的 NaN 而非错误。类型层调用者必须决定如何表示；现有 SQL 表达式层将其视为 NULL，而 session 兼容路径目前直接格式化返回值。
- 空向量：相同维度的空向量可参与所有操作；加减乘返回空向量，和式度量为零，余弦距离为 NaN。
- 浮点比较：`Compare` 只使用 `<`/`>`。正常构造路径拒绝 NaN，但零拷贝反序列化能保留任意 `f32` 位型；若含 NaN，则该分量的两个比较均为假，可能继续比较后续分量或长度。现有测试只验证反序列化 NaN 在算术时被拒绝，未为含 NaN 的 `Compare` 定义额外契约。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部句柄。只读方法可在调用者满足 `VectorFloat32` 所有权/借用规则时并发使用；Rust 的共享借用阻止本文件在计算期间修改输入。

距离和比较为 O(n) 时间、O(1) 额外空间。`Add`、`Sub`、`Mul` 为 O(n) 时间并分配 O(n) 的新向量；结果检查带来第二次线性扫描。错误路径依赖正常 Rust 所有权释放临时结果，无需显式清理。

## 与 Go 版本的对应关系

直接对照文件为 [`pkg/types/vector_functions.go`](vector_functions.go)。Rust 保留了 Go 的算法分层、`float32` 累计策略、错误文本、余弦夹紧、零范数返回 NaN、逐元素结果检查和字典序比较。`L2Norm` 同 Go/pgvector 一样使用双精度累计。

实现形式上的差异是：Go 分别实现 `Add`、`Sub`、`Mul` 的循环，Rust 抽出 `binary_operation` 消除重复；Go 出错时返回 `ZeroVectorFloat32` 加错误，Rust 用 `Result` 的 `Err` 不携带占位向量；Go 的值接收者映射为 Rust 的共享引用；Go 通过索引循环，Rust 使用 `zip`。这些差异不改变已验证的成功值或错误文案。

Go 只在 `Mul` 处记录 underflow TODO；Rust 将统一辅助上的注释视为三个逐元素运算共同的当前限制。Go 测试 [`pkg/types/vector_test.go`](vector_test.go) 直接覆盖 `Compare`、零向量、解析与序列化，但未覆盖本文件其余数值方法；Rust 的 [`pkg/types/truncate_12_aster_unit_test.rs`](truncate_12_aster_unit_test.rs) 补充覆盖距离、范数、加减乘、维度错误、overflow、NaN 和 Go 比较样例。

## 扩展指南

新增双向量度量时，应先决定是否要求同维；若要求，应复用 `checkIdenticalDims` 并沿用 `trace` 传播方式。若新度量对应 SQL 函数，还需同步标量入口 [`pkg/expression/builtin_vec.rs`](../expression/builtin_vec.rs)、向量化入口 [`pkg/expression/builtin_vec_vec.rs`](../expression/builtin_vec_vec.rs) 和 session 兼容入口 [`pkg/session/runtime/relational_value.rs`](../session/runtime/relational_value.rs)，并明确 NaN 是否映射为 SQL NULL。

新增逐元素二元算术优先复用 `binary_operation`；若错误规则不同，不应通过放宽共享检查影响现有 `Add`、`Sub`、`Mul`。若实现 underflow 检查，必须先与 Go/pgvector 语义对齐，并补齐正常次正规数、真正下溢、overflow 和 NaN 的独立测试。

行为测试应放在独立测试文件，不嵌入生产源文件。最直接的同步位置是 [`pkg/types/truncate_12_aster_unit_test.rs`](truncate_12_aster_unit_test.rs) 的 `vector_deserialize_and_arithmetic_match_go`；比较、零向量与格式契约也可在 [`pkg/types/vector_test.rs`](vector_test.rs) 扩展，并同步核对 Go 的 [`pkg/types/vector_test.go`](vector_test.go)。涉及 SQL NULL 或批量执行时，还应扩展 [`pkg/expression/builtin_vec_vec_test.rs`](../expression/builtin_vec_vec_test.rs) 等表达式测试。

兼容性风险主要是浮点累计精度、NaN/零范数映射、错误文本和不同维度行为；性能风险主要是无意把单遍度量改成多遍、引入额外分配，或破坏批量表达式路径。调整 `Compare` 时还需考虑 Datum 排序和 session 关系比较的一致性。

## 验证依据

- RustCodeGraph `status`：索引可用，共 11,467 个文件；`files --filter pkg/types/vector_functions.rs` 识别该文件及 16 个符号。
- RustCodeGraph `query`/`node`：定位 Rust 与 Go 的 `L2SquaredDistance`、`checkIdenticalDims` 以及私有 `binary_operation`，并读取目标文件完整 172 行。精确 `callers`/`callees` 对这些 inherent methods 返回空边，因此调用关系改用精确源码引用搜索补证，未据此误判为“无调用”。
- 读取的实现和装配文件：`pkg/types/vector_functions.rs`、`pkg/types/vector.rs`、`pkg/types/internal/vector/lib.rs`、`pkg/types/internal/vector/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`。
- 读取的上游证据：`pkg/expression/builtin_vec.rs`、`pkg/expression/builtin_vec_vec.rs`、`pkg/session/runtime/relational_value.rs`、`pkg/types/datum.rs`。
- 读取的对照与测试：`pkg/types/vector_functions.go`、`pkg/types/vector_test.go`、`pkg/types/vector_test.rs`、`pkg/types/truncate_12_aster_unit_test.rs`；其中 `vector_deserialize_and_arithmetic_match_go` 验证数值结果、比较、零向量余弦、维度错误、overflow 和反序列化 NaN 算术错误。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行固定章节结构检查并人工核对上述事实、链接和边界描述。
