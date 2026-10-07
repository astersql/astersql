# `pkg/expression/builtin_grouping.rs`

## 文件定位

`builtin_grouping.rs` 是 `astersql-expression` crate 中 `GROUPING(...)` 的元数据与求值内核。crate 根 `pkg/expression/lib.rs` 通过 `#[path = "builtin_grouping.rs"] mod builtin_grouping_kernel;` 私有挂载它；规划器侧的公开签名 `pkg/expression/planner_bridge.rs::BuiltinGroupingImplSig` 将一个 `GroupingSig` 放在 `RwLock` 中，并把该签名从 crate 根再导出。因此，这个文件不是 SQL 函数注册、参数改写或 protobuf 编码的完整实现，而是被这些接线层复用的、与表达式参数无关的计算核心。

该文件属于 `pkg/expression/Cargo.toml` 声明的 `astersql-expression` 库。它的直接外部依赖只有标准库 `HashSet` 和 Cargo 中的 `thiserror = "2"`；SQL 表达式类型、chunk、tipb 模式转换和锁均位于调用它的 `planner_bridge.rs`，没有耦合进本内核。

## 核心职责

本文件承担四项职责：

1. 用 `GroupingMode` 表达优化器重写后可用的三种判定方式，以及尚未初始化的 `Invalid` 状态。
2. 用 `GroupingSig::set_metadata` 安装并校验每个 `GROUPING` 参数对应的 mark 集合；校验失败后保证签名不可求值。
3. 将单个 `grouping_id` 按参数顺序折叠为一个位图整数：每处理一个参数，结果左移一位，再写入该参数是否由聚合层次补成 NULL 的标志位。
4. 为调用层提供标量求值 `eval`、批量求值 `eval_many` 和排序后的可序列化快照 `metadata`。

它不负责从原始列推导 marks。真实推导发生在 `pkg/planner/core/expression_rewriter.rs` 的 `GROUPING` 重写分支：原始参数被替换为 Expand 的 GID 列，同时根据 `logicalop::GroupingMode` 构造 `HashSet<u64>`，再调用 `BuiltinGroupingImplSig::SetMetadata`。

## 主要符号

- `GroupingMode::{Invalid, BitAnd, NumericCmp, NumericSet}`：求值策略枚举。`Default` 是 `Invalid`，它表示不可用状态而不是一种合法 SQL 模式。
- `GroupingMetadata { mode, grouping_marks }`：面向调用层的拥有型快照。内部 `HashSet` 会在 `GroupingSig::metadata` 中转成逐组升序的 `Vec<Vec<u64>>`，使 protobuf 输出和测试回读不受哈希迭代顺序影响。
- `GroupingError`：可比较、可克隆的领域错误，包括未初始化、非法模式，以及 BitAnd/NumericCmp 的某组 mark 数量不等于 1。
- `GroupingSig { mode, grouping_marks, is_meta_inited }`：运行时状态。字段私有，调用者只能通过构造器、安装方法、只读访问器和求值 API 操作。
- `GroupingSig::new` / `Default::default`：创建 `Invalid`、空 marks、未初始化的签名。
- `GroupingSig::set_metadata`：替换模式和全部 marks，设置初始化标志，再调用 `check_metadata`；失败时保留传入值供诊断，但把 `is_meta_inited` 清回 `false`，从可用性角度避免半初始化状态参与求值。
- `GroupingSig::metadata`：重新校验状态并返回排序快照。
- `GroupingSig::eval`：标量入口；校验一次后调用私有 `grouping` 分发。
- `GroupingSig::eval_many`：批量入口；整批开始前校验一次，然后对切片逐项调用同一私有内核并收集结果。
- `grouping_impl_bit_and`、`grouping_impl_numeric_cmp`、`grouping_impl_numeric_set`：三种模式的位生成算法；`grouping` 是模式分发器。

## 执行流程

规划与运行主流程如下：

