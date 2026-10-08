# `pkg/util/deeptest/statictesthelper.rs`

## 文件定位

本文件实现 `astersql-util-deeptest` crate 的深克隆测试断言。它不是 SQL 请求、事务或存储运行时的一环，而是供测试代码描述任意值的结构，并检查“内容相等但独立分配”或“每个可比较位置都不相等”。crate 入口 `pkg/util/deeptest/lib.rs` 声明并全量再导出 `statictesthelper`；根 facade 又在 `pkg/lib.rs` 的 `util::deeptest` 中再导出该 crate。

`pkg/util/deeptest/Cargo.toml` 将库入口设为 `lib.rs`，唯一直接依赖是 `globset = "0.4"`。由于 Rust 没有 Go `reflect` 那样的通用运行时反射，调用方必须先构造 `DeepValue`；因此本文件是 Go `pkg/util/deeptest/statictesthelper.go` 的显式动态值适配层，而不是对任意 Rust 类型自动反射的通用断言。

## 核心职责

- `DeepValue` 把结构体、指针、切片、数组、标量、映射、接口、函数和 channel 表示为一棵可递归遍历的拥有型值树。指针、切片和映射额外保存调用方提供的 `usize` 地址令牌，用于区分共享存储与深克隆。
- `AssertRecursivelyNotEqual` 从根路径 `$` 开始，要求相同动态类型下每个被遍历的公共位置都不相等；类型不同会立即被视为满足“不相等”。这比“整个对象不相等”更强。
- `AssertDeepClonedEqual` 同时验证内容深度相等和分配身份：默认要求指针、切片、映射地址不同；被 `WithPointerComparePath` 命中的路径则要求地址相同，并停止向其内容递归。
- `WithIgnorePath` 与 `WithPointerComparePath` 用 glob 选择路径，分别跳过比较或改用地址比较。路径由字段名（`$.field`）、序号（`$[0]`）或 map 键（`$[key]`）递归组成。

## 主要符号

- `pub enum DeepValue`：动态值模型。`Struct` 保存类型名和有序字段，`Pointer`/`Slice`/`Map` 保存类型、地址和可空内容，`Interface` 保存可空装箱值；`Function`、`Channel` 仅保存可选地址。`Invalid` 表示无效值，`Unsupported` 明确表示无法处理的类型。
- `DeepValue::{structure,pointer,nil_pointer,slice,nil_slice,array,map,nil_map,interface,nil_interface,function,nil_function,channel,nil_channel}`：公开构造器。`slice` 从首元素推导 `type_name`，空切片使用 `"unknown"`；`map` 的类型名固定为 `"map"`，键被收窄为字符串。
- `DeepValue::{type_name,same_type}`：内部类型判定。`same_type` 同时比较 enum 判别项与逻辑类型名，所以数值相等但 Rust 数值类型不同仍被视为不同类型。
- `signed_from!`、`unsigned_from!`、`float_from!` 及 `From<bool/String/&str>`：为常用标量生成或实现 `Into<DeepValue>`；公开断言因此可直接接收这些标量。
- `enum OptionKind`、`pub struct TestOption`：封装忽略与指针比较两类路径策略。字段私有，调用方通过两个 `With...` 函数创建选项。
- `pub fn WithIgnorePath`、`pub fn WithPointerComparePath`：收集字符串模式；真正的 glob 编译延迟到断言调用时执行。
- `struct StaticTestHelper`：单次断言的执行器，保存两组已编译 `GlobMatcher`。`apply_options` 对每类选项采用赋值而非追加，因此同一类别后出现的选项会整体覆盖先前模式。
- `StaticTestHelper::{assert_recursively_not_equal,assert_recursively_not_equal_option,assert_deep_cloned_equal}`：两套递归算法及可空指针内容辅助逻辑。
- `values`、`map_values`：把 `None` 内容视为空切片，供递归分支统一迭代；nil 是否可接受由进入这些辅助函数前的分支决定。
- `pub fn AssertRecursivelyNotEqual`、`pub fn AssertDeepClonedEqual`：公开入口。每次调用新建 helper、应用选项、把输入转成 `DeepValue`，并以 `$` 为根执行断言。

## 执行流程

`AssertRecursivelyNotEqual` 的流程如下：

1. 创建空的 `StaticTestHelper`，按传入顺序编译选项 glob；同类型后一个选项替换前一个。
2. 把两侧输入转换成 `DeepValue`，从 `$` 调用 `assert_recursively_not_equal`。
3. 路径命中 ignore 时立即成功返回；任一侧为 `Invalid` 时，只有“两侧同时无效”会失败；类型不同则立即成功返回。
4. 同类型结构体、数组和切片按 `zip` 遍历公共部分；map 只递归两侧共有键；指针、切片、map 先要求地址不同。普通路径继续检查内容，pointer-compare 路径只以地址不同为准。
5. 标量逐值要求不等；interface 解包后在原路径继续；function 必须命中 pointer-compare（或此前已忽略），再要求地址不同。其余未支持组合会 panic。

