// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Placement Policy 综合测试。
//
// 前半为 Go 侧策略 DDL、继承、GC、PD Bundle（放置规则包）、分区与恢复路径的迁移草稿；
// 后半为可执行用例，覆盖目录状态机、配置校验、引用归一化与 range 规则名解析。

// 职责：placement policy 主测试，覆盖策略 DDL、继承、GC、PD bundle、分区和恢复路径。
// 中文注释按测试入口、辅助函数、断言、资源收尾、事务、failpoint 和外部 IO 位置补充，供后续人工迁移使用。

#![allow(dead_code, unused_variables, non_snake_case)]

// go_step 用于承载原 Go 语句文本，避免误执行数据库、事务或外部依赖动作。
#[allow(dead_code)]
/// 占位：承载原 Go 语句文本，避免误执行数据库或外部依赖。
fn go_step(_source: &str) {}

// bundleCheck 对应 Go 的同名测试辅助结构，字段仅保留 bundle/ID/状态等断言上下文。
#[allow(dead_code)]
/// 草稿：PD Bundle 校验上下文（ID、表 ID、期望 bundle、GC 等待态）。
struct bundleCheckDraft {
    // Go 字段: ID string
    id: String,
    // Go 字段: tableID int64
    table_id: i64,
    // Go 字段: bundle *placement.Bundle
    bundle: String,
    // Go 字段: comment string
    comment: String,
    // Go 字段: waitingGC bool
    waiting_gc: bool,
}

// check 对应 Go 方法 (c *bundleCheck) check，保留接收者携带的测试状态校验语义。
// Go 签名: func (c *bundleCheck) check(t *testing.T, is infoschema.InfoSchema) {
// 参数语义: t *testing.T, is infoschema.InfoSchema
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：check（保留原调用顺序，待接入真实测试框架）。
fn check_go_draft() {

    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: pdGot, err := infosync.GetRuleBundle(context.TODO(), c.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if c.bundle == nil {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: require.True(t, pdGot.IsEmpty(), "bundle should be nil for table: %d, comment: %s", c.tableID, c.comment)
    // Go: } else {
    // Go: expectedJSON, err := json.Marshal(c.bundle)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err, c.comment)
    // Go: pdGotJSON, err := json.Marshal(pdGot)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err, c.comment)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, pdGot, c.comment)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, string(expectedJSON), string(pdGotJSON), c.comment)
    // Go: }
    // Go: isGot, ok := is.PlacementBundleByPhysicalTableID(c.tableID)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if c.bundle == nil || c.waitingGC {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: require.False(t, ok, "bundle should be nil for table: %d, comment: %s", c.tableID, c.comment)
    // Go: } else {
    // Go: expectedJSON, err := json.Marshal(c.bundle)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err, c.comment)
    // Go: isGotJSON, err := json.Marshal(isGot)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err, c.comment)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, isGot, c.comment)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, string(expectedJSON), string(isGotJSON), c.comment)
    // Go: }
}

// checkExistTableBundlesInPD 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func checkExistTableBundlesInPD(t *testing.T, do *domain.Domain, dbName string, tbName string) {
// 参数语义: t *testing.T, do *domain.Domain, dbName string, tbName string
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：check_exist_table_bundles_in_pd（保留原调用顺序，待接入真实测试框架）。
fn check_exist_table_bundles_in_pd_go_draft() {

    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tblInfo, err := do.InfoSchema().TableByName(context.Background(), ast.NewCIStr(dbName), ast.NewCIStr(tbName))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // 事务闭包在 Go 中访问 meta；保留内部事务来源和 mutator 创建顺序。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.NoError(t, kv.RunInNewTxn(ctx, do.Store(), false, func(ctx context.Context, txn kv.Transaction) error {
    // Go: tt := meta.NewMutator(txn)
    // Go: checkTableBundlesInPD(t, do, tt, tblInfo.Meta(), false)
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return nil
    // Go: }))
}

// checkWaitingGCTableBundlesInPD 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func checkWaitingGCTableBundlesInPD(t *testing.T, do *domain.Domain, tblInfo *model.TableInfo) {
// 参数语义: t *testing.T, do *domain.Domain, tblInfo *model.TableInfo
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：check_waiting_gc_table_bundles_in_pd（保留原调用顺序，待接入真实测试框架）。
fn check_waiting_gc_table_bundles_in_pd_go_draft() {

    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // 事务闭包在 Go 中访问 meta；保留内部事务来源和 mutator 创建顺序。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.NoError(t, kv.RunInNewTxn(ctx, do.Store(), false, func(ctx context.Context, txn kv.Transaction) error {
    // Go: tt := meta.NewMutator(txn)
    // Go: checkTableBundlesInPD(t, do, tt, tblInfo, true)
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return nil
    // Go: }))
}

// checkWaitingGCPartitionBundlesInPD 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func checkWaitingGCPartitionBundlesInPD(t *testing.T, do *domain.Domain, partitions []model.PartitionDefinition) {
// 参数语义: t *testing.T, do *domain.Domain, partitions []model.PartitionDefinition
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：check_waiting_gc_partition_bundles_in_pd（保留原调用顺序，待接入真实测试框架）。
fn check_waiting_gc_partition_bundles_in_pd_go_draft() {

    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // 事务闭包在 Go 中访问 meta；保留内部事务来源和 mutator 创建顺序。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.NoError(t, kv.RunInNewTxn(ctx, do.Store(), false, func(ctx context.Context, txn kv.Transaction) error {
    // Go: tt := meta.NewMutator(txn)
    // Go: checkPartitionBundlesInPD(t, do.InfoSchema(), tt, partitions, true)
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return nil
    // Go: }))
}

// checkAllBundlesNotChange 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func checkAllBundlesNotChange(t *testing.T, bundles []*placement.Bundle) {
// 参数语义: t *testing.T, bundles []*placement.Bundle
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：check_all_bundles_not_change（保留原调用顺序，待接入真实测试框架）。
fn check_all_bundles_not_change_go_draft() {

    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: currentBundles, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: bundlesMap := make(map[string]*placement.Bundle)
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, bundle := range currentBundles {
    // Go: bundlesMap[bundle.ID] = bundle
    // Go: }
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, len(currentBundles), len(bundlesMap))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, len(bundles), len(currentBundles))
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, bundle := range bundles {
    // Go: got, ok := bundlesMap[bundle.ID]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: expectedJSON, err := json.Marshal(bundle)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: gotJSON, err := json.Marshal(got)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, string(expectedJSON), string(gotJSON))
    // Go: }
}

// checkPartitionBundlesInPD 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func checkPartitionBundlesInPD(t *testing.T, is infoschema.InfoSchema, tt *meta.Mutator, partitions []model.PartitionDefinition, waitingGC bool) {
// 参数语义: t *testing.T, is infoschema.InfoSchema, tt *meta.Mutator, partitions []model.PartitionDefinition, waitingGC bool
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：check_partition_bundles_in_pd（保留原调用顺序，待接入真实测试框架）。
fn check_partition_bundles_in_pd_go_draft() {

    // Go: checks := make([]*bundleCheck, 0)
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, def := range partitions {
    // Go: bundle, err := placement.NewPartitionBundle(tt, def)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: checks = append(checks, &bundleCheck{
    // Go: ID: placement.GroupID(def.ID),
    // Go: tableID: def.ID,
    // Go: bundle: bundle,
    // Go: comment: fmt.Sprintf("partitionName: %s, physicalID: %d", def.Name, def.ID),
    // Go: waitingGC: waitingGC,
    // Go: })
    // Go: }
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, ck := range checks {
    // Go: ck.check(t, is)
    // Go: }
}

// checkTableBundlesInPD 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func checkTableBundlesInPD(t *testing.T, do *domain.Domain, tt *meta.Mutator, tblInfo *model.TableInfo, waitingGC bool) {
// 参数语义: t *testing.T, do *domain.Domain, tt *meta.Mutator, tblInfo *model.TableInfo, waitingGC bool
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：check_table_bundles_in_pd（保留原调用顺序，待接入真实测试框架）。
fn check_table_bundles_in_pd_go_draft() {

    // Go: is := do.InfoSchema()
    // Go: bundle, err := placement.NewTableBundle(tt, tblInfo)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: tblBundle := &bundleCheck{
    // Go: ID: placement.GroupID(tblInfo.ID),
    // Go: tableID: tblInfo.ID,
    // Go: bundle: bundle,
    // Go: comment: fmt.Sprintf("tableName: %s, physicalID: %d", tblInfo.Name, tblInfo.ID),
    // Go: waitingGC: waitingGC,
    // Go: }
    // Go: tblBundle.check(t, is)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if tblInfo.Partition != nil {
    // Go: pars := tblInfo.Partition.Definitions
    // Go: checkPartitionBundlesInPD(t, is, tt, pars, waitingGC)
    // Go: }
}

// TestPlacementPolicy 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestPlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_placement_policy_body（保留原调用顺序，待接入真实测试框架）。
fn test_placement_policy_body_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // 原 Go 注释: Test for the first time
    // Go: testPlacementPolicy(t)
    // 原 Go 注释: Test again with failpoint.
    // 原 Go 注释: For https://github.com/pingcap/tidb/issues/54796
    // failpoint 注入影响 DDL/InfoSchema 路径；这里只保留触发点和返回值语义。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/infoschema/issyncer/MockTryLoadDiffError", `return("exchangepartition")`)
    // Go: testPlacementPolicy(t)
}

// testPlacementPolicy 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func testPlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_placement_policy（保留原调用顺序，待接入真实测试框架）。
fn test_placement_policy_go_draft() {

    // Go: store := testkit.CreateMockStore(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // Go: var policyID int64
    // failpoint 注入影响 DDL/InfoSchema 路径；这里只保留触发点和返回值语义。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if policyID != 0 {
    // Go: return
    // Go: }
    // 原 Go 注释: job.SchemaID will be assigned when the policy is created.
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if job.SchemaName == "x" && job.Type == model.ActionCreatePlacementPolicy && job.SchemaID != 0 {
    // Go: policyID = job.SchemaID
    // Go: return
    // Go: }
    // Go: })
    // Go: tk.MustExec("create placement policy x " +
    // Go: "LEARNERS=1 " +
    // Go: "LEARNER_CONSTRAINTS=\"[+region=cn-west-1]\" " +
    // Go: "FOLLOWERS=3 " +
    // Go: "FOLLOWER_CONSTRAINTS=\"[+disk=ssd]\"" +
    // Go: "SURVIVAL_PREFERENCES=\"[region, zone]\"")
    // Go: checkFunc := func(policyInfo *model.PolicyInfo) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, true, policyInfo.ID != 0)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "x", policyInfo.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, uint64(3), policyInfo.Followers)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[+disk=ssd]", policyInfo.FollowerConstraints)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, uint64(0), policyInfo.Voters)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "", policyInfo.VoterConstraints)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, uint64(1), policyInfo.Learners)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[+region=cn-west-1]", policyInfo.LearnerConstraints)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, model.StatePublic, policyInfo.State)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "", policyInfo.Schedule)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[region, zone]", policyInfo.SurvivalPreferences)
    // Go: }
    // 原 Go 注释: Check the policy is correctly reloaded in the information schema.
    // Go: po := testGetPolicyByNameFromIS(t, tk.Session(), "x")
    // Go: checkFunc(po)
    // 原 Go 注释: Check the policy is correctly written in the kv meta.
    // Go: po = testGetPolicyByIDFromMeta(t, store, policyID)
    // Go: checkFunc(po)
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("create placement policy x "+
    // Go: "PRIMARY_REGION=\"cn-east-1\" "+
    // Go: "REGIONS=\"cn-east-1,cn-east-2\" ", mysql.ErrPlacementPolicyExists)
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("create placement policy X "+
    // Go: "PRIMARY_REGION=\"cn-east-1\" "+
    // Go: "REGIONS=\"cn-east-1,cn-east-2\" ", mysql.ErrPlacementPolicyExists)
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("create placement policy `X` "+
    // Go: "PRIMARY_REGION=\"cn-east-1\" "+
    // Go: "REGIONS=\"cn-east-1,cn-east-2\" ", mysql.ErrPlacementPolicyExists)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("create placement policy if not exists X " +
    // Go: "PRIMARY_REGION=\"cn-east-1\" " +
    // Go: "REGIONS=\"cn-east-1,cn-east-2\" ")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show warnings").Check(testkit.Rows("Note 8238 Placement policy 'X' already exists"))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundles, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, len(bundles), 0)
    // Go: tk.MustExec("drop placement policy x")
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("drop placement policy x", mysql.ErrPlacementPolicyNotExists)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // 原 Go 注释: nolint:revive,all_revive
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show warnings").Check(testkit.Rows("Note 8239 Unknown placement policy 'x'"))
    // 原 Go 注释: TODO: privilege check & constraint syntax check.
}

// TestCreatePlacementPolicyWithInfo 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestCreatePlacementPolicyWithInfo(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_create_placement_policy_with_info（保留原调用顺序，待接入真实测试框架）。
fn test_create_placement_policy_with_info_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p")
    // Go: tk.MustExec("create placement policy p " +
    // Go: "LEARNERS=1 " +
    // Go: "LEARNER_CONSTRAINTS=\"[+region=cn-west-1]\" " +
    // Go: "FOLLOWERS=3 " +
    // Go: "FOLLOWER_CONSTRAINTS=\"[+disk=ssd]\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec(`CREATE TABLE tp(id int) placement policy p PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100) PLACEMENT POLICY p,
    // Go: PARTITION p1 VALUES LESS THAN (1000))
    // Go: `)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists tp")
    // Go: oldPolicy, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p"))
    // Go: oldPolicy = oldPolicy.Clone()
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // 原 Go 注释: create a non exist policy
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, onExist := range []ddl.OnExist{ddl.OnExistReplace, ddl.OnExistIgnore, ddl.OnExistError} {
    // Go: newPolicy := oldPolicy.Clone()
    // Go: newPolicy.Name = ast.NewCIStr("p2")
    // Go: newPolicy.Followers = 2
    // Go: newPolicy.LearnerConstraints = "[+zone=z2]"
    // Go: tk.Session().SetValue(sessionctx.QueryString, "skip")
    // Go: err := dom.DDLExecutor().CreatePlacementPolicyWithInfo(tk.Session(), newPolicy.Clone(), onExist)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // 原 Go 注释: old policy should not be changed
    // Go: found, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: checkPolicyEquals(t, oldPolicy, found)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: new created policy
    // Go: found, ok = dom.InfoSchema().PolicyByName(ast.NewCIStr("p2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // 原 Go 注释: ID of the created policy should be reassigned
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotEqual(t, newPolicy.ID, found.ID)
    // Go: newPolicy.ID = found.ID
    // Go: checkPolicyEquals(t, newPolicy, found)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go: }
    // 原 Go 注释: create same name policy with on exists error
    // Go: newPolicy := oldPolicy.Clone()
    // Go: newPolicy.ID = oldPolicy.ID + 1
    // Go: tk.Session().SetValue(sessionctx.QueryString, "skip")
    // Go: err := dom.DDLExecutor().CreatePlacementPolicyWithInfo(tk.Session(), newPolicy.Clone(), ddl.OnExistError)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Error(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, infoschema.ErrPlacementPolicyExists.Equal(err))
    // Go: found, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: checkPolicyEquals(t, oldPolicy, found)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: create same name policy with on exist ignore
    // Go: newPolicy = oldPolicy.Clone()
    // Go: newPolicy.ID = oldPolicy.ID + 1
    // Go: tk.Session().SetValue(sessionctx.QueryString, "skip")
    // Go: err = dom.DDLExecutor().CreatePlacementPolicyWithInfo(tk.Session(), newPolicy.Clone(), ddl.OnExistIgnore)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: found, ok = dom.InfoSchema().PolicyByName(ast.NewCIStr("p"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: checkPolicyEquals(t, oldPolicy, found)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: create same name policy with on exist replace
    // Go: newPolicy = oldPolicy.Clone()
    // Go: newPolicy.ID = oldPolicy.ID + 1
    // Go: newPolicy.Followers = 1
    // Go: newPolicy.LearnerConstraints = "[+zone=z1]"
    // Go: tk.Session().SetValue(sessionctx.QueryString, "skip")
    // Go: err = dom.DDLExecutor().CreatePlacementPolicyWithInfo(tk.Session(), newPolicy.Clone(), ddl.OnExistReplace)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: found, ok = dom.InfoSchema().PolicyByName(ast.NewCIStr("p"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // 原 Go 注释: when replace a policy the old policy's id should not be changed
    // Go: newPolicy.ID = oldPolicy.ID
    // Go: checkPolicyEquals(t, newPolicy, found)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
}

// checkPolicyEquals 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func checkPolicyEquals(t *testing.T, expected *model.PolicyInfo, actual *model.PolicyInfo) {
// 参数语义: t *testing.T, expected *model.PolicyInfo, actual *model.PolicyInfo
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：check_policy_equals（保留原调用顺序，待接入真实测试框架）。
fn check_policy_equals_go_draft() {

    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, expected.ID, actual.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, expected.Name, actual.Name)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, *expected.PlacementSettings, *actual.PlacementSettings)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, expected.State, actual.State)
}

// TestPlacementFollowers 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestPlacementFollowers(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_placement_followers（保留原调用顺序，待接入真实测试框架）。
fn test_placement_followers_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store := testkit.CreateMockStore(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists x")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // Go: tk.MustGetErrMsg("create placement policy x FOLLOWERS=99", "invalid placement option: followers should be less than or equal to 8: 99")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // Go: tk.MustExec("create placement policy x FOLLOWERS=4")
    // Go: tk.MustGetErrMsg("alter placement policy x FOLLOWERS=99", "invalid placement option: followers should be less than or equal to 8: 99")
}

// testGetPolicyByIDFromMeta 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func testGetPolicyByIDFromMeta(t *testing.T, store kv.Storage, policyID int64) *model.PolicyInfo {
// 参数语义: t *testing.T, store kv.Storage, policyID int64
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_get_policy_by_id_from_meta（保留原调用顺序，待接入真实测试框架）。
fn test_get_policy_by_id_from_meta_go_draft() {

    // Go: var (
    // Go: policyInfo *model.PolicyInfo
    // Go: err error
    // Go: )
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
    // 事务闭包在 Go 中访问 meta；保留内部事务来源和 mutator 创建顺序。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: err1 := kv.RunInNewTxn(ctx, store, false, func(ctx context.Context, txn kv.Transaction) error {
    // Go: t := meta.NewMutator(txn)
    // Go: policyInfo, err = t.GetPolicy(policyID)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if err != nil {
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return err
    // Go: }
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return nil
    // Go: })
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, err1)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, policyInfo)
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return policyInfo
}

// testGetPolicyByNameFromIS 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func testGetPolicyByNameFromIS(t *testing.T, ctx sessionctx.Context, policy string) *model.PolicyInfo {
// 参数语义: t *testing.T, ctx sessionctx.Context, policy string
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_get_policy_by_name_from_is（保留原调用顺序，待接入真实测试框架）。
fn test_get_policy_by_name_from_is_go_draft() {

    // Go: dom := domain.GetDomain(ctx)
    // 原 Go 注释: Make sure the table schema is the new schema.
    // Go: err := dom.Reload()
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: po, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr(policy))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, true, ok)
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return po
}