1. `pkg/planner/core/expression_rewriter.rs` 解析 `GROUPING` 的列参数，取得 Expand 节点的 GID 列，并按列生成 marks。BitAnd 模式为每列构造一个位掩码；NumericSet 模式取该列需要保留的 GID 集合。
2. 重写器创建 `pkg/expression/planner_bridge.rs::BuiltinGroupingImplSig`，再调用其 `SetMetadata`。桥接层把 `tipb::GroupingMode` 映射为本文件的 `GroupingMode`，取得写锁并调用 `GroupingSig::set_metadata`。
3. `set_metadata` 覆盖旧状态并用 `check_metadata` 验证。BitAnd 和 NumericCmp 要求每一个 mark 集合恰好含一个元素；NumericSet 允许集合含零个或多个元素；`Invalid` 总是拒绝。
4. 标量执行时，`BuiltinGroupingImplSig::evalInt` 先计算唯一的 GID 参数。SQL NULL 直接向上传播；非 NULL 值转为 `u64`，在读锁内调用 `GroupingSig::eval`。
5. `eval` 再次检查元数据，然后 `grouping` 按模式分派。三个算法都按 `grouping_marks` 的外层顺序处理参数，因此参数次序决定结果位次：较早参数最终处于更高位。
6. BitAnd 在 `grouping_id & mark == 0` 时写 1；NumericCmp 在 `grouping_id <= mark` 时写 1；NumericSet 在集合不包含 `grouping_id` 时写 1。写 1 表示该列在当前聚合层次中不需要、由 ROLLUP/CUBE/GROUPING SETS 产生 NULL；写 0 表示该列参与该分组。
7. 表达式编码时，`planner_bridge.rs::groupingModeAndMarks` 调用 `metadata` 获取确定顺序的快照，`expr_to_pb.rs` 再把桥接层生成的 metadata 字节写入 protobuf。

`eval_many` 是本文件提供的独立批量 API，行为等于对同一已校验状态逐项运行 `grouping`。当前生产接线的 `BuiltinGroupingImplSig` 明确声明 `vectorized() == true`，但本文件内核的 `eval_many` 在生产 Rust 代码中没有直接调用点；不能据此把它描述成当前 chunk 向量执行路径。

## 数据与状态

`grouping_marks` 的外层 `Vec` 与 SQL `GROUPING(x, y, z)` 的参数位置一一对应，顺序有语义，不能排序或去重外层元素。每个内层 `HashSet<u64>` 表示该位置的比较数据：BitAnd/NumericCmp 恰好一个数，NumericSet 是任意集合。`metadata` 只排序每个集合内部的数值，不改变参数顺序。

`is_meta_inited` 是可用性门闩。仅仅拥有非 `Invalid` 的 `mode` 或非空 marks 不代表状态有效；所有公开读取/求值路径中，`metadata`、`eval`、`eval_many` 都经 `check_metadata` 检查该标志。空的外层 marks 对三种合法模式都能通过当前校验，并求值得到 0；这是源码现状，不代表 SQL 前端允许零参数。

累计结果使用 `u64`，最终以 `i64` 返回。每个外层参数贡献一位，因此超过 64 个 marks 时高位会被左移丢弃；本文件没有参数数量上限检查。Go 源码注释说明接口设计上最多 64 个参数，但 Rust 内核并未单独强制这一约束。桥接层把返回字段标记为 unsigned BIGINT，调用层负责按 SQL 类型解释这个 `i64` 位模式。

克隆 `GroupingSig` 会深拷贝所有 `HashSet` 和初始化标志；`GroupingMetadata` 同样拥有其数据，不借用可变状态。

## 依赖与调用关系

上游直接关系：

- `pkg/expression/lib.rs` 私有挂载 `builtin_grouping_kernel`；测试配置下另通过 `expression_encryption::builtin_grouping` 暴露测试门面。
- `pkg/expression/planner_bridge.rs::BuiltinGroupingImplSig::new` 创建 `GroupingSig::new`；`SetMetadata` 调用 `set_metadata`；`evalInt` 调用 `eval`；`groupingModeAndMarks` 调用 `metadata`。
- `pkg/planner/core/expression_rewriter.rs` 是生产端元数据来源和安装调用点。它构造 marks 后调用桥接签名的 `SetMetadata`，错误写入 rewriter 状态。
- `pkg/expression/expr_to_pb.rs` 通过 builtin trait 的 `metadata()` 间接消费本文件的排序快照。

