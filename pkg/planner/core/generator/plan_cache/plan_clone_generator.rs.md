# `pkg/planner/core/generator/plan_cache/plan_clone_generator.rs`

## 文件定位

本文件是 `astersql-planner-core-generator-plan_cache` crate 的主体实现，负责**生成 Go 源码**，而不是在 Rust 运行时克隆物理计划。它输出 `pkg/planner/core/operator/physicalop/plan_clone_generated.go` 中各物理算子的 `CloneForPlanCache` 方法，使计划缓存命中后可以把旧计划复制到新的 `base.PlanContext`，同时避免跨会话误共享可变对象。crate 入口 `pkg/planner/core/generator/plan_cache/lib.rs` 公开本模块并再导出其公共 API；同文件还用 `#[path = "plan_clone_test.rs"]` 挂载独立 Rust 测试。

`pkg/planner/core/generator/plan_cache/Cargo.toml` 没有常规运行时依赖。它声明的 physicalop 路径依赖位于 `target.'cfg(any())'.dependencies`，因而不会实际启用；Rust 版本通过本文件的静态字段目录代替 Go 的运行时反射。Bazel 的 `pkg/planner/core/generator/plan_cache/BUILD.bazel` 仍将 Go 原版构建为生成器二进制，并用 Go 测试核对已提交生成文件。

## 核心职责

1. 用 `PHYSICAL_STRUCTURES` 固定 25 个需要生成克隆方法的物理计划类型及顺序，并由 `physical_structure_catalog`、`physical_structure_fields` 提供 Go 反射输入的 Rust 镜像。
2. 用 `CloneTag`、`FieldKind`、`StructField` 和 `Structure` 描述字段类型及 `deep`、`shallow`、`must-nil` 策略。
3. 由 `gen_plan_clone_for_plan_cache` 先做整对象浅复制，再逐字段补上深复制、上下文替换、递归计划克隆或不可缓存检查。
4. 由 `write_special_field` 保留 Reader 算子中由原始计划重建展平计划切片的特殊规则；由 `write_field_clone` 将受支持的 Go 类型映射为具体克隆语句。
5. 由 `generate_plan_clone_for_plan_cache_code` 和 `GenPlanCloneForPlanCacheCode` 拼接固定文件头与全部方法；`write_generated_file` 提供可选的覆盖写文件接口。

这些职责与 Go 原版 `plan_clone_generator.go` 的目的相同：集中生成易随结构体字段变化而漂移的克隆代码，并让未知类型在生成阶段失败，而不是静默共享。

## 主要符号

- `PHYSICAL_STRUCTURES: &[&str]`：25 个目标算子的有序清单，从 DML 算子 `Update`、`Delete`、`Insert`，到 Scan、Join、Reader、PointGet、Union 等物理计划。生成顺序和 Go 原版 `GenPlanCloneForPlanCacheCode` 中的 `structures` 一致。
- `Error` 与 `Result<T>`：本模块的字符串错误包装。错误来源包括不支持的字段类型、代码块缩进不平衡、UTF-8 转换和文件写入失败。
- `CloneTag::{Deep, Shallow, MustNil}`：字段策略。默认 `Deep`；`Shallow` 允许沿用初始 `*cloned = *op` 的结果；`MustNil` 在源字段非空时让生成出的克隆方法返回 `(nil, false)`。
- `FieldKind` 与 `scalar_kind`：识别 Go 标量类型。`bool`、整数、浮点数和 `string` 自动允许浅复制，其他类型归入 `Composite`。
- `StructField`：保存字段名、Go 类型字符串、推导种类和标签；`new` 默认深复制，`with_tag` 覆盖策略。
- `Structure`：保存包名、类型名与字段列表；`physical` 固定包名为 `physicalop`，`full_name` 为特殊字段和错误消息提供限定名。
- `CodeGen`：内部字符串缓冲器。`line` 根据行首 `}` 和行尾 `{` 维护制表符缩进，`raw` 原样追加块，`finish` 检查缩进归零并返回字节。
- `must_nil_field`、`allow_shallow_clone`：字段循环的前置分类；后者允许显式 `Shallow` 或任意标量跳过额外生成。
- `gen_plan_clone_for_plan_cache`：单结构体生成入口。公开 API，返回一个完整 Go 方法的字节。
- `write_special_field`：匹配 `package.structure.field`，处理 `TablePlans`、`IndexPlans` 和 `PartialPlans` 的展平重建。
- `write_field_clone`：类型分派核心，覆盖基础切片、嵌入基类、表达式、Datum、Handle、计划节点、可空指针、MutableRanges 和三类 map。
- `write_abort`、`write_guarded`、`write_map_clone`：分别生成失败返回、非空守卫调用和 map 分配/遍历模板。
- `generate_plan_clone_for_plan_cache_code`：为调用方给出的结构列表生成完整 Go 文件。
- `physical_structure_catalog`、`physical_structure_fields`：构造内置目录。后者省略标量字段，因为 Go 和 Rust 两端都会直接浅复制标量。
- `GenPlanCloneForPlanCacheCode`：与 Go 命名一致的全量入口，严格按 `PHYSICAL_STRUCTURES` 顺序读取目录并生成源码。
- `write_generated_file`：先生成后用 `std::fs::write` 覆盖目标路径；当前仓库检索未发现调用者。
- `CODE_GEN_PLAN_CACHE_PREFIX`：固定写入 Apache 头、生成文件警告、`package physicalop` 及所需 Go import。

