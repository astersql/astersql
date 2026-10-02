# 任务 226: 提交 94a9cbedab 严格对齐 Rust

执行批次：【批次 60】；文件顺序前驱：1、8、18、47、91、127、135、142、143、151、173、175、180、199、200、201、212、214、215、216、218、222、223、225；任务编号：226。同批可并行，实际能力依赖另行核查。

状态：未开始

目的：逐项核对本Go提交在当前Rust的实际覆盖，只修缺失和偏离，保留完整Go逻辑与测试意图。

来源任务：Go提交 `94a9cbedabbb3190fd892a196dd446df48b7ec6e`；父提交 `01af3568b7e131c9f685f39a77da156b13c48da5`；原始提交标题：ddl: support create materialized view and log (#70789)。

预计会话范围：本提交较大，已提取增删行5767、Go路径55个。保留一个提交一个任务；首次只列函数分段清单，每会话处理一个相关行为组，逐段记录证据并续接同一文件，全部段落完成前不标完成。


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


- Go来源：`br/pkg/stream/rewrite_meta_rawkv_test.go`。
- Go来源：`pkg/ddl/backfilling_txn_executor.go`。
- Go来源：`pkg/ddl/create_table.go`。
- Go来源：`pkg/ddl/ddl_test.go`。
- Go来源：`pkg/ddl/delete_range.go`。
- Go来源：`pkg/ddl/delete_range_test.go`。
- Go来源：`pkg/ddl/executor.go`。
- Go来源：`pkg/ddl/job_submitter_test.go`。
- Go来源：`pkg/ddl/job_worker.go`。
- Go来源：`pkg/ddl/job_worker_test.go`。
- Go来源：`pkg/ddl/jobsubmit/submit.go`。
- Go来源：`pkg/ddl/materialized_view.go`。
- Go来源：`pkg/ddl/mview_schedule_expr.go`。
- Go来源：`pkg/ddl/mview_worker.go`。
- Go来源：`pkg/ddl/mview_worker_test.go`。
- Go来源：`pkg/ddl/reorg.go`。
- Go来源：`pkg/ddl/rollingback.go`。
- Go来源：`pkg/ddl/rollingback_internal_test.go`。
- Go来源：`pkg/ddl/sanity_check.go`。
- Go来源：`pkg/ddl/schema_version.go`。
- Go来源：`pkg/ddl/schematracker/checker.go`。
- Go来源：`pkg/ddl/schematracker/dm_tracker.go`。
- Go来源：`pkg/ddl/schematracker/dm_tracker_test.go`。
- Go来源：`pkg/executor/builder.go`。
- Go来源：`pkg/executor/compiler.go`。
- Go来源：`pkg/executor/ddl.go`。
- Go来源：`pkg/executor/import_into.go`。
- Go来源：`pkg/executor/import_into_test.go`。
- Go来源：`pkg/executor/test/ddl/materialized_view_create_test.go`。
- Go来源：`pkg/expression/helper.go`。
- Go来源：`pkg/infoschema/builder.go`。
- Go来源：`pkg/kv/option.go`。
- Go来源：`pkg/meta/model/bdr.go`。
- Go来源：`pkg/meta/model/job.go`。
- Go来源：`pkg/meta/model/job_args.go`。
- Go来源：`pkg/meta/model/job_args_test.go`。
- Go来源：`pkg/meta/model/job_test.go`。
- Go来源：`pkg/meta/model/table.go`。
- Go来源：`pkg/meta/model/table_test.go`。
- Go来源：`pkg/planner/core/logical_plan_builder.go`。
- Go来源：`pkg/planner/core/planbuilder.go`。
- Go来源：`pkg/planner/core/point_get_plan.go`。
- Go来源：`pkg/planner/core/preprocess.go`。
- Go来源：`pkg/planner/core/util.go`。
- Go来源：`pkg/planner/optimize.go`。
- Go来源：`pkg/session/session.go`。
- Go来源：`pkg/sessionctx/vardef/tidb_vars.go`。
- Go来源：`pkg/sessionctx/variable/session.go`。
- Go来源：`pkg/sessionctx/variable/sysvar.go`。
- Go来源：`pkg/sessionctx/variable/sysvar_test.go`。
- Go来源：`pkg/sessionctx/variable/variable.go`。
- Go来源：`pkg/store/gcworker/gc_worker.go`。
- Go来源：`pkg/store/gcworker/gc_worker_test.go`。
- Go来源：`pkg/util/mviewutil/util.go`。
- Go来源：`pkg/util/sqlexec/restricted_sql_executor.go`。

- 已存在Rust候选（先核对，不代表须修改）：`br/pkg/stream/rewrite_meta_rawkv_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/backfilling_txn_executor.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/create_table.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/ddl_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/delete_range.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/delete_range_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/executor.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/job_submitter_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/job_worker.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/job_worker_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/jobsubmit/submit.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/reorg.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/rollingback.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/sanity_check.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/schema_version.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/schematracker/checker.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/schematracker/dm_tracker.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/schematracker/dm_tracker_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/builder.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/compiler.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/ddl.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/import_into.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/executor/import_into_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/expression/helper.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/infoschema/builder.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/kv/option.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/meta/model/bdr.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/meta/model/job.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/meta/model/job_args.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/meta/model/job_args_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/meta/model/job_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/meta/model/table.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/meta/model/table_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/planner/core/logical_plan_builder.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/planner/core/planbuilder.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/planner/core/point_get_plan.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/planner/core/preprocess.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/planner/core/util.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/planner/optimize.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/session/session.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/vardef/tidb_vars.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/variable/session.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/variable/sysvar.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/variable/sysvar_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/sessionctx/variable/variable.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/store/gcworker/gc_worker.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/store/gcworker/gc_worker_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/util/sqlexec/restricted_sql_executor.rs`。

其他来源文件也必须核查，不默认全部无需Rust同步：

- `M → pkg/ddl/BUILD.bazel`
- `M → pkg/ddl/schematracker/BUILD.bazel`
- `M → pkg/executor/test/ddl/BUILD.bazel`
- `M → pkg/planner/core/BUILD.bazel`
- `M → pkg/store/gcworker/BUILD.bazel`
- `A → pkg/util/mviewutil/BUILD.bazel`

回归测试使用真正拥有crate的独立测试文件；测试名称遵循目标 crate 的现有惯例并描述被验证行为，不使用来源提交哈希或 `go_commit_*` 前缀；已有同名*_test.rs优先扩充，autotests=false时必须核验模块注册。本文件只列实际存在候选，不凭路径相似创建重复实现。

## 上下文


从仓库根执行，先读AGENTS.md、PLANS.md、相关包doc.go；DDL先读docs/agents/ddl/README.md。使用skills/rustcodegraph/SKILL.md定位真实实现；未索引、配置或过期文件再rg。当前已有部分移植，不预设缺失，不继承旧任务完成状态。

## Go变更定位清单


以下是Git差异上下文入口，并非完整行为清单；必须补读完整函数、helper、调用方和前后代码：

- `func TestDeleteRangeForMDDLJob2(t *testing.T) {`。
- `func restoreSessCtx(sessCtx sessionctx.Context) func(sessCtx sessionctx.Context)`。
- `func createTable(w *worker, jobCtx *jobContext, job *model.Job, r autoid.Require`。
- `type autoIDType struct {`。
- `func BuildTableInfoWithLike(ident ast.Ident, referTblInfo *model.TableInfo, s *a`。
- `import (`。
- `func TestSetGlobalIndexVersionFlag(t *testing.T) {`。
- `func insertJobIntoDeleteRangeTable(ctx context.Context, wrapper DelRangeExecWrap`。
- `func doBatchDeleteTablesRange(ctx context.Context, wrapper DelRangeExecWrapper,`。
- `+// Copyright 2026 PingCAP, Inc.`。
- `type Executor interface {`。
- `func (e *executor) CreateTableWithInfo(`。
- `func (e *executor) BatchCreateTableWithInfo(ctx sessionctx.Context,`。
- `func getJobCheckInterval(action model.ActionType, i int) (time.Duration, bool) {`。
- `func TestSubmitJobAfterDDLIsClosed(t *testing.T) {`。
- `func JobNeedGC(job *model.Job) bool {`。
- `var DDLBackfillers = map[model.ActionType]string{`。
- `func (w *worker) runOneJobStep(`。
- `func TestJobNeedGC(t *testing.T) {`。
- `func getRequiredGIDCount(specs []*JobSpec) int {`。
- `func assignGIDsForJobs(specs []*JobSpec, ids []int64) {`。
- `func job2TableIDs(spec *JobSpec) string {`。
- `type reorgCtx struct {`。
- `func (rc *reorgCtx) getRowCount() int64 {`。
- `func (w *worker) runReorgJob(`。
- `func rollingbackAddIndex(jobCtx *jobContext, job *model.Job) (ver int64, err err`。
- `func convertJob2RollbackJob(w *worker, jobCtx *jobContext, job *model.Job) (ver`。
- `func expectedDeleteRangeCnt(ctx delRangeCntCtx, job *model.Job) (int, error) {`。
- `func (e *executor) checkHistoryJobInTest(ctx sessionctx.Context, historyJob *mod`。
- `func SetSchemaDiffForCreateTable(diff *model.SchemaDiff, job *model.Job, jobCtx`。
- `func updateSchemaVersion(jobCtx *jobContext, job *model.Job, multiInfos ...schem`。
- `func (d *Checker) CreateView(ctx sessionctx.Context, stmt *ast.CreateViewStmt) e`。
- `func (d *SchemaTracker) CreateView(ctx sessionctx.Context, s *ast.CreateViewStmt`。
- `PARTITION pCentral VALUES IN (16, 17, 18, 19, 20)`。
- `func (b *executorBuilder) buildInsert(v *physicalop.Insert) exec.Executor {`。
- `func (b *executorBuilder) buildImportInto(v *plannercore.ImportInto) exec.Execut`。
- `func (b *executorBuilder) buildLoadData(v *plannercore.LoadData) exec.Executor {`。
- `func (b *executorBuilder) buildUpdate(v *physicalop.Update) exec.Executor {`。
- `func (b *executorBuilder) buildDelete(v *physicalop.Delete) exec.Executor {`。
- `func (c *Compiler) Compile(ctx context.Context, stmtNode ast.StmtNode) (_ *ExecS`。
- `func (e *DDLExec) Next(ctx context.Context, _ *chunk.Chunk) (err error) {`。
- `func newImportIntoExec(b exec.BaseExecutor, selectExec exec.Executor, userSctx s`。
- `func (e *ImportIntoExec) Next(ctx context.Context, req *chunk.Chunk) (err error)`。
- `func (e *ImportIntoExec) importFromSelect(ctx context.Context) error {`。
- `func TestImportIntoValidateColAssignmentsWithEncodeCtx(t *testing.T) {`。
- `func boolToInt64(v bool) int64 {`。
- `func (b *Builder) getTableIDs(m meta.Reader, diff *model.SchemaDiff) (oldTableID`。
- `func (b *Builder) updateBundleForTableUpdate(diff *model.SchemaDiff, newTableID,`。
- `const (`。
- `var BDRActionMap = map[DDLBDRType][]ActionType{`。
- `var ActionMap = map[ActionType]string{`。
- `func (job *Job) MayNeedReorg() bool {`。
- `func (job *Job) IsRollbackable() bool {`。
- `type SubJob struct {`。
- `func (sub *SubJob) ToProxyJob(parentJob *Job, seq int) Job {`。
- `func (sub *SubJob) FromProxyJob(proxyJob *Job, ver int64) {`。
- `type MultiSchemaInfo struct {`。
- `type TimeZoneLocation struct {`。
- `func GetCreateTableArgs(job *Job) (*CreateTableArgs, error) {`。
- `func TestCreateTableArgs(t *testing.T) {`。
- `func TestJobSize(t *testing.T) {`。
- `func TestMayNeedReorg(t *testing.T) {`。
- `type TableInfo struct {`。
- `func (t *TableInfo) Clone() *TableInfo {`。
- `type ViewInfo struct {`。
- `func TestTTLInfoClone(t *testing.T) {`。
- `func (b *PlanBuilder) buildDataSource(ctx context.Context, tn *ast.TableName, as`。
- `func (b *PlanBuilder) buildUpdateLists(ctx context.Context, tableList []*ast.Tab`。
- `func (b *PlanBuilder) buildDelete(ctx context.Context, ds *ast.DeleteStmt) (base`。
- `func (b *PlanBuilder) buildInsert(ctx context.Context, insert *ast.InsertStmt) (`。
- `func (b *PlanBuilder) buildLoadData(ctx context.Context, ld *ast.LoadDataStmt) (`。
- `func (b *PlanBuilder) buildImportInto(ctx context.Context, ld *ast.ImportIntoStm`。
- `func (b *PlanBuilder) buildDDL(ctx context.Context, node ast.DDLNode) (base.Plan`。
- `func tryWhereIn2BatchPointGet(ctx base.PlanContext, selStmt *ast.SelectStmt, res`。
- `func tryPointGetPlan(ctx base.PlanContext, selStmt *ast.SelectStmt, resolveCtx *`。
- `func tryUpdatePointPlan(ctx base.PlanContext, updateStmt *ast.UpdateStmt, resolv`。
- `func tryDeletePointPlan(ctx base.PlanContext, delStmt *ast.DeleteStmt, resolveCt`。
- `func (p *preprocessor) Enter(in ast.Node) (out ast.Node, skipChildren bool) {`。
- `func (p *preprocessor) Leave(in ast.Node) (out ast.Node, ok bool) {`。
- `func optimizeNoCache(ctx context.Context, sctx sessionctx.Context, node *resolve`。
- `func (s *session) getInternalSession(execOption sqlexec.ExecOption) (*session, f`。
- `type SessionVars struct {`。
- `func NewSessionVars(hctx HookContext) *SessionVars {`。
- `type MemQuota struct {`。
- `var defaultSysVars = []*SysVar{`。
- `func TestTiDBEnableFullOuterJoin(t *testing.T) {`。
- `func doGCPlacementRules(se sessionapi.Session, _ uint64,`。
- `func TestCalcDeleteRangeConcurrency(t *testing.T) {`。
- `type ExecOption struct {`。
- `var ExecOptionUseSessionPool = func(option *ExecOption) {`。

新增Go测试入口：`TestGetJobCheckIntervalForCreateMaterializedView`、`TestIsCreateMaterializedViewBaseCheckCancelledErr`、`TestBuildCreateMaterializedViewRefreshInfoUpsertSQL`、`TestBuildCreateMaterializedViewLogPurgeInfoUpsertSQL`、`TestBuildCreateMaterializedViewImportSQL`、`TestNormalizeMVDefinitionHintDBNames`、`TestDoBatchDeleteTablesRangeSkipsRewrittenIDs`、`TestCreateMaterializedViewLogJobTableIDs`、`TestCreateMaterializedViewJobTableIDs`、`TestCreateMaterializedViewJobTableIDsMultiMLog`、`TestInitCreateMaterializedViewBuildSessionAppliesDefinitionDivPrecisionIncrement`、`TestUpdateMaterializedViewBaseInfoOnCreateMissingBaseTable`、`TestRollingbackCreateMaterializedViewCannotCancelKeepsState`、`TestCreateMaterializedViewLogScheduleExprTypeCheck`、`TestCreateMaterializedViewLogRejectMaterializedObjects`、`TestCreateMaterializedViewLogTruncatesLongPhysicalName`、`TestImportIntoChildSessionInheritsMaintenanceFlag`、`TestCreateMaterializedViewAndLog`、`TestCreateMaterializedViewLogBasic`、`TestCreateMaterializedViewLogPreservesTextColumnTypes`、`TestCreateMaterializedViewLogPreSplitOptions`、`TestCreateMaterializedViewLogPurgeExprTypeValidation`、`TestCreateMaterializedViewLogAccumulationAlert`、`TestCreateMaterializedViewLogPurgeInfoNextUnixSecondsDerivation`、`TestCreateMaterializedViewLogPurgeInfoNextUnixSecondsUsesScheduleTimeZone`、`TestCreateMaterializedViewLogMetaColumnNameConflict`、`TestCreateMaterializedViewLogRejectNonBaseObject`、`TestCreateMaterializedViewLogRejectUnsupportedColumns`、`TestCreateMaterializedViewLogUpdatesPlacementBundle`、`TestCreateMaterializedViewLogAllowsGeneratedColumns`、`TestCreateMaterializedViewLogColumnKeyFlag`、`TestCreateMaterializedViewColumnFlags`、`TestMaterializedViewCommentLength`、`TestCreateMaterializedViewRefreshExprTypeValidation`、`TestCreateMaterializedViewRejectsUnsupportedSelectClauses`、`TestCreateTableLikeShouldNotCarryMaterializedViewMetadata`、`TestCreateMaterializedViewRefreshInfoNextUnixSecondsDerivation`、`TestCreateMaterializedViewRefreshInfoNextUnixSecondsUsesScheduleTimeZone`、`TestCreateMaterializedViewRejectNonBaseObject`、`TestCreateMaterializedViewBuildFailureRollback`、`TestCreateMaterializedViewBuildContextCanceledRollback`、`TestCreateMaterializedViewRollbackIgnoreMissingRefreshInfoTable`、`TestCreateMaterializedViewRefreshInfoUpsertFailureRollback`、`TestCreateMaterializedViewLogPurgeInfoFailureRollback`、`TestCreateMaterializedViewRetryWithResidualBuildRowsRollback`、`TestCreateMaterializedViewRetryAfterUpsertFailure`、`TestCreateMaterializedViewLogPrivilege`、`TestCreateMaterializedViewHistoryJobSchemaVersion`、`TestCreateMaterializedViewCancelRollback`、`TestCreateMaterializedViewRefreshInfoRunningAndSuccess`、`TestCreateMaterializedViewBuildReadTSQueryTypeAlignment`、`TestCreateMaterializedViewLogRejectsDuplicateColumns`、`TestCreateMaterializedViewLogNameLengthByRune`、`TestCreateMaterializedViewSuccessRefreshInfoVisibilityBeforeCommit`、`TestCreateMaterializedViewPauseAndResume`、`TestCreateMaterializedViewRollbackable`、`TestMaterializedViewInfoClone`、`TestTiDBMViewEnable`、`TestGCPlacementRulesForCreateMaterializedViewRollback`

## 测试计划


行为：严格覆盖本提交每项生产变更、错误/边界及Go测试意图，记录每项Go源码位置到Rust实际调用路径的映射。

失败验证：在实际拥有模块的独立测试文件新增按目标 crate 现有惯例和行为语义命名的回归测试，输入、输出、错误及副作用来自本Go差异，使用现有真实组件。具体子测试名及数据在修改生产逻辑前写入本任务覆盖记录。

候选失败和通过命令相同，以下manifest均在制定计划时存在；先确认真正拥有模块，只运行受影响者。若实现已移动，在本文件记录新的真实manifest及原因后再运行，不同时遍历所有crate：

    cargo test --manifest-path br/pkg/stream/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/ddl/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/ddl/jobsubmit/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/ddl/schematracker/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/executor/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/executor/test/ddl/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/expression/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/infoschema/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/kv/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/meta/model/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/planner/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/planner/core/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/session/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/sessionctx/vardef/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/sessionctx/variable/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/store/gcworker/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/util/mviewutil/Cargo.toml go_commit_94a9cbedab -- --nocapture
    cargo test --manifest-path pkg/util/sqlexec/Cargo.toml go_commit_94a9cbedab -- --nocapture

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

    git show --format=fuller --stat 94a9cbedabbb3190fd892a196dd446df48b7ec6e
    git diff --find-renames --unified=30 01af3568b7e131c9f685f39a77da156b13c48da5 94a9cbedabbb3190fd892a196dd446df48b7ec6e
    git log --format='%H %s' 94a9cbedabbb3190fd892a196dd446df48b7ec6e..ad193e964b^2 -- br/pkg/stream/rewrite_meta_rawkv_test.go pkg/ddl/backfilling_txn_executor.go pkg/ddl/create_table.go pkg/ddl/ddl_test.go pkg/ddl/delete_range.go pkg/ddl/delete_range_test.go pkg/ddl/executor.go pkg/ddl/job_submitter_test.go pkg/ddl/job_worker.go pkg/ddl/job_worker_test.go pkg/ddl/jobsubmit/submit.go pkg/ddl/materialized_view.go pkg/ddl/mview_schedule_expr.go pkg/ddl/mview_worker.go pkg/ddl/mview_worker_test.go pkg/ddl/reorg.go pkg/ddl/rollingback.go pkg/ddl/rollingback_internal_test.go pkg/ddl/sanity_check.go pkg/ddl/schema_version.go pkg/ddl/schematracker/checker.go pkg/ddl/schematracker/dm_tracker.go pkg/ddl/schematracker/dm_tracker_test.go pkg/executor/builder.go pkg/executor/compiler.go pkg/executor/ddl.go pkg/executor/import_into.go pkg/executor/import_into_test.go pkg/executor/test/ddl/materialized_view_create_test.go pkg/expression/helper.go pkg/infoschema/builder.go pkg/kv/option.go pkg/meta/model/bdr.go pkg/meta/model/job.go pkg/meta/model/job_args.go pkg/meta/model/job_args_test.go pkg/meta/model/job_test.go pkg/meta/model/table.go pkg/meta/model/table_test.go pkg/planner/core/logical_plan_builder.go pkg/planner/core/planbuilder.go pkg/planner/core/point_get_plan.go pkg/planner/core/preprocess.go pkg/planner/core/util.go pkg/planner/optimize.go pkg/session/session.go pkg/sessionctx/vardef/tidb_vars.go pkg/sessionctx/variable/session.go pkg/sessionctx/variable/sysvar.go pkg/sessionctx/variable/sysvar_test.go pkg/sessionctx/variable/variable.go pkg/store/gcworker/gc_worker.go pkg/store/gcworker/gc_worker_test.go pkg/util/mviewutil/util.go pkg/util/sqlexec/restricted_sql_executor.go

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