下游直接关系只有 `std::collections::HashSet` 与 `thiserror::Error` 派生宏。三种求值实现只做整数比较、位运算和集合查询，不调用存储、网络、会话或 chunk API。

RustCodeGraph 的全局索引能定位 Go `BuiltinGroupingImplSig`、Rust `planner_bridge.rs::BuiltinGroupingImplSig`、`SetMetadata` 及相关测试；但 `files --filter pkg/expression/builtin_grouping` 返回“未找到”，对本 `.rs` 的精确 `explore/query/node` 也无结果。因此本文件内部符号和直接引用由源码与 `rg` 补证，不能把缺失的图边解释为“没有调用者”。

## 错误处理与边界

- 未调用 `set_metadata` 就调用 `metadata`、`eval` 或 `eval_many`，返回 `GroupingError::MetadataNotInitialized`。
- 传入 `GroupingMode::Invalid`，`set_metadata` 返回 `InvalidMode` 并清除初始化标志。
- BitAnd 或 NumericCmp 的任一内层集合大小不是 1，返回 `InvalidGroupingIdCount { mode, count }`。`count` 是失败的那一个集合的元素数，而不是外层 marks 总数。
- NumericSet 不限制集合大小；空集合意味着任何 GID 都“不在集合中”，相应参数位恒为 1。
- `set_metadata` 失败后不会恢复先前的有效 mode/marks，只会使当前签名不可用。调用者若想恢复，必须再次安装一份合法元数据。
- 私有 `grouping` 对 `Invalid` 返回 0，但公开求值会先被 `check_metadata` 拒绝，所以正常 API 不会把非法模式静默当作 0。
- `eval_many` 是整批失败：元数据无效时不产生部分结果；元数据有效后，逐项计算本身没有可返回错误。
- 本文件接收 `u64` GID，不定义负数语义。桥接层将表达式求得的 `i64` 直接 `as u64`；该转换的位级语义由调用层承担。

## 并发与资源生命周期

`GroupingSig` 自身不包含锁、原子量或后台任务；`set_metadata` 需要 `&mut self`，只读求值需要 `&self`。生产桥接层在 `planner_bridge.rs` 用 `RwLock<GroupingSig>` 管理并发：安装元数据持写锁，求值、回读和编码持读锁；锁中毒被转换为表达式错误，部分只读 trait 查询则返回 `None` 或使用明确的 `expect`。

单次 `eval` 不分配集合或输出缓冲，只遍历现有 marks。NumericSet 的核心查询是 `HashSet::contains`；BitAnd/NumericCmp 因校验保证单元素集合，仍按集合迭代一次。`eval_many` 为返回值分配一个与输入等长的 `Vec<i64>`，不修改签名。`metadata` 为快照克隆并排序每组数据，适用于规划/编码路径，不应放进逐行热路径。

