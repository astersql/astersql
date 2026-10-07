# [`pkg/planner/core/columnar_index_utils.rs`](columnar_index_utils.rs)

## 文件定位

本文件属于 `astersql-planner-core` crate（见 `pkg/planner/core/Cargo.toml`），负责把规划阶段已经确定的向量索引查询参数组装成列存索引扫描使用的元数据。模块在 `pkg/planner/core/lib.rs` 中以私有模块 `columnar_index_utils` 声明，并通过 `pub use columnar_index_utils::*` 将其公开函数提升到 crate 根。

它位于“选择访问路径”与“物理表扫描携带下推载荷”之间：返回值 `physicalop_dependency::ColumnarIndexExtra` 对应 `pkg/planner/core/operator/physicalop/physical_table_scan.rs` 中的结构，并可进入 `PhysicalTableScan::UsedColumnarIndexes`。当前 Rust 仓库中没有发现生产代码调用该函数，直接调用仅见独立测试 `pkg/planner/core/columnar_index_utils_test.rs`；因此 Rust 侧目前可确认的是构造与导出能力，不能据此声称已接入 Rust 访问路径选择主链。

## 核心职责

文件只承担一项职责：`buildVectorIndexExtra` 将索引元信息、ANN 查询类型、距离度量、Top-K、目标列名、参考向量字节和列 protobuf 描述无损装配为 `ColumnarIndexExtra`。

该函数刻意不承担合法性校验。源码注释以及测试 `build_vector_index_extra_matches_go_payload_without_validation` 都确认：调用者应在进入本函数前派生或验证参数；本函数即使收到 `top_k == 0` 也原样保留。它也不解析 `ref_vec` 的浮点编码、不检查列与索引是否匹配、不判断距离函数是否受支持。

## 主要符号

- `pub fn buildVectorIndexExtra(...) -> ColumnarIndexExtra`：本文件唯一的模块级符号、唯一公开 API，也没有 trait、类型、常量、`impl` 或条件编译项。
- `index_info: &IndexInfo`：索引元信息来源；函数读取 `ID` 写入 ANN protobuf，并通过 `IndexInfo::Clone()` 把完整索引描述存入返回值。
- `query_type: AnnQueryType` 与 `distance_metric: VectorDistanceMetric`：直接写入 `tipb::AnnQueryInfo` 的枚举字段。
- `top_k: u32`、`column_name: &str`、`ref_vec: &[u8]`、`column: &ColumnInfo`：分别复制到 Top-K、列名、参考向量 protobuf 字节字段和列描述字段。
- `AnnQueryInfo`：内层 ANN 查询载荷；包含查询类型、距离度量、Top-K、列名、索引 ID、参考向量和列描述。
- `ColumnarIndexInfo`：外层 protobuf；固定把 `index_type` 设为 `ColumnarIndexType::TypeVector`，并装入上述 ANN 载荷。
- `ColumnarIndexExtra`：最终返回结构，字段为拥有所有权的 `IndexInfo` 和 `tipb::ColumnarIndexInfo`；定义见 `pkg/planner/core/operator/physicalop/physical_table_scan.rs`。

## 执行流程

1. 调用 `AnnQueryInfo::new()` 创建空的 ANN protobuf。
2. 依次用 setter 写入 `query_type`、`distance_metric`、`top_k`、拥有所有权的列名、`index_info.ID`、复制后的参考向量字节以及克隆后的 `ColumnInfo`。
3. 调用 `ColumnarIndexInfo::new()` 创建外层列存索引 protobuf，将索引类型无条件标记为向量索引 `TypeVector`，再装入 ANN 载荷。
4. 构造并返回 `ColumnarIndexExtra`：完整克隆 `index_info`，同时移动刚构造的 `query_info`。

此流程没有早退、分支或循环。它保持输入字段的值，不根据参数重新计算 Top-K、距离类型或向量内容。

## 数据与状态

函数无全局状态，也不修改传入对象。所有结果都由返回值拥有：`column_name.to_owned()` 分配新的字符串，`ref_vec.to_vec()` 复制字节，`column.clone()` 复制 protobuf 列信息，`index_info.Clone()` 复制索引元信息。调用完成后，返回载荷与调用者随后修改原始字符串、切片、列描述或索引对象相互独立。

关键数据不变量是：`AnnQueryInfo.index_id == index_info.ID`，`ColumnarIndexInfo.index_type == TypeVector`，并且其他 ANN 字段与输入逐项一致。`ref_vec_f32` 虽以“F32”命名，在此处仍只是未经解释的 `Vec<u8>`；浮点排列、字节序和维度都由上游约定负责。

## 依赖与调用关系

直接依赖由 `pkg/planner/core/Cargo.toml` 声明：`model-dependency` 映射到本地 `astersql-meta-model`，`physicalop-dependency` 映射到本地 `astersql-planner-core-operator-physicalop`，`tipb` 固定到启用 `protobuf-codec` 的 Git revision。文件本身没有 feature gate，`nextgen` feature 也不改变其编译逻辑。

下游关系为：`buildVectorIndexExtra` 构造 `tipb::AnnQueryInfo`，嵌入 `tipb::ColumnarIndexInfo`，最终返回 `physicalop_dependency::ColumnarIndexExtra`。该结构可存放在 `PhysicalTableScan::UsedColumnarIndexes` 中，作为列存索引扫描的额外元数据和 protobuf 查询载荷。

RustCodeGraph 对 Rust 符号未给出生产调用者，仓库精确搜索也只找到 `pkg/planner/core/columnar_index_utils_test.rs` 的直接调用。作为行为对照，Go 的直接生产调用者位于 `pkg/planner/core/find_best_task.go`：MPP 路径选择检测到向量索引后，先断言索引列与向量属性列匹配、转换距离度量，再将本构造器结果追加到表扫描的 `UsedColumnarIndexes`。