## 执行流程

全量入口的调用链是 `GenPlanCloneForPlanCacheCode` → `physical_structure_catalog` → `physical_structure_fields`，随后把有序结构列表交给 `generate_plan_clone_for_plan_cache_code`。聚合器先写 `CODE_GEN_PLAN_CACHE_PREFIX`，再逐个调用 `gen_plan_clone_for_plan_cache`，结构之间补一个空行，最后由 `CodeGen::finish` 返回完整字节。

单结构体生成时，先输出方法签名、`cloned := new(Type)` 和 `*cloned = *op`。字段随后按声明顺序处理：

1. `allow_shallow_clone` 为真时不再输出语句，保留整对象复制得到的值。
2. `MustNil` 字段生成 nil 检查；非 nil 表示该计划当前不能安全进入缓存克隆路径，生成的方法返回失败。
3. `write_special_field` 优先处理 Reader 派生字段。例如 `PhysicalIndexLookUpReader.IndexPlans` 根据 `IndexLookUpPushDown` 在树展平和列表展平之间选择；`PhysicalIndexMergeReader.PartialPlans` 从已经克隆的 `PartialPlansRaw` 重建二维展平列表。
4. 普通复合字段进入 `write_field_clone`。计划基类调用 `CloneForPlanCacheWithSelf(newCtx, cloned)`，子计划调用自身 `CloneForPlanCache`，计划切片调用 `ClonePhysicalPlansForPlanCache`，表达式和数据结构调用对应工具克隆函数，`planctx.PlanContext` 被替换为 `newCtx`。
5. 任一递归计划克隆返回 false 时，`write_abort` 生成的守卫立即返回 `(nil, false)`；全部字段完成后返回 `(cloned, true)`。

`write_generated_file` 是另一条薄调用链：`write_generated_file` → `generate_plan_clone_for_plan_cache_code` → `std::fs::write`。它要求调用方显式提供路径和结构元数据，并不会自动选择仓库中的生成文件。

## 数据与状态

生成器输入完全由值对象组成。`Structure.fields` 保持字段顺序；`BTreeMap<String, Structure>` 只用于按类型名查找内置元数据，最终顺序仍由 `PHYSICAL_STRUCTURES` 决定，因此输出具有确定性。`StructField.field_type` 是精确匹配的 Go 类型字符串，既是分派键也是错误信息的一部分；新增别名或类型拼写变化若未补充匹配分支，会明确报错。

生成出的 Go 方法先复制全部字段，再选择性替换不能共享的字段。这个顺序形成重要不变量：浅复制字段无需额外赋值，深复制字段必须覆盖初始别名，`MustNil` 字段只有在 nil 时才允许保留。对于 `*expression.Column` 和 `*expression.Constant`，只有 `SafeToShareAcrossSession()` 为真才共享，否则调用 `Clone()`；这比一律深拷贝保留了安全共享的性能收益。

目录元数据是手工维护的 Go 反射快照。`physical_structure_fields` 明确列出复合字段和 `MustNil` 字段，但刻意省略标量字段；因此它的正确性依赖与 Go 结构定义同步。当前目录还把 `SampleInfo`、runtime filter、外键检查/级联等暂不可安全克隆的状态标为 `MustNil`。

## 依赖与调用关系

RustCodeGraph 显示核心内部边为：

