# 任务 299: 提交 b82bed1eca 严格对齐 Rust

执行批次：【批次 93】；文件顺序前驱：127、143、204、215、216、226、285、289、296、297；任务编号：299。同批可并行，实际能力依赖另行核查。

状态：未开始

目的：逐项核对本Go提交在当前Rust的实际覆盖，只修缺失和偏离，保留完整Go逻辑与测试意图。

来源任务：Go提交 `b82bed1eca2fcbe58f0fc074fff53e1fc49daa8c`；父提交 `1cab931bd85a44e8a1563f8261f49572439e425a`；原始提交标题：executor, session: add tidb_dml_max_execution_time for transactional DML (#70568)。

预计会话范围：本提交较大，已提取增删行794、Go路径19个。保留一个提交一个任务；首次只列函数分段清单，每会话处理一个相关行为组，逐段记录证据并续接同一文件，全部段落完成前不标完成。


## 独立任务验收规则（2026-10-02 用户更新）


本节覆盖旧总计划、执行技能及本文件历史记录中与独立验收冲突的要求；总plan.md保留只读。本任务独立交付，只对来源提交包含的增量、必要直接接线及测试意图负责。按文件冲突图分批并行，编号前后关系和Git父提交不是业务依赖，不要求前面所有任务完成或全部回归；仅核查实际用到的具体能力，缺少时在本编号记录接口、源码和影响。

完成条件：全部增量在当前Rust真实路径中已实现且必要聚焦验证通过，或者有完整后续SHA及源码证明该增量已撤销/替代、无需恢复；仍保留的行为和测试意图必须逐项核验。符号存在、源码已读、仅修改状态、零测试、编译失败或旧报告不能代替行为证据。全部被撤销或无Rust影响的提交可用完整来源/去向/消费者证据完成，不强造行为测试。

验证只选本任务必要的受影响测试与仓库按变更类型要求的交付检查。Rust代码交付仍要求cargo fmt、聚焦回归、make lint及diff检查；生产接线改变再做NextGen编译。全仓测试、其他提交回归、全子系统补建、无关Bazel构建和一般性性能/集群验证不属于本任务完成条件，不因未运行它们标“待回归”。如果本提交行为本身依赖真实TiKV、集成或并发条件，相应最小验证仍是本任务必需，不能归入范围外来豁免。

仅使用状态：未开始、进行中、已完成、已阻塞。必要目标行为或必要验证缺失则已阻塞，准确记录缺口及已运行检查；与任务无关的基线失败先用合法目标命令或等价真实边界绕开，确实无法取得必要证据仍标已阻塞。禁止“已完成，待回归”及“核查完成”冒充实现完成。

交接明确列出：本提交范围内完成项、当前必要验证命令/数量/退出码、实际阻塞（若有），另列范围外建议且不影响已完成状态。记录保留，不删除编号，不提交主仓库，不吞并其他提交。历史状态由本文件最新结论覆盖，历史失败和未验证事实不删除。


## 文件冲突与并行执行（2026-10-02 用户更新）


本节更新并行调度，覆盖旧总plan.md及历史记录中的全串行要求；总plan.md仍只读。批次和边仅由候选写入文件交集生成，是编辑顺序约束，不代表业务能力依赖。按编号保留共享文件的先后顺序，同批次可并行，最多10个任务；已完成节点直接满足约束。阻塞节点只延后真正依赖其能力或必须接续其共享文件改动的任务，不阻断不相关分支。

执行前必须在本编号确认实际写入清单、所属crate注册文件及必要helper/调用方。图未覆盖的新写入文件、真实业务依赖或共享可变资源出现时，先向调度者报告并调整文件占用，暂停冲突任务；不能擅自写入另一活动任务占用的文件。来源Git父提交、Cargo只读依赖或共同读取文档不是默认串行理由。没有完成全部函数级依赖分析，因此此图不是“无业务依赖”的证明。

根Cargo.toml/Cargo.lock、cargo fmt --all、make lint、Bazel准备、全局failpoint开关、共用playground以及上游发布由调度者串行安排独占窗口；其他编辑在cargo fmt --all窗口暂停，之后再基于最终文件做必要验证。聚焦测试可在安全资源边界并行，共用Cargo target会由Cargo锁串行等待，不能停别人的Cargo；若隔离target会导致重复巨量编译，优先排队。每任务仍要取得独立目标证据，串行共享交付检查只可复用确实覆盖最终未再改变文件的结果。

本任务的候选写入清单和带路径原因的前驱边见本目录依赖与并行批次.md、文件依赖图.html及并行调度.json。来源候选不是保证写入项；确认某共享注册/manifest无需修改后可在调度层解除相应编辑约束，真实能力依赖必须另以源码证据记录。


## 文件


- Go来源：`pkg/executor/adapter.go`。
- Go来源：`pkg/executor/adapter_internal_test.go`。
- Go来源：`pkg/executor/adapter_test.go`。
- Go来源：`pkg/executor/select.go`。
- Go来源：`pkg/executor/select_internal_test.go`。
- Go来源：`pkg/server/conn_test.go`。
- Go来源：`pkg/session/nontransactional.go`。
- Go来源：`pkg/session/session.go`。
- Go来源：`pkg/session/session_test.go`。
- Go来源：`pkg/session/sessmgr/processinfo.go`。
- Go来源：`pkg/session/test/nontransactionaltest/nontransactional_test.go`。
- Go来源：`pkg/session/test/variable/variable_test.go`。
- Go来源：`pkg/session/test/vars/vars_test.go`。
- Go来源：`pkg/session/tidb.go`。
- Go来源：`pkg/sessionctx/vardef/tidb_vars.go`。
- Go来源：`pkg/sessionctx/variable/session.go`。
- Go来源：`pkg/sessionctx/variable/setvar_affect.go`。
- Go来源：`pkg/sessionctx/variable/sysvar.go`。
- Go来源：`pkg/sessionctx/variable/sysvar_test.go`。

- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/adapter.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/adapter_internal_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/adapter_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/select.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/server/conn_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/nontransactional.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/session.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/session_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/sessmgr/processinfo.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/test/nontransactionaltest/nontransactional_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/test/variable/variable_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/test/vars/vars_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/tidb.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/vardef/tidb_vars.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/variable/session.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/variable/setvar_affect.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/variable/sysvar.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/variable/sysvar_test.rs`。

其他来源文件也必须核查，不默认全部无需Rust同步：

- `M → pkg/server/BUILD.bazel`
- `M → pkg/session/BUILD.bazel`
- `M → pkg/session/test/variable/BUILD.bazel`

回归测试使用真正拥有crate的独立测试文件；测试名称遵循目标 crate 的现有惯例并描述被验证行为，不使用来源提交哈希或 `go_commit_*` 前缀；已有同名*_test.rs优先扩充，autotests=false时必须核验模块注册。本文件只列实际存在候选，不凭路径相似创建重复实现。

## 上下文


从仓库根执行，先读AGENTS.md、PLANS.md、相关包doc.go；DDL先读docs/agents/ddl/README.md。使用skills/rustcodegraph/SKILL.md定位真实实现；未索引、配置或过期文件再rg。当前已有部分移植，不预设缺失，不继承旧任务完成状态。

## Go变更定位清单


以下是Git差异上下文入口，并非完整行为清单；必须补读完整函数、helper、调用方和前后代码：

- `import (`。
- `func (a *ExecStmt) PointGet(ctx context.Context) (*recordSet, error) {`。
- `func (a *ExecStmt) Text() string {`。
- `func IsFastPlan(p base.Plan) bool {`。
- `func (a *ExecStmt) Exec(ctx context.Context) (_ sqlexec.RecordSet, err error) {`。
- `func (a *ExecStmt) handleFKTriggerError(sc *stmtctx.StatementContext) error {`。
- `func (a *ExecStmt) handleNoDelay(ctx context.Context, e exec.Executor, isPessimi`。
- `type stmtStatsTestContext struct {`。
- `func TestRecordSetNextAfterFinish(t *testing.T) {`。
- `func TestMaxExecutionTimeIncludesTSOWaitTime(t *testing.T) {`。
- `func (e *SelectLockExec) Next(ctx context.Context, req *chunk.Chunk) error {`。
- `func checkMaxExecutionTimeExceeded(sctx sessionctx.Context) error {`。
- `func newLockCtx(sctx sessionctx.Context, lockWaitTime int64, numKeys int, inShar`。
- `func TestNewLockCtxPropagatesSharedLockUpgrade(t *testing.T) {`。
- `func TestConnExecutionTimeout(t *testing.T) {`。
- `func HandleNonTransactionalDML(ctx context.Context, stmt *ast.NonTransactionalDM`。
- `func (s *session) retry(ctx context.Context, maxCnt uint) (err error) {`。
- `func getSessionFactoryInternal(store kv.Storage, createSessFn func(store kv.Stor`。
- `func (s *session) SetProcessInfo(sql string, t time.Time, command byte, maxExecu`。
- `func TestGetStartMode(t *testing.T) {`。
- `type ProcessInfo struct {`。
- `func TestNonTransactionalDmlIgnoreMaxExecutionTime(t *testing.T) {`。
- `func TestMaxExecutionTime(t *testing.T) {`。
- `func TestGlobalVarAccessor(t *testing.T) {`。
- `func finishStmt(ctx context.Context, se *session, meetsErr error, sql sqlexec.St`。
- `const (`。
- `type SessionVars struct {`。
- `var isHintUpdatableVerified = map[string]struct{}{`。
- `var defaultSysVars = []*SysVar{`。

新增Go测试入口：`TestCheckMaxExecutionTimeExceededPreservesPendingKillReason`、`TestDMLMaxExecutionTimeExpiresBeforeExecutorOpen`、`TestDMLBuildCancellationPreservesTimeout`、`TestConnDMLExecutionTimeout`、`TestNormalizeStmtCancellationError`、`TestSetProcessInfoDuringRetry`、`TestDMLMaxExecutionTime`

## 测试计划


行为：严格覆盖本提交每项生产变更、错误/边界及Go测试意图，记录每项Go源码位置到Rust实际调用路径的映射。

失败验证：在实际拥有模块的独立测试文件新增按目标 crate 现有惯例和行为语义命名的回归测试，输入、输出、错误及副作用来自本Go差异，使用现有真实组件。具体子测试名及数据在修改生产逻辑前写入本任务覆盖记录。

候选失败和通过命令相同，以下manifest均在制定计划时存在；先确认真正拥有模块，只运行受影响者。若实现已移动，在本文件记录新的真实manifest及原因后再运行，不同时遍历所有crate：

    cargo test --manifest-path pkg/executor/Cargo.toml go_commit_b82bed1eca -- --nocapture
    cargo test --manifest-path pkg/server/Cargo.toml go_commit_b82bed1eca -- --nocapture
    cargo test --manifest-path pkg/session/Cargo.toml go_commit_b82bed1eca -- --nocapture
    cargo test --manifest-path pkg/session/sessmgr/Cargo.toml go_commit_b82bed1eca -- --nocapture
    cargo test --manifest-path pkg/session/test/nontransactionaltest/Cargo.toml go_commit_b82bed1eca -- --nocapture
    cargo test --manifest-path pkg/session/test/variable/Cargo.toml go_commit_b82bed1eca -- --nocapture
    cargo test --manifest-path pkg/session/test/vars/Cargo.toml go_commit_b82bed1eca -- --nocapture
    cargo test --manifest-path pkg/sessionctx/vardef/Cargo.toml go_commit_b82bed1eca -- --nocapture
    cargo test --manifest-path pkg/sessionctx/variable/Cargo.toml go_commit_b82bed1eca -- --nocapture

预期失败：缺失或偏离的Go行为在真实路径出现错误结果、遗漏副作用或状态/错误不符；每个测试具体预期值来自本提交Go逻辑。编译错误、缺资源和零测试不是有效行为红灯。已完整移植的项不人为回退制造失败，注明不适用并取得当前通过证据；新增测试应验证现有实现而非强迫重写。

模拟策略：复用真实SQL/KV/元数据及已有输入，Go要求有行或非空元数据时不能用空样例；只替换明确外部网络、时间与故障边界，记录数据形状、调用顺序、错误及副作用。

## 步骤


1. 运行下面的来源命令，读取本提交前后完整变更函数和直接helper、相邻Go测试；检查后续提交及当前Go是否撤销或替代该逻辑，记录完整SHA及逐项去向。
2. 验证Rust生产入口、工厂接线和相邻独立测试；逐函数建立覆盖清单，区分已等价、未完整、偏离、被后续替代及有证据的无Rust影响，不用同名符号或旧通过报告认定完成。
3. 对已等价项运行实际测试并记录数量与退出码；对缺失项先写真实行为回归、取得准确失败，随后按Go条件、顺序、状态、错误、并发和副作用修复当前所属模块，不新建简化替代链。
4. Rust修改后先cargo fmt --all，运行对应行为回归测试，确保实际执行、无相关ignored；核查Go原有测试分支均有对应覆盖，不能仅测成功路径。
5. 只做本提交必要的前置修复；遇独立架构或外部依赖缺口记录本编号阻塞、准确接口和错误，不扩大为全量DDL/前端重写。Go有意跳过的逻辑不能在Rust加额外行为。
6. 完成自审与适用Ready验证，保存Go→Rust覆盖表、当前命令及红绿证据；大提交逐段续接，不以局部通过宣称完整完成。

## 验证


来源检查：

    git show --format=fuller --stat b82bed1eca2fcbe58f0fc074fff53e1fc49daa8c
    git diff --find-renames --unified=30 1cab931bd85a44e8a1563f8261f49572439e425a b82bed1eca2fcbe58f0fc074fff53e1fc49daa8c
    git log --format='%H %s' b82bed1eca2fcbe58f0fc074fff53e1fc49daa8c..ad193e964b^2 -- pkg/executor/adapter.go pkg/executor/adapter_internal_test.go pkg/executor/adapter_test.go pkg/executor/select.go pkg/executor/select_internal_test.go pkg/server/conn_test.go pkg/session/nontransactional.go pkg/session/session.go pkg/session/session_test.go pkg/session/sessmgr/processinfo.go pkg/session/test/nontransactionaltest/nontransactional_test.go pkg/session/test/variable/variable_test.go pkg/session/test/vars/vars_test.go pkg/session/tidb.go pkg/sessionctx/vardef/tidb_vars.go pkg/sessionctx/variable/session.go pkg/sessionctx/variable/setvar_affect.go pkg/sessionctx/variable/sysvar.go pkg/sessionctx/variable/sysvar_test.go

后续日志只查本提交相关路径；源码更名则追踪重命名，不把日志无命中当作没有后续影响。生成Go追溯生成源和Rust机制，不手抄生成结果；删除路径核对调用者与替代实现。

代码交付Ready命令：

    cargo fmt --all
    cargo fmt --all -- --check
    make lint
    git diff --check

再运行本任务拥有crate的上述聚焦回归。生产接线增加：

    cargo check --manifest-path cmd/tidb-server/Cargo.toml --bin astersql-cmd-tidb-server --features nextgen

Go/Bazel修改前按AGENTS.md判断make bazel_prepare；failpoint/集成记录/RealTiKV按docs/agents/testing-flow.md。禁止make bazel_lint_changed，不停止他人Cargo、不改变他人failpoint。文档、测试资料、构建或无Rust影响任务选择实际适用检查并说明理由，不为非代码提交制造生产修改。

所需证据：全部源路径和Go函数/分支有明确去向，测试意图匹配，Rust实际调用链及字段/错误/副作用等价；缺失项有行为红→绿及数量、退出码，已有正确项有当前绿；每个Ready结果与未验证项明确。0测试不算通过。

## 完成


只在全部本提交行为及测试意图取得证据后标记已完成。已阻塞在本文件记录准确原因及已运行命令，不修改plan.md/prompt.md，不删除本提交的覆盖依据。范围外全仓/集群/性能建议不影响本任务完成；本任务必要行为或验证缺失必须标已阻塞，不使用待回归状态。最终按AGENTS.md报告修改文件、profile与理由、正确性/兼容性/性能风险、确切命令及未验证项；不提交主仓库。

## Go到Rust覆盖记录


执行时按函数或行为分段填写：Go位置/条件与顺序/错误与副作用/Go测试意图 → Rust真实符号及入口 → 已有或修复/后续替代SHA → 具体测试输入输出与命令、退出码和数量。每段完整保留，不用一句“已对齐”替代逐项证据。

## Progress（进度）


- [ ] 读取Go完整来源与后续覆盖关系，列出本提交全部行为分段。
- [ ] 核对现有Rust与实际接线，记录已覆盖和缺口。
- [ ] 逐段补齐必要逻辑并取得适用红绿/当前通过证据。
- [ ] 自审、适用Ready及交接完成。

## Surprises & Discoveries（发现）


尚未执行；Git路径与现存候选是计划事实，Rust行为完整性尚未验证。

## Decision Log（决策）


2026-10-01：一个来源提交对应一个任务；保留已有正确移植，按Go完整逻辑核对，不偏移目标。

## Outcomes & Retrospective（结果）


尚未执行；结束时记录逐项产物、最终证据与未验证项。分段完成只代表该段，不代表整个提交。
