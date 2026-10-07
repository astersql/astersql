# `pkg/expression/aggregation/window_func.rs`

## 文件定位

本文件属于 `astersql-expression-aggregation` crate，是窗口函数在规划期使用的描述层，而不是逐行执行窗口计算的执行器。它把函数名、参数表达式和推导出的返回类型封装为 `WindowFuncDesc`，同时提供窗口帧分类、TiFlash 下推资格检查以及 `tipb::Expr` 序列化。crate 入口 `pkg/expression/aggregation/lib.rs` 通过 `mod window_func` 装配并以 `pub use window_func::*` 导出这些 API；`pkg/expression/aggregation/Cargo.toml` 将本 crate 的库入口设为 `lib.rs`，并声明其对 `expression`、`kv`、parser AST/MySQL 类型和 `tipb` 的依赖。

在已索引的 Rust 生产代码中，最明确的应用主链位于 `pkg/planner/core/operator/physicalop/physical_window.rs` 的 `PhysicalWindow::ToPB`：它为每个物理窗口函数调用 `NewWindowFuncDesc`，再调用 `WindowFuncToPBExpr`，最终把结果写入 `tipb::Window.func_desc`。窗口实际如何按分区、排序和帧执行不在本文件内，而由 planner/executor 的窗口模块承担。

## 核心职责

1. `NewWindowFuncDesc` 在规划期规范化函数名、检查特定窗口参数并复用 `newBaseFuncDesc` 完成类型推导。
2. `NO_FRAME_WINDOW_FUNCS`、`UseDefaultFrame` 和 `NeedFrame` 表达“按整个分区计算”与“需要窗口帧”的分类规则，并为 `ROW_NUMBER` 提供固定的 `CURRENT ROW` 默认帧。
3. `WindowFuncDesc::Clone` 深拷贝基础描述符，避免多个计划节点共享可变的参数表达式描述。
4. `WindowFuncDesc::CanPushDownToTiFlash` 同时检查参数表达式和窗口函数名；内部 `canExprPushDownToTiFlash` 补上类型、标量函数黑名单及 PB 可编码性门禁。
5. `WindowFuncToPBExpr` 把描述符转换为存储端协议表达式，并在客户端不支持表达式类型或任一参数无法编码时拒绝转换。

这些职责都只处理元数据、能力判断和协议对象构造；本文件不保存窗口分区数据，也不计算 `RANK`、`LEAD`、聚合值等运行期结果。

## 主要符号

- `pub struct WindowFuncDesc { pub baseFuncDesc: baseFuncDesc }`：窗口函数描述符。`Deref`/`DerefMut` 将字段访问委托给 `baseFuncDesc`，因此调用方可直接使用 `Name`、`Args`、`RetTp` 以及基础描述符方法。
- `NewWindowFuncDesc(ctx, name, args, skip_check_args) -> Result<Option<WindowFuncDesc>, Error>`：公开构造入口。`Err` 表示基础描述或类型推导失败；`Ok(None)` 表示窗口专属参数不合法；`Ok(Some(_))` 表示构造成功。
- `NO_FRAME_WINDOW_FUNCS`：不使用显式帧的八个函数，即 `CUME_DIST`、`DENSE_RANK`、`LAG`、`LEAD`、`NTILE`、`PERCENT_RANK`、`RANK`、`ROW_NUMBER`。
- `useDefaultFrameWindowFuncs()`：每次调用新建默认帧映射；当前只有 `ROW_NUMBER -> ROWS BETWEEN CURRENT ROW AND CURRENT ROW`。
- `UseDefaultFrame(name)`：大小写不敏感地查询默认帧，未命中时返回 `(false, FrameClause::default())`。
- `NeedFrame(name)`：大小写不敏感地判断函数是否不在 `NO_FRAME_WINDOW_FUNCS` 中；未知函数也会返回 `true`。
- `WindowFuncDesc::Clone()`：通过 `baseFuncDesc::clone_desc` 复制名称、参数和返回类型。
- `WindowFuncDesc::CanPushDownToTiFlash(ctx)`：先验证所有参数表达式，再把函数名限制在明确白名单中。
- `canExprsPushDownToTiFlash` / `canExprPushDownToTiFlash`：文件内私有递归门禁；拒绝 TiFlash 不支持的字段类型、无效 Decimal、被禁用的标量函数或无法转换为 PB 的表达式。
- `WindowFuncToPBExpr(ctx, client, desc) -> Option<tipb::Expr>`：协议转换入口，填充 `tp`、`children` 和 `field_type`。

