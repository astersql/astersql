# `pkg/expression/generator/helper/helper.rs`

## 文件定位

本文件属于独立 crate `astersql-expression-generator-helper`，crate 根为同目录的 `lib.rs`，并由根 `Cargo.toml` 将 `pkg/expression/generator/helper` 列为 workspace member。`lib.rs` 以 `pub mod helper` 装入本文件，再用 `pub use helper::*` 对外重导出全部公开项。

它对应 Go 包文件 `pkg/expression/generator/helper/helper.go`，描述向量化表达式**源码生成阶段**使用的类型模板上下文；它不是 SQL 运行时的表达式求值器，也不保存会话或查询状态。Go 侧 `compare_vec.go`、`control_vec.go`、`other_vec.go`、`string_vec.go` 和 `time_vec.go` 通过点导入该 helper 包实际消费这些定义。当前 Rust 侧的各生成器仍在自己的文件中定义同类 `TypeContext`/常量，根 Cargo 清单也未显示其他 crate 对本 crate 的路径依赖，因此本文件目前是已迁移但尚未统一接入各 Rust 生成器的共享模型。

## 核心职责

`TypeContext` 把一种 TiDB `types.EvalType` 映射为 Go 代码模板需要的五项信息：EvalType 后缀、`VecEval*` 方法后缀、`chunk.Column` API 后缀、生成代码中的 Go 类型名，以及固定长/变长存储分类。七个 `TYPE_*` 常量提供 Int、Real、Decimal、String、Datetime、Duration、JSON 的标准映射。

这些值本身不执行生成、不解析模板，也不访问 `chunk.Column`；它们是供生成器插值和选择模板分支的不可变元数据。特别是 `fixed` 决定生成代码适合使用 `Resize*` 加类型切片批量写入，还是使用 `Reserve*`/`Append*`/`Get*` 的变长值路径；该语义由 Go 模板（例如 `control_vec.go` 与 `compare_vec.go`）和 Rust 迁移测试共同佐证。

## 主要符号

- `pub struct TypeContext`：由五个 `&'static str`/`bool` 字段组成，并派生 `Clone`、`Copy`、`Debug`、`Default`、`Eq`、`PartialEq`。字段均公开，方便模板渲染代码直接读取；字符串保留目标 Go 标识符的大小写。
- `et_name`：形成 `types.ET{{...}}` 一类名称或参与按 EvalType 分支，例如值 `"Datetime"`、`"Json"`。
- `type_name`：形成 `VecEval{{...}}` 等表达式 API 名称；Datetime 的值特意是 `"Time"`，JSON 是全大写 `"JSON"`。
- `type_name_in_column`：形成 `chunk.Column` 的 `Append*`、`Resize*`、`Reserve*`、`Get*` 或 `*s()` API 名称；Int、Real、Duration 分别映射为 `Int64`、`Float64`、`GoDuration`。
- `type_name_go`：模板输出中的具体 Go 类型文本，例如 `types.MyDecimal`、`types.Time`、`time.Duration`、`json.BinaryJSON`。
- `fixed`：固定长列分类。Int、Real、Decimal、Datetime、Duration 为 `true`；String、JSON 为 `false`。
- `TYPE_INT`、`TYPE_REAL`、`TYPE_DECIMAL`、`TYPE_STRING`、`TYPE_DATETIME`、`TYPE_DURATION`、`TYPE_JSON`：七个 `pub const TypeContext`，逐字段对齐 Go 的 `TypeInt` 等包变量。常量采用 Rust 大写命名，值中的 Go 拼写保持不变。

文件没有函数、trait、impl、宏或条件编译项；条件编译只出现在 `lib.rs`，用于把独立测试文件 `migration_aster_unit_test.rs` 挂入测试构建。

## 执行流程

1. crate 加载 `lib.rs`，后者声明 `helper` 模块并重导出本文件符号。
2. 调用方选择与目标 EvalType 对应的 `TYPE_*` 常量，并把 `TypeContext` 交给源码模板渲染逻辑。
3. 模板用 `et_name` 和 `type_name` 拼出 EvalType/向量求值方法名，用 `type_name_in_column` 拼出列访问 API，用 `type_name_go` 写出 Go 静态类型。
4. 模板检查 `fixed`：固定长类型可生成直接取得类型切片并按行写入的代码；String/JSON 则生成保留容量、逐值读取或追加的代码。
5. 生成器再负责格式化和写出目标 Go 文件；这些 I/O 与模板错误处理均不在本文件内。

