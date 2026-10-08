# `pkg/sessionctx/variable/sequence_state.rs`

## 文件定位

本文件位于 `astersql-sessionctx-variable` crate，由 `pkg/sessionctx/variable/lib.rs` 以 `pub mod sequence_state` 公开。它实现一个会话级的 SEQUENCE 最近值缓存，目标语义是按 sequence ID 记录最近一次 `NEXTVAL` 的结果，供 `LASTVAL` 和会话状态迁移使用。crate 边界由 `pkg/sessionctx/variable/Cargo.toml` 定义；本文件自身只依赖 Rust 标准库的 `HashMap` 与 `Mutex`，不直接使用该 manifest 中的外部依赖，也没有 feature 条件编译。

需要区分模块能力和当前接线状态：全仓 Rust 引用中，生产代码只有 `lib.rs` 的模块声明；直接调用 `NewSequenceState` 及其方法的是独立测试 `pkg/sessionctx/variable/error_1_aster_unit_test.rs::sequence_state_is_concurrent_and_copies_state`。当前 Rust 表达式实现 `pkg/expression/builtin_info.rs` 使用另一份 `SessionInfo.sequence_state: HashMap<i64, i64>`，其 `next_val`/`last_val` 直接读写该 map，尚未接入本文件的 `SequenceState`。

## 核心职责

- `SequenceState` 将 `sequenceID -> last nextval` 映射封装在互斥锁中，使共享引用 `&self` 也能安全读写状态。
- `UpdateState` 更新单个序列的最近值；同一 ID 再次写入会覆盖旧值，不同 ID 相互独立。
- `GetLastValue` 将命中值和“未命中即 SQL NULL”的标记一起返回，并保留与 Go 三返回值形式对应的错误槽位。
- `GetAllStates` 返回深拷贝快照，调用者修改返回 map 不影响内部缓存。
- `SetAllStates` 合并输入快照，而不是清空后替换；这精确对应 Go `maps.Copy` 的行为。

本文件不负责解析序列名、鉴权、推进持久化序列、设置序列值或 SQL NULL 转换；这些是上层表达式与序列服务的职责。

## 主要符号

- `pub struct SequenceState`：唯一状态类型。私有字段 `latestValueMap: Mutex<HashMap<i64, i64>>` 阻止外部绕过同步约束。
- `pub fn NewSequenceState() -> SequenceState`：通过 `Default` 构造空 map。与 Go 构造函数返回指针不同，Rust 返回拥有所有权的值；需要共享时由调用者放入 `Arc`，测试即采用 `Arc<SequenceState>`。
- `pub fn SequenceState::UpdateState(&self, sequenceID: i64, value: i64)`：加锁后调用 `HashMap::insert`，覆盖或新增一项。
- `pub fn SequenceState::GetLastValue(&self, sequenceID: i64) -> (i64, bool, Option<String>)`：命中返回 `(value, false, None)`；未命中返回 `(0, true, None)`。布尔值为 `true` 表示缓存缺失，而不是操作成功。
- `pub fn SequenceState::GetAllStates(&self) -> HashMap<i64, i64>`：持锁克隆整个 map，然后把独立副本交给调用者。
- `pub fn SequenceState::SetAllStates(&self, states: &HashMap<i64, i64>)`：把输入迭代器复制进内部 map；同键覆盖、缺失键保留。

文件没有模块级常量、trait、枚举、泛型、异步函数或条件编译项。

## 执行流程

单值路径为：上层获得 sequence ID 与新值，调用 `UpdateState`，方法取得互斥锁并写入 map；随后 `GetLastValue` 以同一 ID 加锁查询，复制 `i64` 值并立即释放锁。若 ID 从未由本会话写入，读取结果用 `(0, true, None)` 表示 SQL 层应返回 NULL；数值 `0` 只是占位，调用者必须同时检查布尔标记。

批量路径为：`GetAllStates` 在锁内克隆完整 map，通常可作为会话迁移/序列化的独立快照；恢复时 `SetAllStates` 在锁内逐项 `extend`。恢复是合并语义，因此调用前已存在但输入未携带的序列值不会消失。

当前 Rust SQL 路径并未经过上述流程。`pkg/expression/builtin_info.rs::next_val` 在鉴权、取得 sequence ID 并调用序列服务获取新值后，直接写 `SessionInfo.sequence_state`；`last_val` 直接从该 map 读取。若未来统一接线，应在这两个入口以及会话状态编解码边界同时替换，不能只替换其中一个调用点。

## 数据与状态

