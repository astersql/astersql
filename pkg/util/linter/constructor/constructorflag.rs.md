# `pkg/util/linter/constructor/constructorflag.rs`

## 文件定位

自身不保存构造函数名称，也不检查任何构造行为；Go 版本中的允许函数列表来自宿主结构体字段上的 ``ctor:"..."`` tag，由 analyzer 读取。

## 核心职责

`Constructor` 提供一个公开、无字段的名义类型，用来保留 Go `constructor.Constructor` 的“此字段是构造器白名单元数据载体”概念。它自身不保存构造函数名称，也不检查任何构造行为；Go 版本中的允许函数列表来自宿主结构体字段上的 ``ctor:"..."`` tag，由 analyzer 读取。

Rust 实现额外派生 `Clone`、`Copy`、`Default`、`Eq` 和 `PartialEq`，把它固定为可复制、可默认创建、可比较的零状态值。直接测试 `migration_aster_unit_test.rs` 验证其大小为零、复制后相等，并验证 `Constructor {}` 与 `Constructor::default()` 相等。

## 主要符号

- `pub struct Constructor {}`：本文件唯一的业务符号和公开 API。空字段使它成为零大小类型；`pub` 令其可经 `constructorflag` 模块及 crate 根再导出访问。
- `#[derive(Clone, Copy, Default, Eq, PartialEq)]`：全部行为由编译器生成。本文件没有手写函数、方法、trait、常量、静态变量、条件编译项或私有实现。
- `Default` 提供统一的空标记构造方式；`Clone`/`Copy` 允许无资源转移地按值复制；`Eq`/`PartialEq` 表明所有该类型的值在状态上等价。由于没有字段，这些 trait 不引入额外状态或业务分支。

## 执行流程

本文件没有主动执行入口，运行流程只有类型使用时的被动路径：

1. crate 根 `lib.rs` 编译 `constructorflag` 模块并再导出 `Constructor`。
2. 使用者若写 `Constructor {}` 或 `Constructor::default()`，只得到一个无字段标记值；复制和相等比较也只调用派生实现。
3. 当前 Rust 生产代码没有进一步消费该值，因此不会触发诊断、注册、I/O 或数据库行为。

作为语义对照，Go analyzer 的流程是另一条工具链：`run` 遍历 Go AST，`handleCompositeLit`、`handleCallExpr` 和 `handleValueSpec` 调用 `getConstructorList`；后者按类型名 `Constructor` 与固定包路径识别标记字段、解析 `ctor` tag，再由 `assertInConstructor` 校验最近外层函数是否在白名单。这个流程解释了标记类型为何存在，但不属于本 Rust 文件已实现的执行逻辑。

## 数据与状态

`Constructor` 没有字段、堆分配或全局状态。`std::mem::size_of::<Constructor>() == 0` 由 `migration_aster_unit_test.rs::constructor_marker_matches_go_empty_struct_semantics` 覆盖；所有实例在可观察状态上相同，因此字面量、默认值和复制值相等。

Go 版本把真正的配置数据放在宿主字段 tag 中，例如 `pkg/sessionctx/stmtctx/stmtctx.go` 的 `ctor:"NewStmtCtxWithTimeZone,Reset"`，以及 `pkg/executor/internal/exec/executor.go` 中针对不同 executor 类型的构造器名单。Rust 类型没有字段 tag 等价物，本文件注释也明确指出这一差距；不能把构造器名单附着到当前 Rust 值上。

## 依赖与调用关系

本文件不导入任何 Rust 模块，也不调用任何函数；其直接下游仅是编译器生成的标准 trait 实现。模块与 crate 关系为 `lib.rs -> constructorflag -> Constructor`，之后由工作区 facade 在 `pkg/lib.rs` 中再导出。

RustCodeGraph 查询 `Constructor` 找到本文件及 Go 对照文件；对该符号的 callers 查询没有返回调用边，callees 报告 `No callees found`。仓库搜索也未发现 Rust 生产调用者。

Go 侧的直接消费者是 `build/linter/constructor/analyzer.go::getConstructorList`：它要求标记的类型名与 `ConstructorUtilPath` 同时匹配。RustCodeGraph 还显示 Go `constructorflag.go` 被 `pkg/sessionctx/stmtctx/stmtctx.go` 和 `pkg/executor/internal/exec/executor.go` 使用；这些是现行约束的真实宿主，而非 Rust 类型的调用者。

## 错误处理与边界

本文件没有返回值为 `Result`/`Option` 的操作，没有 panic、错误转换或诊断输出。空结构体的创建、复制、默认化和相等比较本身均无失败分支。

关键边界在静态检查协议之外：仅出现 `Constructor` 类型并不足以表达允许的构造函数，Go analyzer 还要求精确包路径和合法 `ctor` tag；无效 tag 或缺失 `ctor` 键会被 Go `getConstructorList` 跳过。当前 Rust 类型不携带 tag，且 Rust analyzer 尚未真正接线，因此 Rust 代码不会因在白名单外构造某个宿主结构体而由本文件报错。泛型适配在 Go analyzer 中仍有 TODO，也不能由本标记类型保证。

## 并发与资源生命周期

`Constructor` 不拥有引用、锁、原子变量、线程、任务、通道、事务、文件句柄或网络连接；它没有 `Drop` 实现。复制不共享可变状态，销毁也无需清理资源，所以本文件没有并发顺序或资源生命周期约束。

`Copy` 只说明标记值可按位复制，不等于被标记的宿主类型可复制，也不提供线程安全策略。是否能跨线程使用宿主对象、何时创建或释放宿主资源，均由宿主类型负责，与这个零大小标记无关。

## 与 Go 版本的对应关系

Rust 的 `pub struct Constructor {}` 保留了空结构体和零运行时状态；派生 trait 是为 Rust 值语义提供的便利，不是 Go 源文件中的额外业务状态。

两者目前并非完整功能等价：Go 能在字段 tag 中声明白名单，且 `build/linter/constructor/analyzer.go` 已通过 Go 类型信息和 AST 对复合字面量、`new(T)`、非指针 `var T` 及隐式嵌入零值构造执行检查；Rust 没有直接字段 tag，也没有对应 analyzer。Go 测试 `build/linter/constructor/analyzer_test.go` 用 `analysistest.Run` 验证诊断 fixture，并用反射验证标记包路径；Rust 的独立测试只验证空标记的值/布局语义。

## 扩展指南

若只扩展标记值的 Rust trait，应优先修改 `Constructor` 的 derive 列表，并在独立文件 `pkg/util/linter/constructor/migration_aster_unit_test.rs` 增补对应编译期或运行期断言；不要把测试内嵌回生产源文件。增加字段会破坏零大小和“所有值等价”的既有契约，必须同步评估 `size_of`、`Copy`、默认值以及 Go 空结构体兼容性。

若目标是让 Rust 真正执行构造器白名单约束，不能仅修改本文件：需要先为 Go `ctor` tag 设计明确的 Rust 元数据表示，再选择 Rust 原生工具并接入实际构建链。当前仓库明确不提供这套 Rust analyzer。
