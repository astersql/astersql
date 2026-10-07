# `pkg/extension/function.rs`

## 文件定位

`pkg/extension/function.rs` 属于 `astersql-extension` crate，是扩展框架与表达式内核之间的自定义 SQL 函数契约层。`pkg/extension/lib.rs` 通过 `pub mod function` 和 `pub use function::*` 导出公开类型；`pkg/extension/Cargo.toml` 表明本 crate 通过路径依赖复用 `astersql-util-chunk`、`astersql-types`、parser 身份类型和 session variable 类型。

该文件不实现 SQL 名称解析、参数 CAST、权限判定或实际表达式求值器。它定义扩展作者可见的 `FunctionContext`、`FunctionDef` 和回调类型，并用 `FUNCTION_HOOKS` 打破 `astersql-extension` 与 `pkg/expression` 之间的依赖环。真实内核实现位于 `pkg/expression/extension.rs`：其 `init` 安装注册/删除钩子，`registerExtensionFunc` 把定义转为表达式函数类，`extensionFuncSig` 在查询执行时调用本文件保存的求值回调。

## 核心职责

1. 用 `FunctionContext` 规定自定义函数求值时可读取的会话身份、激活角色、当前数据库、连接信息，以及当前行参数的统一求值入口。
2. 用 `FunctionDef` 描述函数名、返回求值类型、位置参数类型、尾部可选参数数量、字符串/整数求值回调和动态权限需求。
3. 用 `FunctionDef::Validate` 执行本层能够独立判断的最小不变量：名称非空，`OptionalArgsLen` 位于 `0..=ArgTps.len()`。
4. 用 `InstallExtensionFunctionHooks`、`register_extension_function` 和 `remove_extension_function` 建立扩展 Manifest 与表达式注册表之间的反向调用通道，避免 crate 循环依赖。

本文件不校验定义是否与内建函数或其他扩展重名，也不校验返回类型对应的求值回调是否存在；这些约束由 `pkg/expression/extension.rs::registerExtensionFunc` 和 `newExtensionFuncClass` 实施。

## 主要符号

- `FunctionContext: ExtensionContext`：对象安全的求值上下文 trait。`User`、`ConnectionInfo` 返回可选借用，`ActiveRoles` 返回角色引用列表，`CurrentDB` 返回拥有的字符串；`EvalArgs(row)` 将当前签名的全部参数表达式求值为 `Vec<Datum>`，错误统一为 `ExtensionError`。
- `EvalStringFunc`：`Arc<dyn Fn(&dyn FunctionContext, chunk::Row) -> Result<(String, bool), ExtensionError> + Send + Sync>`。二元组第二项表示 SQL NULL；错误向表达式执行层传播。
- `EvalIntFunc`：与字符串回调具有相同并发和错误契约，值类型为 `i64`。
- `RequireDynamicPrivileges`：根据布尔参数（表达式层传入 SEM 是否开启）返回本次函数要求的动态权限名列表。
- `FunctionDef`：公开的函数声明。`ArgTps` 描述所有参数的期望求值类型；`OptionalArgsLen` 只允许尾部若干参数省略，因此最小参数数由消费端计算为 `ArgTps.len() - OptionalArgsLen`。`Default` 产生空名称、`EvalType(0)`、空参数和全部空回调，不能直接成为有效注册定义。
- `FunctionDef::Validate`：公开校验入口，只检查名称和可选参数数量范围，并保持 Go 版本错误文案。
- `RegisterFunction` / `RemoveFunction`：内核钩子类型。注册钩子接收共享的 `Arc<FunctionDef>` 并可失败；删除钩子按名称执行且没有返回值。
- `FunctionHooks`：私有的成对钩子容器，派生 `Clone`，使调用时可以先从锁内复制 `Arc` 再释放读锁。
- `FUNCTION_HOOKS` 与 `function_hooks`：进程级 `OnceLock<RwLock<Option<FunctionHooks>>>` 及其惰性初始化访问器。
- `InstallExtensionFunctionHooks`：公开安装入口，以新钩子对替换旧值。
- `register_extension_function` / `remove_extension_function`：crate 内部桥接函数，分别用于 Manifest Setup 注册和清理链卸载。

## 执行流程

初始化阶段，`pkg/expression/extension.rs::init` 通过 `std::sync::Once` 保证只执行一次，然后调用 `InstallExtensionFunctionHooks`，将 `registerExtensionFunc(Some(definition))` 和 `removeExtensionFunc(name)` 包装成线程安全闭包写入全局钩子槽。

扩展声明阶段，调用方通过 `pkg/extension/manifest.rs::WithCustomFunctions` 把 `Vec<Arc<FunctionDef>>` 放入 `Manifest.funcs`。`newManifestWithSetup` 遍历这些定义并调用 `register_extension_function`：该函数取得读锁、克隆钩子对、释放锁，再调用表达式层注册闭包。表达式层随后执行 `FunctionDef::Validate`，检查内建名冲突和重复名，核对返回类型及相应求值回调，计算最小/最大参数数，并写入 `extensionFuncs`。

