# `pkg/planner/core/operator/physicalop/physical_schema_producer.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`，其 crate 根为同目录的 `lib.rs`；`lib.rs` 以私有模块 `physical_schema_producer` 装入本文件，再通过 `pub use physical_schema_producer::*` 对外导出两个生产者类型。该包在 `Cargo.toml` 的 `package.metadata.porting.go-package` 中声明对应 Go 包为 `pkg/planner/core/operator/physicalop`。

这里的“producer”不是执行数据的算子，而是计划节点的公共状态组件：`PhysicalSchemaProducer` 服务于物理计划，`SimpleSchemaProducer` 服务于不需要完整物理计划基座的简单计划。它们把输出 `expression::Schema` 的保存、惰性初始化、克隆和内存计量集中起来，具体算子通过组合这些类型复用行为。RustCodeGraph 的文件节点显示本文件被 54 个文件使用；可直接核验的使用者包括 `physical_limit.rs`、`physical_window.rs`、`physical_indexmerge_reader.rs` 和 `physical_delete.rs`。

## 核心职责

- `PhysicalSchemaProducer` 将可选 Schema 与 `BasePhysicalPlan` 绑定。首次调用 `Schema` 时，恰有一个孩子便克隆该孩子的 Schema，否则创建空 Schema；具体算子也可先用 `SetSchema` 写入明确输出。
- `SimpleSchemaProducer` 将可选 Schema、输出名 `NameSlice` 和基础 `baseimpl::Plan` 绑定。它没有孩子推导规则，未设置 Schema 时始终惰性创建空 Schema。
- 两类生产者分别提供计划缓存克隆、普通物理计划克隆或上下文重建所需的状态复制规则，并提供与 Go 字段口径对应的内存估算。
- `ResolveIndices` 对物理生产者下沉到 `BasePhysicalPlan`；简单生产者没有需要解析的表达式列引用，直接成功。

这些职责只管理计划元数据，不负责生成行、执行 SQL、选择物理属性或计算代价。具体算子仍须实现自己的 `Plan`/`PhysicalPlan` trait 接线。例如 `physical_limit.rs::PhysicalLimit::ResolveIndices` 先调用生产者的 `ResolveIndices`，然后再解析 `PartitionBy`、输出 Schema 和前缀列。

## 主要符号

### `PhysicalSchemaProducer`

- `schema: Option<Arc<Schema>>`：可缺省且可共享所有权的输出结构缓存；字段私有，避免调用者绕过初始化规则。
- `BasePhysicalPlan: BasePhysicalPlan`：公开的物理计划公共基座，承载上下文、孩子、统计信息及物理计划通用行为。
- `New(base)`：以未初始化 Schema 包装现有基座。
- `Schema(&mut self) -> &Schema`：需要可变借用，因为可能写入缓存。一个孩子时调用 `child.schema().Clone()`，零个或多个孩子时调用 `expression::NewSchema(Vec::new())`。
- `SchemaRef(&self) -> Option<&Schema>`：只观察缓存，不触发初始化；具体算子的克隆实现用它区分“尚未求值”和“已有输出”。
- `SetSchema(schema)`：用新的 `Arc` 覆盖缓存。
- `ResolveIndices()`：原样传播 `BasePhysicalPlan::ResolveIndices` 的 `expression::Error`。
- `MemoryUsage()`：返回基座计量加一个指针槽位；不重复累计 Schema 内容，这是与同路径 Go 实现一致的物理生产者字段口径。
- `CloneForPlanCacheWithSelf(new_ctx)`：共享当前 `Arc<Schema>`，用 `BasePhysicalPlan::CloneWithNewCtx` 重建基座；基座克隆失败时由 `.ok()?` 转为 `None`。
- `CloneWithSelf(new_ctx)`：先通过 `Schema()` 确保输出已初始化，再深克隆 Schema，并传播基座上下文克隆错误。

### `SimpleSchemaProducer`

