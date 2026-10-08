# `pkg/util/injectfailpoint/random_retry.rs`

## 文件定位

本文件是 `astersql-util-injectfailpoint` crate 的随机故障注入实现。crate 入口 `pkg/util/injectfailpoint/lib.rs` 公开 `random_retry` 模块并再导出其全部公开项；`pkg/util/injectfailpoint/Cargo.toml` 声明该 crate 直接依赖 `fail`、`rand` 和 `backtrace`，并以 Go 包 `pkg/util/injectfailpoint` 为移植来源。

它位于正常业务调用与测试故障之间：failpoint 未开启时，公开包装函数保持原成功值或原错误；名为 `DXFRandomError` 的 failpoint 开启后，函数才按概率制造错误或截短读取结果。当前 Rust 生产代码中，经确认的真实接线是 `pkg/dxf/importinto/jobhistory/lib.rs::injectfailpoint::DXFRandomErrorWithOnePercent` 对本文件同名函数的适配，随后由 `pkg/dxf/importinto/jobhistory/history.rs::GetFromHistory` 调用。`pkg/dxf/framework/storage/lib.rs` 中也有同名函数，但它们是该 crate 内“默认恒成功”的本地占位，不是本文件的调用者。

## 核心职责

- 用 `DXFRandomError` 这一统一 failpoint 名称控制 DXF 随机故障是否生效；开关决定是否进入随机判定，概率参数决定进入后是否真正返回错误。
- 为普通操作提供 1% 和 0.1% 两档随机错误入口，并为“已有错误优先”的调用方式提供包装器。
- 为读取操作模拟两种 `UnexpectedEof`：约 20% 的命中分支返回零字节，其余命中分支返回 `[0, n)` 范围内的短读长度。
- 把随机数源下沉到 `random_error_with_rng` 和 `random_read_error_with_rng`，使独立测试可使用确定性 RNG 验证边界，而公开 API 使用线程本地随机数。
- 通过 `getFunctionName` 在普通注入错误消息中附带调用方符号，方便定位是哪条业务路径触发了随机故障。

## 主要符号

- `pub type Error = Box<dyn StdError + Send + Sync + 'static>`：统一的动态错误类型。`Send + Sync + 'static` 允许错误跨线程边界传递，也不借用调用栈中的临时数据。
- `pub fn getFunctionName() -> String`：以 `#[inline(never)]` 保留独立栈帧，遍历并解析 backtrace；看到自身帧后，选择第一个名称不以 `backtrace::` 开头的后续帧，解析失败时返回 `"unknown"`。
- `fn injected_error() -> Error`：创建 `io::ErrorKind::Other`，消息为 `injected random error, caller: <name>`；仅供两个概率入口和包装器内部使用。
- `pub fn DXFRandomErrorWithOnePercent() -> Result<(), Error>`：仅在 `DXFRandomError` 开启时调用 `RandomError(0.01, ...)`；命中返回 `Err`，否则返回 `Ok(())`。
- `pub fn DXFRandomErrorWithOnePercentWrapper(err: Option<Error>) -> Option<Error>`：若参数已有错误则立即原样返回，完全不评估 failpoint；仅在 `None` 路径尝试 1% 注入。
- `pub fn DXFRandomErrorWithOnePerThousand() -> Result<(), Error>`：与百分之一入口相同，但概率为 `0.001`。
- `pub fn RandomErrorForReadWithOnePerPercent(n: i32, err: Option<Error>) -> (i32, Option<Error>)`：先检查 failpoint 是否开启；关闭时原样返回 `(n, err)`，开启时把所有权交给可注入 RNG 的读错误实现。
- `pub fn RandomError(probability: f64, err: Error) -> Option<Error>`：使用 `rand::thread_rng()` 的通用概率选择入口。
- `pub(crate) fn random_error_with_rng<R: Rng + ?Sized>(...)`：以严格的 `sample < probability` 判定是否返回传入错误。
- `pub(crate) fn random_read_error_with_rng<R: Rng + ?Sized>(...)`：实现读路径的早退、1% 判定、零长度短读与部分短读；crate 可见是为了同 crate 的独立测试直接注入 `StepRng`。

## 执行流程

普通 DXF 注入入口的流程如下：调用方进入 `DXFRandomErrorWithOnePercent` 或 `DXFRandomErrorWithOnePerThousand`；`fail::fail_point!` 查询 `DXFRandomError`；若未配置则宏分支不执行并返回 `Ok(())`；若已配置，则先用 `getFunctionName` 构造携带调用位置的错误，再由 `RandomError` 采样；命中时通过 failpoint 分支返回 `Err`，未命中时仍成功。

包装器先处理业务已有错误。`DXFRandomErrorWithOnePercentWrapper(Some(err))` 立即返回同一个错误，不读取 failpoint 状态，也不消耗随机样本；只有 `None` 才进入 1% 注入。这保证测试故障不会覆盖真实错误及其诊断信息。