- `GenPlanCloneForPlanCacheCode` 调用 `physical_structure_catalog` 和 `generate_plan_clone_for_plan_cache_code`。
- `physical_structure_catalog` 调用 `physical_structure_fields` 与 `Structure::physical`。
- `generate_plan_clone_for_plan_cache_code` 调用 `gen_plan_clone_for_plan_cache` 以及 `CodeGen::{raw, finish}`。
- `gen_plan_clone_for_plan_cache` 调用 `allow_shallow_clone`、`must_nil_field`、`write_special_field`、`write_field_clone` 和 `CodeGen::{line, finish}`。
- `write_field_clone` 下沉到 `write_abort`、`write_guarded`、`write_map_clone`，并生成对 Go 侧克隆工具和计划接口的调用文本。

仓库直接引用检索显示，Rust 全量入口和细粒度生成 API 当前由 `plan_clone_test.rs::TestPlanClone` 调用；未发现生产 Rust 调用，也未发现 `write_generated_file` 调用。Go 侧 `main` 调用 Go 版 `GenPlanCloneForPlanCacheCode` 并写入 `plan_clone_generated.go`，`plan_clone_test.go::TestPlanClone` 再将重生成结果与该文件逐字节比较。

本文件源码本身只依赖 Rust 标准库的 `BTreeMap`、格式化和路径/文件 API。它生成的 Go 源码依赖 `expression`、planner `base`、planner `util`/`utilfuncp` 与 `sliceutil`；类型分派文本还引用 `planctx`、`kv`、`types`、`ranger`、`property` 等由目标包其他文件提供的标识。应区分“生成器的 Rust 编译依赖”和“生成结果中的 Go 语义依赖”。

## 错误处理与边界

`write_field_clone` 遇到未覆盖的非标量类型时返回 `Error("can't generate Clone method …")`，并附带 Go 类型和结构限定名。这是字段演进的主要防漂移边界。嵌入类型名使用最后一个 `.` 后的片段；当前表内类型满足该格式。`GenPlanCloneForPlanCacheCode` 对目录查找使用 `unwrap()`，其安全性依赖 `physical_structure_catalog` 总是从同一份 `PHYSICAL_STRUCTURES` 构造键；若未来拆开两者，就应改为可传播错误。

`CodeGen::finish` 仅检查由花括号行驱动的缩进计数是否回到零，不是 Go 语法解析器。与 Go 版的 `format.Source` 不同，Rust 版不执行 gofmt 或完整语法验证；准确性由输出对比测试兜底。`raw` 会确保尾部换行，UTF-8 转换失败会被包装为 `Error`，但当前各单结构体结果由本模块生成，正常情况下必为 UTF-8。

生成代码对若干类型有前置约束。例如 `PushedDownLimit` 和 `PhysPlanPartInfo` 分支直接调用方法而不生成 nil 守卫，沿用 Go 原版的假设；`util.HandleCols`、`kv.Handle`、子计划和 `*int` 则显式检查 nil。`MustNil` 不是生成阶段错误，而是在未来执行生成方法时返回不可缓存。`write_generated_file` 会覆盖目标且把 I/O 错误转成字符串，不负责创建父目录、原子替换、权限保留或 gofmt。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或全局可变状态。每次生成都新建局部 `CodeGen`、目录和字节缓冲；相同输入可并行计算，且测试明确验证连续两次全量生成结果一致。`BTreeMap`、`Vec` 和 `String` 都在调用栈内拥有其数据，返回的 `Vec<u8>` 将所有权交给调用方。

唯一外部资源操作是 `write_generated_file` 的同步覆盖写。并发写同一路径没有协调机制，可能互相覆盖；生成和写入之间也没有临时文件/原子 rename。常规仓库流程由单一生成步骤产出文件，再由测试比较，不应并发调用该接口写同一目标。生成出的 Go 克隆方法则通过为新会话替换 `PlanContext`、深复制可变字段和拒绝 `MustNil` 状态来约束跨会话生命周期；这一运行时效果来自生成结果，不代表 Rust 生成器自身持有计划对象。

## 与 Go 版本的对应关系

Rust 的 `GenPlanCloneForPlanCacheCode`、`gen_plan_clone_for_plan_cache`、`must_nil_field`、`allow_shallow_clone`、`CodeGen` 和固定前缀，分别对应 `plan_clone_generator.go` 中的同名/同职责实现。两边覆盖相同 25 个物理算子、相同字段特殊分支和类型克隆模板，目标都是生成 `physicalop` 包中的 `CloneForPlanCache` 方法。

