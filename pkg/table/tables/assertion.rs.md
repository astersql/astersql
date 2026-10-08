# `pkg/table/tables/assertion.rs`

## 文件定位

该文件属于 `astersql-table-tables` crate，由 `pkg/table/tables/lib.rs` 以公开模块 `pub mod assertion` 装配；crate 的 Go 映射在 `pkg/table/tables/Cargo.toml` 中声明为 `pkg/table/tables`。它提供事务内存缓冲区键断言的最小领域模型和“首次有效断言优先”的写入算法，用来承接 Go `pkg/table/tables/assertion.go` 的语义。

当前 Rust 生产接线仍有限：RustCodeGraph 将本文件标为仅被 `pkg/table/tables/assertion_test.rs` 和 `pkg/table/tables/export_test.rs` 使用，仓库搜索也未发现 `pkg/table/tables/tables.rs` 或 `pkg/table/tables/index.rs` 调用 `set_assertion`。因此它现在是一个公开、可测试的兼容抽象，而不是已进入 Rust 表 DML 主链的真实事务适配层。Go 版本则已经由 `tables.go` 和 `index.go` 的行/索引变更路径调用。

## 核心职责

- `AssertionOp` 表示键应存在、应不存在、状态未知或不附加断言四种意图，对应 Go 的 `kv.AssertionOp` 概念。
- `KeyFlags` 只暴露当前算法需要观察的断言状态，以 `Option<AssertionOp>` 区分“尚无有效断言”和“已经确定断言”。
- `AssertionBuffer` 将读取标志、识别未找到错误和更新断言三项能力从具体事务内存缓冲实现中抽离。
- `set_assertion` 实现优先级不变量：已有有效断言不可覆盖；没有有效断言或键标志不存在时才更新；其它读取错误原样返回。

本文件不负责键编码、事务提交、TiKV prewrite、锁获取，也不直接操作真实 `kv.Transaction`。这些能力在 Rust 端尚未通过 `AssertionBuffer` 适配到本模块。

## 主要符号

- `pub enum AssertionOp { None, Exist, NotExist, Unknown }`：可复制、可比较的断言枚举。`None` 在这里仍是一个操作值；是否把它保存成“无有效断言”由缓冲实现决定，测试替身在 `update_assertion_flags` 中将其转换为 `Option::None`。
- `pub struct KeyFlags { pub assertion: Option<AssertionOp> }`：算法读取的最小标志视图。它并不等价于真实 KV 层包含多种位标志的完整 `KeyFlags`。
- `pub trait AssertionBuffer`：定义关联错误类型 `Error`，以及 `get_flags(&self, key)`、静态分类器 `is_not_found(error)`、`update_assertion_flags(&mut self, key, assertion)`。trait 没有规定存储介质或同步策略。
- `pub fn set_assertion<B: AssertionBuffer>(...) -> Result<(), B::Error>`：文件唯一的执行入口。它公开导出、泛型化且不包装错误。

文件没有模块级常量、条件编译项、异步函数或其它 `impl`。测试模块是否编译由 `lib.rs` 中的 `#[cfg(test)]` 控制，而不是本文件自身控制。

## 执行流程

`set_assertion` 的顺序如下（依据 `assertion.rs:59-73`）：

1. 使用借用的字节键调用 `buffer.get_flags(key)`，不复制或转换键。
2. 若读取成功且 `flags.assertion.is_some()`，立即返回 `Ok(())`；传入的新断言不会覆盖首次有效断言。
3. 若读取成功但断言为 `None`，继续进入更新路径。
4. 若读取失败且 `B::is_not_found(&error)` 为真，把它视为可初始化状态并继续。
5. 若是其它读取错误，立即原样返回 `Err(error)`，且不调用更新方法。
6. 调用 `update_assertion_flags(key, assertion)` 后返回 `Ok(())`。

算法只做一次读取、至多一次更新。它不重试，也不在读取与更新之间重新校验状态；原子性要求必须由调用环境或具体缓冲实现保证。

## 数据与状态