以上第 2 至 5 步是 Go 生成器的实际调用方式。Rust 迁移版当前只完成第 1 步和测试中的常量读取；`rg` 未发现其他 Rust crate 引用 `astersql-expression-generator-helper`，相邻 Rust 生成器使用各自的重复定义。

## 数据与状态

`TypeContext` 只持有静态字符串切片和一个布尔值，不拥有堆分配数据。七个预置项是编译期常量，不会被修改。`Copy` 允许调用方按值传递而不引入借用生命周期，`Eq`/`PartialEq` 允许完整值比较，`Debug` 便于诊断。

`Default` 产生四个空字符串和 `fixed = false`，与 Go 结构体零值相符；它不代表七种已知 EvalType 中的任何一种。因而模板若接收默认值，可能拼出空标识符。当前类型没有构造校验，也没有枚举约束来阻止这种无效组合，正确性依赖调用方选择预置常量或自行保证字段组合有效。

重要不变量是五个字段必须作为一组保持一致。例如 Duration 的求值后缀是 `Duration`、列切片后缀却是 `GoDuration`；Datetime 的 EvalType 后缀是 `Datetime`、求值/列后缀则是 `Time`。只修改单个字段会使生成代码调用不存在的 Go API 或使用错误类型。

## 依赖与调用关系

- 上游模块入口：`pkg/expression/generator/helper/lib.rs` 声明并重导出本文件；根 `Cargo.toml` 注册该独立 workspace member。
- crate 边界：`pkg/expression/generator/helper/Cargo.toml` 只有包、库路径、独立 workspace 和迁移元数据，没有普通依赖、开发依赖或 feature。
- Rust 直接使用者：`migration_aster_unit_test.rs` 导入 `TypeContext` 和全部七个常量，验证字面值、固定长分类和默认值。RustCodeGraph 对目标文件给出的符号主要是 `TypeContext` 与常量集合，精确 callers/callees 查询没有得到函数调用边，这与本文件纯数据定义、无函数体一致。
- Go 实际调用者：五个 `//go:build ignore` 生成器点导入 `pkg/expression/generator/helper`。例如 `compare_vec.go` 的模板读取 `Fixed`、`TypeNameInColumn`、`ETName`，`control_vec.go` 据 `Fixed` 在 `Resize`/`Reserve` 路径间分支。
- 下游语义依赖：字段文本指向生成后 Go 代码的 `expression.VecExpr` 方法、`pkg/util/chunk.Column` API，以及 `pkg/types`、`time`、`pkg/types/json` 中的目标类型；Rust crate 本身不链接这些 Go 包。
- 当前迁移缺口：`pkg/expression/generator/{compare_vec,control_vec,other_vec,string_vec}.rs` 中可见本地 `TypeContext` 和 `TYPE_*` 定义，且 `pkg/expression/generator/Cargo.toml` 没有依赖 helper crate。本文件不能被描述为这些 Rust 生成器当前的真实共享来源。

RustCodeGraph 的文件级“used by”提示曾把同名 `TypeContext` 关联到 `pkg/sessionctx/stmtctx/stmtctx.rs`，但源码搜索表明该处是从 `types_crate::scalar::Context` 重命名导入的另一类型；因此不计为本文件调用者。

## 错误处理与边界

本文件不返回 `Result`、不触发 I/O，也没有显式 panic 路径。风险集中在无验证的字符串协议：拼写、大小写或 `fixed` 分类错误不会在此处报错，而会在生成结果编译、格式化或行为验证时暴露。

边界包括：只预置七种 EvalType；没有未知类型回退；`Default` 是可构造但不可直接安全渲染的零值；`type_name_in_column` 注释提到“未特化时与 `type_name` 相同”，但 Rust 结构不会自动补值，当前七个常量均显式填写。JSON 和 String 被分类为变长；其余五种为固定长。文件也不负责 SQL NULL、排序规则、无符号整数或时间精度等运行时语义，这些由具体生成模板及生成后的表达式实现处理。

## 并发与资源生命周期

这里没有锁、原子量、线程、异步任务、通道、事务或文件句柄。所有预置数据都是只读编译期常量，可跨调用并发读取；`&'static str` 与进程生命周期相同。按值复制 `TypeContext` 不共享可变所有权，也没有析构顺序或资源回收要求。

