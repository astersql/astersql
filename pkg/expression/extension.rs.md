# `pkg/expression/extension.rs`

## 文件定位

对应源码：[extension.rs](extension.rs)。本文件属于 `pkg/expression/Cargo.toml` 定义的 `astersql-expression` crate，由 [lib.rs](lib.rs) 以私有模块 `extension_kernel` 挂载。它位于扩展框架与表达式运行时之间：上游接收 `astersql-extension`（Cargo 中别名 `extension-dependency`）的 `FunctionDef`，下游把定义适配成表达式注册表使用的 `functionClass` 和 `builtinFunc`。

SQL 表达式构建时，[scalar_function.rs](scalar_function.rs) 会先查询内建 `funcs`，再查询 `extensionFuncs`；因此一旦注册成功，扩展函数沿普通标量函数构建路径进入求值。常量等级判断也通过 `builtinFunc::isExtensionFunction` 把它视为不可折叠函数，[cache_snapshot.rs](cache_snapshot.rs) 则显式拒绝为扩展 builtin 制作缓存快照。

必须区分“安装入口存在”和“生产启动已接线”：本文件公开的 `init()` 会把注册/删除闭包安装到 `astersql-extension` 的全局钩子，但 `extension_kernel` 在 crate 根是私有模块；仓库检索只发现独立测试调用 `crate::extension_kernel::init()`，没有生产 Rust 调用边。因此当前源码能证明注册适配和安装机制已经实现，不能证明应用启动时一定自动安装了该钩子。

## 核心职责

- `registerExtensionFunc` 校验扩展定义，把函数名规范化为小写，拒绝与内建函数或既有扩展函数冲突，并原子写入 `extensionFuncs`。
- `newExtensionFuncClass` 只接受字符串和整数返回类型，检查对应回调存在，计算返回列宽和可选参数形成的最小/最大参数数。
- `extensionFuncClass::getFunction` 在表达式构建期检查动态权限和参数数量，推导字符集/排序规则，为每个实参插入声明类型对应的 CAST，并创建运行时签名。
- `extensionFuncSig` 在每次求值前重新取得权限检查器和会话变量，然后调用扩展提供的字符串或整数回调；它还把会话上下文和参数求值能力暴露为 `extension::FunctionContext`。
- `checkPrivileges` 根据 SEM 状态计算所需动态权限，生成与 Go 一致的访问拒绝错误；非 SEM 模式的错误提示同时列出 `SUPER`。
- `init` 用 `Once` 幂等安装跨 crate 的注册/删除钩子。

该文件不负责扩展清单解析、扩展生命周期事务、SQL 名称解析或具体业务函数实现；这些分别属于 `pkg/extension`、表达式构建层和扩展提供方。

## 主要符号

- `registerExtensionFunc(Option<&Arc<FunctionDef>>) -> Result<(), ExtensionError>`：本文件的注册入口。`None` 返回 `extension function def is nil`；`FunctionDef::Validate` 负责空名称和 `OptionalArgsLen` 范围；本函数再检查内建冲突、回调/返回类型和重复扩展名。
- `removeExtensionFunc(&str)`：把调用方给出的键原样交给 `extensionFuncs.Delete`。注册键会转成小写，但删除不会转换，因此调用方必须传入规范化名称；独立 Rust 测试明确覆盖大小写不同不能误删。
- `extensionFuncClass`：构建期函数类，保存 `baseFunctionClass`、`PrivilegeCheckerPropReader`、共享的 `Arc<FunctionDef>` 和返回显示宽度 `flen`。
- `fieldTypeForEval(EvalType) -> FieldType`：把 Int、Real、Decimal、String、Datetime、Timestamp、Duration、Json、VectorFloat32 映射到 MySQL 类型，其他类型映射为 `TypeUnspecified`。注册返回值实际只允许 Int/String，但参数 CAST 可使用其余已映射类型。
- `newExtensionFuncClass`：只允许 `ETString`/`ETInt` 返回类型，分别要求 `EvalStringFunc`/`EvalIntFunc`，并使用 `MaxFieldVarCharLength`/`MaxIntWidth` 设置 `flen`。
- `extensionFuncClass::getFunction`：构建运行时 builtin 的关键入口；实现 `functionClass` 的参数校验和显示名接口。
- `checkPrivileges`：遍历 `RequireDynamicPrivileges(sem_enabled)` 的结果，对每项调用 `PrivilegeChecker::request_dynamic_verification(privilege, false)`。
- `extensionFuncSig`：实现 `CollationInfo` 和 `builtinFunc` 的运行时签名；持有 `RegistryBuiltinBase`、两类 optional-property reader 和共享函数定义。
- `extensionFuncSig::evaluate_context`：运行期权限复检与会话变量读取的公共前置步骤，成功后生成借用当前求值上下文的 `extensionFnContext`。
- `extensionFnContext`：实现扩展侧 `ExtensionContext`/`FunctionContext`，提供 `User`、`ActiveRoles`、`CurrentDB`、`ConnectionInfo` 和 `EvalArgs`。
- `init`：使用进程级 `Once` 调用 `InstallExtensionFunctionHooks`；重复调用不会重复覆盖钩子。