本文件没有条件编译项、trait 定义或模块级可变静态状态。

## 执行流程

构造描述符时，`NewWindowFuncDesc` 先把函数名转为小写。若 `skip_check_args` 为 `false`，它按函数种类执行前置检查：`NTH_VALUE` 的第二个参数和 `NTILE` 的第一个参数必须可解析为无符号常量，非 NULL 的零值被拒绝；`LEAD`/`LAG` 只有在至少两个参数时检查偏移，偏移必须是非 NULL 的无符号常量。缺少 `NTH_VALUE`/`NTILE` 所需位置的参数也返回 `Ok(None)`，避免索引越界。

随后函数调用 `newBaseFuncDesc`。该函数先保存小写名称与参数，再执行 `baseFuncDesc::TypeInfer`；错误通过 `?` 原样传播。类型推导成功后，构造器调整返回类型的 `NotNullFlag`：排名类、`COUNT`、`APPROX_COUNT_DISTINCT` 和位运算聚合被标记为非 NULL；三参数 `LEAD`/`LAG` 仅当取值表达式和默认值都为非 NULL 时标记为非 NULL；其余函数清除该标记。最终返回 `Ok(Some(WindowFuncDesc))`。

物理计划序列化时，`PhysicalWindow::ToPB` 使用 `skip_check_args=true` 重建描述符。这与预处理语句参数尚未初始化时跳过常量检查的语义一致；随后 `WindowFuncToPBExpr` 取得 `GetTiPBExpr(true)` 的协议类型，检查客户端对 `ReqTypeSelect + ExprType` 的支持，逐个转换参数，并设置返回字段类型。任一能力检查或参数转换失败都返回 `None`，调用方将其转成“window function cannot be pushed down”错误。

TiFlash 资格判断是另一条流程。`CanPushDownToTiFlash` 递归检查参数字段类型、标量函数名/完整签名黑名单和 PB 编码能力；全部通过后，窗口函数本身仍必须位于白名单。当前 Rust 搜索只发现测试直接调用该方法，未发现生产 Rust 调用边，因此不能把该方法描述为 `PhysicalWindow::ToPB` 内部已执行的门禁；`WindowFuncToPBExpr` 只执行客户端协议能力和编码检查。

## 数据与状态

`WindowFuncDesc` 拥有一个 `baseFuncDesc`，后者拥有函数名 `Name`、参数向量 `Args` 和可选返回类型 `RetTp`。构造成功的不变量是 `RetTp` 已由类型推导填充；`NewWindowFuncDesc` 和 `WindowFuncToPBExpr` 都基于此不变量，后者在缺失时会触发 `expect("window return type must be inferred")`。

默认帧不是全局可变表：`useDefaultFrameWindowFuncs` 每次创建新的 `HashMap<String, FrameClause>`，`UseDefaultFrame` 对命中的帧执行 `cloned`，调用方拿到独立值。`NO_FRAME_WINDOW_FUNCS` 是只读静态切片。PB 转换创建新的 `tipb::Expr` 和 children 向量，不修改输入描述符。

`PushDownContext` 提供求值上下文和可选客户端；`CanPushDownToTiFlash` 调用 `ctx.Client()` 参与 PB 转换能力判断。标量表达式检查通过 `as_any().downcast_ref::<ScalarFunction>()` 识别节点，并递归检查其参数，因此门禁覆盖整棵标量函数子树，而不只检查根节点。

## 依赖与调用关系