`AssertionOp` 的四个值描述调用意图，其中 `Exist`、`NotExist`、`Unknown` 都被视为有效的既有断言；一旦其中任意值出现在 `KeyFlags.assertion` 中，后续调用均短路。`AssertionOp::None` 是否表现为 `KeyFlags { assertion: None }` 不是类型系统自动保证的，而是 `AssertionBuffer::update_assertion_flags` 实现者需要维持的映射约定；`assertion_test.rs` 的 `MockBuffer` 明确执行了这一转换。

`KeyFlags` 只保存一个 `Option`，不会表达 Go/TiKV 键标志中的 `SetPresumeKeyNotExists`、`SetNeedLocked` 等其它位。Go 测试 `assertion_test.go` 的 `k6` 场景证明真实 mem buffer 更新断言时必须保留这些无关标志；当前 Rust 单元测试只覆盖断言字段，尚未验证与完整位集的共存。

键以 `&[u8]` 传入，生命周期仅限本次调用；是否复制键、怎样索引以及标志存放多久完全由缓冲实现决定。本模块自身没有全局或线程局部状态。

## 依赖与调用关系

下游依赖全部通过标准库类型和 `AssertionBuffer` trait 表达，本文件没有导入 crate 内外的具体事务类型。调用边为 `set_assertion -> AssertionBuffer::get_flags / AssertionBuffer::is_not_found / AssertionBuffer::update_assertion_flags`；RustCodeGraph 的 `callees` 未能输出动态 trait 分派边，但源码中的三处调用可以直接复核。

Rust 上游当前只有：

- `pkg/table/tables/assertion_test.rs`：四个测试覆盖断言不可覆盖、`None` 可替换、已有空标志初始化和非 not-found 错误传播。
- `pkg/table/tables/export_test.rs`：验证公开入口能被外部风格测试调用并写回 `Unknown`。

Go 的实际主链更完整：`pkg/table/tables/tables.go` 在 `UpdateRecord`、`AddRecord` 和 `removeRowData` 周边设置记录键断言；`pkg/table/tables/index.go` 在索引创建、覆盖和删除路径设置索引键断言。断言值取决于 lazy duplicate check、悲观事务、`skipAssert`、索引状态及 failpoint。上述 Go 调用关系只能用于解释预期迁移位置，不能证明 Rust 生产路径已经接线。

## 错误处理与边界

`get_flags` 返回的错误只有在 `is_not_found` 判定为真时才被吞掉并转为初始化；其它错误保持原类型和原值向上传播。由于 `update_assertion_flags` 无返回值，写入失败无法由此接口表达；真实适配器若存在可失败写入，需先调整 trait/函数签名，不能静默丢弃错误。

边界行为包括：空字节键没有被拒绝；`AssertionOp::None` 仍会调用更新（除非已有有效断言）；已有任何有效断言时，即使新旧断言相同也不会重复写；错误路径禁止更新。函数不判断断言与数据库事实是否一致，那是事务 prewrite/存储层的职责。

需要特别防止把 `is_not_found` 实现得过宽，否则真实存储故障会被误当成缺失并继续写入。`non_not_found_lookup_error_is_propagated_without_update` 用更新计数守住了这一不变量。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或资源句柄，也没有 `unsafe`。`set_assertion` 接受 `&mut B`，在一次调用期间通过 Rust 独占借用阻止同一个缓冲对象被安全代码同时修改；它本身不提供跨线程同步或跨调用事务隔离。

读取后更新是两个独立 trait 操作，若具体实现通过内部共享状态绕开独占借用，仍需自行保证“首次有效断言优先”的原子性。函数不负责 commit/rollback，写入标志的生命周期跟随具体 mem buffer。Go 测试在结束时回滚事务并关闭 mock store；Rust 测试仅使用栈上 `HashMap`/`Option`，因此尚未覆盖真实事务资源清理。

## 与 Go 版本的对应关系