- `schema: Option<Arc<Schema>>`、`names: NameSlice`、`Plan: Plan`：分别保存输出列结构、输出列名和简单计划公共状态。
- `New(ctx, tp, offset)`：调用 `NewBasePlan`，Schema 保持未初始化，列名为空。
- `CloneSelfForPlanCache(new_ctx)`：共享 Schema，调用 `NameSlice::Shallow` 复制列名容器语义，并重建底层 Plan。
- `OutputNames` / `SetOutputNames`：读取时返回浅拷贝，写入时整体替换。
- `Schema` / `SchemaRef` / `SetSchema`：分别负责惰性空 Schema、无副作用观察和显式覆盖。
- `SetSchemaAndNames`：在一次可变借用内同步替换 Schema 与输出列名；调用者仍负责保证两者长度和语义匹配。
- `MemoryUsage()`：累计底层 Plan、Schema 指针、切片头、列名容量、已初始化 Schema 内容以及非空 `FieldName` 的内存。
- `ResolveIndices()`：无操作并返回 `Ok(())`。

文件中没有模块级常量、trait、条件编译项或异步入口；全部方法都是两个公开结构体的固有方法。

## 执行流程

物理计划的典型流程如下：

1. 具体算子用 `PhysicalSchemaProducer::New(BasePhysicalPlan::New(...))` 建立公共基座，例如 `PhysicalLimit::New`。
2. 如果算子的输出已知，构造或初始化阶段调用 `SetSchema`；否则保持 `None`。
3. 首次需要输出时调用 `Schema`。它检查缓存：一个孩子则克隆孩子 Schema，其他孩子数则生成空 Schema，然后返回缓存引用。多孩子算子必须自行定义输出，不能依赖默认推导。
4. 解析阶段调用 `ResolveIndices`，先让基座处理其通用内容，再由具体算子处理自身表达式。`PhysicalLimit::ResolveIndices` 展示了这种分层。
5. 普通克隆用 `CloneWithSelf` 得到独立 Schema；计划缓存克隆用 `CloneForPlanCacheWithSelf` 共享 Schema，以避免不必要复制。

简单计划的典型流程是 `SimpleSchemaProducer::New` 后显式设置空或确定的 Schema，再由具体计划转发 `schema`、`output_names` 等 trait 方法。`physical_delete.rs` 中 `Delete::New` 和 `Update::New` 都立即设置空 Schema；它们的计划缓存克隆则调用 `CloneSelfForPlanCache`。若没有显式设置，首次 `Schema` 会创建空 Schema，而不会查看任何孩子。

## 数据与状态

两个 `schema` 字段都以 `Option<Arc<Schema>>` 表示两种状态：`None` 是“尚未初始化”，`Some` 是“已有缓存”，即使其中 Schema 没有列也属于已初始化。`SchemaRef` 保留了这个状态差异，不能用 `Schema` 替代它来做无副作用检查。

`PhysicalSchemaProducer::Schema` 的单孩子规则是默认透传，但保存的是孩子 Schema 的 `Clone`，不是对孩子对象的借用。孩子列表随后变化时缓存不会自动失效；改变孩子或改变输出语义的调用者必须显式重新 `SetSchema`。零孩子与多孩子都默认空 Schema，后者是刻意保守的边界：Join 等多输入节点应提供自身合并后的输出。

`SimpleSchemaProducer` 的 `names` 与 `schema` 是两个独立字段。`SetSchemaAndNames` 便于同步更新，但 `SetSchema` 和 `SetOutputNames` 也允许分别修改，类型本身不检查列数相等。`NameSlice::Shallow` 与 `Arc<Schema>::clone` 都保留共享底层对象的语义，符合计划缓存克隆的只读快照预期。

内存统计是估算协议而非 Rust 分配器的精确账单。物理生产者只计基座和 Schema 指针槽；简单生产者额外计 Schema 内容、`Vec` 容量和每个存在的列名对象。调用者汇总具体算子内存时再把生产者结果与算子自有字段相加。

## 依赖与调用关系

直接依赖如下：

- `base::ContextRef`：克隆时替换计划上下文。
- `baseimpl::{NewBasePlan, Plan}`：构建和保存简单计划基座。
- `expression::{Schema, NewSchema, Error}`：输出结构、空结构构造和索引解析错误类型。
- `types::metadata::NameSlice`：列输出名集合。
- `crate::BasePhysicalPlan`：物理计划公共状态、孩子访问、索引解析、克隆和内存计量。
- `std::sync::Arc`：计划缓存克隆间共享 Schema 所有权。