// TestPlacementValidation 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestPlacementValidation(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_placement_validation（保留原调用顺序，待接入真实测试框架）。
fn test_placement_validation_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store := testkit.CreateMockStore(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // Go: cases := []struct {
    // Go: name string
    // Go: settings string
    // Go: success bool
    // Go: errmsg string
    // Go: }{
    // Go: {
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: name: "Dict is not allowed for common constraint",
    // Go: settings: "LEARNERS=1 " +
    // Go: "LEARNER_CONSTRAINTS=\"[+zone=cn-west-1]\" " +
    // Go: "CONSTRAINTS=\"{'+disk=ssd':2}\"",
    // Go: success: true,
    // Go: },
    // Go: {
    // Go: name: "constraints may be incompatible with itself",
    // Go: settings: "FOLLOWERS=3 LEARNERS=1 " +
    // Go: "LEARNER_CONSTRAINTS=\"[+zone=cn-west-1, +zone=cn-west-2]\"",
    // Go: errmsg: "invalid label constraints format: should be [constraint1, ...] (error conflicting label constraints: '+zone=cn-west-2' and '+zone=cn-west-1'), {constraint1: cnt1, ...} (error yaml: unmarshal errors:\n" +
    // Go: " line 1: cannot unmarshal !!seq into map[string]int), or any yaml compatible representation: invalid LearnerConstraints",
    // Go: },
    // Go: {
    // Go: settings: "PRIMARY_REGION=\"cn-east-1\" " +
    // Go: "REGIONS=\"cn-east-1,cn-east-2\" ",
    // Go: success: true,
    // Go: },
    // Go: }
    // 原 Go 注释: test for create
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, ca := range cases {
    // Go: sql := fmt.Sprintf("%s %s", "create placement policy x", ca.settings)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if ca.success {
    // Go: tk.MustExec(sql)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // Go: } else {
    // Go: err := tk.ExecToErr(sql)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, err, sql)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.EqualErrorf(t, err, ca.errmsg, ca.name)
    // Go: }
    // Go: }
    // 原 Go 注释: test for alter
    // Go: tk.MustExec("create placement policy x primary_region=\"cn-east-1\" regions=\"cn-east-1,cn-east\"")
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, ca := range cases {
    // Go: sql := fmt.Sprintf("%s %s", "alter placement policy x", ca.settings)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if ca.success {
    // Go: tk.MustExec(sql)
    // Go: tk.MustExec("alter placement policy x primary_region=\"cn-east-1\" regions=\"cn-east-1,cn-east\"")
    // Go: } else {
    // Go: err := tk.ExecToErr(sql)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Error(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, ca.errmsg, err.Error())
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show placement where target='POLICY x'").Check(testkit.Rows("POLICY x PRIMARY_REGION=\"cn-east-1\" REGIONS=\"cn-east-1,cn-east\" NULL"))
    // Go: }
    // Go: }
    // Go: tk.MustExec("drop placement policy x")
}

// TestResetSchemaPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestResetSchemaPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_reset_schema_placement（保留原调用顺序，待接入真实测试框架）。
fn test_reset_schema_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store := testkit.CreateMockStore(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop database if exists TestResetPlacementDB;")
    // Go: tk.MustExec("create placement policy `TestReset` followers=4;")
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("create placement policy `default` followers=4;", mysql.ErrReservedSyntax)
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("create placement policy default followers=4;", mysql.ErrParse)
    // Go: tk.MustExec("create database TestResetPlacementDB placement policy `TestReset`;")
    // Go: tk.MustExec("use TestResetPlacementDB")
    // 原 Go 注释: Test for `=default`
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show create database TestResetPlacementDB`).Check(testkit.RowsWithSep("|",
    // Go: "TestResetPlacementDB CREATE DATABASE `TestResetPlacementDB` /*!40100 DEFAULT CHARACTER SET utf8mb4 */ "+
    // Go: "/*T![placement] PLACEMENT POLICY=`TestReset` */",
    // Go: ))
    // Go: tk.MustExec("ALTER DATABASE TestResetPlacementDB PLACEMENT POLICY=default;")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show create database TestResetPlacementDB`).Check(testkit.RowsWithSep("|",
    // Go: "TestResetPlacementDB CREATE DATABASE `TestResetPlacementDB` /*!40100 DEFAULT CHARACTER SET utf8mb4 */",
    // Go: ))
    // 原 Go 注释: Test for `SET DEFAULT`
    // Go: tk.MustExec("ALTER DATABASE TestResetPlacementDB PLACEMENT POLICY=`TestReset`;")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show create database TestResetPlacementDB`).Check(testkit.RowsWithSep("|",
    // Go: "TestResetPlacementDB CREATE DATABASE `TestResetPlacementDB` /*!40100 DEFAULT CHARACTER SET utf8mb4 */ "+
    // Go: "/*T![placement] PLACEMENT POLICY=`TestReset` */",
    // Go: ))
    // Go: tk.MustExec("ALTER DATABASE TestResetPlacementDB PLACEMENT POLICY SET DEFAULT")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show create database TestResetPlacementDB`).Check(testkit.RowsWithSep("|",
    // Go: "TestResetPlacementDB CREATE DATABASE `TestResetPlacementDB` /*!40100 DEFAULT CHARACTER SET utf8mb4 */",
    // Go: ))
    // 原 Go 注释: Test for `= 'DEFAULT'`
    // Go: tk.MustExec("ALTER DATABASE TestResetPlacementDB PLACEMENT POLICY=`TestReset`;")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show create database TestResetPlacementDB`).Check(testkit.RowsWithSep("|",
    // Go: "TestResetPlacementDB CREATE DATABASE `TestResetPlacementDB` /*!40100 DEFAULT CHARACTER SET utf8mb4 */ "+
    // Go: "/*T![placement] PLACEMENT POLICY=`TestReset` */",
    // Go: ))
    // Go: tk.MustExec("ALTER DATABASE TestResetPlacementDB PLACEMENT POLICY = 'DEFAULT'")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show create database TestResetPlacementDB`).Check(testkit.RowsWithSep("|",
    // Go: "TestResetPlacementDB CREATE DATABASE `TestResetPlacementDB` /*!40100 DEFAULT CHARACTER SET utf8mb4 */",
    // Go: ))
    // 原 Go 注释: Test for "= `DEFAULT`"
    // Go: tk.MustExec("ALTER DATABASE TestResetPlacementDB PLACEMENT POLICY=`TestReset`;")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show create database TestResetPlacementDB`).Check(testkit.RowsWithSep("|",
    // Go: "TestResetPlacementDB CREATE DATABASE `TestResetPlacementDB` /*!40100 DEFAULT CHARACTER SET utf8mb4 */ "+
    // Go: "/*T![placement] PLACEMENT POLICY=`TestReset` */",
    // Go: ))
    // Go: tk.MustExec("ALTER DATABASE TestResetPlacementDB PLACEMENT POLICY = `DEFAULT`")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show create database TestResetPlacementDB`).Check(testkit.RowsWithSep("|",
    // Go: "TestResetPlacementDB CREATE DATABASE `TestResetPlacementDB` /*!40100 DEFAULT CHARACTER SET utf8mb4 */",
    // Go: ))
    // Go: tk.MustExec("drop placement policy `TestReset`;")
    // Go: tk.MustExec("drop database TestResetPlacementDB;")
}

// TestCreateOrReplacePlacementPolicy 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestCreateOrReplacePlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_create_or_replace_placement_policy（保留原调用顺序，待接入真实测试框架）。
fn test_create_or_replace_placement_policy_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // 原 Go 注释: If the policy does not exist, CREATE OR REPLACE PLACEMENT POLICY is the same as CREATE PLACEMENT POLICY
    // Go: tk.MustExec("create or replace placement policy x primary_region=\"cn-east-1\" regions=\"cn-east-1,cn-east\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists x")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create placement policy x").Check(testkit.Rows("x CREATE PLACEMENT POLICY `x` PRIMARY_REGION=\"cn-east-1\" REGIONS=\"cn-east-1,cn-east\""))
    // 原 Go 注释: create a table refers the policy
    // Go: tk.MustExec(`CREATE TABLE tp(id int) placement policy x PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100) PLACEMENT POLICY x,
    // Go: PARTITION p1 VALUES LESS THAN (1000))
    // Go: `)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists tp")
    // 原 Go 注释: If the policy does exist, CREATE OR REPLACE PLACEMENT_POLICY is the same as ALTER PLACEMENT POLICY.
    // Go: tk.MustExec("create or replace placement policy x primary_region=\"cn-east-1\" regions=\"cn-east-1\"")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create placement policy x").Check(testkit.Rows("x CREATE PLACEMENT POLICY `x` PRIMARY_REGION=\"cn-east-1\" REGIONS=\"cn-east-1\""))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: Cannot be used together with the if not exists clause. Ref: https://mariadb.com/kb/en/create-view
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustGetErrMsg("create or replace placement policy if not exists x primary_region=\"cn-east-1\" regions=\"cn-east-1\"", "[ddl:1221]Incorrect usage of OR REPLACE and IF NOT EXISTS")
}

