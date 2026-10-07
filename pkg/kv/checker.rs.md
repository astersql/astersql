# `pkg/kv/checker.rs`

## 文件定位

`checker.rs` 属于 `astersql-kv` crate；crate 清单为 `pkg/kv/Cargo.toml`，入口 `pkg/kv/lib.rs` 通过 `include!("checker.rs")` 将其装入公开的 `checker` 模块，再以 `pub use checker::*` 重导出。因此调用方通常使用 `kv::RequestTypeSupportedChecker` 和 `kv::ExprType`，而不是引用文件级模块路径。

该文件位于 SQL 表达式/算子转换与存储客户端之间，是一个同步、纯计算的“能力白名单”。它只回答某类请求或表达式类型是否被历史 KV 下推协议接受，不负责生成 protobuf、选择具体存储引擎、发送请求或验证某个引擎的全部能力。实际生产接线可见 `pkg/store/driver/kv_adapter.rs` 的 `Client::IsRequestTypeSupported` 实现，以及 `pkg/session/runtime/planning.rs` 的规划期 `SessionPushdownCapabilityClient`。

## 核心职责

- `RequestTypeSupportedChecker::IsRequestTypeSupported` 先按请求大类分流，再判断请求子类型或表达式类型是否在白名单内。
- `RequestTypeSupportedChecker::supportExpr` 保存 Go 版本的历史表达式、聚合函数和窗口函数白名单，并保留两个不属于 protobuf `ExprType` 的 KV 子类型常量。
- `ExprType` 以 `#[repr(i64)]` 固定本文件用到的 tipb `expression.proto` 数值，使这一能力检查不直接依赖 gRPC/protobuf 运行时。它是协议编号的局部镜像，不是完整的 tipb 枚举。
- 返回值只是粗粒度能力门槛。源码注释明确指出，多存储引擎场景应在 planner 做更精确的检查；例如 `GroupConcat` 在本文件返回 `true`，但 TiKV/TiFlash 差异必须由外层继续判断。

## 主要符号

- `pub struct RequestTypeSupportedChecker`：零字段单元结构体，无需构造参数，可直接写作 `kv::RequestTypeSupportedChecker`。公开方法只借用 `&self`，不保存状态。
- `pub enum ExprType`：公开的协议编号集合，分为常量/列引用（`Null` 至 `ColumnRef` 等）、聚合（`Count` 至 `MaxCount`）和窗口函数（`RowNumber` 至 `NthValue`）。显式判别值来自 tipb；调用者可用 `as i64` 传入检查器。
- `pub fn IsRequestTypeSupported(&self, reqType: i64, subType: i64) -> bool`：唯一公开行为入口。名称和参数大小写刻意与 Go API 对齐。
- `fn supportExpr(&self, exprType: i64) -> bool`：私有白名单实现。进入匹配前执行 `exprType as i32 as i64`，复现 Go 将 `int64` 转成 `tipb.ExprType`（底层 `int32`）时的高位截断和符号扩展。
- 请求大类和 KV 子类型并不定义在本文件，而来自同 crate 的 `pkg/kv/kv.rs`：`ReqTypeSelect`、`ReqTypeIndex`、`ReqTypeDAG`、`ReqTypeAnalyze`、`ReqTypeChecksum` 以及 `ReqSubTypeBasic`、`ReqSubTypeDesc`、`ReqSubTypeGroupBy`、`ReqSubTypeTopN`、`ReqSubTypeSignature` 等。

## 执行流程

1. 调用方通过 `kv::Client::IsRequestTypeSupported` 提交 `reqType` 与 `subType`。真实 TiKV 客户端适配器在 `pkg/store/driver/kv_adapter.rs` 将调用委托给无状态的 `RequestTypeSupportedChecker`；规划期适配器在 `pkg/session/runtime/planning.rs` 做同样委托，但其 `Send` 明确不可调用。
2. `IsRequestTypeSupported` 对 `Select`/`Index` 先识别三个请求级子类型：`GroupBy`、`Basic`、`TopN` 立即返回 `true`；其他值进入表达式白名单。
3. `DAG` 的 `subType` 直接按表达式类型检查；`Analyze` 无条件返回 `true`，完全忽略 `subType`；未知请求类型（包括当前的 `Checksum`）返回 `false`。
4. `supportExpr` 先按 Go 语义把输入截断为 32 位有符号数，再依次匹配基础字面量/列引用、聚合函数、窗口函数，以及 `ReqSubTypeDesc` 和 `ReqSubTypeSignature`。
5. 任一白名单分支命中即返回 `true`，其余值返回 `false`。该函数不执行 I/O，也不提供失败原因。

