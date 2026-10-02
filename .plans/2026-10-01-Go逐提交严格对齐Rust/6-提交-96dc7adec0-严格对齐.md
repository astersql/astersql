# 任务 6: 提交 96dc7adec0 严格对齐 Rust

执行批次：【批次 6】；文件顺序前驱：3、5；任务编号：6。同批可并行，实际能力依赖另行核查。

状态：进行中

目的：逐项核对本Go提交在当前Rust的实际覆盖，只修缺失和偏离，保留完整Go逻辑与测试意图。

来源任务：Go提交 `96dc7adec0342fa78d1f7a1355cddcc329af19f8`；父提交 `52f7a7a3e65823326a723a17aacc863be16743f4`；原始提交标题：ddl: tolerate missing masking policy table during upgrade (#69531)。

预计会话范围：限定本提交的变更行为及直接依赖，先复用已有正确移植；不扩大为所属子系统全量重写。


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


- Go来源：`pkg/ddl/ddl.go`。
- Go来源：`pkg/ddl/masking_policy.go`。
- Go来源：`pkg/ddl/masking_policy_internal_test.go`。
- Go来源：`pkg/ddl/table.go`。
- Go来源：`pkg/session/test/bootstraptest/bootstrap_upgrade_test.go`。

- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/ddl.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/masking_policy.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/table.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/test/bootstraptest/bootstrap_upgrade_test.rs`。

其他来源文件也必须核查，不默认全部无需Rust同步：

- `M → pkg/ddl/BUILD.bazel`

回归测试使用真正拥有crate的独立测试文件，前缀 `go_commit_96dc7adec0`；已有同名*_test.rs优先扩充，autotests=false时必须核验模块注册。本文件只列实际存在候选，不凭路径相似创建重复实现。

## 上下文


从仓库根执行，先读AGENTS.md、PLANS.md、相关包doc.go；DDL先读docs/agents/ddl/README.md。使用skills/rustcodegraph/SKILL.md定位真实实现；未索引、配置或过期文件再rg。当前已有部分移植，不预设缺失，不继承旧任务完成状态。

## Go变更定位清单


以下是Git差异上下文入口，并非完整行为清单；必须补读完整函数、helper、调用方和前后代码：

- `type ddlCtx struct {`。
- `func (d *ddl) Start(startMode StartMode, ctxPool *pools.ResourcePool) error {`。
- `import (`。
- `func (w *worker) queryMaskingPoliciesFromSysTable(ctx context.Context, query str`。
- `func (w *worker) dropMaskingPoliciesOnTable(jobCtx *jobContext, tableID int64) e`。
- `func (w *worker) dropMaskingPoliciesByDBName(jobCtx *jobContext, dbName string)`。
- `func (w *worker) updateMaskingPolicyTableIDAfterTruncate(jobCtx *jobContext, old`。
- `func (w *worker) dropMaskingPoliciesOnColumn(jobCtx *jobContext, tableID, column`。
- `func (w *worker) updateMaskingPolicyNamesAfterRename(`。
- `func (w *worker) syncMaskingPolicyForModifiedColumn(`。
- `+// Copyright 2026 PingCAP, Inc.`。
- `func (w *worker) onRenameTable(jobCtx *jobContext, job *model.Job) (ver int64, _`。
- `func (w *worker) onRenameTables(jobCtx *jobContext, job *model.Job) (ver int64,`。
- `func TestUpgradeVersion260MaskingPolicy(t *testing.T) {`。

新增Go测试入口：`TestMaskingPolicyMissingSysTableRequiresBootstrapOrUpgrade`

## 测试计划


行为：严格覆盖本提交每项生产变更、错误/边界及Go测试意图，记录每项Go源码位置到Rust实际调用路径的映射。

失败验证：在实际拥有模块的独立测试文件新增 `go_commit_96dc7adec0` 前缀回归，输入、输出、错误及副作用来自本Go差异，使用现有真实组件。具体子测试名及数据在修改生产逻辑前写入本任务覆盖记录。

候选失败和通过命令相同，以下manifest均在制定计划时存在；先确认真正拥有模块，只运行受影响者。若实现已移动，在本文件记录新的真实manifest及原因后再运行，不同时遍历所有crate：

    cargo test --manifest-path pkg/ddl/Cargo.toml go_commit_96dc7adec0 -- --nocapture
    cargo test --manifest-path pkg/session/test/bootstraptest/Cargo.toml go_commit_96dc7adec0 -- --nocapture

预期失败：缺失或偏离的Go行为在真实路径出现错误结果、遗漏副作用或状态/错误不符；每个测试具体预期值来自本提交Go逻辑。编译错误、缺资源和零测试不是有效行为红灯。已完整移植的项不人为回退制造失败，注明不适用并取得当前通过证据；新增测试应验证现有实现而非强迫重写。

模拟策略：复用真实SQL/KV/元数据及已有输入，Go要求有行或非空元数据时不能用空样例；只替换明确外部网络、时间与故障边界，记录数据形状、调用顺序、错误及副作用。

## 步骤


1. 运行下面的来源命令，读取本提交前后完整变更函数和直接helper、相邻Go测试；检查后续提交及当前Go是否撤销或替代该逻辑，记录完整SHA及逐项去向。
2. 验证Rust生产入口、工厂接线和相邻独立测试；逐函数建立覆盖清单，区分已等价、未完整、偏离、被后续替代及有证据的无Rust影响，不用同名符号或旧通过报告认定完成。
3. 对已等价项运行实际测试并记录数量与退出码；对缺失项先写真实行为回归、取得准确失败，随后按Go条件、顺序、状态、错误、并发和副作用修复当前所属模块，不新建简化替代链。
4. Rust修改后先cargo fmt --all，运行对应前缀回归，确保实际执行、无相关ignored；核查Go原有测试分支均有对应覆盖，不能仅测成功路径。
5. 只做本提交必要的前置修复；遇独立架构或外部依赖缺口记录本编号阻塞、准确接口和错误，不扩大为全量DDL/前端重写。Go有意跳过的逻辑不能在Rust加额外行为。
6. 完成自审与适用Ready验证，保存Go→Rust覆盖表、当前命令及红绿证据；大提交逐段续接，不以局部通过宣称完整完成。

## 验证


来源检查：

    git show --format=fuller --stat 96dc7adec0342fa78d1f7a1355cddcc329af19f8
    git diff --find-renames --unified=30 52f7a7a3e65823326a723a17aacc863be16743f4 96dc7adec0342fa78d1f7a1355cddcc329af19f8
    git log --format='%H %s' 96dc7adec0342fa78d1f7a1355cddcc329af19f8..ad193e964b^2 -- pkg/ddl/ddl.go pkg/ddl/masking_policy.go pkg/ddl/masking_policy_internal_test.go pkg/ddl/table.go pkg/session/test/bootstraptest/bootstrap_upgrade_test.go

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


## 2026-10-02 本轮依赖、来源核查与基础阻塞


已应用仓库 skills/do-task-plans/SKILL.md 与 skills/rustcodegraph/SKILL.md，已读 PLANS.md、只读总计划、DDL README、pkg/ddl/doc.go 及本编号。用户要求保留编号优先于执行技能默认删除规则。当前 HEAD 为264abdbd8de5c9465273a90f33845f471809cb5c；开始工作区已有任务4、任务5及19个Rust/Cargo相关文件差异，均保留，未回退、未提交。

依赖门槛已审查：批次1最终14目标及4兼容、批次2最终23唯一目标及兼容、批次3最终9目标及8兼容均记录通过及适用Ready退出0；批次4为无Rust影响静态核验、已完成待回归。批次5末节明确用户接受批次4该状态并允许顺序推进，最终10目标及4兼容通过，fmt/lint/NextGen/diff退出0，标记已完成待回归并明确后续任务6可推进。此处只审查依赖证据，不把旧报告当成本任务当前测试证据。前置不是本轮阻塞原因。

### 来源分段和后续去向

来源96dc7adec0342fa78d1f7a1355cddcc329af19f8，父52f7a7a3e65823326a723a17aacc863be16743f4：6路径、186插入/47删除。已读完整差异、masking_policy.go查询/清理/改名/改列完整函数及直接helper，核查全部相关路径的后续日志。

关键后续531e40cd25989404f9fd1f51cddf278326983af1明确撤销本提交容忍逻辑，未恢复已撤销代码。该后续源码及当前Go作为去向依据，不能把本提交认定“无Rust影响”：查询failpoint和升级189/254场景仍保留并被后续强化。

| 来源项 | 本提交行为与后续去向 | 当前Rust证据及未验证项 |
| --- | --- | --- |
| ddl.go ddlCtx.startMode、Start赋值 | executor failpoint后的实际mode写入worker上下文；531e40cd完整删除该字段和赋值 | ddl.rs StartMode存在但不是这条worker系统表链；不增加已删除字段 |
| masking_policy.go queryMaskingPoliciesFromSysTable | failpoint先于Execute返回ErrTableNotExists(mysql,tidb_masking_policy)，否则14列SELECT按policy_id排序，解析失败即返回 | masking_policy.rs只有BTreeMap存储；全pkg Rust检索没有同等worker查询函数/故障边界，不能证明错误身份 |
| allowMissingMaskingPolicyTableDuringBootstrap | 先检查错误身份、jobCtx/oldDDLCtx非nil，仅Bootstrap/Upgrade容忍；531e40cd删除整个helper | 不重新引入helper；当前验收需真实查询缺表错误 |
| dropMaskingPoliciesOnTable/dropMaskingPoliciesOnColumn | 来源用mode限制容忍，成功逐policy ID DELETE，错误即返回；531e40cd撤销容忍，当前始终返回查询错误 | MaskingPolicyStore::drop_on_table/drop_on_column仅retain，返回void，不能表达数据库错误、逐行副作用及中途失败 |
| updateMaskingPolicyTableIDAfterTruncate | 来源mode容忍，否则逐ID更新table_id/updated_at且同一个now；531e40cd撤销容忍 | update_table_id_after_truncate仅内存更新，无系统表SQL与事务链 |
| updateMaskingPolicyNamesAfterRename、table.go两调用 | 来源接jobCtx并从stepCtx查询、按小写名无变化则skip，克隆后更新名字/时间；531e40cd还原ctx参数及两个调用并撤销容忍 | update_names_after_rename仅内存更新，未找到生产调用，不能验证单表/多表rename错误顺序 |
| syncMaskingPolicyForModifiedColumn | nil输入早返回、查询错误、按tableID与旧ID/旧名/新名筛选后校验类型、克隆并重写表达式/更新；531e40cd撤销容忍 | sync_modified_column有内存对应但先校验新列类型，未接SQL；本轮不重写既有完整策略子系统 |
| dropMaskingPoliciesByDBName（直接helper） | 本来源没有行为修改，当前仍仅ErrTableNotExists best-effort，其他错误返回；531e40cd只改注释 | drop_by_database_name仅retain，无SQL错误；不能用空内存当作缺表容忍证据 |
| masking_policy_internal_test.go | 来源5 helper×Normal/Bootstrap/Upgrade，Normal保持ErrTableNotExists；531e40cd替换成5 helper全部保持ErrTableNotExists | lib.rs只注册masking_policy_test，未有同等系统表测试；现有内存测试不作为本任务证据 |
| bootstrap_upgrade_test.go | 254扩至189/254，meta FinishBootstrap→DropTableOrView→cache清理→关闭domain→bootstrap→当前版本和完整14列/3索引 | 同名Rust测试文件只含mock升级SQL录制；boot_test.rs有真实初始建表schema断言，但没有来源189/254删除元数据后重启场景 |
| BUILD.bazel | 注册新增Go内部测试；后续保留注册 | 当前没有对应Rust内部系统表回归模块；未改Go/Bazel，无需bazel_prepare |

531e40cd在session/session.go把容忍替换为升级锁内重读version后initBootstrapDependentTables：只对Classic、0<version<260（后续版本映射须核查）且目标版本足够时，按现有名字跳过、全局分配新ID、真实事务建表；并在并发已升级分支切Normal避免误重建已rename表。本轮只追踪其作为本提交去向，不吞并这个后续提交全部实现。其bootstrap测试新增非reserved ID、新物理ID以及rename/restart和并发版本重读场景，也未用旧测试覆盖本任务。

### 准确阻塞与续接条件

当前独立基础缺口是生产DDL worker的masking-policy系统表查询/修改会话及接线，不是无关测试失败：MaskingPolicyStore在全pkg Rust中只由masking_policy_test使用；JobWorker/JobContext没有对应sess系统表执行或策略清理调用，persistent_actions及normal_ddl_service未找到策略接入。图callers drop_on_table返回No callers found，原始源码检索进一步验证。仅infoschema另有14列读取SQL，不能替代DDL写路径。当前Canonical升级入口upgrade_canonical_domain只执行变量/summary/bind digest迁移；RuntimeFactory::BootstrapSession会take storage，已有store重复调用报already bootstrapped。不能把初始自动建系统表或mock录制升级当作元数据删除重启及升级锁验证。

按本任务步骤5“不扩大为全量DDL/前端重写”及执行技能“遇到阻碍（缺少依赖、测试失败、指令不明确）立即停止执行”，停在源码/接线核查阶段。没有创建模拟缺表flag、伪造worker SQL接口或恢复后续撤销容忍；没有新增行为测试，因为缺少可调用的真实系统表生产路径，编译错误或0测试不能构成红灯。尚未取得本提交任何当前行为红绿证据，不能标已完成或已完成待回归。

下一步由本编号续接：先恢复真实worker系统表会话及五操作生产接线这一基础，或取得用户对本提交必要范围的明确授权；保留后续撤销逻辑，以当前Go的查询错误身份验收，并核查189/254元数据删除重启所需真实运行时边界。不得直接推进任务7或把基础缺口算作已通过。

### 本轮已运行检查和限制

以下检查从仓库根执行：

    git status --short
    git rev-parse HEAD
    cat skills/do-task-plans/SKILL.md
    cat skills/rustcodegraph/SKILL.md
    cat PLANS.md
    cat docs/agents/ddl/README.md
    cat pkg/ddl/doc.go
    ~/.rustcodegraph/bin/rustcodegraph status
    git show --format=fuller --stat 96dc7adec0342fa78d1f7a1355cddcc329af19f8
    git diff --unified=8 52f7a7a3e65823326a723a17aacc863be16743f4 96dc7adec0342fa78d1f7a1355cddcc329af19f8
    git log --format='%H %s' 96dc7adec0342fa78d1f7a1355cddcc329af19f8..ad193e964b^2 -- pkg/ddl/ddl.go pkg/ddl/masking_policy.go pkg/ddl/masking_policy_internal_test.go pkg/ddl/table.go pkg/session/test/bootstraptest/bootstrap_upgrade_test.go
    git show 531e40cd25989404f9fd1f51cddf278326983af1 -- pkg/ddl/ddl.go pkg/ddl/masking_policy.go pkg/ddl/masking_policy_internal_test.go pkg/ddl/table.go pkg/session/session.go pkg/session/test/bootstraptest/bootstrap_upgrade_test.go
    rg -n 'queryMaskingPoliciesFromSysTable|query_masking_policies|drop_on_table\(|update_names_after_rename\(|MaskingPolicyStore|tidb_masking_policy' pkg --glob '*.rs'
    git diff --check
    git diff -- .plans/2026-10-01-Go逐提交严格对齐Rust/plan.md .plans/2026-10-01-Go逐提交严格对齐Rust/prompt.md

以上Git/主要来源检索退出0；RustCodeGraph status退出0，11339文件。另用graph explore/node读ddl.rs、masking_policy.rs、table.rs、job_worker.rs；原始读取核查工作区已变化session/upgrade_run.rs、runtime/session.rs及图未覆盖资料。单个图callers drop_on_table完成返回无调用者，后续callers update_names_after_rename长时间未返回，手动结束本轮自有只读命令，退出130；该批后续命令未执行，已用独立rg/sed及Git命令补查，未当作成功。rg查不存在.agents/skills退出2（verify-profile不存在），以及局部无匹配检索退出1，不当作行为失败。初次宽读取有输出截断，不以截断结果声称完整源码覆盖。

本轮只有依赖审查、来源追踪与生产接线分析，代码验证profile不适用；未运行cargo测试、fmt、make lint、NextGen check、Go/Bazel、RealTiKV，没有以这些未运行项宣称验收。只改本编号，最终git diff --check结果见收尾。总plan.md/prompt.md未改，无Git提交、无其他任务推进、无子代理。正确性风险：五真实系统表路径及189/254升级尚未验证；兼容性/性能没有生产改动，但其行为仍有基础缺口。

Progress：来源撤销/替代和基础接线检查已记录；行为实现、回归与Ready仍未完成，原有未勾选项保持。结果：已阻塞，保留本编号供同任务续接。

收尾：记录更新后 `git diff --check` 退出0；只读总plan.md/prompt.md差异为空。没有获得行为通过证据，状态保持已阻塞。


## 2026-10-02 最小真实接线替代方案复核


收到调度授权：只复核本提交五项必要的最小真实worker系统表查询/修改接线，若仍需扩大范围则补充结论并停止。复用前节来源与撤销证据，没有重跑前节相同无变化的来源/依赖检查，也没有重新询问已给定授权。

更正前节过宽表述：真实SQL执行和事务承载能力已经存在，不是缺少所有worker SQL接口。job_worker.rs:210起DurableJobSession及JobExecutionContext提供query、with_transaction和with_execution_context；system_session.rs:967起ConcreteJobExecutionContext实现它们，1158的query允许单条SELECT/INSERT/UPDATE/DELETE，1176的with_transaction使用同一ConcreteSession活跃事务；正常JobWorker在begin→读取job→executor.step→更新job→commit内运行。因此无需新建SQL桥或复制运行时。准确缺口是五项操作的真实持久action handler及其调用链。

最小替代方案逐项判断：

1. 独立SQL helper可复用JobExecutionContext并严格返回缺表错误，但persistent_actions.rs:20 handler_available没有DROP TABLE(4)、DROP COLUMN(6)、TRUNCATE TABLE(11)、MODIFY COLUMN(12)、RENAME TABLE(14)/RENAME TABLES(47)。值来自meta/model/job.rs:62–72和105，table_mode.rs:190–199对未注册活跃job明确返回normal DDL persistent handler unavailable。新增helper仍没有这六个action的生产调用，单独测试helper不能满足五项操作实际接线验收。
2. 直接把helper塞进当前SQL前台入口并非真实worker接线。runtime/ddl.rs的execute_drop_table(762)、execute_rename_table_pairs(836)、execute_truncate_table(3455)、ModifyColumn(2125)、DropColumn(3270)调用Domain.ddl_*，没有经过这些持久action。Domain/domain.rs:3594、3670、3682、3815、3890委托DdlMetadataService；canonical_domain.rs:116–204的mutate_with_kv自己Begin/Commit。前台另开SQL会话执行策略写入会让策略变更与元数据分开提交；放在之后会使失败时已发布结构，放在之前会先提交策略清理。这不能匹配Go同一worker事务及错误/重试顺序，不能为“最小”而改成该替代路径。
3. 迁移五操作到持久worker需要新增六action的metadata/state transition/原子多表rename/rollback及DDL任务提交入口，再在相应阶段调用策略SQL；不能仅注册空壳、一次性public发布或借用内存模型。该工作属于原先缺失的DDL action框架，超过当前来源只改缺表错误容忍/参数传递的范围。本次授权限定最小接线，未授权实现这些完整action，因此不扩展。
4. 保持531e40cd25989404f9fd1f51cddf278326983af1的撤销：即使helper补建，缺表在五项操作当前Go仍须报错，不再根据StartMode吞错。系统表初始化和189/254升级仍需其正确事务/锁边界，不能用helper通过覆盖升级未验证项。

复核结论：存在可复用真实SQL/事务桥，但缺少五项行为承载的持久action；没有在本任务限定范围内兼顾真实worker接线与完整Go语义的安全局部替代。状态保持已阻塞。没有生产/测试代码改动，没有本提交行为回归运行或红绿证据，没有把零测试、独立helper测试或旧报告算作完成；Ready不适用本次只读架构复核。下一步先由对应基础DDL范围实现/提供上述持久action，再从本编号继续五项查询错误与升级行为验收；不推进其他编号。

本次新增实际检查：用sed读取pkg/ddl/persistent_actions.rs(1–260、430–500)、pkg/ddl/job_worker.rs(170–470)、pkg/ddl/table_mode.rs(175–225)、pkg/session/runtime/normal_ddl_service.rs(1–260)、system_session.rs(1–210、1145–1210)、runtime/ddl.rs上述五入口；用rg核查action定义和Domain方法，再读取domain.rs(3585–3935相关段)、canonical_domain.rs五方法及事务边界(110–155、192–209)。主要读取/定位退出0；初次假设meta/model/group_3.rs及group_3目录的rg退出2（实际是lib模块别名，已改定位真实job.rs），部分宽输出截断后已用精确段补读。不重复耗时graph callers。没有运行cargo、Go、fmt或lint，因为没有实现可验收的行为修改。本轮仅在本编号追加复核结论，最后git diff --check作为文档差异检查。


## 2026-10-02 用户调整验收范围后的最终结论


用户明确：“这里，主要看这次提交包含没有，包含了就算完成”。本任务验收据此限定为本来源提交包含的增量及后续去向核查，不再把既有DDL action架构补齐作为本任务完成条件。此前两节阻塞结论为旧验收条件下的记录，由本节覆盖；源码发现及未验证边界继续保留。

本提交增量已逐项记录：ddlCtx模式字段及Start赋值、缺表判断helper、五项操作的缺表分支、rename两个调用传jobCtx，以及内部测试注册与189/254升级测试扩充。531e40cd25989404f9fd1f51cddf278326983af1撤销模式字段、helper、五分支和rename参数变化，当前不重新移植已撤销逻辑。查询failpoint与测试意图的去向也已核查并在覆盖表记录，不把基础worker改造归入本提交。

按用户本轮限定，本编号标记已完成，保留编号，不改总plan.md/prompt.md、不提交主仓库。此状态表示本提交范围和去向核查完成，不表示五项Rust生产路径或189/254升级回归已通过：这些仍未运行，现有系统表接线缺口和测试覆盖缺口保留为后续回归/基础实现边界。没有新增Rust改动或行为红绿证据，不能把此完成状态引用为Rust严格行为等价证明。

本次只调整本编号状态和验收说明，复用已取得源码证据；验证为git diff --check，代码验证profile不适用。正确性/兼容性/性能的未验证项见前节，未作额外成功声明。下一步可依用户限定的顺序任务契约继续后续编号，基础接线和升级回归另行处理。


## 2026-10-02 独立验收规则下的状态更正

前节把来源去向核查当作完成，未证明仍保留的查询failpoint/错误行为和189/254升级测试意图已在Rust真实路径覆盖，故不符合用户要求的独立实现验收。本编号恢复已阻塞，具体缺口见最小接线复核；不是等待全仓回归或其他编号全部完成。已撤销的五处容忍逻辑无需恢复；未验证行为不能因为撤销证据而一并算完成。无新增生产代码或行为通过证据。


## 2026-10-02 任务1–5当前回归后的定向复核


已重读本编号最新独立验收/并行规则及前两次阻塞复核，按收到的续接指令只确认缺口是否变化。任务1–5当前记录均为已完成；任务3末节当前初始化目标9项、任务5末节目标10项及兼容4项通过，但这些记录不代表增加任务6处理器，不代替本任务验证。

实际检查（根目录）：

    git status --short -- pkg/ddl/persistent_actions.rs pkg/ddl/job_worker.rs pkg/ddl/table_mode.rs pkg/session/runtime/system_session.rs pkg/session/runtime/ddl.rs pkg/domain/canonical_domain.rs
    git diff --stat -- pkg/ddl/persistent_actions.rs pkg/ddl/job_worker.rs pkg/ddl/table_mode.rs pkg/session/runtime/system_session.rs pkg/session/runtime/ddl.rs pkg/domain/canonical_domain.rs
    sed -n '18,65p' pkg/ddl/persistent_actions.rs
    sed -n '187,202p' pkg/ddl/table_mode.rs
    rg -n 'masking|Masking' pkg/ddl/persistent_actions.rs pkg/session/runtime/system_session.rs pkg/session/runtime/ddl.rs pkg/domain/canonical_domain.rs
    tail -14 .plans/2026-10-01-Go逐提交严格对齐Rust/3-提交-d0dfde35b7-严格对齐.md
    tail -14 .plans/2026-10-01-Go逐提交严格对齐Rust/5-提交-52f7a7a3e6-严格对齐.md

Git状态与差异均为空，退出0；精确源码读取退出0；策略检索退出0且唯一命中仍是runtime/ddl.rs:95固定系统表ID映射。persistent_actions::handler_available仍只允许1/3/7/10/17/26/32/39/55/75/76/85/86/93及materialized-view attributes，不含4/6/11/12/14/47；NormalDdlExecutor::step仍对这些未注册活跃action返回明确unavailable错误。当前回归没有消除阻塞，缺口不在格式化或测试资源。

接口和安全替代复核结论沿用已验证的实际边界：JobExecutionContext::query及with_transaction已有真实SQL/共享事务承载；缺的是上述六action在同一worker事务内推进五项策略操作的处理器和生产调用。单独增加SQL helper无真实action消费者；塞进前台Domain.ddl_*会与其自有Begin/Commit拆分事务；恢复mode吞错会重新引入531e40cd25989404f9fd1f51cddf278326983af1已撤销行为。这三条替代均不能满足本提交真实路径/错误身份/副作用要求。完整新增持久action及提交入口必然超出本来源增量范围，此续接指令也明确要求对此保留阻塞，不实施架构补建。

结果：状态保持已阻塞；本提交仍保留的查询错误测试和189/254元数据删除升级场景未取得Rust当前证据。不重复来源Git日志、图caller或已无变化架构阅读，不运行没有消费者的零执行前缀测试，不把任务1–5绿灯当成本任务绿灯。没有代码变更、Rust测试或Ready新证据；只读定向复核不适用代码验证profile。仅追加本编号，未改总计划、prompt、其他任务或生产文件，无主仓库提交。正确性风险仍为五项策略实际worker操作与升级路径未验证；兼容性/性能没有本轮生产改动，未新增相关验证。下一步待对应真实持久action能力提供后在本编号续接；不要求全仓回归，不阻塞无依赖分支。

记录更新后git diff --check作为本轮资料验证；此前架构发现仍保留，不新增实现完成声明。


## 2026-10-02 授权突破与实施里程碑


最新续接授权允许实现五项策略操作直接需要的持久DDL action能力及事务接线，覆盖旧“不实施处理器”停止决定。仍保持当前Go严格缺表错误，不恢复撤销逻辑，不修改Go、不提交主仓库。任务1–5编号已被其他会话删除、任务7记录已有修改，均保留现场，不恢复/覆盖他人文件。

依次实施：A真实worker DROP TABLE三阶段及系统表清理/事务回滚；B TRUNCATE与单/多表RENAME；C DROP COLUMN与MODIFY COLUMN；D189/254升级场景、Go边界核对及Ready。全部完成前状态进行中；局部通过不能算整个提交完成。

实际预备读当前Go table.go完整drop、truncate、rename主函数/辅助函数，column.go drop四阶段与modify_column.go两个策略调用位置；读持久action调度、NormalDdlExecutor statement staging、真实SystemSession及normal_ddl_fixture。新增测试独立runtime/normal_ddl_masking_policy_test.rs，前缀go_commit_96dc7adec0。先用真实Fixture/SQL及persisted queue运行DROP TABLE：Public→WriteOnly→DeleteOnly→None；非空策略删除、目标无数据读取替代；最终阶段缺表仍1146、statement cleanup恢复meta、不误完成queue。预计当前unavailable为实际行为断言失败，先取红灯，再注册处理器。预计划写入pkg/ddl/persistent_masking_actions.rs、persistent_actions.rs、lib.rs、session/runtime.rs及新增测试；后续必要接线发现继续记录，共享根fmt/lint窗口单独执行。


## 2026-10-02 实施续接：真实 DROP TABLE 行为红灯

用户再次要求遇到任务要突破，继续已授权里程碑A，未停在架构缺口报告。新增runtime/normal_ddl_masking_policy_test.rs并注册到session runtime lib test；真实Fixture持久队列的action 4在public阶段触发生产调度器。`cargo test --manifest-path pkg/session/Cargo.toml --lib go_commit_96dc7adec0 -- --nocapture`执行1项，0通过、1失败、0ignored、退出101，断言实际返回normal DDL persistent handler unavailable for action 4，期望Ok(1)，为有效行为红灯。首次不带--lib触发既有tests/system_session.rs:234的crate::runtime无法解析，已用合法实际所属lib目标绕开，不算行为红灯。

正在新增persistent_masking_actions.rs并接入persistent_actions action 4，使用JobExecutionContext真实SQL和共享事务，不新造内存策略链。候选新增写入包括pkg/ddl/Cargo.toml及Cargo.lock（已有workspace astersql-types路径依赖）、job_worker.rs、session/runtime/system_session.rs、create_table_resources.rs；用于14字段解析和TTL/TiFlash/affinity/label真实资源接线。运行中编译修复不作为通过证据。其他会话出现dxf和server handler差异，保留且不修改。

状态继续进行中；A完整回归、B/C持久actions、D升级场景及Ready尚未完成，不删除编号或修改总计划。


里程碑A当前结果：真实worker三项回归通过（3 passed/0 failed/0ignored、退出0）。覆盖非空策略清理、系统表通过真实rename移除后的1146及statement回滚/修复后重试、策略status异常时元数据和策略保留。追加超过错误上限的5次缺表重试先失败（job被取消并移出queue，退出101），随后给NormalDdlExecutor错误上限条件加current.is_rollbackable()，与job_worker.go:937一致，当前3项再次通过。错误身份首次被编码成1105已通过准确系统表SQL错误转换为schema:1146并保留包装前缀修复。A仍待V1/约束/通知历史检查和生产提交接线完整核查；未认定全部任务完成。

里程碑B：真实队列TRUNCATE action11和RENAME action14新增入口回归有效失败（同一命令5项：3通过/2失败、退出101，分别handler unavailable11/14）。正实现单/多表rename两步public→Done和标签/外键引用更新；新增必要pkg/meta/reader.rs::drop_table_only接口，保留rename的auto-ID字段，纳入本编号实际写入范围。TRUNCATE当前还未注册。尚未完成B/C/D及Ready。没有主仓库提交。


## 2026-10-02 突破续接：四类持久 action 与真实升级回归

B 当前真实 TRUNCATE/RENAME 两项已通过，保留事务 SQL 操作、标签/placement/TiFlash/affinity/TTL 边界，尚未证明全部 partition、多表链式 rename、preSplitAndScatter 和前台提交接线，不能据此认定 B 完整。

C 已新增 persistent_drop_column.rs 并注册 action6。使用完整 ColumnInfo 解码 Go V1/V2：原 group2 参数 DTO 只有 ID/Name 且要求大写 JSON ID，实际 Go 元数据导致 missing field ID 并取消真实队列，已用处理器所属完整参数结构保留全部列字段解决；不改测试伪造载荷。覆盖 Public→WriteOnly→DeleteOnly→DeleteReorganization→None、单列索引删除/finished args、默认值、生成列/索引/CHECK/FK/TTL/MLog 约束和 MultiSchema 不可回滚阶段。实际有效红灯先为 action6 unavailable；后续真实参数 missing field ID 红灯，再取得四阶段绿灯。列 ID 从实际 metadata 获取，V1 仅列名参数也可正确清理策略。策略 SELECT 严格限定 table_id AND column_id，无关列 malformed policy 不影响 drop。清理失败后元数据回滚且 job.SchemaState 保留 DeleteReorganization。

D189/254：实际 Go BootstrapKey 与 SQL 版本降级、真实删除系统表 metadata、关闭旧 Domain/池并用共享 Store 新建 Domain，重启 bootstrap 后完整 information_schema.columns 14字段类型/nullability 和 statistics 5行（PRIMARY + 两个 unique index）与 Go helper 一致。初始有效失败发现 canonical 私有 catalog 在同版本覆盖 Go meta 删除，随后发现 Go TableInfo JSON 省略 DBID 导致删除 key 错误；read_catalog 现以存在 DBs 的 Go meta 为准并恢复 DBID=database.ID。原 raw Indices.len=3 断言已纠正为 Go SQL statistics 断言，聚簇主键本就不在普通 Indices 中。新增同目录 canonical_domain_test.rs 回归验证同版本真实删除及 DBID 恢复。

WIP 当前准确验证：

    cargo fmt --all
    cargo test --manifest-path pkg/session/Cargo.toml --lib go_commit_96dc7adec0 -- --nocapture --test-threads=1
    cargo test --manifest-path pkg/session/Cargo.toml --lib go_commit_96dc7adec0_drop_column -- --nocapture --test-threads=1
    cargo test --manifest-path pkg/session/Cargo.toml --lib go_commit_96dc7adec0_upgrade -- --nocapture --test-threads=1
    git diff --check

最近全前缀9项：9passed/0failed/0ignored，退出0；drop_column子集2项2passed；upgrade子集1项通过（内部189/254各一次）；fmt及diff检查退出0。早前7项是6passed/1failed（列参数问题），未用其作为通过证据。Domain 专项新增回归正在验证。main repo未提交，未删除本编号，未改plan.md/prompt.md，他人现场变更保留。

状态继续进行中。下一步必须完成 action12 MODIFY COLUMN 无重组及重组两条真实策略同步路径、前台持久提交接线、完整残留Go failpoint错误边界、TRUNCATE split/scatter/partition和链式rename核查，以及 Ready make lint/fmt check/必要NextGen检查。当前9项绿灯不代表全部提交完成。正确性风险为剩余未接入操作和完整副作用；兼容性风险为V1/多表/partition及完整modify覆盖不足；性能未做负载验证，Go meta优先加载新增扫描需评估缓存边界。以上仅记录当前事实，不把未验证项标完成。

Domain 专项完成：`cargo test --manifest-path pkg/domain/Cargo.toml --lib go_commit_96dc7adec0 -- --nocapture`，1passed/0failed/0ignored，退出0；此前新增测试编译错误为mock事务Delete返回unit误unwrap，已修复，不算行为红灯。最近cargo fmt --all及git diff --check退出0。Ready未执行，整体未完成。