// TestAlterPlacementPolicy 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestAlterPlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_alter_placement_policy（保留原调用顺序，待接入真实测试框架）。
fn test_alter_placement_policy_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // Go: tk.MustExec("create placement policy x primary_region=\"cn-east-1\" regions=\"cn-east-1,cn-east\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists x")
    // 原 Go 注释: create a table ref to policy x, testing for alter policy will update PD bundles
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) placement policy x PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000) placement policy x
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists tp")
    // Go: policy, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("x"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // 原 Go 注释: test for normal cases
    // Go: tk.MustExec("alter placement policy x PRIMARY_REGION=\"bj\" REGIONS=\"bj,sh\"")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show placement where target='POLICY x'").Check(testkit.Rows("POLICY x PRIMARY_REGION=\"bj\" REGIONS=\"bj,sh\" NULL"))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("select * from information_schema.placement_policies where policy_name = 'x'").Check(testkit.Rows(strconv.FormatInt(policy.ID, 10) + " def x bj bj,sh 2 0"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: tk.MustExec("alter placement policy x " +
    // Go: "PRIMARY_REGION=\"bj\" " +
    // Go: "REGIONS=\"bj\" " +
    // Go: "SCHEDULE=\"EVEN\"")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show placement where target='POLICY x'").Check(testkit.Rows("POLICY x PRIMARY_REGION=\"bj\" REGIONS=\"bj\" SCHEDULE=\"EVEN\" NULL"))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("select * from INFORMATION_SCHEMA.PLACEMENT_POLICIES WHERE POLICY_NAME='x'").Check(testkit.Rows(strconv.FormatInt(policy.ID, 10) + " def x bj bj EVEN 2 0"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: tk.MustExec("alter placement policy x " +
    // Go: "LEADER_CONSTRAINTS=\"[+region=us-east-1]\" " +
    // Go: "FOLLOWER_CONSTRAINTS=\"[+region=us-east-2]\" " +
    // Go: "FOLLOWERS=3")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show placement where target='POLICY x'").Check(
    // Go: testkit.Rows("POLICY x LEADER_CONSTRAINTS=\"[+region=us-east-1]\" FOLLOWERS=3 FOLLOWER_CONSTRAINTS=\"[+region=us-east-2]\" NULL"),
    // Go: )
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("SELECT POLICY_NAME,LEADER_CONSTRAINTS,FOLLOWER_CONSTRAINTS,FOLLOWERS FROM information_schema.PLACEMENT_POLICIES WHERE POLICY_NAME = 'x'").Check(
    // Go: testkit.Rows("x [+region=us-east-1] [+region=us-east-2] 3"),
    // Go: )
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: tk.MustExec("alter placement policy x " +
    // Go: "VOTER_CONSTRAINTS=\"[+region=bj]\" " +
    // Go: "LEARNER_CONSTRAINTS=\"[+region=sh]\" " +
    // Go: "CONSTRAINTS=\"[+disk=ssd]\"" +
    // Go: "VOTERS=5 " +
    // Go: "LEARNERS=3")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show placement where target='POLICY x'").Check(
    // Go: testkit.Rows("POLICY x CONSTRAINTS=\"[+disk=ssd]\" VOTERS=5 VOTER_CONSTRAINTS=\"[+region=bj]\" LEARNERS=3 LEARNER_CONSTRAINTS=\"[+region=sh]\" NULL"),
    // Go: )
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("SELECT " +
    // Go: "CATALOG_NAME,POLICY_NAME," +
    // Go: "PRIMARY_REGION,REGIONS,CONSTRAINTS,LEADER_CONSTRAINTS,FOLLOWER_CONSTRAINTS,LEARNER_CONSTRAINTS," +
    // Go: "SCHEDULE,FOLLOWERS,LEARNERS FROM INFORMATION_SCHEMA.placement_policies WHERE POLICY_NAME='x'").Check(
    // Go: testkit.Rows("def x [+disk=ssd] [+region=sh] 2 3"),
    // Go: )
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: test alter not exist policies
    // Go: tk.MustExec("drop table tp")
    // Go: tk.MustExec("drop placement policy x")
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter placement policy x REGIONS=\"bj,sh\"", mysql.ErrPlacementPolicyNotExists)
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter placement policy x2 REGIONS=\"bj,sh\"", mysql.ErrPlacementPolicyNotExists)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("select * from INFORMATION_SCHEMA.PLACEMENT_POLICIES WHERE POLICY_NAME='x'").Check(testkit.Rows())
}

// TestCreateTableWithPlacementPolicy 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestCreateTableWithPlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_create_table_with_placement_policy（保留原调用顺序，待接入真实测试框架）。
fn test_create_table_with_placement_policy_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store := testkit.CreateMockStore(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t,t_range_p,t_hash_p,t_list_p")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists y")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func() {
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t,t_range_p,t_hash_p,t_list_p")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists y")
    // Go: }()
    // 原 Go 注释: special constraints may be incompatible with common constraint.
    // Go: _, err := tk.Exec("create placement policy pn " +
    // Go: "FOLLOWERS=2 " +
    // Go: "FOLLOWER_CONSTRAINTS=\"[+zone=cn-east-1]\" " +
    // Go: "CONSTRAINTS=\"[+disk=ssd,-zone=cn-east-1]\"")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Error(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Regexp(t, ".*conflicting label constraints.*", err.Error())
    // 原 Go 注释: Only placement policy should check the policy existence.
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("create table t(a int)"+
    // Go: "PLACEMENT POLICY=\"x\"", mysql.ErrPlacementPolicyNotExists)
    // Go: tk.MustExec("create placement policy x " +
    // Go: "FOLLOWERS=2 " +
    // Go: "CONSTRAINTS=\"[+disk=ssd]\" ")
    // Go: tk.MustExec("create placement policy z " +
    // Go: "FOLLOWERS=1 " +
    // Go: "SURVIVAL_PREFERENCES=\"[region, zone]\"")
    // Go: tk.MustExec("create placement policy y " +
    // Go: "FOLLOWERS=3 " +
    // Go: "CONSTRAINTS=\"[+region=bj]\" ")
    // Go: tk.MustExec("create table t(a int)" +
    // Go: "PLACEMENT POLICY=\"x\"")
    // Go: tk.MustExec("create table tt(a int)" +
    // Go: "PLACEMENT POLICY=\"z\"")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, TIDB_PLACEMENT_POLICY_NAME FROM information_schema.Tables WHERE TABLE_SCHEMA='test' AND TABLE_NAME = 't'").Check(testkit.Rows(`def test t x`))
    // Go: tk.MustExec("create table t_range_p(id int) placement policy x partition by range(id) (" +
    // Go: "PARTITION p0 VALUES LESS THAN (100)," +
    // Go: "PARTITION p1 VALUES LESS THAN (1000) placement policy y," +
    // Go: "PARTITION p2 VALUES LESS THAN (10000))",
    // Go: )
    // Go: tk.MustExec("create table t_list_p(name varchar(10)) placement policy x partition by list columns(name) (" +
    // Go: "PARTITION p0 VALUES IN ('a', 'b')," +
    // Go: "PARTITION p1 VALUES IN ('c', 'd') placement policy y," +
    // Go: "PARTITION p2 VALUES IN ('e', 'f'))",
    // Go: )
    // Go: tk.MustExec("create table t_hash_p(id int) placement policy x partition by HASH(id) PARTITIONS 4")
    // Go: policyX := testGetPolicyByName(t, tk.Session(), "x", true)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "x", policyX.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, true, policyX.ID != 0)
    // Go: policyY := testGetPolicyByName(t, tk.Session(), "y", true)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "y", policyY.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, true, policyY.ID != 0)
    // Go: policyZ := testGetPolicyByName(t, tk.Session(), "z", true)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "z", policyZ.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, true, policyZ.ID != 0)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[region, zone]", policyZ.SurvivalPreferences)
    // Go: tbl := external.GetTableByName(t, tk, "test", "tt")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl.Meta().PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "z", tbl.Meta().PlacementPolicyRef.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policyZ.ID, tbl.Meta().PlacementPolicyRef.ID)
    // Go: tbl = external.GetTableByName(t, tk, "test", "t")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl.Meta().PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "x", tbl.Meta().PlacementPolicyRef.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policyX.ID, tbl.Meta().PlacementPolicyRef.ID)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t")
    // Go: checkPartitionTableFunc := func(tblName string) {
    // Go: tbl = external.GetTableByName(t, tk, "test", tblName)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl.Meta().PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "x", tbl.Meta().PlacementPolicyRef.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policyX.ID, tbl.Meta().PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl.Meta().Partition)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 3, len(tbl.Meta().Partition.Definitions))
    // Go: p0 := tbl.Meta().Partition.Definitions[0]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, p0.PlacementPolicyRef)
    // Go: p1 := tbl.Meta().Partition.Definitions[1]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, p1.PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "y", p1.PlacementPolicyRef.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policyY.ID, p1.PlacementPolicyRef.ID)
    // Go: p2 := tbl.Meta().Partition.Definitions[2]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, p2.PlacementPolicyRef)
    // Go: }
    // Go: checkPartitionTableFunc("t_range_p")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t_range_p")
    // Go: checkPartitionTableFunc("t_list_p")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t_list_p")
    // Go: tbl = external.GetTableByName(t, tk, "test", "t_hash_p")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl.Meta().PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "x", tbl.Meta().PlacementPolicyRef.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policyX.ID, tbl.Meta().PlacementPolicyRef.ID)
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, p := range tbl.Meta().Partition.Definitions {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, p.PlacementPolicyRef)
    // Go: }
}

// getClonedTable 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func getClonedTable(dom *domain.Domain, dbName string, tableName string) (*model.TableInfo, error) {
// 参数语义: dom *domain.Domain, dbName string, tableName string
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：get_cloned_table（保留原调用顺序，待接入真实测试框架）。
fn get_cloned_table_go_draft() {

    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tbl, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr(dbName), ast.NewCIStr(tableName))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if err != nil {
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return nil, err
    // Go: }
    // Go: tblMeta := tbl.Meta()
    // Go: tblMeta = tblMeta.Clone()
    // Go: policyRef := *tblMeta.PlacementPolicyRef
    // Go: tblMeta.PlacementPolicyRef = &policyRef
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return tblMeta, nil
}

// getClonedDatabase 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func getClonedDatabase(dom *domain.Domain, dbName string) (*model.DBInfo, bool) {
// 参数语义: dom *domain.Domain, dbName string
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：get_cloned_database（保留原调用顺序，待接入真实测试框架）。
fn get_cloned_database_go_draft() {

    // Go: db, ok := dom.InfoSchema().SchemaByName(ast.NewCIStr(dbName))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if !ok {
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return nil, ok
    // Go: }
    // Go: db = db.Clone()
    // Go: policyRef := *db.PlacementPolicyRef
    // Go: db.PlacementPolicyRef = &policyRef
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return db, true
}

// TestCreateTableWithInfoPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestCreateTableWithInfoPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_create_table_with_info_placement（保留原调用顺序，待接入真实测试框架）。
fn test_create_table_with_info_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop database if exists test2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p1 followers=1")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create table t1(a int) placement policy p1")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t1")
    // Go: tk.MustExec("create database test2")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop database if exists test2")
    // Go: tbl, err := getClonedTable(dom, "test", "t1")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: policy, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy.ID, tbl.PlacementPolicyRef.ID)
    // Go: tk.MustExec("alter table t1 placement policy='default'")
    // Go: tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("create placement policy p1 followers=2")
    // Go: tk.Session().SetValue(sessionctx.QueryString, "skip")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, dom.DDLExecutor().CreateTableWithInfo(tk.Session(), ast.NewCIStr("test2"), tbl, nil, ddl.WithOnExist(ddl.OnExistError)))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("t1 CREATE TABLE `t1` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table test2.t1").Check(testkit.Rows("t1 CREATE TABLE `t1` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */"))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show placement where target='TABLE test2.t1'").Check(testkit.Rows("TABLE test2.t1 FOLLOWERS=2 PENDING"))
    // 原 Go 注释: The ref id for new table should be the new policy id
    // Go: tbl2, err := getClonedTable(dom, "test2", "t1")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: policy2, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy2.ID, tbl2.PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, policy2.ID != policy.ID)
    // 原 Go 注释: Test policy not exists
    // Go: tbl2.Name = ast.NewCIStr("t3")
    // Go: tbl2.PlacementPolicyRef.Name = ast.NewCIStr("pxx")
    // Go: tk.Session().SetValue(sessionctx.QueryString, "skip")
    // Go: err = dom.DDLExecutor().CreateTableWithInfo(tk.Session(), ast.NewCIStr("test2"), tbl2, nil, ddl.WithOnExist(ddl.OnExistError))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[schema:8239]Unknown placement policy 'pxx'", err.Error())
}

// TestCreateSchemaWithInfoPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestCreateSchemaWithInfoPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_create_schema_with_info_placement（保留原调用顺序，待接入真实测试框架）。
fn test_create_schema_with_info_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop database if exists test2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop database if exists test3")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p1 followers=1")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create database test2 placement policy p1")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop database if exists test2")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop database if exists test3")
    // Go: db, ok := getClonedDatabase(dom, "test2")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: policy, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy.ID, db.PlacementPolicyRef.ID)
    // Go: db2 := db.Clone()
    // Go: db2.Name = ast.NewCIStr("test3")
    // Go: tk.MustExec("alter database test2 placement policy='default'")
    // Go: tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("create placement policy p1 followers=2")
    // Go: tk.Session().SetValue(sessionctx.QueryString, "skip")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, dom.DDLExecutor().CreateSchemaWithInfo(tk.Session(), db2, ddl.OnExistError))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create database test2").Check(testkit.Rows("test2 CREATE DATABASE `test2` /*!40100 DEFAULT CHARACTER SET utf8mb4 */"))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create database test3").Check(testkit.Rows("test3 CREATE DATABASE `test3` /*!40100 DEFAULT CHARACTER SET utf8mb4 */ /*T![placement] PLACEMENT POLICY=`p1` */"))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show placement where target='DATABASE test3'").Check(testkit.Rows("DATABASE test3 FOLLOWERS=2 SCHEDULED"))
    // 原 Go 注释: The ref id for new table should be the new policy id
    // Go: db2, ok = getClonedDatabase(dom, "test3")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: policy2, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy2.ID, db2.PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, policy2.ID != policy.ID)
    // 原 Go 注释: Test policy not exists
    // Go: db2.Name = ast.NewCIStr("test4")
    // Go: db2.PlacementPolicyRef.Name = ast.NewCIStr("p2")
    // Go: tk.Session().SetValue(sessionctx.QueryString, "skip")
    // Go: err := dom.DDLExecutor().CreateSchemaWithInfo(tk.Session(), db2, ddl.OnExistError)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[schema:8239]Unknown placement policy 'p2'", err.Error())
}