本文件没有条件编译项；其直接单元测试由 `lib.rs` 的 `#[cfg(test)]` 显式挂载到独立文件 [extension_runtime_aster_unit_test.rs](extension_runtime_aster_unit_test.rs)。

## 执行流程

注册链如下：

1. 扩展框架持有 `Arc<FunctionDef>`，经已安装的 `RegisterFunction` 钩子进入 `registerExtensionFunc`。
2. 空定义先失败；随后 `FunctionDef::Validate` 检查名称非空、`OptionalArgsLen` 非负且不超过 `ArgTps.len()`。
3. 名称转小写后先查询内建 `funcs`，避免扩展覆盖内建函数。
4. `newExtensionFuncClass` 根据返回类型选择回调和列宽，并以 `ArgTps.len() - OptionalArgsLen`、`ArgTps.len()` 构造参数数量边界。
5. `extensionFuncs.LoadOrStore` 原子注册；已有同名键时保留旧值并返回重复注册错误。

构建与求值链如下：

1. `scalar_function.rs` 规范化 SQL 函数名后，从 `funcs` 或 `extensionFuncs` 取得 `functionClass`，动态派发到 `extensionFuncClass::getFunction`。
2. 构建期从 `BuildContext::GetEvalCtx` 读取权限检查器，执行 `checkPrivileges`，然后校验实参数量。
3. `CheckAndDeriveCollationFromExprs` 根据函数名、返回求值类型和实参推导 collation；返回 `FieldType` 写入该 charset/collation 和预先确定的 `flen`。
4. 实参与 `FunctionDef::ArgTps` 按位置配对，每项调用 `BuildCastFunction`，使扩展回调通过 `EvalArgs` 看到声明的求值类型。可选参数缺省时只处理实际传入的前缀。
5. 构建上下文被标记 `SetSkipPlanCache("extension function should not be cached")`；随后创建 `RegistryBuiltinBase::new_never`，所以签名报告 `vectorized() == false`。
6. `evalString`/`evalInt` 先确认调用入口与声明返回类型匹配，再由 `evaluate_context` 复检权限并读取会话变量，最后调用扩展回调。回调可通过 `extensionFnContext::EvalArgs` 按当前行依次求值全部已 CAST 参数。

权限在构建期和每次运行期各检查一次。这保证 prepare 阶段即可拒绝无权用户，同时执行时不会仅依赖先前构建时的权限快照。

## 数据与状态

进程级可变状态有两处。表达式 crate 的 `extensionFuncs` 是线程安全的函数类注册表：键为注册时生成的小写名称，值为 `Arc<dyn functionClass>`；`LoadOrStore` 保证并发重复注册只有一个定义生效。扩展 crate 的函数钩子存放在 `OnceLock<RwLock<Option<FunctionHooks>>>`，本文件的 `init` 又用 `Once` 把安装限制为一次。

`FunctionDef` 由 `Arc` 在注册表函数类和各运行时签名间共享。`Clone` 运行时签名时会克隆 `RegistryBuiltinBase` 和 `Arc`，不会复制扩展定义内容。`equal` 要求两个签名指向同一个 `FunctionDef` 分配（`Arc::ptr_eq`）且 base 相等；内容相同但分别分配的定义不视为相等。