`AssertDeepClonedEqual` 的流程如下：

1. 同样构造 helper、编译选项并从 `$` 进入递归。
2. ignore 路径直接返回；否则先要求两侧 `same_type`。
3. 结构体要求字段数量和字段名顺序一致，数组/切片要求长度一致，map 要求条目数相等且左侧每个键在右侧存在；随后递归比较内容。
4. 非 nil 指针、切片和 map 默认要求地址不同后比较内容；pointer-compare 路径改为要求地址相同，并不检查其内部内容。两侧均 nil 的同类值可直接通过，一侧 nil 一侧非 nil 会失败。
5. 标量要求相等；interface 解包；非 nil function 只能按地址比较；channel 只接受两侧均 nil。无法识别的类型会 panic。

## 数据与状态

所有比较状态都局限于一次公开函数调用。`StaticTestHelper` 只持有编译后的 ignore 和 pointer-compare 匹配器，不写入输入，也没有全局缓存。递归路径是临时 `String`，字段、数组/切片索引和 map 键分别形成点号或方括号片段。

`DeepValue` 的 `address` 是不透明身份令牌，本文件既不解引用也不验证其是否来自真实地址；`0` 主要由 nil 构造器使用，但非 nil 构造器不会拒绝调用方传入 `0`。内容使用 `Box`/`Vec` 拥有，因此正常安全构造形成无环树；实现没有访问集合或递归深度限制，极深输入仍可能消耗大量栈和路径分配。

结构体字段和 map 条目都由 `Vec` 保存。深相等要求结构体字段顺序一致；map 不要求顺序一致，而是按字符串键线性查找，因此最坏比较成本为二次方。递归不等对结构体、切片、数组使用公共前缀，对 map 使用公共键，不要求两侧长度相同；这是该断言的既定语义，不等同于容器整体 `!=`。

## 依赖与调用关系

上游装配链为 `pkg/util/deeptest/lib.rs` → `pub mod statictesthelper` / `pub use statictesthelper::*`，再由 `pkg/lib.rs` 的 `util::deeptest` facade 暴露。workspace 根 `Cargo.toml` 以 `facade_util_deeptest` 指向本 crate；`pkg/ddl/Cargo.toml` 也声明了路径依赖。

可执行的直接调用证据主要在独立测试 `pkg/util/deeptest/statictesthelper_test.rs` 与 `pkg/util/deeptest/migration_aster_unit_test.rs`。仓库搜索还在 `pkg/meta/metabuild/context_test.rs` 找到 API 名称，但它位于 `GO_REFERENCE` 原始字符串中，不是当前 Rust 测试的执行调用，不能当作实际调用边。

下游依赖只有 `globset::{Glob, GlobMatcher}` 以及标准库的判别项、容器和断言宏。RustCodeGraph 能索引本文件 39 个符号及上述测试辅助符号，但对四个公开入口执行 `callers`/`callees` 未返回边；因此调用范围以模块导出、Cargo 声明和文本位置交叉核验，未把空图结果解释为“无人调用”。

## 错误处理与边界

本文件是测试断言工具，不返回 `Result`：所有不满足条件、非法配置或不支持类型都通过 `assert!`、`assert_eq!`、`assert_ne!` 或显式 `panic!` 终止当前测试。`apply_options` 中 `Glob::new` 失败会报告具体模式和 glob 错误。

关键边界包括：

- `AssertRecursivelyNotEqual` 的同类型结构体只 zip 字段，不核对字段数和字段名；手工构造出与真实类型形状不一致的 `DeepValue::Struct` 时，结果只覆盖公共位置。
- nil pointer 的递归不等由 `assert_recursively_not_equal_option` 处理：两侧都 nil 失败，只有一侧 nil 通过。slice/map 的 nil 内容经辅助函数表现为空集合，但地址仍先参与不等断言。
- `AssertDeepClonedEqual` 的 pointer、slice、map 对“两侧都 nil”有快速成功路径；一侧 nil 的指针会触发非 nil 断言，slice/map 随后的长度或地址检查也会阻止把不同状态当作相等。
- function 没有内容可比：非 nil function 必须显式选择 pointer-compare；channel 在深相等中只允许两侧 nil，而递归不等没有 channel 专门分支，会落入 unsupported panic。
- `Unsupported`、递归不等中的 channel，以及其他未覆盖组合最终都会报告类型名并 panic。浮点比较沿用 Rust `PartialEq`，因此 `NaN` 不会深相等，但会满足“不等”。
- glob 按生成的路径文本匹配；字段名或字符串 map 键中的 `.`, `[`, `]` 等字符没有额外转义层，扩展调用方时应先用测试固定预期。