// TestAlterRangePlacementPolicy 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestAlterRangePlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_alter_range_placement_policy（保留原调用顺序，待接入真实测试框架）。
fn test_alter_range_placement_policy_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store := testkit.CreateMockStore(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("create placement policy fiveReplicas followers=4")
    // Go: tk.MustExec("alter range global placement policy fiveReplicas")
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err := infosync.GetRuleBundle(context.TODO(), placement.TiDBBundleRangePrefixForGlobal)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundle.Rules))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 0, len(bundle.Rules[0].LocationLabels))
    // Go: tk.MustExec("alter range meta placement policy fiveReplicas")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show placement;`).Sort().Check(testkit.Rows(
    // Go: "POLICY fiveReplicas FOLLOWERS=4 NULL",
    // Go: "RANGE TiDB_GLOBAL FOLLOWERS=4 PENDING",
    // Go: "RANGE TiDB_META FOLLOWERS=4 PENDING"))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err = infosync.GetRuleBundle(context.TODO(), placement.TiDBBundleRangePrefixForMeta)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundle.Rules))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 0, len(bundle.Rules[0].LocationLabels))
    // 原 Go 注释: Test Issue #51712
    // Go: tk.MustExec("alter placement policy fiveReplicas followers=4 SURVIVAL_PREFERENCES=\"[region]\"")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery(`show placement;`).Sort().Check(testkit.Rows(
    // Go: "POLICY fiveReplicas FOLLOWERS=4 SURVIVAL_PREFERENCES=\"[region]\" NULL",
    // Go: "RANGE TiDB_GLOBAL FOLLOWERS=4 SURVIVAL_PREFERENCES=\"[region]\" PENDING",
    // Go: "RANGE TiDB_META FOLLOWERS=4 SURVIVAL_PREFERENCES=\"[region]\" PENDING"))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err = infosync.GetRuleBundle(context.TODO(), placement.TiDBBundleRangePrefixForGlobal)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundle.Rules))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundle.Rules[0].LocationLabels))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "region", bundle.Rules[0].LocationLabels[0])
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err = infosync.GetRuleBundle(context.TODO(), placement.TiDBBundleRangePrefixForMeta)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundle.Rules))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundle.Rules[0].LocationLabels))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "region", bundle.Rules[0].LocationLabels[0])
    // 原 Go 注释: Test Issue #52257
    // Go: tk.MustExec("create placement policy fiveRepl followers=4 SURVIVAL_PREFERENCES=\"[region]\"")
    // Go: tk.MustExec("drop placement policy fiveRepl")
    // Go: err = tk.ExecToErr("drop placement policy fiveReplicas")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.EqualError(t, err, "[ddl:8241]Placement policy 'fiveReplicas' is still in use")
    // Go: tk.MustExec("alter range global placement policy default")
    // Go: tk.MustExec("alter range meta placement policy default")
    // Go: err = tk.ExecToErr("drop placement policy fiveReplicas")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
}

// TestDropPlacementPolicyInUse 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestDropPlacementPolicyInUse(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_drop_placement_policy_in_use（保留原调用顺序，待接入真实测试框架）。
fn test_drop_placement_policy_in_use_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store := testkit.CreateMockStore(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("create database if not exists test2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists test.t11, test.t12, test2.t21, test2.t21, test2.t22")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p3")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p4")
    // 原 Go 注释: p1 is used by test.t11 and test2.t21
    // Go: tk.MustExec("create placement policy p1 " +
    // Go: "PRIMARY_REGION=\"cn-east-1\" " +
    // Go: "REGIONS=\"cn-east-1, cn-east-2\" " +
    // Go: "SCHEDULE=\"EVEN\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create table test.t11 (id int) placement policy 'p1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists test.t11")
    // Go: tk.MustExec("create table test2.t21 (id int) placement policy 'p1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists test2.t21")
    // 原 Go 注释: p1 is used by test.t12
    // Go: tk.MustExec("create placement policy p2 " +
    // Go: "PRIMARY_REGION=\"cn-east-1\" " +
    // Go: "REGIONS=\"cn-east-1, cn-east-2\" " +
    // Go: "SCHEDULE=\"EVEN\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create table test.t12 (id int) placement policy 'p2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists test.t12")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, TIDB_PLACEMENT_POLICY_NAME FROM information_schema.Tables WHERE TABLE_SCHEMA='test' AND TABLE_NAME = 't12'").Check(testkit.Rows(`def test t12 p2`))
    // 原 Go 注释: p3 is used by test2.t22
    // Go: tk.MustExec("create placement policy p3 " +
    // Go: "PRIMARY_REGION=\"cn-east-1\" " +
    // Go: "REGIONS=\"cn-east-1, cn-east-2\" " +
    // Go: "SCHEDULE=\"EVEN\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p3")
    // Go: tk.MustExec("create table test.t21 (id int) placement policy 'p3'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists test.t21")
    // 原 Go 注释: p4 is used by test_p
    // Go: tk.MustExec("create placement policy p4 " +
    // Go: "PRIMARY_REGION=\"cn-east-1\" " +
    // Go: "REGIONS=\"cn-east-1, cn-east-2\" " +
    // Go: "SCHEDULE=\"EVEN\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p4")
    // Go: tk.MustExec("create database test_p placement policy 'p4'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop database if exists test_p")
    // Go: txn, err := store.Begin()
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func() {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, txn.Rollback())
    // Go: }()
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, policyName := range []string{"p1", "p2", "p3", "p4"} {
    // Go: err := tk.ExecToErr(fmt.Sprintf("drop placement policy %s", policyName))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, fmt.Sprintf("[ddl:8241]Placement policy '%s' is still in use", policyName), err.Error())
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: err = tk.ExecToErr(fmt.Sprintf("drop placement policy if exists %s", policyName))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, fmt.Sprintf("[ddl:8241]Placement policy '%s' is still in use", policyName), err.Error())
    // Go: }
}

// testGetPolicyByName 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func testGetPolicyByName(t *testing.T, ctx sessionctx.Context, name string, mustExist bool) *model.PolicyInfo {
// 参数语义: t *testing.T, ctx sessionctx.Context, name string, mustExist bool
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_get_policy_by_name（保留原调用顺序，待接入真实测试框架）。
fn test_get_policy_by_name_go_draft() {

    // Go: dom := domain.GetDomain(ctx)
    // 原 Go 注释: Make sure the table schema is the new schema.
    // Go: err := dom.Reload()
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: po, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr(name))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if mustExist {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, true, ok)
    // Go: }
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return po
}

