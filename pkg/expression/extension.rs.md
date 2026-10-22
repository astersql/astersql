# `pkg/expression/extension.rs`

## 文件定位

`extensionFuncSig` 在每次求值前重新取得权限检查器和会话变量，然后调用扩展提供的字符串或整数回调；它还把会话上下文和参数求值能力暴露为 `extension::FunctionContext`。

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

实现形态差异如下：

- Go 包 `init()` 自动给全局函数变量赋值；Rust 把钩子放入 `OnceLock<RwLock<_>>`，并要求显式调用本文件 `init()`。当前未找到生产 Rust 调用点。
- Go 的 `extensionFuncClass` 和 `extensionFuncSig` 按值复制 `FunctionDef`；Rust 用 `Arc<FunctionDef>` 共享，并在相等性判断中要求指针相同。
- Go `newBaseBuiltinFuncWithTp` 统一创建参数 CAST 和 base 元数据；Rust 显式推导 collation、逐项调用 `BuildCastFunction`，再构造 `RegistryBuiltinBase::new_never`。
- Go 返回类型不匹配时回退到 base builtin 的默认求值错误；Rust 在 `evalString`/`evalInt` 开头返回专用错误。
- Go `extensionFnContext` 内嵌 `context.TODO()`；Rust 的 `ExtensionContext` 是空 trait，没有对应的取消/deadline 上下文。
- Go 零参数 `EvalArgs` 返回 `nil` slice；Rust 返回空 `Vec`。

Rust 独立测试目前直接覆盖空定义、缺回调、内建冲突、成功/重复注册、精确键删除和 `init` 幂等；不能据此声称 Go 的完整 SQL/权限集成用例已经在 Rust 中全部回归。

## 扩展指南

新增返回类型时，应同时修改 `newExtensionFuncClass` 的回调存在性与 `flen`、`fieldTypeForEval` 的 MySQL 类型、`builtinFunc` 的对应 `eval*` 入口，以及 `extension-dependency::FunctionDef` 的回调字段/类型。只放宽 `fieldTypeForEval` 不会使该类型成为可注册返回值。测试应继续放在独立的 [extension_runtime_aster_unit_test.rs](extension_runtime_aster_unit_test.rs)，并同步核对 Go `pkg/expression/extension.go` 与 `pkg/extension/function_test.go`。

新增参数类型或改变 cast/collation 时，重点修改 `fieldTypeForEval` 和 `getFunction` 的 cast 构建，验证必选/可选参数前缀、NULL、字符集/排序规则及 `EvalArgs` 所见 Datum。需避免让未知 EvalType 静默以 `TypeUnspecified` 进入回调。

改变权限语义时必须同时保留构建期与运行期检查，覆盖 SEM 开关、空权限列表、多权限首个失败、optional-property 缺失和 prepare 路径。`RequireDynamicPrivileges` 的布尔参数在本文件实际传入 `sem::IsEnabled()`；扩展依赖文件的注释若与此不同，应以调用代码和 Go 对照为准并同步修正文档。

若补齐生产安装接线，应在明确的 crate/进程初始化位置调用并暴露 `extension_kernel::init`，添加独立集成测试证明 `extension::Setup` 能从启动路径到达 `extensionFuncs`，而不只是直接在 crate 内测试私有函数。还应验证 teardown/Reset 的删除回调、部分注册回滚和再次 setup 行为。

主要兼容风险是 Go/Rust 初始化时机差异、精确删除键遗留、权限错误消息差异、可选参数与 cast 类型漂移、Arc 指针相等语义和扩展回调错误类型被字符串化。性能风险集中在每次求值的权限/会话属性读取、参数逐项 Eval 与扩展回调自身；扩展签名明确非向量化且禁用计划缓存，不能在没有行为证明时放宽。