读取入口先用 `fail::eval("DXFRandomError", |_| ())` 判断开关。关闭时直接保留 `(n, err)`；开启时，`random_read_error_with_rng` 依次检查 `n == 0`、已有错误和首个随机样本是否大于 `0.01`，任何一项成立都原样返回。真正命中后再取一个样本：小于 `0.2` 返回 `(0, UnexpectedEof)`，否则通过 `gen_range(0..n)` 返回部分长度及 `UnexpectedEof`。

## 数据与状态

文件没有模块级可变状态。failpoint 配置由 `fail` crate 管理，随机状态来自每次公开调用取得的 `rand::thread_rng()`；两个内部泛型函数只临时借用调用者给出的 `Rng`。

`Error` 和 `Option<Error>` 均按所有权移动。包装器的提前返回保证已有错误不会被替换；读取入口先评估 failpoint，再决定是否移动 `err` 到内部函数，从而使禁用分支能无损返回原值。

概率比较有意使用严格小于。根据 `pkg/util/injectfailpoint/migration_aster_unit_test.rs`：概率 `0.0` 不命中，`1.0` 对 `[0,1)` 样本必命中，`NaN` 不命中；样本等于阈值时也不注入。读路径的长度不变量是：正常或早退时保持 `n`；注入时结果位于 `0..n`，错误种类为 `io::ErrorKind::UnexpectedEof`。

## 依赖与调用关系

下游依赖如下：`fail::fail_point!` 和 `fail::eval` 提供开关与提前返回语义；`rand::Rng`、`thread_rng` 和 `gen_range` 提供概率与短读长度采样；`backtrace::trace/resolve_frame` 获取诊断用调用方名称；`std::io::Error` 承载普通注入错误与 `UnexpectedEof`。

内部调用边为：三个普通公开包装函数调用 `injected_error` 和/或 `RandomError`；`injected_error` 调用 `getFunctionName`；`RandomError` 调用 `random_error_with_rng`；`RandomErrorForReadWithOnePerPercent` 调用 `random_read_error_with_rng`。这些边可直接由 `pkg/util/injectfailpoint/random_retry.rs` 的函数体复核。

RustCodeGraph 将本文件索引为 10 个符号，并报告文件级被 `pkg/objstore/s3like/retry.rs`、`pkg/planner/cardinality/row_count_index.rs`、本 crate 测试和一份 RealTiKV 测试使用；但精确 `callers/callees` 对路径限定符号产生了大量同名误匹配。进一步用精确文本与 Cargo 依赖核验后，前三个生产文件中前两者没有调用本文件 API。可确认的 Rust 生产链是 `jobhistory/history.rs::GetFromHistory -> jobhistory/lib.rs` 的适配函数 `-> random_retry.rs::DXFRandomErrorWithOnePercent`。`pkg/ingestor/ingestctrl/Cargo.toml` 与 `pkg/dxf/framework/taskexecutor/Cargo.toml` 虽声明了依赖，但当前 Rust 源中未找到对本 API 的引用，因此不能据此宣称已接线。

Go 版本的生产调用面更广：DXF storage/history 调用两个返回 `error` 的概率入口，ingestor 调用保留已有错误的 wrapper，objstore 的 S3/KS3 读取路径调用短读注入。它们是迁移意图与未来 Rust 接线依据，不应误写成当前 Rust 调用关系。

## 错误处理与边界

- failpoint 关闭是正常生产路径：所有无参数入口返回 `Ok(())`，wrapper 返回原参数，读入口返回原 `(n, err)`。
- `getFunctionName` 无法解析栈符号时不会再产生错误，而是把调用方写为 `unknown`。普通注入错误使用 `io::ErrorKind::Other`，调用方通常按动态错误处理。
- wrapper 对 `Some(err)` 的优先级高于 failpoint，测试 `wrapper_preserves_an_existing_error_before_failpoint_evaluation` 甚至把 failpoint 配为 panic，以证明该分支没有被评估。
- 读路径对 `n == 0` 或已有错误不再制造新错误；启用的 failpoint 回调仍会先被评估，这一点由 `read_passthrough_still_evaluates_the_enabled_failpoint` 固定。
- `random_read_error_with_rng` 假设需要执行 `gen_range(0..n)` 时 `n > 0`。公开逻辑会排除 `n == 0`，但负数 `n` 不属于有效读取结果，当前没有显式校验；调用者不得传入负长度，否则范围采样可能 panic。
- 概率参数没有裁剪：负数和 `NaN` 永不命中，大于 `1.0` 对合法样本总会命中。这与通用辅助函数的直接比较语义一致，但新增调用者应传入 `[0,1]` 内的明确概率。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或持久资源。`rand::thread_rng()` 的状态局限于当前线程与单次借用；动态错误满足 `Send + Sync`，可以交给上层并发任务。backtrace 遍历也只在构造实际候选注入错误时发生，不保留栈帧引用。