`Cargo.toml` 将这些分别接到 workspace 内的 `astersql-planner-core-base`、`astersql-planner-core-operator-baseimpl`、`astersql-expression` 和 `astersql-types` 包；本文件没有直接使用该 manifest 中的网络依赖或 feature 开关。

上游不是单一函数，而是一组通过组合转发公共行为的算子。已核验的代表包括：`PhysicalLimit`、`PhysicalWindow`、`PhysicalIndexMergeReader` 持有 `PhysicalSchemaProducer`；`Delete` 和 `Update` 持有 `SimpleSchemaProducer`。`lib.rs` 的公开再导出使包外上层也能经 physicalop crate 使用这些类型。下游调用集中在 `BasePhysicalPlan::{Children, ResolveIndices, MemoryUsage, CloneWithNewCtx}`、`Plan::{CloneWithNewCtx, MemoryUsage}`、`Schema::{Clone, MemoryUsage}` 与 `NameSlice::Shallow`。

RustCodeGraph 的精确方法级查询因仓库内大量同名 `Schema` 节点未返回可用边并超时，因此这里不声称 54 个文件都是直接方法调用者；“被 54 个文件使用”仅采用图工具给出的文件级关系，具体代表边由上述 Rust 文件中的显式字段和调用核验。

## 错误处理与边界

- `PhysicalSchemaProducer::ResolveIndices` 和 `CloneWithSelf` 使用 `Result<_, expression::Error>`，不包装或吞掉下游错误。
- `CloneForPlanCacheWithSelf` 把基座上下文克隆错误折叠成 `None`，表示该生产者不能用于该次计划缓存克隆；它不暴露具体错误文本。
- `Schema` 在赋值后用 `expect("schema initialized")` 取引用。按本函数控制流，无论孩子数为何都会写入 `Some`，该断言是内部不变量检查而非正常输入错误分支。
- 一个孩子才允许默认透传；多个孩子默认空 Schema，防止未经定义地选择某一输入，但也要求具体多输入算子显式设置正确输出。
- `SimpleSchemaProducer::ResolveIndices` 始终成功，只适用于自身不持有待解析表达式的简单基座；具体计划若新增表达式，必须在具体类型中实现解析，不能误以为该空实现会处理它。
- Rust 方法通过引用调用，不存在 Go `MemoryUsage` 的 nil receiver 分支；同路径 Go 实现对 nil receiver 返回 0，而 Rust 不可构造等价的空引用。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。惰性初始化通过 `&mut self` 完成，因此同一个生产者不能在安全 Rust 中被多个调用方并发修改；这里也没有内部锁。

`Arc` 只负责 Schema 的共享所有权和释放时机：最后一个生产者克隆被释放后，底层 Schema 才可释放。计划缓存克隆共享同一个 Schema 分配，独立测试以 `std::ptr::eq` 验证这一点；普通 `CloneWithSelf` 则创建新的 Schema 值。`Arc` 本身不等于本文件承诺 Schema 可被并发修改，本模块也不提供修改共享 Schema 内部的接口。

上下文生命周期通过拥有的 `ContextRef` 间接保存在 `BasePhysicalPlan` 或 `Plan` 中。两个克隆入口都使用传入的 `new_ctx` 重建基座，避免克隆计划继续绑定旧会话上下文。

## 与 Go 版本的对应关系

同路径 `physical_schema_producer.go` 是直接语义对照：两边都定义 `PhysicalSchemaProducer` 和 `SimpleSchemaProducer`，都采用一个孩子透传、其他孩子数为空 Schema的物理默认规则，也都让简单生产者默认返回空 Schema。Rust 的 `Option<Arc<Schema>>` 对应 Go 的 `*expression.Schema` nil/非 nil 状态。

主要保持点：

- 计划缓存克隆共享 Schema；Rust 测试 `plan_cache_clones_share_schema_like_go` 对物理和简单生产者都以指针相等验证。
- 普通物理克隆先复制基座，再克隆 Schema；Rust 会在 Schema 尚未初始化时先执行同样的惰性推导。
- `ResolveIndices` 对物理生产者委托基座，对简单生产者直接成功。
- 物理生产者内存只增加一个指针大小；简单生产者累计指针、切片、容量、Schema 和列名对象。Rust 测试 `memory_usage_matches_go_field_accounting` 固定了这一口径。