缓存键和值均为 `i64`：键是稳定的 sequence ID，值是该会话最近一次成功取得的序列值。状态初始为空；不存在全局静态数据，也不与其他会话共享，除非上层显式共享同一个 `SequenceState` 实例。

`Default` 依次默认构造 `Mutex` 和空 `HashMap`。单值返回依赖 `i64: Copy`，批量读取依赖 `HashMap: Clone`。`GetAllStates` 的快照只保证获得锁时的一致视图；返回后其他线程可继续更新内部状态，快照不会自动跟随变化。`SetAllStates` 的输入由共享借用传入，并在持锁期间复制，不保存对输入 map 的引用。

关键不变量是：内部 map 的所有访问都发生在同一个 mutex guard 生命周期内；外部无法直接取得字段；快照不会泄露内部可变引用；合并恢复不删除旧键。测试 `sequence_state_is_concurrent_and_copies_state` 覆盖了这些不变量中的并发写、快照隔离和保留旧键语义。

## 依赖与调用关系

下游依赖仅有 `std::collections::HashMap` 和 `std::sync::Mutex`。方法内部的有效调用关系分别是 `Default::default`、`Mutex::lock`、`HashMap::insert/get/clone/extend`；没有 I/O、网络、存储事务或跨 crate 调用。

Rust 上游现状如下：

- `pkg/sessionctx/variable/lib.rs` 公开模块，但没有再导出具体类型。
- `pkg/sessionctx/variable/error_1_aster_unit_test.rs::sequence_state_is_concurrent_and_copies_state` 是唯一直接 Rust 调用者，使用 `Arc` 让 8 个线程并发调用 `UpdateState`，再验证读取、快照与合并。
- `pkg/expression/builtin_info.rs::{next_val,last_val}` 实现相同业务方向，但操作的是 `SessionInfo` 内独立的裸 `HashMap`，不是本类型；因此不能据 Go 调用链宣称本文件已经位于 Rust SQL 请求主链。

Go 对照链路是完整的：`pkg/sessionctx/variable/session.go::NewSessionVars` 构造 `SequenceState`；`pkg/expression/builtin_info.go` 的 `NEXTVAL` 路径调用 `UpdateState`、`LASTVAL` 路径调用 `GetLastValue`；`SessionVars.EncodeSessionStates`/`DecodeSessionStates` 分别调用 `GetAllStates`/`SetAllStates`。这些 Go 文件是本 Rust API 设计与预期接线位置的直接依据，而不是当前 Rust 调用边。

## 错误处理与边界

正常缓存操作没有可恢复错误：`GetLastValue` 的 `Option<String>` 当前永远是 `None`，对应 Go 返回值中的 `nil error`。未知 ID 不是错误，而是 `(0, true, None)`；上层应把第二项转换为 SQL NULL，不能单独使用第一项。

唯一异常路径是 mutex poisoned：四个访问方法都用 `lock().expect("sequence state mutex poisoned")`，一旦其他持锁线程 panic 导致锁中毒，后续访问会 panic，而不是返回 `Result`。由于锁内逻辑仅包含内存 map 操作，正常业务错误不会毒化锁；但扩展时不应在 guard 存活期间加入可能 panic、阻塞或调用外部组件的逻辑。

边界语义还包括：任意 `i64` ID 和值都会被接受；文件不校验 ID 是否对应真实序列，不判断权限，也不限制缓存大小。`SetAllStates` 对空输入是无操作而非清空。若需要“完全替换”或“清空”能力，应新增语义明确的方法并单独测试，不能悄悄改变现有合并契约。

## 并发与资源生命周期

`Mutex<HashMap<...>>` 让全部公开状态方法可通过 `&self` 调用，`SequenceState` 因其字段类型而可在线程间安全共享；测试通过 `Arc` 明确验证并发调用。每次方法调用只持有一把非递归互斥锁，不存在本文件内部的多锁顺序或死锁环。

`UpdateState` 和 `GetLastValue` 的临界区是单项 map 操作；`GetAllStates` 的临界区覆盖整图克隆，`SetAllStates` 的临界区覆盖全部输入复制，所以大状态量会延长其他线程的等待时间。锁 guard 在方法返回或表达式结束时自动释放；返回值不携带 guard。对象销毁时 mutex 与 map 一并释放，没有后台任务、通道、文件句柄、事务或显式清理步骤。