每次注册成功后，`newManifestWithSetup` 立即向 `clearFuncBuilder` 登记一个按 `FunctionDef.Name` 调用 `remove_extension_function` 的反向清理函数。后续步骤失败时清理链回滚已注册项；正常 Reset/清理时也通过同一路径删除。删除桥接在没有安装钩子时静默返回。

查询执行时不再经过本文件的全局钩子。`pkg/expression/extension.rs::extensionFuncClass::getFunction` 检查动态权限和实参数量、为参数插入声明类型的 CAST、推导返回类型和排序规则，并禁用计划缓存；`extensionFuncSig::evalString` 或 `evalInt` 构造 `extensionFnContext`，再次检查权限，然后调用 `FunctionDef` 中对应的 `Arc` 回调。`extensionFnContext::EvalArgs` 按当前行逐一求值已经 CAST 的参数表达式。

## 数据与状态

`FunctionDef` 拥有名称、参数类型数组和可选回调；注册链以 `Arc<FunctionDef>` 共享同一份不可变定义，表达式函数类和每个运行时签名只克隆 `Arc`。回调闭包也由 `Arc` 共享，闭包捕获的内部可变状态必须由扩展实现者自行同步。

`OptionalArgsLen` 是有符号 `i32`，因此必须先由 `Validate` 排除负数，再由表达式层转成 `usize` 并计算最小参数数。当前调用顺序满足这个不变量；绕过注册流程直接使用未校验定义会破坏该前置条件。

唯一的本地全局可变状态是 `FUNCTION_HOOKS`。`OnceLock` 只固定内部锁的地址，并不限制安装次数；每次 `InstallExtensionFunctionHooks` 都可以用写锁替换当前钩子对。状态没有“卸载为 None”的公开操作。函数定义本身不保存在本文件，实际名称映射位于表达式层的 `extensionFuncs`。

## 依赖与调用关系

上游声明/注册主链是 `WithCustomFunctions` → `Manifest.funcs` → `newManifestWithSetup` → `register_extension_function` → 已安装的 `RegisterFunction` → `pkg/expression/extension.rs::registerExtensionFunc`。反向清理链是 `clearFuncBuilder` → `remove_extension_function` → 已安装的 `RemoveFunction` → `removeExtensionFunc` → `extensionFuncs.Delete`。

运行时链是 SQL 表达式构建 → `extensionFuncClass::getFunction` → `extensionFuncSig::{evalString, evalInt}` → `EvalStringFunc`/`EvalIntFunc`。`FunctionContext` 的真实 Rust 实现是 `pkg/expression/extension.rs::extensionFnContext`，它从 `SessionVars` 和 `EvalContext` 提供用户、角色、当前库、连接信息，并从签名参数求值出 `Datum`。

直接类型依赖来自 crate 内再导出：`auth_identity::{UserIdentity, RoleIdentity}`、`chunk::Row`、`types::{Datum, EvalType}`、`variable::ConnectionInfo`；`util::{ExtensionContext, ExtensionError}` 提供上下文基 trait 和统一错误。标准库的 `Arc`、`OnceLock`、`RwLock` 分别承担共享所有权、进程级惰性初始化和安装/读取同步。

RustCodeGraph 能完整读取目标文件及相邻实现，但对 `InstallExtensionFunctionHooks`、`register_extension_function` 和 `remove_extension_function` 的 callers 查询未返回跨 crate 调用边；上述边由已索引的 `manifest.rs` 与 `expression/extension.rs` 源码直接核验，不把空图结果解释为“没有调用者”。

## 错误处理与边界

`Validate` 按固定顺序短路：空名称先返回 `extension function name should not be empty`；然后拒绝负数或大于参数总数的 `OptionalArgsLen`，错误为 `invalid OptionalArgsLen: <值>`。零个可选参数和全部参数均可选都合法。它不检查名称大小写、参数类型合法性、回调存在性或动态权限名。

注册桥接在钩子尚未安装时返回 `RegisterExtensionFunc is not installed`。锁中毒不会导致 panic：读写路径都以 `poisoned.into_inner()` 继续使用锁内数据。已安装注册闭包的错误原样作为 `ExtensionError` 返回。删除桥接是尽力而为：未安装时不报错，已安装后删除闭包也没有错误通道。

本文件不捕获求值回调 panic，也不重试回调。表达式层仅把回调返回的 `ExtensionError` 文本转换为自己的错误。NULL 由回调返回值中的布尔位表达，扩展实现必须保持值与 NULL 标志的约定。当前表达式实现只接受字符串和整数返回类型；`FunctionDef` 的字段类型本身并未静态限制其他 `EvalType`，注册时才会报不支持。

## 并发与资源生命周期

全部公开回调 trait object 均要求 `Send + Sync`，并放在 `Arc` 中，允许跨会话、跨线程共享。`FunctionContext` 继承 `ExtensionContext`；当前后者是标记 trait，具体上下文由一次表达式求值借用 `EvalContext`、`SessionVars` 和签名，因此扩展回调不得保存这些借用超过调用期。