上游对结果的处理取决于场景：`pkg/expression/expr_to_pb.rs` 对不支持的字面量或列引用返回 `None`，让表达式留在 root 执行；`pkg/expression/aggregation/agg_to_pb.rs` 将不支持的聚合转换成错误；`pkg/expression/aggregation/window_func.rs` 返回 `None`；`pkg/session/runtime/relational_scan.rs` 在基础 DAG 不支持时放弃 coprocessor 路径并返回 `Ok(None)`。

## 数据与状态

检查器自身没有字段、缓存或可变状态。全部判断只依赖两个 `i64` 输入和编译期常量，因此相同输入总得到相同结果。

`ExprType` 的数值是兼容性数据：基础类型集中在 `0..=201` 的离散编号，聚合位于 `3001..=3023` 的选定成员，窗口函数位于 `4001..=4011`。`ReqSubTypeDesc = 10000` 与 `ReqSubTypeSignature = 10003` 来自 `pkg/kv/kv.rs`，虽非 protobuf 表达式枚举成员，仍由历史 Go 白名单接受。

一个重要不变量是请求级快速分支发生在 32 位转换之前：例如原值 `ReqSubTypeGroupBy` 只在未增加高位偏移、且请求为 `Select`/`Index` 时走快速放行；进入 `supportExpr` 后则遵循 Go 的 `int32` 截断语义。`pkg/kv/checker_test.rs::test_expr_type_conversion_matches_go` 专门覆盖了正负 `2^32` 偏移、`i64::MIN/MAX` 和边界值。

## 依赖与调用关系

直接下游依赖只有同 crate 从 `pkg/kv/kv.rs` 重导出的请求常量；`supportExpr` 内没有函数调用。`pkg/kv/Cargo.toml` 也没有为本文件单独引入 tipb：协议枚举值由本地 `ExprType` 表达，符合源码“避免耦合 gRPC 运行时”的说明。

关键调用边如下：

- `pkg/store/driver/kv_adapter.rs`：生产存储客户端的 `kv::Client` 实现委托给本检查器。
- `pkg/session/runtime/planning.rs`：规划阶段的只读能力客户端委托给本检查器，使 protobuf 构造可以查询能力而不发送请求。
- `pkg/expression/expr_to_pb.rs`：在编码常量、MySQL 时间、列引用和 DAG 基础协议前调用 `Client::IsRequestTypeSupported`。
- `pkg/expression/aggregation/agg_to_pb.rs` 与 `window_func.rs`：分别在聚合、窗口函数转为 `tipb::Expr` 前检查对应 `ExprType`。
- `pkg/session/runtime/relational_scan.rs`：在进入关系表 coprocessor 扫描前检查 `DAG + Basic`。

RustCodeGraph 对目标文件给出的内部调用边为 `IsRequestTypeSupported -> supportExpr`；索引还显示该文件被表达式聚合、session 规划和独立测试等文件使用。由于同名 Go/Rust trait 方法较多，精确生产调用者以以上实现文件和文本引用交叉核验。

## 错误处理与边界

本文件没有 `Result`、异常、日志或分配失败路径；不支持、未知或不在白名单统一表现为 `false`。因此调用者不能从返回值区分“未知请求类型”“未知表达式编号”或“已知但不允许下推”，需要在各自层面选择回退或构造上下文错误。

主要边界如下：

- `Analyze` 对任意 `subType`（包括负数和极值）均返回 `true`。
- 未知请求大类和 `ReqTypeChecksum` 均返回 `false`；给 `reqType` 添加 `2^32` 不会被截断，因为截断仅作用于表达式 `subType`。
- `Select`/`Index` 的 `Basic`、`GroupBy`、`TopN` 是请求级特例；`DAG` 不享受这些特例，只有其数值经表达式白名单命中时才支持。
- `supportExpr` 的 `i64 -> i32 -> i64` 转换会接受高位不同但低 32 位等于白名单值的输入。这不是普通 Rust 枚举转换，而是为保持 Go 行为而显式保留的兼容规则。
- `GroupConcat` 返回 `true` 不代表所有存储引擎都能执行它；源码明确要求 TiKV 场景在外部追加检查。
- 本地 `ExprType` 若与上游 tipb 编号漂移，编译器无法自动发现；更新协议时必须人工同步并由测试覆盖。

## 并发与资源生命周期

该检查器不持有锁、原子量、事务、任务、通道、文件描述符或网络连接，也不借用调用方资源超过单次调用。所有数据都是栈上的整数和编译期匹配分支，所以没有初始化、关闭、取消或回收阶段。

`RequestTypeSupportedChecker` 的不可变、零状态设计允许并发调用而无需同步。其方法当前只接受 `&self`，但类型未显式派生 `Clone`/`Copy`；调用方通常直接构造单元结构体，或通过实现了 `kv::Client` 的共享对象间接使用。