桥接签名的 `Clone` 在读锁下深拷贝 `GroupingSig` 并创建新锁，因此克隆计划后的元数据状态独立；本文件没有共享引用计数资源，也无需显式释放。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/expression/builtin_grouping.go`，核心字段和算法分别对应：

- Rust `GroupingSig` 对应 Go `BuiltinGroupingImplSig` 中的 `mode`、`groupingMarks`、`isMetaInited` 状态子集；Go 结构还嵌入 `baseBuiltinFunc`，Rust 把它留在 `planner_bridge.rs::BuiltinGroupingImplSig`。
- `set_metadata` 对应 Go `SetMetadata`：先写状态、标记已初始化、再校验，失败时把初始化标志清零。
- `check_metadata` 与三种 `groupingImpl*` 算法保持同一判定；多参数均通过“左移后追加当前位”组成结果。
- Rust `metadata` 对应 Go `metadata` 的数据语义，但错误接口不同：Go 校验失败时记录日志并返回空 protobuf，Rust 返回结构化 `GroupingError`；日志/错误类型转换由桥接层处理。
- Rust `eval` 对应 Go `evalInt` 中元数据检查后的计算部分。Go 同时负责表达式参数求值和 NULL/错误传播，Rust 的这些职责在 `planner_bridge.rs::evalInt`。
- Rust `eval_many` 只接受 GID 切片并返回新 `Vec`；Go `vecEvalInt` 从 chunk 获取临时列缓冲并复用结果列。两者共享算法，但资源管理和 chunk 接口并非一比一移植。

`pkg/expression/builtin_grouping_test.rs::test_grouping` 逐项复刻 Go `builtin_grouping_test.go::TestGrouping` 的 19 组 BitAnd、NumericCmp、NumericSet fixture。额外的 `pkg/expression/planner_bridge_test.rs::grouping_metadata_is_installed_atomically` 验证桥接状态从未初始化到可回读/可编码；`builtin_encryption_11_aster_unit_test.rs` 还覆盖克隆、排序快照、批量求值及非法单元素约束。

## 扩展指南

新增合法模式时，至少需要同步修改 `GroupingMode`、`check_metadata`、`grouping` 分发、具体算法，以及 `planner_bridge.rs` 中 tipb 与内核模式的双向映射；随后补充独立 Rust 测试文件中的标量、批量、metadata 回读和非法元数据用例，并核对 Go `builtin_grouping.go` 与 tipb 枚举是否已有对应语义。不要把测试嵌入本生产文件。

修改 marks 表示时，应保持两个不变量：外层顺序决定返回位顺序，任何可用状态都必须先通过 `check_metadata`。若改变 `metadata` 的排序或形状，还要同步检查 `planner_bridge.rs::metadata`、`expr_to_pb.rs` 和 protobuf 兼容性，避免同一逻辑状态产生不稳定编码。

优化性能时，先区分热路径：`eval` 的目标是零额外分配；`metadata` 的克隆排序属于低频编码路径；`eval_many` 当前不是生产 chunk 接线的已验证调用点。若要将它接入向量执行，应同时验证 SQL NULL 传播、输入列类型、结果缓冲复用和锁持有范围，而不能只比较数值 fixture。

边界增强最值得补充的是：多参数位序、空外层 marks、NumericSet 空集合、失败安装覆盖旧状态、64/超过 64 个参数，以及负 `i64` GID 经桥接转换后的预期。若正式要求最多 64 个参数，应在元数据安装或更上游参数校验处返回明确错误，并与 Go 行为共同更新，不能依赖位移自然截断。

## 验证依据

本说明读取并交叉核对了以下直接证据：

- 目标实现：`pkg/expression/builtin_grouping.rs`，包括全部枚举、结构、错误类型、状态安装、校验、三种算法和两个公开求值入口。
- crate 与模块边界：`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`；前者确认 `astersql-expression`、`thiserror` 和手工测试配置，后者确认生产挂载、测试挂载及桥接再导出。
- 生产接线：`pkg/expression/planner_bridge.rs::BuiltinGroupingImplSig`、`pkg/planner/core/expression_rewriter.rs` 的 GROUPING 重写分支、`pkg/expression/expr_to_pb.rs` 的 metadata 编码调用。
- Go 对照：`pkg/expression/builtin_grouping.go` 与 `pkg/expression/builtin_grouping_test.go`。
- Rust 测试：`pkg/expression/builtin_grouping_test.rs`、`pkg/expression/planner_bridge_test.rs::grouping_metadata_is_installed_atomically`、`pkg/expression/builtin_encryption_11_aster_unit_test.rs` 中的 GROUPING 覆盖。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query BuiltinGroupingImplSig` 和 `query SetMetadata` 定位到 Go/Rust 桥接符号与测试，但目标 `.rs` 未被 `files --filter`/精确符号查询覆盖，故文件内事实使用源码及 `rg` 直接验证。

本任务是纯文档分析，未修改 Rust/Go/Cargo 行为，也未运行 Cargo。结构验收以文档存在且固定二级标题恰好 11 个为准；内容复核重点是能从上述符号回查“文件为何存在、运行时如何进入、状态如何验证、如何安全扩展”。