## 错误处理与边界

函数返回裸 `ColumnarIndexExtra` 而非 `Result`，没有显式错误路径。所有枚举值、`top_k`、列名、索引 ID、参考向量字节和列描述均直接接受；独立 Rust 测试专门断言 `top_k == 0` 不会被拒绝或改写。

边界责任属于调用者：应确保索引确为目标向量列的索引、查询类型和距离度量有效、参考向量采用 TiPB 期望的 float32 序列化格式、列描述与索引/表一致。Go 主链中的断言与距离度量检查是这一前置责任的实例，但 Rust 当前没有对应生产调用边可证明已执行同样校验。

代码中没有显式 panic、日志或错误吞并。常规内存分配与 clone 的资源耗尽行为由 Rust 运行时/分配器处理，不属于本函数的业务错误协议。

## 并发与资源生命周期

本函数是同步、无状态的纯构造过程：不加锁、不启动任务、不使用通道、事务、I/O 或缓存，也不保存输入引用。只要输入引用在调用期间有效即可；返回值拥有全部复制后的数据，可独立跨越输入生命周期。

并发调用之间没有共享可变状态，因此本文件不引入额外竞争条件。实际返回类型能否跨线程由 `IndexInfo` 和 TiPB 生成类型的 trait 实现决定，本文件没有额外声明或同步保证。复制完整索引信息、列 protobuf 和参考向量的成本与其大小线性相关；高频或大向量调用时应关注分配与复制开销。

## 与 Go 版本的对应关系

Go 对照实现是 `pkg/planner/core/columnar_index_utils.go::buildVectorIndexExtra`。两者保持相同的七项输入和相同的嵌套载荷：完整索引信息、固定向量索引类型，以及 ANN 查询中的查询类型、距离度量、Top-K、列名、索引 ID、参考向量和列描述。

所有权表达不同但语义对齐：Go 返回指针并在结构中保存 `indexInfo` 指针、`refVec` 切片以及列值副本；Rust 的 `ColumnarIndexExtra` 使用拥有所有权的值，因此克隆索引、参考向量和列描述。Rust 使用 protobuf setter 构造 oneof 对应的 ANN 载荷，Go 使用 `ColumnarIndexInfo_AnnQueryInfo` 包装器；两者最终都设置 ANN 分支。

Go 生产主链已在 `find_best_task.go` 使用该函数，Go 测试 `pkg/planner/core/task_heavy_function_optimize_test.go` 也用它构造向量索引扫描场景。Rust 当前仅由 `columnar_index_utils_test.rs` 验证字段映射和“不校验 Top-K”的兼容行为，尚未发现与 Go `find_best_task.go` 对应的 Rust 生产调用，因此迁移接线状态不完全等价。

## 扩展指南

- 新增 ANN protobuf 字段时，应在 `buildVectorIndexExtra` 中保持与 Go 同名构造器一致的字段映射，并在 `pkg/planner/core/columnar_index_utils_test.rs` 增加精确断言；不要把测试内嵌回生产文件。
- 新增列存索引种类时，不应让本函数根据隐式条件改变类型；可为新载荷建立独立构造器，或显式扩展 API，并同步 `ColumnarIndexExtra` 的消费端。
- 若要把 Rust 接入访问路径选择，接线点应对应 Go `find_best_task.go` 中向 `PhysicalTableScan.UsedColumnarIndexes` 追加结果的位置，并先移植同等的列匹配、距离度量和向量序列化前置检查。
- 若要引入校验，应先确认 Go 行为和调用契约；直接拒绝 `top_k == 0` 会破坏现有 Rust 回归测试所固定的无校验语义。
- 性能修改应保留载荷所有权安全，同时评估 `IndexInfo`、`ColumnInfo` 和大 `ref_vec` 的复制成本；任何借用化设计都会影响 `PhysicalTableScan` 的生命周期和公开类型边界。
- 兼容性风险主要来自 TiPB schema/oneof 表达、距离度量枚举值和参考向量编码。更新 `tipb` revision 时应重新核对 setter 与字段名称。

## 验证依据

- RustCodeGraph：`status` 确认索引包含本仓库 Rust/Go 文件；`query buildVectorIndexExtra --kind function` 定位 Go/Rust 同名实现；`node --file pkg/planner/core/columnar_index_utils.rs` 核对完整 53 行源码；`callers/callees buildVectorIndexExtra` 未发现 Rust 生产调用或可解析的内部调用；`query ColumnarIndexExtra --kind struct` 与 `node --file pkg/planner/core/operator/physicalop/physical_table_scan.rs` 核对返回类型及其在 `UsedColumnarIndexes` 中的位置。
- Rust 源与模块入口：`pkg/planner/core/columnar_index_utils.rs`、`pkg/planner/core/lib.rs`。
- crate 与依赖：`pkg/planner/core/Cargo.toml`。
- Go 对照与生产调用：`pkg/planner/core/columnar_index_utils.go`、`pkg/planner/core/find_best_task.go`。
- 独立测试：`pkg/planner/core/columnar_index_utils_test.rs`；补充的 Go 使用场景为 `pkg/planner/core/task_heavy_function_optimize_test.go`。
- 相关载荷定义：`pkg/planner/core/operator/physicalop/physical_table_scan.rs`。该文件的测试另行覆盖 `UsedColumnarIndexes` 克隆及计划缓存拒绝行为，但不属于本构造器的直接测试。
- 未运行 Cargo 或代码测试：本任务只生成说明文档，按计划以事实查询、源文件对照和固定章节结构检查验收。