`extensionFnContext` 只在一次回调调用期间存在，借用 `EvalContext`、`SessionVars` 和当前签名，不拥有会话资源。`ActiveRoles` 创建一个引用向量；`CurrentDB` 创建字符串；`EvalArgs` 创建 `Vec<Datum>`，按参数顺序求值并在首个错误处停止。空参数自然收集为空向量；Go 对照显式返回 `nil`，两者在容器表示上不同。

返回类型元数据取构建时推导的 charset/collation。所有参数目标类型也使用同一推导结果；这意味着新增参数类型或 collation 行为时必须同时检查 CAST 和回调所见 Datum，而不能只改返回类型。

## 依赖与调用关系

crate/模块链为 `pkg/expression/Cargo.toml` → `pkg/expression/lib.rs::extension_kernel` → 本文件。直接外部依赖是 `astersql-extension`、`astersql-util-sem` 和 `astersql-expression-expropt`；类型、chunk、collation、错误、builtin 基类和注册表从同 crate 门面取得。

已核对的上游与下游关系包括：

- `pkg/extension/function.rs::register_extension_function` 克隆全局钩子并调用本文件安装的注册闭包；`remove_extension_function` 通过删除闭包进入 `removeExtensionFunc`。
- `pkg/expression/scalar_function.rs` 在内建表查找失败后调用 `extensionFuncs.Load`，取得本文件生成的函数类。
- `getFunction` 下游调用 `PrivilegeCheckerPropReader`、`checkPrivileges`、`baseFunctionClass::verifyArgs`、`CheckAndDeriveCollationFromExprs`、`BuildCastFunction` 和 `SetSkipPlanCache`。
- `evalString`/`evalInt` 下游调用 `evaluate_context`，再调用 `FunctionDef` 保存的扩展回调；回调反向使用 `FunctionContext::EvalArgs` 求值签名参数。
- `pkg/expression/scalar_function.rs::ConstLevel` 通过 `isExtensionFunction` 阻止常量折叠；`pkg/expression/cache_snapshot.rs` 拒绝扩展 builtin 的 cache snapshot。

RustCodeGraph 精确确认了文件内部边：`registerExtensionFunc -> newExtensionFuncClass`、`getFunction -> fieldTypeForEval/checkPrivileges/extensionFuncSig`、`evaluate_context -> checkPrivileges/extensionFnContext`、`evalString|evalInt -> evaluate_context`、`init -> registerExtensionFunc/removeExtensionFunc`。trait 动态派发和闭包钩子没有形成可靠的直接 callers 输出，因此上述跨模块边由对应源码接线与限定路径检索补证。

## 错误处理与边界

注册错误使用 `extension::ExtensionError`：空定义、定义校验失败、与内建冲突、缺少相应回调、不支持的返回类型和重复扩展名都会终止注册。`FunctionDef::Validate` 在减法计算 `min_args` 前保证 `OptionalArgsLen` 合法，避免负数转换或下溢。

表达式构建/求值错误使用 crate 的 `Error`。optional-property reader 的错误和扩展回调错误都会以 `error.to_string()` 包装，保留消息但不保留原错误的具体 Rust 类型。权限失败使用 `errSpecificAccessDenied`：SEM 开启时只报告所需权限；SEM 关闭时报告 `SUPER or <privilege>`。权限列表为空时成功；多个权限逐项检查，首个失败即返回。

只支持字符串和整数返回回调。若错误地通过不匹配的 builtin 求值入口调用签名，Rust 显式返回 `extension builtin does not return string/int`；正常注册路径已保证正确回调存在，之后对回调的 `expect("validated ... callback")` 依赖该不变量。参数类型映射允许更多 EvalType，但未知类型会变成 `TypeUnspecified`，本层没有额外拒绝。

删除是精确键删除而非大小写无关删除；用原始混合大小写名称调用可能遗留小写注册项。注册与删除之间也没有跨多个函数的事务：扩展框架若批量 setup 中途失败，必须由上层记录已注册项并按名回滚，本文件只提供单项原子注册/删除原语。