## 并发与资源生命周期

公开函数每次创建独立 helper，输入被按值转换并在调用结束时释放；没有静态可变状态、锁、线程、异步任务、通道操作或外部资源句柄。`DeepValue::Channel` 只是 channel 身份的描述，不创建或使用真实 channel。

因此不同线程可独立调用这些函数，只要传入值本身满足 `Into<DeepValue>` 的移动规则；实现没有跨调用共享 matcher。panic 展开时，helper、已编译 glob、临时路径和拥有的 `DeepValue` 按 Rust RAII 正常释放。递归过程不捕获 panic；测试若要验证失败路径，应像现有 Rust 测试一样在独立测试文件中使用 `catch_unwind`。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/util/deeptest/statictesthelper.go` 使用 `reflect.Value` 自动读取真实值的 kind、字段、地址和 map 键；Rust 版把同一分派显式编码到 `DeepValue`，并要求调用方提供类型名及地址令牌。两版均从 `$` 生成路径，支持 ignore/pointer-compare glob，并保持“同类后一个 option 覆盖前一个”的 `applyOptions` 语义。

两套核心算法的分支意图基本对应：递归不等遍历公共切片前缀和公共 map 键；深克隆相等默认要求内容相同但指针/切片/map 存储不同；pointer-compare 改为验证共享身份并停止内容递归；函数只能忽略或按地址比较；深相等只允许 nil channel。

Rust 版有必须留意的表示差异：map 键限定为 `String`；slice 的逻辑类型名从首元素推导；`UnsafePointer` 没有独立变体；结构体字段可由调用方任意命名和排序；地址的真实性无法由 helper 校验。Go 的 `require.TestingT` 将失败交给测试对象，Rust 版直接 panic。`pkg/util/deeptest/statictesthelper_test.rs` 复刻 Go 主测试，`migration_aster_unit_test.rs` 额外固定 glob 覆盖顺序、channel 和迁移边界。

## 扩展指南

新增值种类时，应同时扩展 `DeepValue`、`type_name`，并在两套递归函数中明确“不等”和“深克隆相等”的语义；不要只加构造器后让新变体落入默认 panic。若新类型持有独立存储，还应定义地址身份、nil 表示、pointer-compare 是否停止递归，以及路径如何构造。

扩展路径选项时，优先修改 `OptionKind`、`TestOption`、`apply_options` 和两个递归入口的前置判断，并明确新选项与“同类后者覆盖前者”的兼容关系。若要支持非字符串 map 键或自动从业务类型生成 `DeepValue`，应在调用侧或单独适配模块实现稳定编码，避免把领域依赖引入这个通用 crate。

任何行为变化都应同步更新独立测试 `pkg/util/deeptest/statictesthelper_test.rs`；涉及迁移差异、glob 顺序、channel 或额外边界时也更新 `pkg/util/deeptest/migration_aster_unit_test.rs`，并核对 Go 文件及 `statictesthelper_test.go`。测试逻辑不应内嵌回生产源文件。主要兼容风险是改变既有 panic 条件、路径字符串或 option 覆盖规则；主要性能风险是 map 线性查找和深层递归，不能在未测量时悄悄改变可观察顺序或错误路径。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/util/deeptest` 列出 Rust/Go 实现和测试；`node --file pkg/util/deeptest/statictesthelper.rs` 读取全部 661 行；`query` 定位 `DeepValue`、两个公开断言和两个 option 构造器；对应 `callers`/`callees` 查询为空，已按索引限制处理。
- 源码与装配：`pkg/util/deeptest/statictesthelper.rs`、`pkg/util/deeptest/lib.rs`、`pkg/util/deeptest/Cargo.toml`、workspace `Cargo.toml`、`pkg/lib.rs`、`pkg/ddl/Cargo.toml`。
- Go 对照：`pkg/util/deeptest/statictesthelper.go` 与 `pkg/util/deeptest/statictesthelper_test.go`。
- Rust 测试：`pkg/util/deeptest/statictesthelper_test.rs` 与 `pkg/util/deeptest/migration_aster_unit_test.rs`；它们覆盖标量、nil、结构体、指针、切片、map、interface、function、channel、glob 及 option 覆盖顺序。
- 仓库引用复核：对公开符号执行 `rg`，确认当前真实 Rust 调用集中在上述独立测试；`pkg/meta/metabuild/context_test.rs` 的命中属于 `GO_REFERENCE` 字符串。
- 本任务只新增说明文档；按总计划要求不运行 Cargo。交付前使用任务指定命令验证文件存在且恰有 11 个固定二级标题，并人工复核未把索引空调用边或 Go 参考字符串写成实际运行关系。