全局 failpoint 配置可能被多个测试共享，因此独立测试文件以 `FAILPOINT_TEST_LOCK: Mutex<()>` 串行化相关用例，并用 `fail::FailScenario::setup()` 管理配置生命周期；作用域结束时场景对象负责清理。`FAILPOINT_CALLBACKS: AtomicUsize` 仅用于验证回调评估次数。生产函数自身不负责启停 failpoint，也不做全局同步。

## 与 Go 版本的对应关系

`pkg/util/injectfailpoint/random_retry.go` 是逐项对照源：Rust 保留相同公开名称、`DXFRandomError` failpoint 名、`0.01/0.001` 概率、严格小于比较、wrapper 的已有错误优先级，以及读注入的 1% 总概率、20% 零长度分支和 `[0,n)` 部分读取范围。

类型映射为 Go `error` 对 Rust `Box<dyn Error + Send + Sync>`，Go `nil` 对 Rust `Option::None` 或 `Result::Ok(())`，`io.ErrUnexpectedEOF` 对 `io::ErrorKind::UnexpectedEof`。Go 使用进程级 `math/rand`，Rust 公开入口使用线程本地 RNG，并额外拆出可注入 RNG 的 crate 内函数以实现确定性测试。

调用方名称获取方式存在实现差异：Go 固定使用 `runtime.Caller(2)`，Rust 因 backtrace 帧布局不同而扫描到自身后第一个非 `backtrace::` 帧，并在解析失败时返回 `unknown`。错误文本保持 `injected random error, caller: ...` 格式，但 Rust 不承诺符号字符串与 Go 完全相同。

迁移尚未覆盖 Go 的全部接线面。当前 Rust jobhistory 已使用真实实现；DXF framework storage 仍使用本地恒成功占位，Rust ingestor/taskexecutor 仅在 Cargo 中声明依赖，objstore 读路径也未找到本函数调用。扩展时应按具体 Go 调用点逐处接线和验证，而不是因为依赖存在便认为行为已经启用。

## 扩展指南

新增概率档位时，应复用 `RandomError`，保留“先由 failpoint 决定是否启用、再做概率采样”的两级语义，并在 `pkg/util/injectfailpoint/migration_aster_unit_test.rs` 增加确定性 RNG 边界测试。不要把测试写回生产源文件。

修改读错误策略时，重点维护四个契约：已有错误不被覆盖、零长度不构造错误、部分长度严格小于 `n`、错误种类仍能被上层识别为 `UnexpectedEof`。若要接受负数或改用 `usize`，需同时审计 Go 对照和未来 objstore 接线，避免改变合法范围或引入 panic。

扩展 Rust 生产接线前，应先确认调用处使用的是本 crate，而非局部同名 `injectfailpoint` 模块。DXF storage 的同名占位尤其容易造成误判；替换它会改变大量存储操作的测试行为，必须配套独立回归。jobhistory 的适配层还会把动态错误转换为该 crate 的 `Error::new(error.to_string())`，新增结构化错误信息可能在这里丢失类型，仅保留文本。

若改变 failpoint 名称、错误文本或调用方解析，应同步检查依赖这些字符串的 failpoint 配置、日志/指标与测试。若改变全局 failpoint 的测试方式，继续使用串行锁和作用域清理，避免并行测试互相污染。

## 验证依据

- 源实现：`pkg/util/injectfailpoint/random_retry.rs`，RustCodeGraph `node --file` 确认全文件 144 行及 10 个符号；内部调用与所有权、概率、错误分支均由此核对。
- crate 边界：`pkg/util/injectfailpoint/lib.rs` 的模块公开与测试装配；`pkg/util/injectfailpoint/Cargo.toml` 的 `backtrace`、`fail`、`rand` 依赖和 Go 包映射。
- Go 对照：`pkg/util/injectfailpoint/random_retry.go`；另以 `pkg/objstore/s3like/io.go`、`pkg/ingestor/ingestctrl/job_worker.go` 和 DXF storage Go 文件的调用点确认原始应用场景。
- Rust 生产调用：`pkg/dxf/importinto/jobhistory/lib.rs` 的错误适配与 `pkg/dxf/importinto/jobhistory/history.rs::GetFromHistory`；Cargo 精确搜索确认该 crate 还被 ingestor/taskexecutor 声明，但对应 Rust API 引用未出现。
- 独立测试：`pkg/util/injectfailpoint/migration_aster_unit_test.rs`，覆盖概率 0/1/NaN、严格小于、已有错误优先、禁用透传、启用回调、零长度/已有读错误、完整及部分 `UnexpectedEof`。
- RustCodeGraph 查询：`status` 显示索引包含 11,467 文件、307,296 节点；`files --filter pkg/util/injectfailpoint` 找到 crate 的 4 个索引文件；`query` 找到本文件公开/内部符号；`node` 核对目标、Go 对照和测试。精确调用图对路径限定符号出现同名污染，故调用边结论由 RustCodeGraph 文件级线索再结合 `rg` 和源码/Cargo 复核，不采用其误匹配结果。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构检查，并人工复核唯一产物、真实符号、当前接线与迁移缺口。