生成器所涉及的模板缓冲区、输出文件和格式化过程属于调用方生命周期，不由本文件管理。未来若把字段改为拥有型 `String`、加入可变注册表或惰性缓存，应重新评估 `Copy`、线程安全和初始化顺序，而不能沿用当前“零资源生命周期”结论。

## 与 Go 版本的对应关系

Rust `TypeContext` 与 Go `TypeContext` 逐字段对应：`et_name`/`ETName`、`type_name`/`TypeName`、`type_name_in_column`/`TypeNameInColumn`、`type_name_go`/`TypeNameGo`、`fixed`/`Fixed`。七个 Rust 常量也逐字对齐 Go 的 `TypeInt`、`TypeReal`、`TypeDecimal`、`TypeString`、`TypeDatetime`、`TypeDuration`、`TypeJSON`。

语言层差异是：Go 使用可变包级 `var` 和拥有型 `string`，Rust 使用不可变 `const` 与 `&'static str`；Rust 额外派生复制、比较、调试和默认能力。Go 的导出字段可被调用方修改副本或包变量，Rust 常量本身不能被修改，但公开字段允许调用方构造任意组合。

Go 侧已形成共享 helper 包并被五个生成器点导入；Rust 侧当前把 helper 做成单独 crate，却尚未替换相邻生成器里的重复模型。这是接线状态差异，不是字段语义差异。`migration_aster_unit_test.rs` 的三项测试分别锁定七组字面值、固定长/变长集合和 Go 零值语义。

## 扩展指南

- 新增 EvalType 时，应在 `TypeContext` 语义允许的前提下新增对应预置常量，并同步 Go `helper.go`；逐项确认目标 `VecEval*`、`chunk.Column` API 和 Go 类型名真实存在。
- 修改现有映射时，应把五个字段作为协议整体审查，尤其核对 Datetime、Duration、JSON 的不等名映射和 `fixed` 分支。同步更新独立测试 `pkg/expression/generator/helper/migration_aster_unit_test.rs`，不要把测试嵌回生产源文件。
- 若要让 Rust 生成器真正复用本 crate，应在其 Cargo 清单增加显式依赖，并逐个删除/替换重复定义；这是跨文件接线任务，不属于本文件当前实现事实。还需运行各生成器的独立渲染测试，比较生成文本而非只验证 helper 常量。
- 若要防止非法组合，可考虑私有字段、枚举或受校验构造器，但这会改变公开 API，并可能失去 `const`/`Copy` 的简单性；需先盘点所有调用方。
- 兼容风险主要是生成出的 Go 标识符变化；正确性风险是错误选择固定长分支；性能风险来自把固定长类型误走逐值 Append/Get 路径。新增字段还会影响所有结构体字面量和派生比较。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引覆盖 Rust/Go；`node --file pkg/expression/generator/helper/helper.rs --offset 1 --limit 400` 读取了完整 101 行源文件；`query TypeContext --kind struct`、`query TYPE_INT --kind constant`、`callers/callees TypeContext`、`callers/callees TYPE_INT` 和 `files --filter pkg/expression/generator` 用于核对符号、无函数调用边以及相邻生成器重复定义。
- Rust 源与入口：`pkg/expression/generator/helper/helper.rs`、`pkg/expression/generator/helper/lib.rs`、`pkg/expression/generator/lib.rs`，以及相邻 `compare_vec.rs`、`control_vec.rs`、`other_vec.rs`、`string_vec.rs` 的搜索命中。
- 清单与构建边界：根 `Cargo.toml`、`pkg/expression/generator/helper/Cargo.toml`、`pkg/expression/generator/Cargo.toml`、`pkg/expression/generator/helper/BUILD.bazel`。
- Go 对照与真实模板消费：`pkg/expression/generator/helper/helper.go`，以及 `compare_vec.go`、`control_vec.go`、`other_vec.go`、`string_vec.go`、`time_vec.go` 的 helper 点导入和模板字段使用。
- 独立测试：`pkg/expression/generator/helper/migration_aster_unit_test.rs`；同目录没有 Go `_test.go`，该 Rust 测试是目标 helper 的直接回归证据。相邻生成器测试验证的是各自的 Rust 重复定义，不能替代本 crate 测试。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构命令和人工事实复核作为交付验证。