Rust `set_assertion` 逐分支对应 Go `setAssertion`：Go 先从 `txn.GetMemBuffer()` 读取 `GetFlags`，忽略 `kv.IsErrNotFound`，遇到其它错误返回；若 `HasAssertionFlags()` 为真则短路；否则调用 `UpdateAssertionFlags`。Rust 将事务和 mem buffer 折叠为 `AssertionBuffer` 泛型参数，并把 Go 位集判断简化为 `Option::is_some()`。

已对齐的可观察语义包括首次 `AssertExist`/`AssertNotExist`/`AssertUnknown` 不可被后续值覆盖、`AssertNone` 后可设置有效断言、已有键但无断言时可初始化，以及非 not-found 错误禁止写入。这些分别由 Rust `assertion_test.rs` 与 Go `assertion_test.go` 支持。

尚未对齐或未验证之处：Rust 枚举是独立类型，未直接复用 `pkg/kv` 的 `AssertionOp`；`KeyFlags` 不是完整位集；没有真实 `kv.Transaction` 适配器；Rust 生产 `tables.rs`/`index.rs` 未调用本入口；Go 测试对其它 flag 保留、真实 mock store、锁键以及事务 rollback 的覆盖没有等价地出现在本文件的 Rust 测试中。因此当前迁移状态应描述为“核心决策逻辑已移植并由替身测试覆盖，生产集成未完成”。

## 扩展指南

若把该逻辑接入 Rust 表 DML，优先在真实事务/mem-buffer 边界实现 `AssertionBuffer`，并明确 `AssertionOp` 与仓库 KV 类型、完整 `KeyFlags` 位集之间的无损转换；随后在 Rust `tables.rs` 的行新增/更新/删除路径及 `index.rs` 的索引新增/删除路径接入，逐一核对 Go 中 lazy check、悲观事务、`skipAssert`、DDL 临时索引和 failpoint 分支。不要仅调用本函数而遗漏 Go 调用点的条件选择。

修改决策逻辑时应同步独立测试文件 `pkg/table/tables/assertion_test.rs`，不要把测试内嵌进生产源文件；若改变公开可见性或外部调用方式，还应同步 `pkg/table/tables/export_test.rs`。生产适配应补充等价于 Go `assertion_test.go` 中 `k5`/`k6`/`k7` 的集成测试，尤其验证断言更新不清除其它标志、锁键不凭空获得断言以及事务清理。

兼容风险主要是断言枚举映射错误和 not-found 分类错误；正确性风险是覆盖首次断言或丢失其它 key flags；性能上当前算法是一次查找加至多一次更新，扩展时应避免额外 snapshot 读取或键复制。若让更新可失败，应显式传播错误并增加“更新失败不伪装成功”的测试。

## 验证依据

- Rust 源码：`pkg/table/tables/assertion.rs`，核对 `AssertionOp`、`KeyFlags`、`AssertionBuffer` 和 `set_assertion` 的完整定义。
- crate 边界：`pkg/table/tables/Cargo.toml` 与 `pkg/table/tables/lib.rs`，核对 crate 名称、Go 包映射、公开模块和独立测试模块装配；断言模块不受 `expression-runtime` feature 控制。
- Rust 测试：`pkg/table/tables/assertion_test.rs`、`pkg/table/tables/export_test.rs`，核对四类分支、不覆盖不变量、错误路径零更新及公开入口。
- Go 对照：`pkg/table/tables/assertion.go`、`pkg/table/tables/assertion_test.go`，核对真实事务 mem buffer 算法、其它标志保留和事务生命周期。
- Go 调用点：`pkg/table/tables/tables.go` 的记录新增/更新/删除断言，以及 `pkg/table/tables/index.go` 的索引新增/删除断言。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/table/tables` 找到目标及对照文件；`node --file pkg/table/tables/assertion.rs` 显示完整 73 行源码并报告仅两个测试使用者；`explore` 识别 `set_assertion` 的五个测试调用和三个 trait 方法下游；带文件限定的 `callers/callees` 未返回额外生产边。仓库 `rg` 复核也未发现目标 crate 的生产调用。
- 未运行 Cargo：任务是纯文档分析，计划明确禁止 Cargo。结构验证在文档落盘后单独执行。