## 与 Go 版本的对应关系

权威对照是 `pkg/kv/checker.go`，测试对照是 `pkg/kv/checker_test.go`。Rust 的请求分流、基础类型、聚合、窗口函数、`Desc`/`Signature` 特例及默认拒绝分支，与 Go 的两个 `switch` 保持一致；`MaxCount`、`MinCount` 也由 `pkg/kv/checker_test.rs::go_merge_4_max_min_count_are_supported` 补充验证。

实现形式存在三点差异：

- Go 直接使用生成的 `tipb.ExprType`；Rust 在本文件定义所需编号的 `ExprType` 子集，以避免该纯能力判断耦合 protobuf/gRPC 运行时。
- Go 的 `tipb.ExprType(subType)` 隐式按底层 `int32` 转换；Rust 用 `as i32 as i64` 显式复现。Rust 独立测试比 Go 测试覆盖了更多高位截断和整数极值。
- Rust 的方法和局部参数保留 Go 风格名称，服务于逐文件移植的一致性；这不是常规 Rust snake_case 风格。

两边同样只做历史粗粒度白名单，不应把它解释为某个具体 TiKV/TiFlash 版本的完整能力协商。Go 注释中的弃用方向和聚合额外检查说明也已保留在 Rust `supportExpr` 文档中。

## 扩展指南

新增或调整能力时，应先判断它属于哪一层：

- 新请求大类或请求级子类型：修改 `pkg/kv/kv.rs` 的常量及 `IsRequestTypeSupported` 的外层分流；同时核对 `pkg/kv/kv.go` 和 `pkg/kv/checker.go`，避免 Rust/Go 协议分歧。
- 新 tipb 表达式类型：在确认上游 `expression.proto` 的稳定编号后扩展本文件 `ExprType` 和 `supportExpr`，并检查 `pkg/expression/expr_to_pb.rs` 是否具备真实编码实现。仅把编号加入白名单并不能构成可用下推。
- 引擎特定能力：不要扩大这个通用白名单来替代 planner 或 store-specific 检查；应沿现有 `GroupConcat` 注释的边界，在规划/转换层结合 `StoreType` 判断。
- 改变转换规则：必须保持 Go `tipb.ExprType` 的 `int32` 转换语义，除非 Go 侧协议也同步改变；尤其不可直接用完整 `i64` 匹配取代当前截断。

测试逻辑必须继续放在独立的 `pkg/kv/checker_test.rs`，不要嵌入生产文件。至少同步覆盖新增正例、相邻未知编号负例、三个请求路径（`Select`/`Index`/`DAG`）、`Analyze` 的无条件分支和高 32 位输入。若变更影响实际客户端接线，还应检查 `pkg/store/driver/coprocessor_adapter_test.rs`；若影响表达式转换，则应同步相应的 `pkg/expression/*_test.rs` 独立测试。兼容性风险主要是协议编号漂移和错误扩大下推范围；性能风险较低，但热路径中应继续保持无分配的常数时间判断。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/kv/checker.rs`；`files --filter pkg/kv/checker.rs` 确认目标文件有 44 个符号；`node --file pkg/kv/checker.rs --offset 1 --limit 500` 读取完整 213 行；`query` 定位 `RequestTypeSupportedChecker`、`IsRequestTypeSupported`、`supportExpr`、`ExprType` 和相关常量；`callees` 确认 Rust `IsRequestTypeSupported` 调用私有 `supportExpr`，而 `supportExpr` 无下游函数调用。
- 源码与装配：`pkg/kv/checker.rs`、`pkg/kv/kv.rs`、`pkg/kv/lib.rs`、`pkg/kv/Cargo.toml`。
- 直接生产调用证据：`pkg/store/driver/kv_adapter.rs`、`pkg/session/runtime/planning.rs`、`pkg/expression/expr_to_pb.rs`、`pkg/expression/aggregation/agg_to_pb.rs`、`pkg/expression/aggregation/window_func.rs`、`pkg/session/runtime/relational_scan.rs`。
- Go 对照：`pkg/kv/checker.go` 与请求常量/`Client` 接口所在的 `pkg/kv/kv.go`。
- 测试证据：`pkg/kv/checker_test.rs` 覆盖典型白名单、`MaxCount`/`MinCount`、32 位转换、未知请求和整数边界；`pkg/kv/checker_test.go` 覆盖 Go 基准行为。任务为纯文档分析，按计划未运行 Cargo 或代码测试。
- 人工复核结论：该文件存在的原因是为 planner、表达式 protobuf 转换和真实存储客户端提供与 Go 一致的粗粒度下推能力判断；安全扩展必须同时维护协议编号、Go 对照、独立测试及更精确的引擎特定外层检查。