并发保证只覆盖单个方法的原子性。`GetAllStates` 后再调用 `SetAllStates` 不是跨调用事务，中间可能插入其他线程的更新；新功能若要求 compare-and-swap 或复合原子操作，应在 `SequenceState` 内以一次持锁实现并增加竞态测试。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/sessionctx/variable/sequence_state.go`。字段布局和四个方法的核心语义一致：两者都以单 mutex 保护 `map[int64]int64`/`HashMap<i64, i64>`；`UpdateState` 覆盖单值；`GetLastValue` 用第二返回值表达缺失；`GetAllStates` 返回副本；`SetAllStates` 使用复制合并且不移除输入中缺失的键。

可见差异有三点。第一，Go `NewSequenceState` 返回 `*SequenceState`，Rust 返回值类型，由上层决定是否装入 `Arc`。第二，Go 错误类型是 `error` 且当前返回 `nil`，Rust 用 `Option<String>` 模拟且当前恒为 `None`；它不是通用错误抽象。第三，Go 已把该对象作为 `SessionVars.SequenceState` 接入 `NEXTVAL`、`LASTVAL` 与会话状态编解码，Rust 本文件目前只导出模块并受单测覆盖；Rust 表达式层另用 `SessionInfo.sequence_state`。

Go 的集成行为还由 `pkg/sessionctx/sessionstates/session_states_test.go` 中 `check SequenceState` 用例验证：状态迁移后 `LASTVAL` 仍为 1，继续 `NEXTVAL` 得到 2。对应的 Rust 文件 `pkg/sessionctx/sessionstates/session_states_test.rs` 当前保留了同形测试文本，但本文件没有被该测试引用；不能把它当成本模块已完成会话迁移接线的证据。

## 扩展指南

若只扩展缓存能力，优先在 `SequenceState` 的 impl 内新增小而完整的原子操作，继续保持字段私有，并在独立测试文件中补充成功、缺失、覆盖、空输入和并发交错场景。Rust 测试逻辑应放在同目录独立测试文件（现有最近入口是 `pkg/sessionctx/variable/error_1_aster_unit_test.rs`），不要嵌入生产源文件；如果规模扩大，宜新建同目录专用测试文件并从 `lib.rs` 的 `#[cfg(test)]` 模块区挂载。

若目标是接入生产 SQL 主链，需同步审查 `pkg/expression/builtin_info.rs::SessionInfo`、`next_val`、`last_val` 以及 Rust 会话状态编码/解码实现，保证 `NEXTVAL` 写入、`LASTVAL` 读取和会话迁移使用同一个状态对象。迁移时必须保留 Go 的权限检查顺序、NULL/缺失语义、合并恢复行为和独立会话隔离，并增加端到端 Rust 测试；不能仅删除表达式层 `HashMap` 或把本类型接到单一入口。

兼容风险主要是改变 `GetLastValue` 三元组、把 `SetAllStates` 从合并改为替换、或使 mutex poisoning 从 panic 改成静默恢复。性能风险集中在锁竞争和全量 clone/extend；优化可考虑缩短临界区或批量 API，但应先用实际会话状态规模验证，且不能牺牲快照一致性。若引入可恢复错误，应使用 crate 的统一错误类型并同步 Go 对照调用契约，而不是继续扩张 `Option<String>`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标仓库已初始化。
- RustCodeGraph `node --file pkg/sessionctx/variable/sequence_state.rs --offset 1 --limit 240`：读取目标文件全部 82 行，并确认符号、字段、签名和锁内实现。
- RustCodeGraph `query SequenceState/NewSequenceState/GetLastValue` 与 callers/callees 查询：确认 Rust/Go 同名定义；调用边对 Go 风格大写方法未能可靠消歧，因此用全仓精确引用搜索补齐。
- RustCodeGraph `node`：核对 `pkg/expression/builtin_info.rs::SessionInfo`、`next_val`、`last_val` 的独立 Rust 状态路径，以及 `pkg/sessionctx/variable/error_1_aster_unit_test.rs::sequence_state_is_concurrent_and_copies_state` 的测试行为。
- 直接读取 `pkg/sessionctx/variable/Cargo.toml` 与 `lib.rs`：确认 crate 名称、无 feature 声明、模块公开方式和独立测试挂载位置。
- 直接读取 `pkg/sessionctx/variable/sequence_state.go`、`session.go`、`pkg/expression/builtin_info.go`：确认 Go 数据结构、SQL 调用链和会话状态编解码接线。
- 读取 `pkg/sessionctx/sessionstates/session_states_test.go` 与 `.rs` 的 `check SequenceState` 段落：确认 Go 会话迁移期望，并识别 Rust 测试文本不构成本模块调用证据。
- `rg` 全仓 Rust 引用：除 `lib.rs` 和 `error_1_aster_unit_test.rs` 外未发现本模块调用者；因此文档将生产接线状态明确标为尚未接入，而未推断为已支持。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务指定命令验证目标文档存在且固定二级章节恰为 11 个。