当前接线限制同样属于边界：若 `init` 尚未在应用启动路径调用，`pkg/extension/function.rs::register_extension_function` 会返回 `RegisterExtensionFunc is not installed`。这是源码检索得到的当前事实，不应由本文件的测试幂等性推导为生产已安装。

## 并发与资源生命周期

`Arc<FunctionDef>` 及其回调要求 `Send + Sync`，函数类和签名可被线程间共享。注册表的 `LoadOrStore` 提供单键竞争的原子性；两个同名并发注册者中一个成功，另一个收到重复错误。删除和查找仍可与注册并发发生，调用者不能从本文件获得跨操作事务保证。

`init` 的 `Once` 保证同一进程内安装闭包至多一次。扩展 crate 用 `RwLock` 保护钩子，读写时即使锁中毒也通过 `into_inner` 恢复；注册调用前会在读锁外使用克隆后的 `Arc` 闭包，避免执行扩展逻辑时长期持锁。

求值过程同步执行，不创建线程、异步任务、通道、事务、文件或网络句柄。一次回调的上下文和 Datum 向量在调用结束后释放；注册表中的类和 `Arc<FunctionDef>` 持续到删除且所有克隆签名释放为止。参数 CAST 和 `EvalArgs` 会按实际参数数线性分配/求值，动态权限列表也会逐项检查；扩展回调自身的资源与阻塞行为不由本文件管理。

`SafeToShareAcrossSession` 委托给 `RegistryBuiltinBase`，但签名每次求值都从传入 `EvalContext` 重新读取会话变量和权限，不把某个会话的这些引用持久化到签名中。这是共享时避免会话状态泄漏的关键不变量。

## 与 Go 版本的对应关系

直接对照是 [extension.go](extension.go)。Rust 保留了 Go 的主结构与命名：`registerExtensionFunc`、`removeExtensionFunc`、`extensionFuncClass`、`newExtensionFuncClass`、`getFunction`、`checkPrivileges`、`extensionFuncSig`、`extensionFnContext`、`EvalArgs` 和初始化钩子都有一一对应项。

一致语义包括：注册名前转小写、拒绝内建/扩展重名、只支持 String/Int 返回、按可选参数数计算范围、构建期和求值期双重权限检查、非 SEM 提示包含 `SUPER`、参数按声明类型转换、跳过计划缓存、每行回调可取用户/角色/当前库/连接信息和参数 Datum。

实现形态差异如下：

- Go 包 `init()` 自动给全局函数变量赋值；Rust 把钩子放入 `OnceLock<RwLock<_>>`，并要求显式调用本文件 `init()`。当前未找到生产 Rust 调用点。
- Go 的 `extensionFuncClass` 和 `extensionFuncSig` 按值复制 `FunctionDef`；Rust 用 `Arc<FunctionDef>` 共享，并在相等性判断中要求指针相同。
- Go `newBaseBuiltinFuncWithTp` 统一创建参数 CAST 和 base 元数据；Rust 显式推导 collation、逐项调用 `BuildCastFunction`，再构造 `RegistryBuiltinBase::new_never`。
- Go 返回类型不匹配时回退到 base builtin 的默认求值错误；Rust 在 `evalString`/`evalInt` 开头返回专用错误。
- Go `extensionFnContext` 内嵌 `context.TODO()`；Rust 的 `ExtensionContext` 是空 trait，没有对应的取消/deadline 上下文。
- Go 零参数 `EvalArgs` 返回 `nil` slice；Rust 返回空 `Vec`。

Go 测试 `pkg/extension/function_test.go` 覆盖批量 setup 的 nil、空名、内建冲突、同/跨扩展重名，以及非 SEM/SEM 下构建（prepare）和运行权限行为。Rust 独立测试目前直接覆盖空定义、缺回调、内建冲突、成功/重复注册、精确键删除和 `init` 幂等；不能据此声称 Go 的完整 SQL/权限集成用例已经在 Rust 中全部回归。

## 扩展指南