// testGetPolicyDependency 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func testGetPolicyDependency(storage kv.Storage, name string) []int64 {
// 参数语义: storage kv.Storage, name string
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_get_policy_dependency（保留原调用顺序，待接入真实测试框架）。
fn test_get_policy_dependency_go_draft() {

    // Go: ids := make([]int64, 0, 32)
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
    // 事务闭包在 Go 中访问 meta；保留内部事务来源和 mutator 创建顺序。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: err1 := kv.RunInNewTxn(ctx, storage, false, func(ctx context.Context, txn kv.Transaction) error {
    // Go: t := meta.NewMutator(txn)
    // Go: dbs, err := t.ListDatabases()
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if err != nil {
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return err
    // Go: }
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, db := range dbs {
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tbls, err := t.ListTables(context.Background(), db.ID)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if err != nil {
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return err
    // Go: }
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, tbl := range tbls {
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if tbl.PlacementPolicyRef != nil && tbl.PlacementPolicyRef.Name.L == name {
    // Go: ids = append(ids, tbl.ID)
    // Go: }
    // Go: }
    // Go: }
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return nil
    // Go: })
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if err1 != nil {
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return []int64{}
    // Go: }
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return ids
}

// TestPolicyCacheAndPolicyDependency 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestPolicyCacheAndPolicyDependency(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_policy_cache_and_policy_dependency（保留原调用顺序，待接入真实测试框架）。
fn test_policy_cache_and_policy_dependency_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store := testkit.CreateMockStore(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // 原 Go 注释: Test policy cache.
    // Go: tk.MustExec("create placement policy x primary_region=\"r1\" regions=\"r1,r2\" schedule=\"EVEN\";")
    // Go: po := testGetPolicyByName(t, tk.Session(), "x", true)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, po)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show placement where target='POLICY x'").Check(testkit.Rows("POLICY x PRIMARY_REGION=\"r1\" REGIONS=\"r1,r2\" SCHEDULE=\"EVEN\" NULL"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t")
    // Go: tk.MustExec("create table t (a int) placement policy \"x\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE, TIDB_PLACEMENT_POLICY_NAME FROM information_schema.Tables WHERE TABLE_SCHEMA='test' AND TABLE_NAME = 't'").Check(testkit.Rows(`def test t BASE TABLE x`))
    // Go: tbl := external.GetTableByName(t, tk, "test", "t")
    // 原 Go 注释: Test policy dependency cache.
    // Go: dependencies := testGetPolicyDependency(store, "x")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, dependencies)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(dependencies))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, tbl.Meta().ID, dependencies[0])
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t2")
    // Go: tk.MustExec("create table t2 (a int) placement policy \"x\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t2")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE, TIDB_PLACEMENT_POLICY_NAME FROM information_schema.Tables WHERE TABLE_SCHEMA='test' AND TABLE_NAME = 't'").Check(testkit.Rows(`def test t BASE TABLE x`))
    // Go: tbl2 := external.GetTableByName(t, tk, "test", "t2")
    // Go: dependencies = testGetPolicyDependency(store, "x")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, dependencies)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 2, len(dependencies))
    // Go: in := func() bool {
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return slices.Contains(dependencies, tbl2.Meta().ID)
    // Go: }
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, true, in())
    // 原 Go 注释: Test drop policy can't succeed cause there are still some table depend on them.
    // Go: tk.MustGetErrMsg("drop placement policy x", "[ddl:8241]Placement policy 'x' is still in use")
    // 原 Go 注释: Drop depended table t firstly.
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t")
    // Go: dependencies = testGetPolicyDependency(store, "x")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, dependencies)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(dependencies))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, tbl2.Meta().ID, dependencies[0])
    // Go: tk.MustGetErrMsg("drop placement policy x", "[ddl:8241]Placement policy 'x' is still in use")
    // 原 Go 注释: Drop depended table t2 secondly.
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t2")
    // Go: dependencies = testGetPolicyDependency(store, "x")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, dependencies)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 0, len(dependencies))
    // Go: po = testGetPolicyByName(t, tk.Session(), "x", true)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, po)
    // Go: tk.MustExec("drop placement policy x")
    // Go: po = testGetPolicyByName(t, tk.Session(), "x", false)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, po)
    // Go: dependencies = testGetPolicyDependency(store, "x")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, dependencies)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 0, len(dependencies))
}

// TestAlterTablePartitionWithPlacementPolicy 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestAlterTablePartitionWithPlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_alter_table_partition_with_placement_policy（保留原调用顺序，待接入真实测试框架）。
fn test_alter_table_partition_with_placement_policy_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func() {
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // Go: }()
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists x")
    // 原 Go 注释: Direct placement option: special constraints may be incompatible with common constraint.
    // Go: tk.MustExec("create table t1 (c int) PARTITION BY RANGE (c) " +
    // Go: "(PARTITION p0 VALUES LESS THAN (6)," +
    // Go: "PARTITION p1 VALUES LESS THAN (11)," +
    // Go: "PARTITION p2 VALUES LESS THAN (16)," +
    // Go: "PARTITION p3 VALUES LESS THAN (21));")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t1")
    // Go: checkExistTableBundlesInPD(t, dom, "test", "t1")
    // 原 Go 注释: Only placement policy should check the policy existence.
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table t1 partition p0 "+
    // Go: "PLACEMENT POLICY=\"x\"", mysql.ErrPlacementPolicyNotExists)
    // Go: tk.MustExec("create placement policy x " +
    // Go: "FOLLOWERS=2 ")
    // Go: tk.MustExec("alter table t1 partition p0 " +
    // Go: "PLACEMENT POLICY=\"x\"")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, PARTITION_NAME, TIDB_PLACEMENT_POLICY_NAME FROM information_schema.Partitions WHERE TABLE_SCHEMA='test' AND TABLE_NAME = 't1' AND PARTITION_NAME = 'p0'").Check(testkit.Rows(`def test t1 p0 x`))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "t1")
    // Go: policyX, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("x"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: ptDef := testGetPartitionDefinitionsByName(t, tk.Session(), "test", "t1", "p0")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, ptDef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, ptDef.PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "x", ptDef.PlacementPolicyRef.Name.L)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policyX.ID, ptDef.PlacementPolicyRef.ID)
}

// testGetPartitionDefinitionsByName 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func testGetPartitionDefinitionsByName(t *testing.T, ctx sessionctx.Context, db string, table string, ptName string) model.PartitionDefinition {
// 参数语义: t *testing.T, ctx sessionctx.Context, db string, table string, ptName string
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_get_partition_definitions_by_name（保留原调用顺序，待接入真实测试框架）。
fn test_get_partition_definitions_by_name_go_draft() {

    // Go: dom := domain.GetDomain(ctx)
    // 原 Go 注释: Make sure the table schema is the new schema.
    // Go: err := dom.Reload()
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tbl, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr(db), ast.NewCIStr(table))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tbl)
    // Go: var ptDef model.PartitionDefinition
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, def := range tbl.Meta().Partition.Definitions {
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if ptName == def.Name.L {
    // Go: ptDef = def
    // Go: break
    // Go: }
    // Go: }
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return ptDef
}

// TestPolicyInheritance 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestPolicyInheritance(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_policy_inheritance（保留原调用顺序，待接入真实测试框架）。
fn test_policy_inheritance_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop database if exists mydb")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func() {
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop database if exists mydb")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go: }()
    // 原 Go 注释: test table inherit database's placement rules.
    // Go: tk.MustExec("create placement policy p1 constraints=\"[+zone=hangzhou]\"")
    // Go: tk.MustExec("create database mydb placement policy p1")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create database mydb").Check(testkit.Rows("mydb CREATE DATABASE `mydb` /*!40100 DEFAULT CHARACTER SET utf8mb4 */ /*T![placement] PLACEMENT POLICY=`p1` */"))
    // Go: tk.MustExec("use mydb")
    // Go: tk.MustExec("create table t(a int)")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t").Check(testkit.Rows("t CREATE TABLE `t` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */"))
    // Go: checkExistTableBundlesInPD(t, dom, "mydb", "t")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t")
    // Go: tk.MustExec("create placement policy p2 constraints=\"[+zone=suzhou]\"")
    // Go: tk.MustExec("create table t(a int) placement policy p2")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t").Check(testkit.Rows("t CREATE TABLE `t` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p2` */"))
    // Go: checkExistTableBundlesInPD(t, dom, "mydb", "t")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t")
    // 原 Go 注释: test create table like should not inherit database's placement rules.
    // Go: tk.MustExec("create table t0 (a int) placement policy 'default'")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t0").Check(testkit.Rows("t0 CREATE TABLE `t0` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    // Go: checkExistTableBundlesInPD(t, dom, "mydb", "t0")
    // Go: tk.MustExec("create table t1 like t0")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("t1 CREATE TABLE `t1` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    // Go: checkExistTableBundlesInPD(t, dom, "mydb", "t1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t0, t")
    // 原 Go 注释: table will inherit db's placement rules, which is shared by all partition as default one.
    // Go: tk.MustExec("create table t(a int) partition by range(a) (partition p0 values less than (100), partition p1 values less than (200))")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t").Check(testkit.Rows("t CREATE TABLE `t` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`a`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (200))"))
    // Go: checkExistTableBundlesInPD(t, dom, "mydb", "t")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t")
    // 原 Go 注释: partition's specified placement rules will override the default one.
    // Go: tk.MustExec("create table t(a int) partition by range(a) (partition p0 values less than (100) placement policy p2, partition p1 values less than (200))")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t").Check(testkit.Rows("t CREATE TABLE `t` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`a`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100) /*T![placement] PLACEMENT POLICY=`p2` */,\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (200))"))
    // Go: checkExistTableBundlesInPD(t, dom, "mydb", "t")
    // Go: tk.MustExec("alter table t last partition less than (400)")
    // Go: tk.MustExec("alter table t first partition less than (200)")
    // Go: err := tk.ExecToErr("alter table t last partition less than (600) PLACEMENT POLICY=`p2`")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Error(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: require.Equal(t, "[parser:1064]You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use line 1 column 54 near \"PLACEMENT POLICY=`p2`\" ", err.Error())
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t").Check(testkit.Rows(
    // Go: "t CREATE TABLE `t` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`a`)\n" +
    // Go: "(PARTITION `p1` VALUES LESS THAN (200),\n" +
    // Go: " PARTITION `P_LT_300` VALUES LESS THAN (300),\n" +
    // Go: " PARTITION `P_LT_400` VALUES LESS THAN (400))"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t")
    // Go: err = tk.ExecToErr("create table t (a int) placement policy = `p2` partition by range(a) INTERVAL (100) first partition less than (100) last partition less than (300) placement policy=`p1`")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Error(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: require.Equal(t, "[parser:1064]You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use line 1 column 156 near \"placement policy=`p1`\" ", err.Error())
    // Go: tk.MustExec("create table t (a int) placement policy = `p2` partition by range(a) INTERVAL (100) first partition less than (100) last partition less than (300)")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t").Check(testkit.Rows(
    // Go: "t CREATE TABLE `t` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p2` */\n" +
    // Go: "PARTITION BY RANGE (`a`)\n" +
    // Go: "(PARTITION `P_LT_100` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `P_LT_200` VALUES LESS THAN (200),\n" +
    // Go: " PARTITION `P_LT_300` VALUES LESS THAN (300))"))
    // 原 Go 注释: test partition override table's placement rules.
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t")
    // Go: tk.MustExec("create table t(a int) placement policy p2 partition by range(a) (partition p0 values less than (100) placement policy p1, partition p1 values less than (200))")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t").Check(testkit.Rows("t CREATE TABLE `t` (\n" +
    // Go: " `a` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p2` */\n" +
    // Go: "PARTITION BY RANGE (`a`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100) /*T![placement] PLACEMENT POLICY=`p1` */,\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (200))"))
    // Go: checkExistTableBundlesInPD(t, dom, "mydb", "t")
}

// TestDatabasePlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestDatabasePlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_database_placement（保留原调用顺序，待接入真实测试框架）。
fn test_database_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop database if exists db2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r2' regions='r1,r2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p2")
    // Go: policy1, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: tk.MustExec(`create database db2`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop database db2")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create database db2").Check(testkit.Rows(
    // Go: "db2 CREATE DATABASE `db2` /*!40100 DEFAULT CHARACTER SET utf8mb4 */",
    // Go: ))
    // Go: policy2, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // 原 Go 注释: alter with policy
    // Go: tk.MustExec("alter database db2 placement policy p1")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create database db2").Check(testkit.Rows(
    // Go: "db2 CREATE DATABASE `db2` /*!40100 DEFAULT CHARACTER SET utf8mb4 */ /*T![placement] PLACEMENT POLICY=`p1` */",
    // Go: ))
    // Go: db, ok := dom.InfoSchema().SchemaByName(ast.NewCIStr("db2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy1.ID, db.PlacementPolicyRef.ID)
    // Go: tk.MustExec("alter database db2 placement policy p2")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create database db2").Check(testkit.Rows(
    // Go: "db2 CREATE DATABASE `db2` /*!40100 DEFAULT CHARACTER SET utf8mb4 */ /*T![placement] PLACEMENT POLICY=`p2` */",
    // Go: ))
    // Go: db, ok = dom.InfoSchema().SchemaByName(ast.NewCIStr("db2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy2.ID, db.PlacementPolicyRef.ID)
    // 原 Go 注释: reset with placement policy 'default'
    // Go: tk.MustExec("alter database db2 placement policy default")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create database db2").Check(testkit.Rows(
    // Go: "db2 CREATE DATABASE `db2` /*!40100 DEFAULT CHARACTER SET utf8mb4 */",
    // Go: ))
    // Go: db, ok = dom.InfoSchema().SchemaByName(ast.NewCIStr("db2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, db.PlacementPolicyRef)
    // 原 Go 注释: error invalid policy
    // Go: err := tk.ExecToErr("alter database db2 placement policy px")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[schema:8239]Unknown placement policy 'px'", err.Error())
    // 原 Go 注释: failed alter has no effect
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create database db2").Check(testkit.Rows(
    // Go: "db2 CREATE DATABASE `db2` /*!40100 DEFAULT CHARACTER SET utf8mb4 */",
    // Go: ))
}

// TestDropDatabaseGCPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestDropDatabaseGCPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_drop_database_gc_placement（保留原调用顺序，待接入真实测试框架）。
fn test_drop_database_gc_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // 原 Go 注释: clearAllBundles(t)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func(originGC bool) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if originGC {
    // Go: util.EmulatorGCEnable()
    // Go: } else {
    // Go: util.EmulatorGCDisable()
    // Go: }
    // Go: }(util.IsEmulatorGCEnable())
    // Go: util.EmulatorGCDisable()
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop database if exists db2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("use test")
    // Go: tk.MustExec("create placement policy p1 primary_region='r0' regions='r0'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create table t (id int) placement policy p1")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t")
    // Go: tk.MustExec("create database db2")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop database if exists db2")
    // Go: tk.MustExec("create table db2.t0 (id int)")
    // Go: tk.MustExec("create table db2.t1 (id int) placement policy p1")
    // Go: tk.MustExec(`create table db2.t2 (id int) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100) placement policy p2,
    // Go: PARTITION p1 VALUES LESS THAN (1000)
    // Go: )`)
    // Go: is := dom.InfoSchema()
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tt, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: tk.MustExec("drop database db2")
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundles, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 4, len(bundles))
    // Go: gcWorker, err := gcworker.NewMockGCWorker(store)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundles, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundles))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, placement.GroupID(tt.Meta().ID), bundles[0].ID)
}

// TestDropTableGCPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestDropTableGCPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_drop_table_gc_placement（保留原调用顺序，待接入真实测试框架）。
fn test_drop_table_gc_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // 原 Go 注释: clearAllBundles(t)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func(originGC bool) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if originGC {
    // Go: util.EmulatorGCEnable()
    // Go: } else {
    // Go: util.EmulatorGCDisable()
    // Go: }
    // Go: }(util.IsEmulatorGCEnable())
    // Go: util.EmulatorGCDisable()
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t0,t1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create placement policy p1 primary_region='r0' regions='r0'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create table t0 (id int)")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t0")
    // Go: tk.MustExec("create table t1 (id int) placement policy p1")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t1")
    // Go: tk.MustExec(`create table t2 (id int) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100) placement policy p2,
    // Go: PARTITION p1 VALUES LESS THAN (1000)
    // Go: )`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t2")
    // Go: is := dom.InfoSchema()
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: t1, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: tk.MustExec("drop table t2")
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundles, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 3, len(bundles))
    // Go: gcWorker, err := gcworker.NewMockGCWorker(store)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundles, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundles))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, placement.GroupID(t1.Meta().ID), bundles[0].ID)
    // Go: bundles = dom.InfoSchema().AllPlacementBundles()
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 1, len(bundles))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, placement.GroupID(t1.Meta().ID), bundles[0].ID)
}

// TestAlterTablePlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestAlterTablePlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_alter_table_placement（保留原调用顺序，待接入真实测试框架）。
fn test_alter_table_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p1")
    // Go: policy, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000)
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop table tp")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: alter with policy
    // Go: tk.MustExec("alter table tp placement policy p1")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tb, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy.ID, tb.Meta().PlacementPolicyRef.ID)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: reset with placement policy 'default'
    // Go: tk.MustExec("alter table tp placement policy default")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: error invalid policy
    // Go: err = tk.ExecToErr("alter table tp placement policy px")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[schema:8239]Unknown placement policy 'px'", err.Error())
    // 原 Go 注释: failed alter has no effect
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
}

// TestDropTablePartitionGCPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestDropTablePartitionGCPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_drop_table_partition_gc_placement（保留原调用顺序，待接入真实测试框架）。
fn test_drop_table_partition_gc_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // 原 Go 注释: clearAllBundles(t)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func(originGC bool) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if originGC {
    // Go: util.EmulatorGCEnable()
    // Go: } else {
    // Go: util.EmulatorGCDisable()
    // Go: }
    // Go: }(util.IsEmulatorGCEnable())
    // Go: util.EmulatorGCDisable()
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t0,t1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p3")
    // Go: tk.MustExec("create placement policy p1 primary_region='r0' regions='r0'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create placement policy p3 primary_region='r2' regions='r2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create table t0 (id int)")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t0")
    // Go: tk.MustExec("create table t1 (id int) placement policy p1")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t1")
    // Go: tk.MustExec(`create table t2 (id int) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100) placement policy p2,
    // Go: PARTITION p1 VALUES LESS THAN (1000) placement policy p3
    // Go: )`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t2")
    // Go: is := dom.InfoSchema()
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: t1, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: t2, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: tk.MustExec("alter table t2 drop partition p0")
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundles, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 4, len(bundles))
    // Go: gcWorker, err := gcworker.NewMockGCWorker(store)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundles, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 3, len(bundles))
    // Go: bundlesMap := make(map[string]*placement.Bundle)
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, bundle := range bundles {
    // Go: bundlesMap[bundle.ID] = bundle
    // Go: }
    // Go: _, ok := bundlesMap[placement.GroupID(t1.Meta().ID)]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: _, ok = bundlesMap[placement.GroupID(t2.Meta().ID)]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: _, ok = bundlesMap[placement.GroupID(t2.Meta().Partition.Definitions[1].ID)]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: bundles = dom.InfoSchema().AllPlacementBundles()
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 3, len(bundles))
    // Go: bundlesMap = make(map[string]*placement.Bundle)
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, bundle := range bundles {
    // Go: bundlesMap[bundle.ID] = bundle
    // Go: }
    // Go: _, ok = bundlesMap[placement.GroupID(t1.Meta().ID)]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: _, ok = bundlesMap[placement.GroupID(t2.Meta().ID)]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: _, ok = bundlesMap[placement.GroupID(t2.Meta().Partition.Definitions[1].ID)]
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
}

// TestAlterTablePartitionPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestAlterTablePartitionPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_alter_table_partition_placement（保留原调用顺序，待接入真实测试框架）。
fn test_alter_table_partition_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // 原 Go 注释: clearAllBundles(t)
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p0")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p0 primary_region='r0' regions='r0'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p0")
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p1")
    // Go: policy, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) placement policy p0 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000)
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop table tp")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p0` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: alter with policy
    // Go: tk.MustExec("alter table tp partition p0 placement policy p1")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p0` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100) /*T![placement] PLACEMENT POLICY=`p1` */,\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tb, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy.ID, tb.Meta().Partition.Definitions[0].PlacementPolicyRef.ID)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: reset with placement policy 'default'
    // Go: tk.MustExec("alter table tp partition p1 placement policy default")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p0` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100) /*T![placement] PLACEMENT POLICY=`p1` */,\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: tk.MustExec("alter table tp partition p0 placement policy default")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p0` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: error invalid policy
    // Go: err = tk.ExecToErr("alter table tp partition p1 placement policy px")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[schema:8239]Unknown placement policy 'px'", err.Error())
    // 原 Go 注释: error invalid partition name
    // Go: err = tk.ExecToErr("alter table tp partition p2 placement policy p1")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[table:1735]Unknown partition 'p2' in table 'tp'", err.Error())
    // 原 Go 注释: failed alter has no effect
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p0` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: tk.MustExec(`alter table tp reorganize partition p1 into (partition p1 values less than (750) placement policy p1, partition p2 values less than (1500) placement policy p0)`)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p0` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (750) /*T![placement] PLACEMENT POLICY=`p1` */,\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (1500) /*T![placement] PLACEMENT POLICY=`p0` */)"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
}

// TestAddPartitionWithPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestAddPartitionWithPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_add_partition_with_placement（保留原调用顺序，待接入真实测试框架）。
fn test_add_partition_with_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // 原 Go 注释: clearAllBundles(t)
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r2' regions='r2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p2")
    // Go: policy2, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) PLACEMENT POLICY p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000)
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop table tp")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: Add partitions
    // Go: tk.MustExec(`alter table tp add partition (
    // Go: partition p2 values less than (10000) placement policy p2,
    // Go: partition p3 values less than (100000),
    // Go: partition p4 values less than (1000000) placement policy default
    // Go: )`)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000),\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (10000) /*T![placement] PLACEMENT POLICY=`p2` */,\n" +
    // Go: " PARTITION `p3` VALUES LESS THAN (100000),\n" +
    // Go: " PARTITION `p4` VALUES LESS THAN (1000000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tb, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy2.ID, tb.Meta().Partition.Definitions[2].PlacementPolicyRef.ID)
    // 原 Go 注释: error invalid policy
    // Go: err = tk.ExecToErr("alter table tp add partition (partition p5 values less than (10000000) placement policy px)")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, "[schema:8239]Unknown placement policy 'px'", err.Error())
    // 原 Go 注释: failed alter has no effect
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000),\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (10000) /*T![placement] PLACEMENT POLICY=`p2` */,\n" +
    // Go: " PARTITION `p3` VALUES LESS THAN (100000),\n" +
    // Go: " PARTITION `p4` VALUES LESS THAN (1000000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
}

// TestTruncateTableWithPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestTruncateTableWithPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_truncate_table_with_placement（保留原调用顺序，待接入真实测试框架）。
fn test_truncate_table_with_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func(originGC bool) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if originGC {
    // Go: util.EmulatorGCEnable()
    // Go: } else {
    // Go: util.EmulatorGCDisable()
    // Go: }
    // Go: }(util.IsEmulatorGCEnable())
    // Go: util.EmulatorGCDisable()
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t1, tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r2' regions='r2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p2")
    // Go: policy1, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: policy2, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: tk.MustExec(`CREATE TABLE t1 (id INT) placement policy p1`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop table t1")
    // 原 Go 注释: test for normal table
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */"))
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: t1, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "t1")
    // Go: tk.MustExec("TRUNCATE TABLE t1")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */"))
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: newT1, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, newT1.Meta().ID != t1.Meta().ID)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "t1")
    // Go: checkWaitingGCTableBundlesInPD(t, dom, t1.Meta())
    // 原 Go 注释: test for partitioned table
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000) placement policy p2,
    // Go: PARTITION p2 VALUES LESS THAN (10000)
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop table tp")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tp, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy1.ID, tp.Meta().PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy2.ID, tp.Meta().Partition.Definitions[1].PlacementPolicyRef.ID)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000) /*T![placement] PLACEMENT POLICY=`p2` */,\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (10000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: tk.MustExec("TRUNCATE TABLE tp")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: newTp, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, newTp.Meta().ID != tp.Meta().ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy1.ID, newTp.Meta().PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy2.ID, newTp.Meta().Partition.Definitions[1].PlacementPolicyRef.ID)
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for i := range []int{0, 1, 2} {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, newTp.Meta().Partition.Definitions[i].ID != tp.Meta().Partition.Definitions[i].ID)
    // Go: }
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: checkWaitingGCTableBundlesInPD(t, dom, tp.Meta())
    // 原 Go 注释: do GC
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err := infosync.GetRuleBundle(context.TODO(), placement.GroupID(t1.Meta().ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.False(t, bundle.IsEmpty())
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err = infosync.GetRuleBundle(context.TODO(), placement.GroupID(tp.Meta().ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.False(t, bundle.IsEmpty())
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, def := range tp.Meta().Partition.Definitions {
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err = infosync.GetRuleBundle(context.TODO(), placement.GroupID(def.ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if def.PlacementPolicyRef != nil {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.False(t, bundle.IsEmpty())
    // Go: } else {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, bundle.IsEmpty())
    // Go: }
    // Go: }
    // Go: gcWorker, err := gcworker.NewMockGCWorker(store)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "t1")
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err = infosync.GetRuleBundle(context.TODO(), placement.GroupID(t1.Meta().ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, bundle.IsEmpty())
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err = infosync.GetRuleBundle(context.TODO(), placement.GroupID(tp.Meta().ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, bundle.IsEmpty())
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, def := range tp.Meta().Partition.Definitions {
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err = infosync.GetRuleBundle(context.TODO(), placement.GroupID(def.ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, bundle.IsEmpty())
    // Go: }
}

// TestTruncateTablePartitionWithPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestTruncateTablePartitionWithPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_truncate_table_partition_with_placement（保留原调用顺序，待接入真实测试框架）。
fn test_truncate_table_partition_with_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func(originGC bool) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if originGC {
    // Go: util.EmulatorGCEnable()
    // Go: } else {
    // Go: util.EmulatorGCDisable()
    // Go: }
    // Go: }(util.IsEmulatorGCEnable())
    // Go: util.EmulatorGCDisable()
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p3")
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r2' regions='r2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p2")
    // Go: tk.MustExec("create placement policy p3 primary_region='r3' regions='r3'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p3")
    // Go: policy1, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: policy2, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p2"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: policy3, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p3"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // 原 Go 注释: test for partitioned table
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000) placement policy p2,
    // Go: PARTITION p2 VALUES LESS THAN (10000) placement policy p3,
    // Go: PARTITION p3 VALUES LESS THAN (100000)
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop table tp")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tp, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: checkOldPartitions := make([]model.PartitionDefinition, 0, 2)
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, p := range tp.Meta().Partition.Definitions {
    // Go: switch p.Name.L {
    // Go: case "p1":
    // Go: checkOldPartitions = append(checkOldPartitions, p.Clone())
    // Go: case "p3":
    // Go: p.PlacementPolicyRef = tp.Meta().PlacementPolicyRef
    // Go: checkOldPartitions = append(checkOldPartitions, p.Clone())
    // Go: }
    // Go: }
    // Go: tk.MustExec("ALTER TABLE tp TRUNCATE partition p1,p3")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: newTp, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, tp.Meta().ID, newTp.Meta().ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy1.ID, newTp.Meta().PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 4, len(newTp.Meta().Partition.Definitions))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, newTp.Meta().Partition.Definitions[0].PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy2.ID, newTp.Meta().Partition.Definitions[1].PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy3.ID, newTp.Meta().Partition.Definitions[2].PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, newTp.Meta().Partition.Definitions[3].PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, tp.Meta().Partition.Definitions[0].ID, newTp.Meta().Partition.Definitions[0].ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, newTp.Meta().Partition.Definitions[1].ID != tp.Meta().Partition.Definitions[1].ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, tp.Meta().Partition.Definitions[2].ID, newTp.Meta().Partition.Definitions[2].ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, newTp.Meta().Partition.Definitions[3].ID != tp.Meta().Partition.Definitions[3].ID)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000) /*T![placement] PLACEMENT POLICY=`p2` */,\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (10000) /*T![placement] PLACEMENT POLICY=`p3` */,\n" +
    // Go: " PARTITION `p3` VALUES LESS THAN (100000))"))
    // Go: dom.Reload()
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: checkWaitingGCPartitionBundlesInPD(t, dom, checkOldPartitions)
    // 原 Go 注释: add new partition will not override bundle waiting for GC
    // Go: tk.MustExec("alter table tp add partition (partition p4 values less than(1000000))")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: newTp2, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 5, len(newTp2.Meta().Partition.Definitions))
    // Go: checkWaitingGCPartitionBundlesInPD(t, dom, checkOldPartitions)
    // 原 Go 注释: do GC
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, par := range checkOldPartitions {
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err := infosync.GetRuleBundle(context.TODO(), placement.GroupID(par.ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.False(t, bundle.IsEmpty())
    // Go: }
    // Go: gcWorker, err := gcworker.NewMockGCWorker(store)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, par := range checkOldPartitions {
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err := infosync.GetRuleBundle(context.TODO(), placement.GroupID(par.ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, bundle.IsEmpty())
    // Go: }
}

// TestDropTableWithPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestDropTableWithPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_drop_table_with_placement（保留原调用顺序，待接入真实测试框架）。
fn test_drop_table_with_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func(originGC bool) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if originGC {
    // Go: util.EmulatorGCEnable()
    // Go: } else {
    // Go: util.EmulatorGCDisable()
    // Go: }
    // Go: }(util.IsEmulatorGCEnable())
    // Go: util.EmulatorGCDisable()
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p3")
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r2' regions='r2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p2")
    // Go: tk.MustExec("create placement policy p3 primary_region='r3' regions='r3'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p3")
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000) placement policy p2,
    // Go: PARTITION p2 VALUES LESS THAN (10000) placement policy p3,
    // Go: PARTITION p3 VALUES LESS THAN (100000)
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists tp")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tp, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: tk.MustExec("drop table tp")
    // Go: checkWaitingGCTableBundlesInPD(t, dom, tp.Meta())
    // 原 Go 注释: do GC
    // Go: gcWorker, err := gcworker.NewMockGCWorker(store)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundles, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 0, len(bundles))
}

// TestDropPartitionWithPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestDropPartitionWithPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_drop_partition_with_placement（保留原调用顺序，待接入真实测试框架）。
fn test_drop_partition_with_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func(originGC bool) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if originGC {
    // Go: util.EmulatorGCEnable()
    // Go: } else {
    // Go: util.EmulatorGCDisable()
    // Go: }
    // Go: }(util.IsEmulatorGCEnable())
    // Go: util.EmulatorGCDisable()
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p3")
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r2' regions='r2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p2")
    // Go: tk.MustExec("create placement policy p3 primary_region='r3' regions='r3'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop placement policy p3")
    // Go: policy1, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: policy3, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("p3"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // 原 Go 注释: test for partitioned table
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000) placement policy p2,
    // Go: PARTITION p2 VALUES LESS THAN (10000) placement policy p3,
    // Go: PARTITION p3 VALUES LESS THAN (100000)
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer tk.MustExec("drop table tp")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tp, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: checkOldPartitions := make([]model.PartitionDefinition, 0, 2)
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, p := range tp.Meta().Partition.Definitions {
    // Go: switch p.Name.L {
    // Go: case "p1":
    // Go: checkOldPartitions = append(checkOldPartitions, p.Clone())
    // Go: case "p3":
    // Go: p.PlacementPolicyRef = tp.Meta().PlacementPolicyRef
    // Go: checkOldPartitions = append(checkOldPartitions, p.Clone())
    // Go: }
    // Go: }
    // Go: tk.MustExec("ALTER TABLE tp DROP partition p1,p3")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: newTp, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, tp.Meta().ID, newTp.Meta().ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy1.ID, newTp.Meta().PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 2, len(newTp.Meta().Partition.Definitions))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Nil(t, newTp.Meta().Partition.Definitions[0].PlacementPolicyRef)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy3.ID, newTp.Meta().Partition.Definitions[1].PlacementPolicyRef.ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, tp.Meta().Partition.Definitions[0].ID, newTp.Meta().Partition.Definitions[0].ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, newTp.Meta().Partition.Definitions[1].ID == tp.Meta().Partition.Definitions[2].ID)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: checkWaitingGCPartitionBundlesInPD(t, dom, checkOldPartitions)
    // 原 Go 注释: add new partition will not override bundle waiting for GC
    // Go: tk.MustExec("alter table tp add partition (partition p4 values less than(1000000))")
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: newTp2, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, 3, len(newTp2.Meta().Partition.Definitions))
    // Go: checkWaitingGCPartitionBundlesInPD(t, dom, checkOldPartitions)
    // 原 Go 注释: do GC
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, par := range checkOldPartitions {
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err := infosync.GetRuleBundle(context.TODO(), placement.GroupID(par.ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.False(t, bundle.IsEmpty())
    // Go: }
    // Go: gcWorker, err := gcworker.NewMockGCWorker(store)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, par := range checkOldPartitions {
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundle, err := infosync.GetRuleBundle(context.TODO(), placement.GroupID(par.ID))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, bundle.IsEmpty())
    // Go: }
}

// TestExchangePartitionWithPlacement 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestExchangePartitionWithPlacement(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_exchange_partition_with_placement（保留原调用顺序，待接入真实测试框架）。
fn test_exchange_partition_with_placement_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // Go: tk.MustExec("create placement policy pp1 primary_region='r1' regions='r1'")
    // Go: tk.MustExec("create placement policy pp2 primary_region='r2' regions='r2'")
    // Go: tk.MustExec("create placement policy pp3 primary_region='r3' regions='r3'")
    // Go: policy1, ok := dom.InfoSchema().PolicyByName(ast.NewCIStr("pp1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, ok)
    // Go: tk.MustExec(`CREATE TABLE t1 (id INT) placement policy pp1`)
    // Go: tk.MustExec(`CREATE TABLE t2 (id INT)`)
    // Go: tk.MustExec(`CREATE TABLE t3 (id INT) placement policy pp3`)
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: t1, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: t1ID := t1.Meta().ID
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) placement policy pp3 PARTITION BY RANGE (id) (
    // Go: PARTITION p1 VALUES LESS THAN (100) placement policy pp1,
    // Go: PARTITION p2 VALUES LESS THAN (1000) placement policy pp2,
    // Go: PARTITION p3 VALUES LESS THAN (10000)
    // Go: )`)
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tp, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: tpID := tp.Meta().ID
    // Go: par0ID := tp.Meta().Partition.Definitions[0].ID
    // 原 Go 注释: exchange par1, t1
    // Go: tk.MustExec("alter table tp exchange partition p1 with table t1")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */"))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("" +
    // Go: "tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp3` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p1` VALUES LESS THAN (100) /*T![placement] PLACEMENT POLICY=`pp1` */,\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (1000) /*T![placement] PLACEMENT POLICY=`pp2` */,\n" +
    // Go: " PARTITION `p3` VALUES LESS THAN (10000))"))
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: tp, err = dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("tp"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, tpID, tp.Meta().ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, t1ID, tp.Meta().Partition.Definitions[0].ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NotNil(t, tp.Meta().Partition.Definitions[0].PlacementPolicyRef)
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: t1, err = dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t1"))
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, par0ID, t1.Meta().ID)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Equal(t, policy1.ID, t1.Meta().PlacementPolicyRef.ID)
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // 原 Go 注释: exchange par2, t1
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table tp exchange partition p2 with table t1", mysql.ErrTablesDifferentMetadata)
    // 原 Go 注释: exchange par3, t1
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table tp exchange partition p3 with table t1", mysql.ErrTablesDifferentMetadata)
    // 原 Go 注释: exchange par1, t2
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table tp exchange partition p1 with table t2", mysql.ErrTablesDifferentMetadata)
    // 原 Go 注释: exchange par2, t2
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table tp exchange partition p2 with table t2", mysql.ErrTablesDifferentMetadata)
    // 原 Go 注释: exchange par3, t2
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table tp exchange partition p3 with table t2", mysql.ErrTablesDifferentMetadata)
    // 原 Go 注释: exchange par1, t3
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table tp exchange partition p1 with table t3", mysql.ErrTablesDifferentMetadata)
    // 原 Go 注释: exchange par2, t3
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table tp exchange partition p2 with table t3", mysql.ErrTablesDifferentMetadata)
    // 原 Go 注释: exchange par3, t3
    // Go: tk.MustExec("alter table tp exchange partition p3 with table t3")
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp")
    // Go: checkExistTableBundlesInPD(t, dom, "test", "t3")
}

// TestPDFail 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestPDFail(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_pd_fail（保留原调用顺序，待接入真实测试框架）。
fn test_pd_fail_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func() {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/domain/infosync/putRuleBundlesError"))
    // Go: }()
    // Go: store := testkit.CreateMockStore(t)
    // 原 Go 注释: clearAllBundles(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists t1, t2, tp")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create placement policy p1 primary_region=\"cn-east-1\" regions=\"cn-east-1,cn-east\"")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p2 followers=1")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create table t1(id int)")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists t1")
    // Go: tk.MustExec(`CREATE TABLE tp (id INT) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100),
    // Go: PARTITION p1 VALUES LESS THAN (1000) placement policy p1
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists tp")
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: existBundles, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/domain/infosync/putRuleBundlesError", "return(true)"))
    // 原 Go 注释: alter policy
    // Go: err = tk.ExecToErr("alter placement policy p1 primary_region='rx' regions='rx'")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // Go: require.True(t, infosync.ErrHTTPServiceError.Equal(err))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create placement policy p1").Check(testkit.Rows("p1 CREATE PLACEMENT POLICY `p1` PRIMARY_REGION=\"cn-east-1\" REGIONS=\"cn-east-1,cn-east\""))
    // Go: checkAllBundlesNotChange(t, existBundles)
    // 原 Go 注释: create table
    // Go: err = tk.ExecToErr("create table t2 (id int) placement policy p1")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // Go: require.True(t, infosync.ErrHTTPServiceError.Equal(err))
    // Go: err = tk.ExecToErr("show create table t2")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.True(t, infoschema.ErrTableNotExists.Equal(err))
    // Go: checkAllBundlesNotChange(t, existBundles)
    // 原 Go 注释: alter table
    // Go: err = tk.ExecToErr("alter table t1 placement policy p1")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // Go: require.True(t, infosync.ErrHTTPServiceError.Equal(err))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    // Go: checkAllBundlesNotChange(t, existBundles)
    // 原 Go 注释: add partition
    // Go: err = tk.ExecToErr("alter table tp add partition (" +
    // Go: "partition p2 values less than (10000) placement policy p1," +
    // Go: "partition p3 values less than (100000)" +
    // Go: ")")
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // Go: require.True(t, infosync.ErrHTTPServiceError.Equal(err))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000) /*T![placement] PLACEMENT POLICY=`p1` */)"))
    // Go: checkAllBundlesNotChange(t, existBundles)
    // 原 Go 注释: alter partition
    // Go: err = tk.ExecToErr(`alter table tp PARTITION p1 placement policy p2`)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // Go: require.True(t, infosync.ErrHTTPServiceError.Equal(err))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000) /*T![placement] PLACEMENT POLICY=`p1` */)"))
    // Go: checkAllBundlesNotChange(t, existBundles)
    // 原 Go 注释: exchange partition
    // 错误码断言对应 Go 的 MustGetErrCode，保留预期 errno 语义。
    // Go: tk.MustGetErrCode("alter table tp exchange partition p1 with table t1", mysql.ErrTablesDifferentMetadata)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp").Check(testkit.Rows("tp CREATE TABLE `tp` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`p1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000) /*T![placement] PLACEMENT POLICY=`p1` */)"))
    // Go: checkAllBundlesNotChange(t, existBundles)
}

// TestRecoverTableWithPlacementPolicy 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestRecoverTableWithPlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_recover_table_with_placement_policy（保留原调用顺序，待接入真实测试框架）。
fn test_recover_table_with_placement_policy_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // 原 Go 注释: clearAllBundles(t)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // Go: defer func(originGC bool) {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // failpoint 依赖外部注入框架；不启用真实 failpoint。
    // Go: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if originGC {
    // Go: util.EmulatorGCEnable()
    // Go: } else {
    // Go: util.EmulatorGCDisable()
    // Go: }
    // Go: }(util.IsEmulatorGCEnable())
    // Go: util.EmulatorGCDisable()
    // Go: store, dom := testkit.CreateMockStoreAndDomain(t)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p1")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p2")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop placement policy if exists p3")
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: tk.MustExec("drop table if exists tp1, tp2")
    // Go: safePointSQL := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '')
    // Go: ON DUPLICATE KEY
    // Go: UPDATE variable_value = '%[1]s'`
    // Go: tk.MustExec(fmt.Sprintf(safePointSQL, "20060102-15:04:05 -0700 MST"))
    // Go: tk.MustExec("create placement policy p1 primary_region='r1' regions='r1,r2'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p1")
    // Go: tk.MustExec("create placement policy p2 primary_region='r2' regions='r2,r3'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p2")
    // Go: tk.MustExec("create placement policy p3 primary_region='r3' regions='r3,r4'")
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop placement policy if exists p3")
    // 原 Go 注释: test recover
    // Go: tk.MustExec(`CREATE TABLE tp1 (id INT) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100) placement policy p2,
    // Go: PARTITION p1 VALUES LESS THAN (1000),
    // Go: PARTITION p2 VALUES LESS THAN (10000) placement policy p3
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists tp1")
    // Go: tk.MustExec("drop table tp1")
    // Go: tk.MustExec("recover table tp1")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp1").Check(testkit.Rows("tp1 CREATE TABLE `tp1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000),\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (10000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp1")
    // 原 Go 注释: test flashback
    // Go: tk.MustExec(`CREATE TABLE tp2 (id INT) placement policy p1 PARTITION BY RANGE (id) (
    // Go: PARTITION p0 VALUES LESS THAN (100) placement policy p2,
    // Go: PARTITION p1 VALUES LESS THAN (1000),
    // Go: PARTITION p2 VALUES LESS THAN (10000) placement policy p3
    // Go: );`)
    // Go defer 表示测试收尾动作；只记录清理顺序，不真正注册析构回调。
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: defer tk.MustExec("drop table if exists tp2")
    // Go: tk.MustExec("drop table tp1")
    // Go: tk.MustExec("drop table tp2")
    // Go: tk.MustExec("flashback table tp2")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp2").Check(testkit.Rows("tp2 CREATE TABLE `tp2` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000),\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (10000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp2")
    // 原 Go 注释: test recover after police drop
    // Go: tk.MustExec("drop table tp2")
    // Go: tk.MustExec("drop placement policy p1")
    // Go: tk.MustExec("drop placement policy p2")
    // Go: tk.MustExec("drop placement policy p3")
    // Go: tk.MustExec("flashback table tp2 to tp3")
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table tp3").Check(testkit.Rows("tp3 CREATE TABLE `tp3` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p0` VALUES LESS THAN (100),\n" +
    // Go: " PARTITION `p1` VALUES LESS THAN (1000),\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (10000))"))
    // Go: checkExistTableBundlesInPD(t, dom, "test", "tp3")
}

// getChangedBundles 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func getChangedBundles(oldBundle, newBundle []*placement.Bundle) (retOld, retNew []*placement.Bundle) {
// 参数语义: oldBundle, newBundle []*placement.Bundle
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：get_changed_bundles（保留原调用顺序，待接入真实测试框架）。
fn get_changed_bundles_go_draft() {

    // Go: OldLoop:
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for i := range oldBundle {
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for j := range newBundle {
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if oldBundle[i].ID == newBundle[j].ID {
    // Go: continue OldLoop
    // Go: }
    // Go: }
    // Go: retOld = append(retOld, oldBundle[i])
    // Go: }
    // Go: NewLoop:
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for i := range newBundle {
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for j := range oldBundle {
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if oldBundle[j].ID == newBundle[i].ID {
    // Go: continue NewLoop
    // Go: }
    // Go: }
    // Go: retNew = append(retNew, newBundle[i])
    // Go: }
    // 返回语句保持 Go 辅助函数的结果形状。
    // Go: return retOld, retNew
}

// TestAlterPartitioningWithPlacementPolicy 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestAlterPartitioningWithPlacementPolicy(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_alter_partitioning_with_placement_policy（保留原调用顺序，待接入真实测试框架）。
fn test_alter_partitioning_with_placement_policy_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: util.EmulatorGCDisable()
    // Go: store, do := testkit.CreateMockStoreAndDomain(t)
    // Go: gcWorker, err := gcworker.NewMockGCWorker(store)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: tk := testkit.NewTestKit(t, store)
    // Go: tk.MustExec("use test")
    // Go: tk.MustExec("create placement policy pp1 primary_region='r1' regions='r1,r2'")
    // Go: tk.MustExec("create placement policy pp2 primary_region='r2' regions='r1,r2'")
    // Go: tk.MustExec(`CREATE TABLE t1 (id INT)`)
    // Go: tk.MustExec(`INSERT INTO t1 values (1),(2),(100),(150),(200),(213)`)
    // Go: tk.MustExec(`ALTER TABLE t1 placement policy pp1`)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: origBundles, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: tk.MustExec(`ALTER TABLE t1 PARTITION BY HASH (id) PARTITIONS 3`)
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesBeforeGC, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesAfterGC, err := infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: oldBundles, newBundles := getChangedBundles(origBundles, bundlesBeforeGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 1)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // Go: oldBundles, newBundles = getChangedBundles(bundlesBeforeGC, bundlesAfterGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 1)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */\n" +
    // Go: "PARTITION BY HASH (`id`) PARTITIONS 3"))
    // Go: checkExistTableBundlesInPD(t, do, "test", "t1")
    // Go: origBundles = bundlesAfterGC
    // Go: tk.MustExec(`ALTER TABLE t1 ADD PARTITION (PARTITION p3 placement policy 'pp2')`)
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesBeforeGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesAfterGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: oldBundles, newBundles = getChangedBundles(origBundles, bundlesBeforeGC)
    // 原 Go 注释: One new partition level bundle
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 1)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // Go: oldBundles, newBundles = getChangedBundles(bundlesBeforeGC, bundlesAfterGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // 原 Go 注释: No old bundles removed
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */\n" +
    // Go: "PARTITION BY HASH (`id`)\n" +
    // Go: "(PARTITION `p0`,\n" +
    // Go: " PARTITION `p1`,\n" +
    // Go: " PARTITION `p2`,\n" +
    // Go: " PARTITION `p3` /*T![placement] PLACEMENT POLICY=`pp2` */)"))
    // Go: checkExistTableBundlesInPD(t, do, "test", "t1")
    // Go: origBundles = bundlesAfterGC
    // Go: tk.MustExec(`ALTER TABLE t1 REMOVE PARTITIONING`)
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesBeforeGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesAfterGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: oldBundles, newBundles = getChangedBundles(origBundles, bundlesBeforeGC)
    // 原 Go 注释: One table level bundle, due to new table id.
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 1)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // Go: oldBundles, newBundles = getChangedBundles(bundlesBeforeGC, bundlesAfterGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // 原 Go 注释: One table level due to new table id and one partition level policy removed
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 2)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */"))
    // Go: checkExistTableBundlesInPD(t, do, "test", "t1")
    // Go: origBundles = bundlesAfterGC
    // Go: tk.MustExec(`ALTER TABLE t1 PARTITION BY RANGE (id) (partition p1 values less than (100) placement policy pp2,partition p2 values less than (maxvalue))`)
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesBeforeGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesAfterGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: oldBundles, newBundles = getChangedBundles(origBundles, bundlesBeforeGC)
    // 原 Go 注释: One new bundle for the new table ID and one for the partition specific
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 2)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // Go: oldBundles, newBundles = getChangedBundles(bundlesBeforeGC, bundlesAfterGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // 原 Go 注释: Only one old table level bundle
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 1)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p1` VALUES LESS THAN (100) /*T![placement] PLACEMENT POLICY=`pp2` */,\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (MAXVALUE))"))
    // Go: checkExistTableBundlesInPD(t, do, "test", "t1")
    // Go: origBundles = bundlesAfterGC
    // Go: tk.MustExec(`ALTER TABLE t1 REORGANIZE PARTITION p2 into (partition p2 values less than (200) placement policy pp1,partition pMax values less than (maxvalue) placement policy 'pp2')`)
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesBeforeGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesAfterGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // 原 Go 注释: REORGANIZE keeps the table id, but the internal rules may change
    // Go: oldBundles, newBundles = getChangedBundles(origBundles, bundlesBeforeGC)
    // 原 Go 注释: Two new partition level bundles
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 2)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // Go: oldBundles, newBundles = getChangedBundles(bundlesBeforeGC, bundlesAfterGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // 原 Go 注释: No change in table ID and the reorganized partition did not have a partition level policy.
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p1` VALUES LESS THAN (100) /*T![placement] PLACEMENT POLICY=`pp2` */,\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (200) /*T![placement] PLACEMENT POLICY=`pp1` */,\n" +
    // Go: " PARTITION `pMax` VALUES LESS THAN (MAXVALUE) /*T![placement] PLACEMENT POLICY=`pp2` */)"))
    // Go: checkExistTableBundlesInPD(t, do, "test", "t1")
    // Go: origBundles = bundlesAfterGC
    // Go: tk.MustExec(`ALTER TABLE t1 TRUNCATE PARTITION pMax`)
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesBeforeGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesAfterGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: oldBundles, newBundles = getChangedBundles(origBundles, bundlesBeforeGC)
    // 原 Go 注释: One new partition level bundle
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 1)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // Go: oldBundles, newBundles = getChangedBundles(bundlesBeforeGC, bundlesAfterGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // 原 Go 注释: One old partition level bundle
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 1)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p1` VALUES LESS THAN (100) /*T![placement] PLACEMENT POLICY=`pp2` */,\n" +
    // Go: " PARTITION `p2` VALUES LESS THAN (200) /*T![placement] PLACEMENT POLICY=`pp1` */,\n" +
    // Go: " PARTITION `pMax` VALUES LESS THAN (MAXVALUE) /*T![placement] PLACEMENT POLICY=`pp2` */)"))
    // Go: checkExistTableBundlesInPD(t, do, "test", "t1")
    // Go: origBundles = bundlesAfterGC
    // Go: tk.MustExec(`ALTER TABLE t1 DROP PARTITION p1,pMax`)
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesBeforeGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesAfterGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: oldBundles, newBundles = getChangedBundles(origBundles, bundlesBeforeGC)
    // 原 Go 注释: No new partition level bundles
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // Go: oldBundles, newBundles = getChangedBundles(bundlesBeforeGC, bundlesAfterGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // 原 Go 注释: Two dropped partition level bundles.
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 2)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p2` VALUES LESS THAN (200) /*T![placement] PLACEMENT POLICY=`pp1` */)"))
    // Go: checkExistTableBundlesInPD(t, do, "test", "t1")
    // Go: origBundles = bundlesAfterGC
    // Go: tk.MustExec(`ALTER TABLE t1 ADD PARTITION (PARTITION pMax VALUES LESS THAN (MAXVALUE) placement policy 'pp2')`)
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesBeforeGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // context 传递 Go 调用链取消/来源信息；仅保留调用位置。
    // Go: bundlesAfterGC, err = infosync.GetAllRuleBundles(context.TODO())
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: oldBundles, newBundles = getChangedBundles(origBundles, bundlesBeforeGC)
    // 原 Go 注释: One new partition level bundles
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 1)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // Go: oldBundles, newBundles = getChangedBundles(bundlesBeforeGC, bundlesAfterGC)
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, newBundles, 0)
    // 原 Go 注释: No change in table ID.
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Len(t, oldBundles, 0)
    // 查询断言保留期望行文本，方便人工核对 SHOW/Information Schema 输出。
    // Go: tk.MustQuery("show create table t1").Check(testkit.Rows("" +
    // Go: "t1 CREATE TABLE `t1` (\n" +
    // Go: " `id` int(11) DEFAULT NULL\n" +
    // Go: ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![placement] PLACEMENT POLICY=`pp1` */\n" +
    // Go: "PARTITION BY RANGE (`id`)\n" +
    // Go: "(PARTITION `p2` VALUES LESS THAN (200) /*T![placement] PLACEMENT POLICY=`pp1` */,\n" +
    // Go: " PARTITION `pMax` VALUES LESS THAN (MAXVALUE) /*T![placement] PLACEMENT POLICY=`pp2` */)"))
    // Go: checkExistTableBundlesInPD(t, do, "test", "t1")
}

// TestCheckBundle 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestCheckBundle(t *testing.T) {
// 参数语义: t *testing.T
#[test]
#[allow(non_snake_case, unused_variables, dead_code)]
/// Go 测试草稿：test_check_bundle（保留原调用顺序，待接入真实测试框架）。
fn test_check_bundle_go_draft() {

    // 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
    // Go: type tc struct {
    // Go: bundle *placement.Bundle
    // Go: success bool
    // Go: }
    // Go: testCases := []tc{
    // Go: {
    // Go: bundle: &placement.Bundle{
    // Go: ID: "TiDB_DDL_1",
    // Go: Index: 1,
    // Go: Override: false,
    // Go: Rules: []*pd.Rule{
    // Go: {
    // Go: GroupID: "TiDB_DDL_1",
    // Go: ID: "TiDB_DDL_1",
    // Go: Override: false,
    // Go: StartKeyHex: "F0",
    // Go: EndKeyHex: "F2",
    // Go: Role: pd.Leader,
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_1",
    // Go: ID: "TiDB_DDL_1",
    // Go: Override: false,
    // Go: StartKeyHex: "01",
    // Go: EndKeyHex: "02",
    // Go: Role: pd.Leader,
    // Go: },
    // Go: },
    // Go: },
    // Go: success: true,
    // Go: },
    // Go: {
    // 原 Go 注释: What issue #55705 looked like, i.e. both partition and table had the same range.
    // Go: bundle: &placement.Bundle{
    // Go: ID: "TiDB_DDL_112",
    // Go: Index: 40,
    // Go: Override: true,
    // Go: Rules: []*pd.Rule{
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "table_rule_112_0",
    // Go: Index: 40,
    // Go: StartKeyHex: "7480000000000000ff7000000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7100000000000000f8",
    // Go: Role: "leader",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "table_rule_112_1",
    // Go: Index: 40,
    // Go: StartKeyHex: "7480000000000000ff7000000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7100000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "table_rule_112_2",
    // Go: Index: 40,
    // Go: StartKeyHex: "7480000000000000ff7000000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7100000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_112_0",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7000000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7100000000000000f8",
    // Go: Role: "leader",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_112_1",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7000000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7100000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_112_2",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7000000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7100000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_115_0",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7300000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7400000000000000f8",
    // Go: Role: "leader",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_115_1",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7300000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7400000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_115_2",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7300000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7400000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_116_0",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7400000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7500000000000000f8",
    // Go: Role: "leader",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_116_1",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7400000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7500000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_116_2",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7400000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7500000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_117_0",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7500000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7600000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_117_1",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7500000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7600000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: {
    // Go: GroupID: "TiDB_DDL_112",
    // Go: ID: "partition_rule_117_2",
    // Go: Index: 80,
    // Go: StartKeyHex: "7480000000000000ff7500000000000000f8",
    // Go: EndKeyHex: "7480000000000000ff7600000000000000f8",
    // Go: Role: "voter",
    // Go: },
    // Go: },
    // Go: },
    // Go: success: false,
    // Go: },
    // Go: }
    // 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
    // Go: for _, test := range testCases {
    // infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
    // Go: err := infosync.CheckBundle(test.bundle)
    // 条件分支保留 Go 的错误处理或状态判断语义。
    // Go: if test.success {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.NoError(t, err)
    // Go: } else {
    // require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
    // Go: require.Error(t, err)
    // Go: }
    // Go: }
}

use crate::placement_policy::{
    PlacementObject, PlacementOptionType, PlacementPolicyCatalog, PlacementSettings, PolicyError,
    PolicyInfo, PolicyRef, PolicyState, build_policy_info, get_range_placement_policy_name,
    handle_table_placement, set_direct_placement_opt,
};

/// 构造带默认 primary_region/regions/followers 的测试用 PolicyInfo。
fn policy(id: i64, name: &str) -> PolicyInfo {
    PolicyInfo {
        id,
        name: name.into(),
        state: PolicyState::None,
        settings: PlacementSettings {
            primary_region: "r1".into(),
            regions: "r1,r2".into(),
            followers: 3,
            ..PlacementSettings::default()
        },
    }
}

#[test]
/// 验证创建、同名冲突、OR REPLACE、alter 与删除状态机 Public→WriteOnly→DeleteOnly→None。
fn placement_policy_create_replace_alter_and_drop_state_machine() {
    let mut catalog = PlacementPolicyCatalog::default();
    // 创建 → 同名拒绝 → OR REPLACE 更新 followers → alter learners → 分步删除。
    assert_eq!(1, catalog.create(policy(1, "p1"), false).unwrap());
    assert_eq!(PolicyState::Public, catalog.by_name("P1").unwrap().state);
    assert_eq!(
        Err(PolicyError::AlreadyExists),
        catalog.create(policy(2, "p1"), false)
    );

    let mut replacement = policy(9, "p1");
    replacement.settings.followers = 5;
    assert_eq!(2, catalog.create(replacement, true).unwrap());
    assert_eq!(1, catalog.by_name("p1").unwrap().id);
    assert_eq!(5, catalog.by_name("p1").unwrap().settings.followers);

    // Go onCreatePlacementPolicy always resets the incoming state to None before creation.
    let mut pre_public = policy(2, "p2");
    pre_public.state = PolicyState::Public;
    assert_eq!(3, catalog.create(pre_public, false).unwrap());

    let mut settings = catalog.by_name("p1").unwrap().settings.clone();
    settings.learners = 2;
    assert_eq!(4, catalog.alter(1, settings).unwrap());
    assert_eq!(PolicyState::WriteOnly, catalog.drop_step(1).unwrap());
    assert_eq!(PolicyState::DeleteOnly, catalog.drop_step(1).unwrap());
    assert_eq!(PolicyState::None, catalog.drop_step(1).unwrap());
    assert!(catalog.by_name("p1").is_none());
}

#[test]
/// 验证 schedule 原样赋值、followers/voters 互斥、主区域必须属于 regions。
fn placement_policy_validation_matches_go_option_rules() {
    // buildPolicyInfo/SetDirectPlacementOpt 与 Go 一样只负责赋值，不改变字符串大小写。
    let built = build_policy_info(
        7,
        "regional",
        &[
            (PlacementOptionType::PrimaryRegion, "r1".into(), 0),
            (PlacementOptionType::Regions, "r1,r2".into(), 0),
            (PlacementOptionType::FollowerCount, String::new(), 3),
            (
                PlacementOptionType::Schedule,
                "majority_in_primary".into(),
                0,
            ),
        ],
    )
    .unwrap();
    assert_eq!("majority_in_primary", built.settings.schedule);

    // followers 与 voters 同时为正时配置非法。
    let mut conflicting = built.settings.clone();
    conflicting.voters = 3;
    assert_eq!(
        Err(PolicyError::InvalidSettings),
        catalog_for_validation().create(
            PolicyInfo {
                id: 8,
                name: "bad".into(),
                state: PolicyState::None,
                settings: conflicting,
            },
            false,
        )
    );

    let invalid_region = build_policy_info(
        9,
        "bad-region",
        &[
            (PlacementOptionType::PrimaryRegion, "r3".into(), 0),
            (PlacementOptionType::Regions, "r1,r2".into(), 0),
        ],
    )
    .unwrap();
    assert_eq!(
        Err(PolicyError::InvalidSettings),
        catalog_for_validation().create(invalid_region, false)
    );
}

#[test]
/// Go 的 onAlterPlacementPolicy 先读取策略，再校验新 settings，保持错误优先级一致。
fn alter_reports_missing_policy_before_invalid_settings() {
    let mut catalog = PlacementPolicyCatalog::default();
    let mut invalid = PlacementSettings::default();
    invalid.followers = 1;
    invalid.voters = 1;

    assert_eq!(Err(PolicyError::NotFound), catalog.alter(404, invalid));
}

#[test]
/// SetDirectPlacementOpt 对 schedule 做机械赋值，不承担语义校验或大小写归一化。
fn direct_schedule_assignment_preserves_go_input() {
    let mut settings = PlacementSettings::default();
    set_direct_placement_opt(
        &mut settings,
        PlacementOptionType::Schedule,
        "majority_in_primary",
        0,
    )
    .unwrap();
    assert_eq!("majority_in_primary", settings.schedule);
}

/// 返回空的 PlacementPolicyCatalog，供校验用例复用。
fn catalog_for_validation() -> PlacementPolicyCatalog {
    PlacementPolicyCatalog::default()
}

#[test]
/// 验证表/分区引用按名归一化 ID，占用中的策略不可 drop。
fn placement_refs_inherit_normalize_and_block_policy_drop() {
    let mut catalog = PlacementPolicyCatalog::default();
    catalog.create(policy(11, "hot"), false).unwrap();

    // 引用仅填名称时，handle_table_placement 应回填真实 policy id。
    let mut table = PlacementObject {
        id: 100,
        policy_ref: Some(PolicyRef {
            id: 0,
            name: "HOT".into(),
        }),
        partitions: vec![PlacementObject {
            id: 101,
            policy_ref: Some(PolicyRef {
                id: 0,
                name: "hot".into(),
            }),
            partitions: vec![],
        }],
    };
    assert!(!handle_table_placement(&mut table, &catalog, false).unwrap());
    assert_eq!(11, table.policy_ref.as_ref().unwrap().id);
    assert_eq!(11, table.partitions[0].policy_ref.as_ref().unwrap().id);

    catalog.tables.push(table);
    assert_eq!(Err(PolicyError::InUse), catalog.drop_step(11));
    assert_eq!(
        (Vec::<i64>::new(), vec![101], vec![100]),
        catalog.depended_object_ids(11).unwrap()
    );
}

#[test]
/// 验证 ignore 清除引用、default 归一为空、以及 range rule ID 解析策略名。
fn placement_ignore_default_and_range_rule_parsing_match_go() {
    let mut catalog = PlacementPolicyCatalog::default();
    catalog.create(policy(21, "archive"), false).unwrap();

    // ignore=true 时清除表与分区上的放置引用。
    let mut table = PlacementObject {
        id: 200,
        policy_ref: Some(PolicyRef {
            id: 21,
            name: "archive".into(),
        }),
        partitions: vec![PlacementObject {
            id: 201,
            policy_ref: Some(PolicyRef {
                id: 21,
                name: "archive".into(),
            }),
            partitions: vec![],
        }],
    };
    assert!(handle_table_placement(&mut table, &catalog, true).unwrap());
    assert!(table.policy_ref.is_none());
    assert!(table.partitions[0].policy_ref.is_none());
    assert_eq!(
        None,
        catalog
            .normalize_ref(Some(PolicyRef {
                id: 0,
                name: "default".into()
            }))
            .unwrap()
    );
    assert_eq!(
        "archive",
        get_range_placement_policy_name(Some("archive_rule_42"))
    );
    assert_eq!("", get_range_placement_policy_name(Some("malformed")));
}