安装使用 `RwLock` 串行写入，而注册和删除只短暂持有读锁：两者先克隆 `FunctionHooks`，随后在锁外调用外部闭包，避免回调期间长期占锁或形成重入死锁。并发重新安装与注册之间的线性化点是各自取得锁的时刻；已经克隆旧钩子的调用会继续使用旧钩子完成。

本文件不创建线程、任务、通道或事务。`OnceLock` 与其 `RwLock` 生命周期覆盖整个进程；钩子以及其捕获资源通常也驻留到进程退出或被后续安装替换。函数定义的实际生命周期由 Manifest、表达式注册表和已构建表达式持有的 `Arc` 共同决定；从注册表删除不保证已经构建的签名立即释放定义。

## 与 Go 版本的对应关系

`pkg/extension/function.go` 是直接语义基准。`FunctionContext` 的五个方法、`FunctionDef` 的七个字段及 `Validate` 的两项检查与错误文案保持一致。Go 的函数值在 Rust 中变为 `Option<Arc<dyn Fn + Send + Sync>>`，指针/切片分别对应借用或 `Arc`/`Vec`，Go 的 `error` 对应 `ExtensionError`。

Go 使用包级可变函数变量 `RegisterExtensionFunc` 和 `RemoveExtensionFunc` 来规避依赖环；Rust 以私有的 `OnceLock<RwLock<Option<FunctionHooks>>>` 封装同一机制，并增加显式安装函数、未安装注册错误以及锁中毒恢复。Go 的 `FunctionDef` 可以用 `nil` 指针表示缺失定义，Rust Manifest 使用 `Vec<Arc<FunctionDef>>`，因此 Rust 表达式注册函数仍保留 `Option<&Arc<FunctionDef>>` 来覆盖 Go 的 nil 错误语义，但本文件的桥接签名本身不接受空定义。

`pkg/extension/function_test.rs::canonical_extension_function_validates_name_and_optional_arity` 覆盖 Rust 本层的空名、合法可选参数、负数和超过参数总数，并核对精确错误文本。Go 的 `function_test.go` 还覆盖会话上下文、字符串/整数求值、可选参数、冲突/重复、动态权限和不被常量折叠；其中大部分行为属于表达式消费层。对应 Rust 的注册原子性和钩子幂等证据在 `pkg/expression/extension_runtime_aster_unit_test.rs`。

## 扩展指南

新增函数元数据时，应先修改 `FunctionDef` 及 `Default`，再同步 `pkg/extension/manifest.rs` 的存储/清理需求和 `pkg/expression/extension.rs` 的校验、函数类构造及运行时消费。若语义来自 Go，还应同步核对 `function.go` 与 `function_test.go`；不要只给字段加默认值而遗漏注册期不变量。

增加返回类型时，至少需要新增对应回调类型和 `FunctionDef` 字段，并扩展表达式层的 `fieldTypeForEval`、`newExtensionFuncClass` 与 `builtinFunc` 求值方法。测试应独立放在 `pkg/extension/function_test.rs`（本层定义校验）和 `pkg/expression/extension_runtime_aster_unit_test.rs` 或同目录其他独立测试文件（注册及运行时），不得嵌入生产源文件。

调整参数规则时必须保持“可选参数只能位于尾部”的现有表示，或显式迁移数据模型；特别要在任何 `i32 → usize` 转换和减法之前维持 `Validate` 调用。调整名称规则时要同时评估注册时小写化、删除时精确键行为、内建冲突和错误兼容性。

若修改全局钩子，应保留锁外调用外部闭包的结构，并为未安装、重复安装、并发安装/调用和锁中毒策略补充独立测试。性能上需避免在逐行求值路径复制大对象；兼容性上需关注计划缓存禁用、权限在构建期与执行期双检，以及已构建签名在注册表删除后的 `Arc` 生命周期。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/extension` 列出目标 Rust/Go 文件和独立测试。
- RustCodeGraph 目标证据：`node --file pkg/extension/function.rs` 完整读取 155 行；`query FunctionDef`、`query InstallExtensionFunctionHooks` 定位定义和相邻消费者；`callees register_extension_function` 确认其调用 `function_hooks`。跨 crate callers 未被图解析，改由相邻已索引源码核验。
- 注册与运行时证据：读取 `pkg/extension/manifest.rs::{WithCustomFunctions,newManifestWithSetup}`、`pkg/expression/extension.rs::{registerExtensionFunc,newExtensionFuncClass,extensionFuncSig,extensionFnContext,init}` 和 `pkg/extension/lib.rs`。
- crate 与 Go 对照证据：读取 `pkg/extension/Cargo.toml`、`pkg/extension/function.go` 和 `pkg/extension/function_test.go`。
- Rust 测试证据：读取 `pkg/extension/function_test.rs::canonical_extension_function_validates_name_and_optional_arity`；读取 `pkg/expression/extension_runtime_aster_unit_test.rs::{extension_registration_matches_go_validation_and_atomicity,extension_hooks_can_be_installed_idempotently}`。
- 本任务为纯文档分析，按计划未运行 Cargo 或代码测试；验证采用源码/调用关系人工复核与固定十一章节结构检查。