新增返回类型时，应同时修改 `newExtensionFuncClass` 的回调存在性与 `flen`、`fieldTypeForEval` 的 MySQL 类型、`builtinFunc` 的对应 `eval*` 入口，以及 `extension-dependency::FunctionDef` 的回调字段/类型。只放宽 `fieldTypeForEval` 不会使该类型成为可注册返回值。测试应继续放在独立的 [extension_runtime_aster_unit_test.rs](extension_runtime_aster_unit_test.rs)，并同步核对 Go `pkg/expression/extension.go` 与 `pkg/extension/function_test.go`。

新增参数类型或改变 cast/collation 时，重点修改 `fieldTypeForEval` 和 `getFunction` 的 cast 构建，验证必选/可选参数前缀、NULL、字符集/排序规则及 `EvalArgs` 所见 Datum。需避免让未知 EvalType 静默以 `TypeUnspecified` 进入回调。

改变权限语义时必须同时保留构建期与运行期检查，覆盖 SEM 开关、空权限列表、多权限首个失败、optional-property 缺失和 prepare 路径。`RequireDynamicPrivileges` 的布尔参数在本文件实际传入 `sem::IsEnabled()`；扩展依赖文件的注释若与此不同，应以调用代码和 Go 对照为准并同步修正文档。

若补齐生产安装接线，应在明确的 crate/进程初始化位置调用并暴露 `extension_kernel::init`，添加独立集成测试证明 `extension::Setup` 能从启动路径到达 `extensionFuncs`，而不只是直接在 crate 内测试私有函数。还应验证 teardown/Reset 的删除回调、部分注册回滚和再次 setup 行为。

主要兼容风险是 Go/Rust 初始化时机差异、精确删除键遗留、权限错误消息差异、可选参数与 cast 类型漂移、Arc 指针相等语义和扩展回调错误类型被字符串化。性能风险集中在每次求值的权限/会话属性读取、参数逐项 Eval 与扩展回调自身；扩展签名明确非向量化且禁用计划缓存，不能在没有行为证明时放宽。

## 验证依据

- 目标源码：[extension.rs](extension.rs)，完整核对 424 行以及 `registerExtensionFunc`、`newExtensionFuncClass`、`getFunction`、`checkPrivileges`、`extensionFuncSig`、`extensionFnContext` 和 `init`。
- crate 与装配：[Cargo.toml](Cargo.toml) 的 `astersql-expression`、`extension-dependency`、`sem-dependency`、`expropt` 声明；[lib.rs](lib.rs) 的私有 `extension_kernel` 和独立测试挂载。包目录无 `doc.go`，因此没有可读取的 Go 包契约文件。
- 运行时接线：[scalar_function.rs](scalar_function.rs) 的 `extensionFuncs.Load`，`builtin.rs` 的线程安全注册表和扩展函数列表，[cache_snapshot.rs](cache_snapshot.rs) 的扩展 cache-snapshot 拒绝，以及 `scalar_function.rs::ConstLevel` 的不可折叠判断。
- 扩展边界：`pkg/extension/function.rs` 的 `FunctionContext`、`FunctionDef::Validate`、钩子锁、注册/删除转发；限定检索确认 `InstallExtensionFunctionHooks` 的生产实现位置和本文件 `init` 当前没有生产调用点。
- Rust 独立测试：[extension_runtime_aster_unit_test.rs](extension_runtime_aster_unit_test.rs)，覆盖注册验证/原子重复、大小写精确删除和钩子幂等安装；该文件还含相邻 BuildSimpleExpr 工厂并发测试，后者不是本文件行为证据。
- Go 对照：[extension.go](extension.go) 和 `pkg/extension/function_test.go`，用于核对注册、构建、权限、求值上下文、SEM 错误与 setup 回滚测试意图。
- RustCodeGraph：`status` 显示索引有 11,467 个文件、307,296 个节点、1,848,419 条边；使用 `node --file` 完整读取目标文件，查询 `registerExtensionFunc`、`removeExtensionFunc`、`extensionFuncSig`、`extensionFnContext`，并对关键入口运行 callers/callees。图确认了上述内部调用链；跨 trait/闭包的动态边由源码与 `rg` 补证。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付前仅运行任务指定的 11 章节结构验证，并人工复核唯一产物、源码链接、未接线事实和测试文件分离建议。