关键差异是元数据来源与格式化：Go 用 `reflect.TypeOf` 枚举真实结构字段并读取 `plan-cache-clone` tag，Rust 用 `physical_structure_fields` 手工镜像非标量字段和标签；Go 最后用 `go/format.Source` 校验并格式化，Rust 用带缩进状态的 `CodeGen` 直接产出目标格式。因此 Rust 版本新增了 `FieldKind`、`StructField`、`Structure` 和静态目录，也承担了手工同步风险。

Go `main` 固定写 `plan_clone_generated.go` 并以日志终止报告失败；Rust 没有二进制 `main`，而是提供返回 `Result` 的库 API和通用 `write_generated_file`。Go 测试只比较重生成字节与提交文件；Rust 的 `plan_clone_test.rs::TestPlanClone` 除同样比较外，还验证二次生成确定性、25 个方法完整性，以及 `MustNil`、`Shallow`、嵌入基类、表达式切片等字段级模板。当前 `plan_clone_generated.go` 确实包含清单中全部 25 个方法及 Reader 展平分支。

## 扩展指南

新增或修改物理算子字段时，先核对 Go 结构体和 `plan-cache-clone` tag，再同步 `physical_structure_fields`；标量仍可省略，复合字段必须记录精确 Go 类型。若出现新的克隆语义，应优先在 `write_field_clone` 增加窄而明确的类型分支；只有依赖具体“算子 + 字段”组合的派生状态，才放入 `write_special_field`。无法安全跨会话复制的字段应标为 `CloneTag::MustNil`，不要为通过生成而误用 `Shallow`。

新增算子必须同时加入 `PHYSICAL_STRUCTURES` 和 `physical_structure_fields`，并与 Go `structures` 顺序保持一致。修改固定 import 或生成文件头时同步 `CODE_GEN_PLAN_CACHE_PREFIX` 与 Go 原版。若启用 `write_generated_file` 进入生产流程，应额外设计原子写、格式化和并发写约束；当前接口不提供这些保证。

测试应继续放在独立文件 `pkg/planner/core/generator/plan_cache/plan_clone_test.rs`，不要嵌入生产源文件。字段模板变化应增加精确片段断言；全量目录变化应保持与 `pkg/planner/core/operator/physicalop/plan_clone_generated.go` 的字节对比。Go 侧同步更新 `plan_clone_generator.go` 和 `plan_clone_test.go`，并按仓库生成流程刷新目标文件。主要风险是：漏字段导致跨会话别名（正确性/隔离风险），错误深拷贝导致性能回退，错误 `MustNil` 导致计划缓存命中减少，以及 Rust 静态目录与 Go 反射结果漂移。

## 验证依据

- Rust 主实现：`pkg/planner/core/generator/plan_cache/plan_clone_generator.rs`，重点为 `PHYSICAL_STRUCTURES`、`gen_plan_clone_for_plan_cache`、`write_special_field`、`write_field_clone`、`physical_structure_catalog`、`physical_structure_fields`、`GenPlanCloneForPlanCacheCode` 和 `write_generated_file`。
- crate/构建边界：`pkg/planner/core/generator/plan_cache/Cargo.toml`、`lib.rs`、`BUILD.bazel`。同目录没有 `doc.go`；包边界由上述三个文件及 Go 的 `package main` 声明核对。
- Go 对照：`pkg/planner/core/generator/plan_cache/plan_clone_generator.go`，核对反射字段遍历、tag 语义、特殊字段、类型分派、gofmt、`main` 写文件入口和固定前缀。
- 独立测试：`pkg/planner/core/generator/plan_cache/plan_clone_test.rs` 与 `plan_clone_test.go`。Rust 测试覆盖确定性、全量方法、提交文件漂移和字段模板；Go 测试覆盖重生成字节一致性。
- 生成结果：`pkg/planner/core/operator/physicalop/plan_clone_generated.go`，核对 25 个 `CloneForPlanCache` 方法、递归失败分支及 Reader 展平调用确实落地。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/planner/core/generator/plan_cache` 找到 Rust/Go 实现与独立测试；`node --file` 完整读取 762 行实现；`query` 区分 Go/Rust 同名入口；`callees` 核对上述内部调用边。由于同名符号使 `callers` 结果不精确，另用限定目录的 `rg` 直接引用检索确认 Rust API 只有测试调用、`write_generated_file` 无仓库内调用。
- 按任务约束未运行 Cargo。结构验证使用任务指定的 `test -f` 与 11 个固定二级标题计数；交付前还人工核对没有把生成期逻辑误述为 Rust 运行时计划克隆，也没有建议把测试放回生产源文件。