- 上游：`pkg/planner/core/operator/physicalop/physical_window.rs::PhysicalWindow::ToPB` 调用 `NewWindowFuncDesc(..., true)` 和 `WindowFuncToPBExpr`，把结果装入 `tipb::Window`。RustCodeGraph 还显示本文件被 `base_func.rs`、`descriptor.rs`、`aggregation_aster_unit_test.rs` 和 `window_func_test.rs` 引用；其中后两者是测试证据。
- 基础描述：`NewWindowFuncDesc -> newBaseFuncDesc -> baseFuncDesc::TypeInfer`，位于 `pkg/expression/aggregation/base_func.rs`。
- 表达式与类型：依赖 `expression::GetUint64FromConstant`、表达式的 `GetType`、`NewPBConverter`、`ExprToPB`、`ToPBFieldType` 和推送黑名单查询。
- 协议与存储能力：依赖 `kv::Client::IsRequestTypeSupported`、`kv::StoreType::TiFlash`、`tipb::Expr` 与 `tipb::ExprType`。
- SQL 常量和帧模型：函数名及 `FrameClause`、`FrameExtent`、`FrameBound` 来自 parser AST；NULL 属性和字段类型来自 parser MySQL 类型。

`Cargo.toml` 证明 `expression`、`kv`、parser AST/MySQL 和 `tipb` 都是该 crate 的直接依赖；其中 `tipb` 固定到 Git revision `07f0ea6b6bffa9d8ac100d81ee51dbbfe4dda3bf` 并启用 `protobuf-codec`。

## 错误处理与边界

- `NewWindowFuncDesc` 用三态返回值区分类型推导错误和窗口参数不合法，调用方必须同时处理 `Result` 与 `Option`。
- `NTH_VALUE`/`NTILE` 允许 NULL 常量，但拒绝非 NULL 的零值和不可提取的表达式；Rust 版本额外用 `get`/`first` 处理缺参并返回 `Ok(None)`。
- `LEAD`/`LAG` 少于两个参数时不在本层检查偏移；参数个数与其他签名规则由基础类型推导或更上层校验负责。
- `skip_check_args=true` 会完全绕过上述窗口专属常量检查，只应用基础类型推导和返回类型 NULL 属性修正。
- 未知函数名在 `NeedFrame` 中被保守地归类为“需要帧”，在 TiFlash 白名单中被拒绝。
- TiFlash 参数检查显式拒绝 Enum、Bit、Set、Geometry、Unspecified 和无效 Decimal。对标量函数，它既检查裸函数名，也检查带 PB signature 的完整名，并递归检查子参数。
- `WindowFuncToPBExpr` 对客户端不支持、任一参数 PB 转换失败返回 `None`；它不会产生部分结果。若成功构造的描述符违反 `RetTp` 必须存在的不变量，则会 panic，而不是返回错误。
- `CanPushDownToTiFlash` 的 PB 转换门禁和 `WindowFuncToPBExpr` 的转换存在职责重叠，但两者不是同一个调用链；调用方不能仅凭函数名白名单推断协议转换必然成功。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或后台资源。所有描述符、帧对象和 PB 对象都由调用栈拥有，依靠 Rust 所有权在离开作用域时释放。`Clone` 产生独立描述符；默认帧查询也返回克隆值，因此本文件本身没有共享可变状态或锁顺序问题。