需要注意的接口差异：Go 的物理克隆方法接收 `newSelf base.PhysicalPlan`，Rust 当前只接收 `new_ctx` 并调用 `CloneWithNewCtx`；文档不能据此推断 Rust 支持 Go 的 self-rebinding 全部细节。Go `SetSchema` 直接保存传入指针，Rust 接收拥有的 `Schema` 后包装为新 `Arc`。Go 的 `MemoryUsage` 可处理 nil receiver，Rust 引用方法没有该状态。Go 的 `names = s.names` 是切片浅复制，Rust 通过 `NameSlice::Shallow` 明确表达该语义。

## 扩展指南

- 新增单输入、输出不变的物理算子时，可组合 `PhysicalSchemaProducer` 并依赖默认单孩子 Schema 克隆；新增零输入、多输入或改变列布局的算子时，应在构造/初始化或解析阶段显式调用 `SetSchema`。
- 改变孩子集合后要审查已有缓存是否失效。若输出依赖孩子，必须在孩子变化后重建 Schema，不能假设下一次 `Schema` 会重新推导。
- 为简单计划新增列名或 Schema 变换时，优先用 `SetSchemaAndNames` 保持二者一致；若分开设置，应添加独立测试覆盖长度、顺序和字段语义。
- 新增表达式字段时，应在具体计划的独立实现中解析索引，并传播 `expression::Error`；不要把逻辑塞进当前始终成功的 `SimpleSchemaProducer::ResolveIndices`，否则会影响所有简单计划。
- 修改克隆策略时必须分别验证普通克隆的独立 Schema、计划缓存克隆的共享 Schema以及新上下文绑定；现有直接测试在 `physical_schema_producer_test.rs`，具体算子的行为测试继续放在各自独立 `*_test.rs` 文件，不能内嵌进生产源文件。
- 修改内存口径时同步核对 Go 文件的 `MemoryUsage`、Rust 的 `memory_usage_matches_go_field_accounting` 以及组合算子的内存测试。风险包括重复计量共享 `Arc`、漏计容量和破坏上层汇总稳定性。
- 兼容性风险集中在默认多孩子空 Schema、计划缓存共享状态和 Go/Rust 克隆接口差异；性能风险集中在误用普通深克隆代替缓存共享，以及频繁使 Schema 缓存失效。

## 验证依据

本说明基于以下直接证据：

- `pkg/planner/core/operator/physicalop/physical_schema_producer.rs`：RustCodeGraph 文件节点读取的完整 177 行，包含两个结构体及全部方法。
- `pkg/planner/core/operator/physicalop/physical_schema_producer.go`：RustCodeGraph 文件节点读取的完整 166 行，用于逐项核对 Go 语义和接口差异。
- `pkg/planner/core/operator/physicalop/physical_schema_producer_test.rs`：RustCodeGraph 文件节点读取的完整 106 行；覆盖计划缓存 Schema 共享和 Go 内存字段口径。
- `pkg/planner/core/operator/physicalop/Cargo.toml`：确认 crate 名称、crate 根、workspace 依赖和 Go 包映射；该包未声明 feature 段。
- `pkg/planner/core/operator/physicalop/lib.rs`：确认模块装入、测试模块独立存在以及公开再导出。
- `pkg/planner/core/operator/physicalop/physical_limit.rs`：确认物理生产者的构造、Schema 克隆/覆盖和分层索引解析流程。
- `pkg/planner/core/operator/physicalop/physical_delete.rs`：确认 `Delete`/`Update` 对简单生产者的空 Schema 初始化、trait 转发和计划缓存克隆用法。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter` 找到唯一目标文件；文件节点报告 27 个符号及 54 个使用文件。通用名 `Schema` 的 `explore` 结果跨仓库噪声较大，文件限定的精确方法调用查询超时，故代表调用关系另以限定源码搜索核验，并明确保留这一验证限制。

本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 章节结构检查，并人工复核未把测试写入生产文件、未把推测描述为已支持行为。