唯一可观察的外部状态来自表达式下推黑名单和客户端能力。`canExprPushDownToTiFlash` 只读取黑名单；`pkg/expression/aggregation/window_func_test.rs` 使用 `BlacklistGuard::drop` 清理测试写入的全局黑名单，说明测试若修改该状态必须保证恢复。客户端在这些函数中只被借用并执行能力查询/表达式转换，不发送请求；测试客户端的 `Send` 明确 panic，以验证此路径不会发起网络请求。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/aggregation/window_func.go`。Rust 保留了 Go 的主要结构和语义：规划期 `WindowFuncDesc`、窗口专属常量参数检查、无帧函数集合、`ROW_NUMBER` 默认帧、深拷贝、PB 转换和 TiFlash 函数白名单均一一对应。

可见差异如下：

- Rust 构造器返回 `Result<Option<_>, Error>` 来表达 Go 的 `(*WindowFuncDesc, error)` 中“nil 且无 error”的参数拒绝结果。
- Rust 对 `NTH_VALUE`/`NTILE` 使用安全索引；Go 对照代码直接访问 `args[1]`/`args[0]`，因此 Rust 在缺参时返回 `Ok(None)`，不会在本层越界。
- Rust 在读取 `RetTp` 前已用 `?` 传播 `newBaseFuncDesc` 错误；Go 对照代码在检查 `err` 前访问 `base.RetTp`。文档只记录可见控制流差异，不推断上层是否能触发 Go 的异常路径。
- Go 的非 NULL 函数列表还包含 `AggFuncMaxCount` 和 `AggFuncMinCount`，Rust 当前列表没有这两个名称。这是当前源码差异，扩展或对齐时应先确认这些函数是否会通过 Rust 的窗口描述符路径。
- Go 直接调用通用 `expression.CanExprsPushDown(..., TiFlash)`；Rust 在本文件实现了递归门禁，以字段类型、函数黑名单和 PB 编码还原所需检查。`aggregation_aster_unit_test.rs` 的 Enum 用例和 `window_func_test.rs` 的 TiFlash 黑名单用例为这一差异提供回归证据。

同目录未搜索到直接引用这些符号的 Go `*_test.go`；因此 Go 行为依据来自生产对照文件，而 Rust 边界证据来自独立 Rust 测试。

## 扩展指南

- 新增窗口函数时，先在基础类型推导处建立签名与返回类型，再判断它是否应加入 `NO_FRAME_WINDOW_FUNCS`、默认帧表、非 NULL 列表和 TiFlash 白名单；这四类规则含义不同，不应因名称相似而同步盲加。
- 新增带常量位置/桶数参数的函数，应在 `NewWindowFuncDesc` 增加安全索引和 `GetUint64FromConstant` 校验，并在独立的 `window_func_test.rs` 中覆盖缺参、NULL、零、负数/不可转换表达式、`skip_check_args` 两种模式以及类型推导错误。
- 修改返回值 NULL 属性时，应同步核对 Go 的 `NewWindowFuncDesc`、planner 输出 schema 和空窗口帧语义，尤其是 `LEAD`/`LAG` 默认值与取值表达式的组合。
- 扩展 TiFlash 下推时，必须同时验证参数类型、嵌套标量函数的裸名与 signature 黑名单、PB 可编码性、客户端支持的 ExprType，以及窗口函数白名单。只改 `CanPushDownToTiFlash` 不会改变 `WindowFuncToPBExpr` 的生产序列化路径；若需要统一门禁，应先确认 planner 的调用契约。
- 修改 PB 结构时，应更新 `WindowFuncToPBExpr` 及 `PhysicalWindow::ToPB` 的相关独立测试，覆盖客户端不支持、子表达式转换失败和返回字段类型。测试逻辑应继续放在 `window_func_test.rs` 或其他独立测试文件中，不嵌入生产源文件。
- 性能上，`UseDefaultFrame` 当前每次分配一个 `HashMap`；若默认帧种类或调用频率增加，可在保持返回独立帧值和线程安全的前提下评估静态匹配或惰性只读表。递归下推检查也会为每个节点创建 PB converter/编码结果，改变时需避免放宽语义门禁。

## 验证依据

- 目标源码：`pkg/expression/aggregation/window_func.rs`，RustCodeGraph `node --file` 显示 275 行、17 个符号，并列出 5 个 Rust 使用文件。
- 调用链：`pkg/planner/core/operator/physicalop/physical_window.rs:603` 附近的 `PhysicalWindow::ToPB`，明确调用 `NewWindowFuncDesc` 和 `WindowFuncToPBExpr`。
- 基础实现：RustCodeGraph `node newBaseFuncDesc` 显示 Rust 版本调用 `baseFuncDesc::TypeInfer` 并通过 `Result` 返回。
- crate 边界：`pkg/expression/aggregation/Cargo.toml` 与 `pkg/expression/aggregation/lib.rs`。
- Go 对照：`pkg/expression/aggregation/window_func.go`。
- 独立 Rust 测试：`pkg/expression/aggregation/window_func_test.rs::window_pushdown_honors_tiflash_specific_scalar_blacklist`；`pkg/expression/aggregation/aggregation_aster_unit_test.rs::pushdown_and_frame_classification_matches_go_lists` 和 `window_pushdown_rejects_enum_for_tiflash`。
- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标文件在索引中。宽泛 `explore` 的同名跨语言结果经过目标文件、明确调用点和 `rg` 复核后才写入本文。
- 未运行 Cargo：本任务仅新增说明文档，计划明确禁止 Cargo。结构检查和文档差异复核作为交付验证。
