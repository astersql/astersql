// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 计划归一化（plan normalization）与 prefer-range-scan 等相关用例的 Go 迁移草稿。
//
// 文件主体以注释形式保留 Go 测试流程：NormalizePlan / NormalizeFlatPlan、
// plan digest（计划指纹）、binding 归一化与 hint 保留等。
// 文末少量可运行用例直接调用 parser 的 Normalize* API 做冒烟验证。

// GoAny/TestKitDraft/RandDraft/GoMap 是中的占位类型名，用来说明 Go any、testkit、rand 和 map 的数据流。
// 当前文件不会创建真实 store、不会执行 SQL、不会读写 testdata，也不会启动外部依赖。
// type GoAny = String;
// type GoMap<K, V> = std::collections::BTreeMap<K, V>;
// struct TestKitDraft;
// struct RandDraft;
//
// 对应 Go 的 getPlanRows：把 explain 文本中的 tab 替换为空格并拆成行。
// pub fn get_plan_rows(/* Go args: planStr string; Go returns: []string */) {
//     planStr = strings.Replace(planStr, "\t", " ", -1)
//     return strings.Split(planStr, "\n")
// }
//
// 对应 Go 的 compareStringSlice：逐项比较两组 plan 行长度。
// pub fn compare_string_slice(/* Go args: t *testing.T, ss1, ss2 []string */) {
//     require.Equal(t, len(ss1), len(ss2))
//     for i, s := range ss1 {
//         require.Equal(t, len(s), len(ss2[i]))
//     }
// }
//
// 对应 Go 的 TestPreferRangeScan：构造统计信息后比较 prefer range scan 的归一化计划。
// #[test]
// pub fn test_prefer_range_scan(/* Go args: t *testing.T */) {
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
//     testkit.RunTestUnderCascades(t, func(t *testing.T, tk *testkit.TestKit, cascades, caller string) {
// MustExec 表示测试必须成功执行 SQL。
//         tk.MustExec("use test")
//         tk.MustExec(`set @@tidb_enable_non_prepared_plan_cache=0`) // affect this ut: tidb_opt_prefer_range_scan
//         tk.MustExec("drop table if exists test;")
//         tk.MustExec("create table test(`id` int(10) NOT NULL AUTO_INCREMENT,`name` varchar(50) NOT NULL DEFAULT 'tidb',`age` int(11) NOT NULL,`addr` varchar(50) DEFAULT 'The ocean of stars',PRIMARY KEY (`id`),KEY `idx_age` (`age`))")
//         tk.MustExec("insert into test(age) values(5);")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("insert into test(name,age,addr) select name,age,addr from test;")
//         tk.MustExec("analyze table test;")
//
// Default RPC encoding may cause statistics explain result differ and then the test unstable.
// MustExec 表示测试必须成功执行 SQL。
//         tk.MustExec("set @@tidb_enable_chunk_rpc = on")
//
//         var input []string
//         var output []struct {
//             SQL  string
//             Plan []string
//         }
//         planNormalizedSuiteData := GetPlanNormalizedSuiteData()
// testdata.LoadTestCases 会按 cascades/caller 选择 fixture；这里保留输入/输出绑定语义。
//         planNormalizedSuiteData.LoadTestCases(t, &input, &output, cascades, caller)
//         for i, tt := range input {
//             switch i {
//             case 0:
// MustExec 表示测试必须成功执行 SQL。
//                 tk.MustExec("set session tidb_opt_prefer_range_scan=0")
//             case 1:
//                 tk.MustExec("set session tidb_opt_prefer_range_scan=1")
//             }
//             tk.Session().GetSessionVars().PlanID.Store(0)
//             tk.MustExec(tt)
//             info := tk.Session().ShowProcess()
//             require.NotNil(t, info)
//             p, ok := info.Plan.(base.Plan)
//             require.True(t, ok)
// NormalizePlan/NormalizeFlatPlan 的比较用于确保 plan digest 稳定；保留双路径校验。
//             normalized, digest := core.NormalizePlan(p)
//
// test the new normalization code
//             flat := core.FlattenPhysicalPlan(p, false)
//             newNormalized, newDigest := core.NormalizeFlatPlan(flat)
//             require.Equal(t, normalized, newNormalized)
//             require.Equal(t, digest, newDigest)
//
//             normalizedPlan, err := plancodec.DecodeNormalizedPlan(normalized)
//             normalizedPlanRows := getPlanRows(normalizedPlan)
//             require.NoError(t, err)
// record 模式会把实际输出写回 fixture；保留写回字段，不做真实文件 IO。
//             testdata.OnRecord(func() {
//                 output[i].SQL = tt
//                 output[i].Plan = normalizedPlanRows
//             })
//             compareStringSlice(t, normalizedPlanRows, output[i].Plan)
//         }
//     })
// }
//
// 对应 Go 的 TestPreferRangeScanForDNF：验证 DNF 条件下 prefer range scan 的 IndexLookUp/TableReader 选择。
// #[test]
// pub fn test_prefer_range_scan_for_dnf(/* Go args: t *testing.T */) {
//     store := testkit.CreateMockStore(t)
//     tk := testkit.NewTestKit(t, store)
// MustExec 表示测试必须成功执行 SQL。
//     tk.MustExec("set @@session.tidb_opt_prefer_range_scan=1")
//     tk.MustExec("set @@global.tidb_enable_auto_analyze='OFF'")
//     tk.MustExec("use test")
//     tk.MustExec("drop table if exists t")
// Create table without inserting data to test pseudo stats behavior
//     tk.MustExec("create table t (a int, b int, c int, index idx_a_b(a, b))")
//
// DNF with only equal predicates - should prefer IndexLookUp
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
//     result := tk.MustQuery("explain format = 'plan_tree' select * from t where (a = 1 and b = 1) or (a = 2 and b = 2)")
//     require.Contains(t, result.Rows()[0][0], "IndexLookUp")
//
// Longer DNF with only equal predicates - should prefer IndexLookUp
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
//     result = tk.MustQuery("explain format = 'plan_tree' select * from t where a = 1 or a = 3 or a = 5 or a = 7 or a = 9 or a = 11 or a = 13 or a = 15 or a = 17 or a = 19 or a = 21 or a = 23 or a = 25 or a = 27 or a = 29 or a = 31 or a = 33 or a = 35 or a = 37 or a = 39 or a = 41 or a = 43 or a = 45 or a = 47 or a = 49 or a = 51 or a = 53 or a = 55 or a = 57 or a = 59")
//     require.Contains(t, result.Rows()[0][0], "IndexLookUp")
//
// DNF with leading equal conditions plus range predicates - should prefer IndexLookUp
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
//     result = tk.MustQuery("explain format = 'plan_tree' select * from t where (a = 1 and b > 0) or (a = 2 and b < 5)")
//     require.Contains(t, result.Rows()[0][0], "IndexLookUp")
//
// DNF with NOT operators - should not prefer IndexLookUp
// This should fall back to TableReader since NOT operators don't qualify
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
//     result = tk.MustQuery("explain format = 'plan_tree' select * from t where (a = 1 and b = 1) or not (a = 2 and b = 2)")
//     require.Contains(t, result.Rows()[0][0], "TableReader")
//
// DNF with mixed predicates - should not prefer IndexLookUp
// This should fall back to TableReader since it contains non-equal predicates
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
//     result = tk.MustQuery("explain format = 'plan_tree' select * from t where (a = 1 and b = 1) or (a >= 2 and b <= 3) or (a = 4 and b = 4) or (a = 5 and b > 0) or (a < 6 and b < 6)")
//     require.Contains(t, result.Rows()[0][0], "TableReader")
//
// Disabling prefer_range_scan should not use IndexLookUp for long set of DNF conditions
// NOTE: This test could become flaky if "cost" of IndexLookup is lowered in future. Consider adding
// more "or a = N" terms if that happens.
// MustExec 表示测试必须成功执行 SQL。
//     tk.MustExec("set @@session.tidb_opt_prefer_range_scan=0")
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
//     result = tk.MustQuery("explain format = 'plan_tree' select * from t where a = 1 or a = 3 or a = 5 or a = 7 or a = 9 or a = 11 or a = 13 or a = 15 or a = 17 or a = 19 or a = 21 or a = 23 or a = 25 or a = 27 or a = 29 or a = 31 or a = 33 or a = 35 or a = 37 or a = 39 or a = 41 or a = 43 or a = 45 or a = 47 or a = 49 or a = 51 or a = 53 or a = 55 or a = 57 or a = 59")
//     require.Contains(t, result.Rows()[0][0], "TableReader")
//
// Restore settings
// MustExec 表示测试必须成功执行 SQL。
//     tk.MustExec("set @@global.tidb_enable_auto_analyze='ON'")
//     tk.MustExec("set @@session.tidb_opt_prefer_range_scan=1")
// }
//
// 对应 Go 的 testNormalizedPlan：准备多表和外键后比较 NormalizePlan 与 flat plan 结果。
// pub fn test_normalized_plan_helper(/* Go args: t *testing.T, tk *testkit.TestKit, cascades, caller string */) {
// MustExec 表示测试必须成功执行 SQL。
//     tk.MustExec("use test")
//     tk.MustExec("set @@tidb_partition_prune_mode='static';")
//     tk.MustExec("drop table if exists t1,t2,t3,t4")
//     tk.MustExec("create table t1 (a int key,b int,c int, index (b));")
//     tk.MustExec("create table t2 (a int key,b int,c int, index (b));")
//     tk.MustExec("create table t3 (a int key,b int) partition by hash(a) partitions 2;")
//     tk.MustExec("create table t4 (a int, b int, index(a)) partition by range(a) (partition p0 values less than (10),partition p1 values less than MAXVALUE);")
//     tk.MustExec("set @@global.tidb_enable_foreign_key=1")
//     tk.MustExec("set @@foreign_key_checks=1")
//     tk.MustExec("create table t5 (id int key, id2 int, id3 int, unique index idx2(id2), index idx3(id3));")
//     tk.MustExec("create table t6 (id int,     id2 int, id3 int, index idx_id(id), index idx_id2(id2), " +
//         "foreign key fk_1 (id) references t5(id) ON UPDATE CASCADE ON DELETE CASCADE, " +
//         "foreign key fk_2 (id2) references t5(id2) ON UPDATE CASCADE, " +
//         "foreign key fk_3 (id3) references t5(id3) ON DELETE CASCADE);")
//     tk.MustExec("insert into t5 values (1,1,1), (2,2,2)")
//     var input []string
//     var output []struct {
//         SQL  string
//         Plan []string
//     }
//     planNormalizedSuiteData := GetPlanNormalizedSuiteData()
// testdata.LoadTestCases 会按 cascades/caller 选择 fixture；这里保留输入/输出绑定语义。
//     planNormalizedSuiteData.LoadTestCases(t, &input, &output, cascades, caller)
//     for i, tt := range input {
//         tk.Session().GetSessionVars().PlanID.Store(0)
//         tk.MustExec(tt)
//         info := tk.Session().ShowProcess()
//         require.NotNil(t, info)
//         p, ok := info.Plan.(base.Plan)
//         require.True(t, ok)
// NormalizePlan/NormalizeFlatPlan 的比较用于确保 plan digest 稳定；保留双路径校验。
//         normalized, digest := core.NormalizePlan(p)
//
// test the new normalization code
//         flat := core.FlattenPhysicalPlan(p, false)
//         newNormalized, newDigest := core.NormalizeFlatPlan(flat)
//         require.Equal(t, normalized, newNormalized)
//         require.Equal(t, digest, newDigest)
// Test for GenHintsFromFlatPlan won't panic.
//         core.GenHintsFromFlatPlan(flat)
//
//         normalizedPlan, err := plancodec.DecodeNormalizedPlan(normalized)
//         normalizedPlanRows := getPlanRows(normalizedPlan)
//         require.NoError(t, err)
// record 模式会把实际输出写回 fixture；保留写回字段，不做真实文件 IO。
//         testdata.OnRecord(func() {
//             output[i].SQL = tt
//             output[i].Plan = normalizedPlanRows
//         })
//         compareStringSlice(t, normalizedPlanRows, output[i].Plan)
//     }
// }
//
// 对应 Go 的 TestNormalizedPlan：非 next-gen 模式下运行归一化计划测试。
// #[test]
// pub fn test_normalized_plan(/* Go args: t *testing.T */) {
//     if kerneltype.IsNextGen() {
//         t.Skip("Please run TestNormalizedPlanForNextGen under the next-gen mode")
//     }
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
//     testkit.RunTestUnderCascades(t, testNormalizedPlan)
// }
//
// 对应 Go 的 TestNormalizedPlanForNextGen：next-gen 模式下运行归一化计划测试。
// #[test]
// pub fn test_normalized_plan_for_next_gen(/* Go args: t *testing.T */) {
//     if !kerneltype.IsNextGen() {
//         t.Skip("Please run TestNormalizedPlan under the non next-gen mode")
//     }
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
//     testkit.RunTestUnderCascades(t, testNormalizedPlan)
// }
//
// 对应 Go 的 TestPlanDigest4InList：迁移 in-list plan digest 用例。
// #[test]
// pub fn test_plan_digest4_in_list(/* Go args: t *testing.T */) {
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
//     testkit.RunTestUnderCascades(t, func(t *testing.T, tk *testkit.TestKit, cascades, caller string) {
// MustExec 表示测试必须成功执行 SQL。
//         tk.MustExec("use test")
//         tk.MustExec("drop table if exists t")
//         tk.MustExec("create table t (a int);")
//         tk.Session().GetSessionVars().PlanID.Store(0)
//         var queriesGroup1, queriesGroup2 []string
//         queriesGroup1 = []string{
//             "select * from t where a in (1, 2);",
//             "select a in (1, 2) from t;",
//         }
//         queriesGroup2 = []string{
//             "select * from t where a in (1, 2, 3);",
//             "select a in (1, 2, 3) from t;",
//         }
//         for i := range queriesGroup1 {
//             query1 := queriesGroup1[i]
//             query2 := queriesGroup2[i]
//             t.Run(query1+" vs "+query2, func(t *testing.T) {
//                 tk.MustExec(query1)
//                 info1 := tk.Session().ShowProcess()
//                 require.NotNil(t, info1)
//                 p1, ok1 := info1.Plan.(base.Plan)
//                 require.True(t, ok1)
// NormalizePlan/NormalizeFlatPlan 的比较用于确保 plan digest 稳定；保留双路径校验。
//                 _, digest1 := core.NormalizePlan(p1)
//                 tk.MustExec(query2)
//                 info2 := tk.Session().ShowProcess()
//                 require.NotNil(t, info2)
//                 p2, ok2 := info2.Plan.(base.Plan)
//                 require.True(t, ok2)
//                 _, digest2 := core.NormalizePlan(p2)
//                 require.Equal(t, digest1, digest2)
//             })
//         }
//
// Issue 66623: same plans with different in-list lengths should have the same plan digest
//         t.Run("issue 66623: select * from t where a in (...) with varying lengths", func(t *testing.T) {
//             queries := []string{
//                 "select * from t where a in (1, 2);",
//                 "select * from t where a in (1, 2, 3);",
//                 "select * from t where a in (1, 2, 3, 4);",
//                 "select * from t where a in (1, 2, 3, 4, 5);",
//                 "select * from t where a in (1, 2, 3, 4, 5, 6);",
//                 "select * from t where a in (1, 2, 3, 4, 5, 6, 7);",
//             }
//             var firstDigest *parser.Digest
//             for i, query := range queries {
// MustExec 表示测试必须成功执行 SQL。
//                 tk.MustExec(query)
//                 info := tk.Session().ShowProcess()
//                 require.NotNil(t, info)
//                 p, ok := info.Plan.(base.Plan)
//                 require.True(t, ok)
// NormalizePlan/NormalizeFlatPlan 的比较用于确保 plan digest 稳定；保留双路径校验。
//                 _, digest := core.NormalizePlan(p)
//                 if i == 0 {
//                     firstDigest = digest
//                 } else {
//                     require.Equal(t, firstDigest, digest, "query %d: %s", i, query)
//                 }
//             }
//         })
//
// MustExec 表示测试必须成功执行 SQL。
//         tk.MustExec("drop table if exists t3,t4,t5")
//         tk.MustExec("create table t3(a int, b int, c int);")
//         tk.MustExec("create table t4(a int, b int, c int, primary key (a, b) clustered);")
//         tk.MustExec("create table t5(a int, b int, c int, key idx_a_b (a, b));")
//         tk.Session().GetSessionVars().PlanID.Store(0)
//         queriesGroup1 = []string{
//             "explain format = 'plan_tree' select /* issue:47634 */ /*+ inl_join(t4) */
//  * from t3 join t4 on t3.b = t4.b where t4.a = 1;",
// "explain format = 'plan_tree' select /* issue:47634 */
// /*+ inl_join(t5) */
//  * from t3 join t5 on t3.b = t5.b where t5.a = 1;",
// 		}
// 		queriesGroup2 = []string{
// "explain format = 'plan_tree' select /* issue:47634 */
// /*+ inl_join(t4) */
//  * from t3 join t4 on t3.b = t4.b where t4.a = 2;",
// "explain format = 'plan_tree' select /* issue:47634 */
// /*+ inl_join(t5) */
//  * from t3 join t5 on t3.b = t5.b where t5.a = 2;",
// 		}
// 		for i := range queriesGroup1 {
// 			query1 := queriesGroup1[i]
// 			query2 := queriesGroup2[i]
// 			t.Run(query1+" vs "+query2, func(t *testing.T) {
// 				tk.MustExec(query1)
// 				info1 := tk.Session().ShowProcess()
// 				require.NotNil(t, info1)
// 				p1, ok1 := info1.Plan.(base.Plan)
// 				require.True(t, ok1)
// NormalizePlan/NormalizeFlatPlan 的比较用于确保 plan digest 稳定；保留双路径校验。
// 				_, digest1 := core.NormalizePlan(p1)
// 				tk.MustExec(query2)
// 				info2 := tk.Session().ShowProcess()
// 				require.NotNil(t, info2)
// 				p2, ok2 := info2.Plan.(base.Plan)
// 				require.True(t, ok2)
// 				_, digest2 := core.NormalizePlan(p2)
// 				require.Equal(t, digest1, digest2)
// 			})
// 		}
// 	})
// }
//
// 对应 Go 的 TestNormalizedPlanForDiffStore：迁移不同 store 下的 normalized plan。
// #[test]
// pub fn test_normalized_plan_for_diff_store(/* Go args: t *testing.T */) {
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
// 	testkit.RunTestUnderCascadesWithDomain(t, func(t *testing.T, tk *testkit.TestKit, dom *domain.Domain, cascades, caller string) {
// MustExec 表示测试必须成功执行 SQL。
// 		tk.MustExec("use test")
// 		tk.MustExec("drop table if exists t1")
// 		tk.MustExec("create table t1 (a int, b int, c int, primary key(a))")
// 		tk.MustExec("insert into t1 values(1,1,1), (2,2,2), (3,3,3)")
// 		tbl, err := dom.InfoSchema().TableByName(context.Background(), ast.CIStr{O: "test", L: "test"}, ast.CIStr{O: "t1", L: "t1"})
// 		require.NoError(t, err)
// Set the hacked TiFlash replica for explain tests.
// 		tbl.Meta().TiFlashReplica = &model.TiFlashReplicaInfo{Count: 1, Available: true}
//
// 		var input []string
// 		var output []struct {
// 			Digest string
// 			Plan   []string
// 		}
// 		planNormalizedSuiteData := GetPlanNormalizedSuiteData()
// testdata.LoadTestCases 会按 cascades/caller 选择 fixture；这里保留输入/输出绑定语义。
// 		planNormalizedSuiteData.LoadTestCases(t, &input, &output, cascades, caller)
// 		lastDigest := ""
// 		for i, tt := range input {
// 			tk.Session().GetSessionVars().PlanID.Store(0)
// MustExec 表示测试必须成功执行 SQL。
// 			tk.MustExec(tt)
// 			info := tk.Session().ShowProcess()
// 			require.NotNil(t, info)
// 			ep, ok := info.Plan.(*core.Explain)
// 			require.True(t, ok)
// NormalizePlan/NormalizeFlatPlan 的比较用于确保 plan digest 稳定；保留双路径校验。
// 			normalized, digest := core.NormalizePlan(ep.TargetPlan)
//
// test the new normalization code
// 			flat := core.FlattenPhysicalPlan(ep.TargetPlan, false)
// 			newNormalized, newPlanDigest := core.NormalizeFlatPlan(flat)
// 			require.Equal(t, digest, newPlanDigest)
// 			require.Equal(t, normalized, newNormalized)
//
// 			normalizedPlan, err := plancodec.DecodeNormalizedPlan(normalized)
// 			normalizedPlanRows := getPlanRows(normalizedPlan)
// 			require.NoError(t, err)
// record 模式会把实际输出写回 fixture；保留写回字段，不做真实文件 IO。
// 			testdata.OnRecord(func() {
// 				output[i].Digest = digest.String()
// 				output[i].Plan = normalizedPlanRows
// 			})
// 			compareStringSlice(t, normalizedPlanRows, output[i].Plan)
// 			require.NotEqual(t, digest.String(), lastDigest)
// 			lastDigest = digest.String()
// 		}
// 	})
// }
//
// 对应 Go 的 testJSONPlanInExplain：解析 explain format=json 并校验 JSON 字段。
// pub fn test_json_plan_in_explain_helper(/* Go args: t *testing.T, tk *testkit.TestKit, cascades, caller string */) {
// MustExec 表示测试必须成功执行 SQL。
// 	tk.MustExec("use test")
// 	tk.MustExec("drop table if exists t1, t2")
// 	tk.MustExec("create table t1(id int, key(id))")
// 	tk.MustExec("create table t2(id int, key(id))")
//
// 	var input []string
// 	var output []struct {
// 		SQL      string
// 		JSONPlan []*core.ExplainInfoForEncode
// 	}
// 	planSuiteData := GetJSONPlanSuiteData()
// testdata.LoadTestCases 会按 cascades/caller 选择 fixture；这里保留输入/输出绑定语义。
// 	planSuiteData.LoadTestCases(t, &input, &output, cascades, caller)
//
// 	for i, test := range input {
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
// 		resJSON := tk.MustQuery(test).Rows()
// 		var res []*core.ExplainInfoForEncode
// 		require.NoError(t, json.Unmarshal([]byte(resJSON[0][0].(string)), &res))
// record 模式会把实际输出写回 fixture；保留写回字段，不做真实文件 IO。
// 		testdata.OnRecord(func() {
// 			output[i].SQL = test
// 			output[i].JSONPlan = res
// 		})
// 		for j, expect := range output[i].JSONPlan {
// 			require.Equal(t, expect.ID, res[j].ID)
// 			require.Equal(t, expect.EstRows, res[j].EstRows)
// 			require.Equal(t, expect.ActRows, res[j].ActRows)
// 			require.Equal(t, expect.TaskType, res[j].TaskType)
// 			require.Equal(t, expect.AccessObject, res[j].AccessObject)
// 			require.Equal(t, expect.OperatorInfo, res[j].OperatorInfo)
// 		}
// 	}
// }
//
// 对应 Go 的 TestJSONPlanInExplain：非 next-gen 模式 explain json 测试入口。
// #[test]
// pub fn test_json_plan_in_explain(/* Go args: t *testing.T */) {
// 	if kerneltype.IsNextGen() {
// 		t.Skip("Please run TestJSONPlanInExplainForNextGen under the next-gen mode")
// 	}
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
// 	testkit.RunTestUnderCascades(t, testJSONPlanInExplain)
// }
//
// 对应 Go 的 TestJSONPlanInExplainForNextGen：next-gen 模式 explain json 测试入口。
// #[test]
// pub fn test_json_plan_in_explain_for_next_gen(/* Go args: t *testing.T */) {
// 	if !kerneltype.IsNextGen() {
// 		t.Skip("Please run TestJSONPlanInExplain under the non next-gen mode")
// 	}
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
// 	testkit.RunTestUnderCascades(t, testJSONPlanInExplain)
// }
//
// 对应 Go 的 TestHandleEQAll：迁移 handle = all 子查询计划用例。
// #[test]
// pub fn test_handle_eq_all(/* Go args: t *testing.T */) {
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
// 	testkit.RunTestUnderCascades(t, func(t *testing.T, tk *testkit.TestKit, cascades, caller string) {
// MustExec 表示测试必须成功执行 SQL。
// 		tk.MustExec("use test")
// 		tk.MustExec("CREATE TABLE t1 (c1 int, c2 int, UNIQUE i1 (c1, c2));")
// 		tk.MustExec("INSERT INTO t1 VALUES (7, null),(5,1);")
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
// tk.MustQuery("SELECT c1 FROM t1 WHERE ('m' = ALL (SELECT /*+ IGNORE_INDEX(t1, i1) */
//  c2 FROM t1)) IS NOT UNKNOWN; ").Check(testkit.Rows("5", "7"))
// tk.MustQuery("SELECT c1 FROM t1 WHERE ('m' = ALL (SELECT /*+ use_INDEX(t1, i1) */
//  c2 FROM t1)) IS NOT UNKNOWN; ").Check(testkit.Rows("5", "7"))
// tk.MustQuery("select (null = ALL (SELECT /*+ NO_INDEX() */
//  c2 FROM t1)) IS NOT UNKNOWN").Check(testkit.Rows("0"))
// 		tk.MustExec("CREATE TABLE t2 (c1 int, c2 int, UNIQUE i1 (c1, c2));")
// 		tk.MustExec("INSERT INTO t2 VALUES (7, null),(5,null);")
// tk.MustQuery("select (null = ALL (SELECT /*+ NO_INDEX() */
//  c2 FROM t2)) IS NOT UNKNOWN").Check(testkit.Rows("0"))
// tk.MustQuery("SELECT c1 FROM t2 WHERE ('m' = ALL (SELECT /*+ IGNORE_INDEX(t2, i1) */
//  c2 FROM t2)) IS NOT UNKNOWN; ").Check(testkit.Rows())
// tk.MustQuery("SELECT c1 FROM t2 WHERE ('m' = ALL (SELECT /*+ use_INDEX(t2, i1) */
//  c2 FROM t2)) IS NOT UNKNOWN; ").Check(testkit.Rows())
// 		tk.MustExec("truncate table t2")
// 		tk.MustExec("INSERT INTO t2 VALUES (7, null),(7,null);")
// tk.MustQuery("select c1 from t2 where (c1 = all (select /*+ IGNORE_INDEX(t2, i1) */
//  c1 from t2))").Check(testkit.Rows("7", "7"))
// tk.MustQuery("select c1 from t2 where (c1 = all (select /*+ use_INDEX(t2, i1) */
//  c1 from t2))").Check(testkit.Rows("7", "7"))
// tk.MustQuery("select c2 from t2 where (c2 = all (select /*+ IGNORE_INDEX(t2, i1) */
//  c2 from t2))").Check(testkit.Rows())
// tk.MustQuery("select c2 from t2 where (c2 = all (select /*+ use_INDEX(t2, i1) */
//  c2 from t2))").Check(testkit.Rows())
// 		tk.MustExec("drop table if exists t")
// 		tk.MustExec("create table t (c int)")
// 		tk.MustExec("insert into t values (1)")
//
// 		expr := "(not exists (select 1 from t)) <= all (select c from t)"
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
// 		tk.MustQuery("select " + expr).Check(testkit.Rows("1"))
// 		tk.MustQuery("select * from t where " + expr).Check(testkit.Rows("1"))
// 		tk.MustNotHavePlan("select * from t where "+expr, "TableDual")
// 	})
// }
//
// 对应 Go 的 TestOuterJoinElimination：迁移外连接消除计划与 warning 校验。
// #[test]
// pub fn test_outer_join_elimination(/* Go args: t *testing.T */) {
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
// 	testkit.RunTestUnderCascades(t, func(t *testing.T, tk *testkit.TestKit, cascades, caller string) {
// MustExec 表示测试必须成功执行 SQL。
// 		tk.MustExec("use test")
// 		tk.MustExec(`create table t1 (a int, b int, c int)`)
// 		tk.MustExec(`create table t2 (a int, b int, c int)`)
// 		tk.MustExec(`create table t2_k (a int, b int, c int, key(a))`)
// 		tk.MustExec(`create table t2_uk (a int, b int, c int, unique key(a))`)
// 		tk.MustExec(`create table t2_nnuk (a int not null, b int, c int, unique key(a))`)
// 		tk.MustExec(`create table t2_pk (a int, b int, c int, primary key(a))`)
// 		tk.MustExec(`create table t1_window (a int, b int, c int, key idx_a(a))`)
// 		tk.MustExec(`create table t2_window (a int, b int, c int, key(a))`)
//
// 		tk.MustNotHavePlan("select * from t1 left join t2 on false", "Join")
// 		tk.MustNotHavePlan("select * from t1 right join t2 on false", "Join")
//
// only when t2.a has unique attribute, we can eliminate the outer join.
// nullable unique index is not allowed to trigger the outer join elinimation.
// 		tk.MustHavePlan("select count(*) from t1 left join t2 on t1.a = t2.a", "Join")
// 		tk.MustHavePlan("select count(*) from t1 left join t2_k on t1.a = t2_k.a", "Join")
// 		tk.MustNotHavePlan("select count(*) from t1 left join t2_uk on t1.a = t2_uk.a", "Join")
// 		tk.MustNotHavePlan("select count(*) from t1 left join t2_nnuk on t1.a = t2_nnuk.a", "Join")
// 		tk.MustNotHavePlan("select count(*) from t1 left join t2_pk on t1.a = t2_pk.a", "Join")
//
// 		tk.MustHavePlan("select count(*) from t1 left join t2 on t1.a = t2.a group by t1.a", "Join")
// 		tk.MustHavePlan("select count(*) from t1 left join t2_k on t1.a = t2_k.a group by t1.a", "Join")
// 		tk.MustNotHavePlan("select count(*) from t1 left join t2_uk on t1.a = t2_uk.a group by t1.a", "Join")
// 		tk.MustNotHavePlan("select count(*) from t1 left join t2_nnuk on t1.a = t2_nnuk.a group by t1.a", "Join")
// 		tk.MustNotHavePlan("select count(*) from t1 left join t2_pk on t1.a = t2_pk.a group by t1.a", "Join")
//
// test distinct aggregation
// 		tk.MustNotHavePlan("select distinct t1.a from t1 left join t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct t1.a from t1 left join t2_k t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct t1.a from t1 left join t2_uk t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct t1.a from t1 left join t2_nnuk t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct t1.a from t1 left join t2_pk t2 on t1.a = t2.a", "Join")
// test constant columns with distinct
// 		tk.MustNotHavePlan("select distinct 1 from t1 left join t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct 1 from t1 left join t2_k t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct 1 from t1 left join t2_uk t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct 1 from t1 left join t2_nnuk t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct 1 from t1 left join t2_pk t2 on t1.a = t2.a", "Join")
// test constant columns with distinct
// 		tk.MustHavePlan("select 1 from t1 left join t2 on t1.a = t2.a", "Join")
// 		tk.MustHavePlan("select 1 from t1 left join t2_k t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select 1 from t1 left join t2_uk t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select 1 from t1 left join t2_nnuk t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select 1 from t1 left join t2_pk t2 on t1.a = t2.a", "Join")
// 		tk.MustHavePlan("select count(*) from t1 left join t2_uk t2 on t1.a <=> t2.a", "Join")
// 		tk.MustNotHavePlan("select count(*) from t1 left join t2_nnuk t2 on t1.a <=> t2.a", "Join")
// 		tk.MustNotHavePlan("select count(*) from t1 left join t2_pk t2 on t1.a <=> t2.a", "Join")
// test subqueries
// 		tk.MustHavePlan("select 1 from (select distinct a from t1) t1 left join t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct 1 from (select distinct a from t1) t1 left join t2 on t1.a = t2.a", "Join")
// 		tk.MustHavePlan("select t1.a from (select distinct a from t1) t1 left join t2 on t1.a = t2.a", "Join")
// 		tk.MustNotHavePlan("select distinct t1.a from (select distinct a from t1) t1 left join t2 on t1.a = t2.a", "Join")
// test subqueries in the select list with no_decorrelate_in_select=OFF
// MustExec 表示测试必须成功执行 SQL。
// 		tk.MustExec("set @@tidb_opt_enable_no_decorrelate_in_select=OFF")
// 		tk.MustHavePlan("select t1a.a, if(exists(select 1 from t2_uk t2b where t2b.a = t1a.a), 1, 0) as founda from t1 t1a left join t2_pk t2 on t1a.a = t2.a", "Join")
// test subqueries in the select list with no_decorrelate_in_select=ON
// 		tk.MustExec("set @@tidb_opt_enable_no_decorrelate_in_select=ON")
// 		tk.MustNotHavePlan("select t1a.a, if(exists(select 1 from t2_uk t2b where t2b.a = t1a.a), 1, 0) as founda from t1 t1a left join t2_pk t2 on t1a.a = t2.a", "Join")
// next query correlates on t2, so outer join elimination can't be applied
// 		tk.MustHavePlan("select t1a.a, if(exists(select 1 from t2_uk t2b where t2b.a = t2.a), 1, 0) as founda from t1 t1a left join t2_pk t2 on t1a.a = t2.a", "Join")
//
// MustExec 表示测试必须成功执行 SQL。
// 		tk.MustExec("insert into t1_window values (1, 10, 100), (2, 20, 200)")
// 		tk.MustExec("insert into t2_window values (1, 10, 1), (1, 10, 2), (2, 20, 3)")
// 		sql := `select t1.a from t1_window t1 use index(idx_a) left join (
// 			select a, row_number() over(partition by a order by c desc) as rn
// 			from t2_window
// 		) t2 on t1.a = t2.a and t2.rn = 1
// 		where t1.a = 1`
// MustQuery 表示执行 SQL 并断言结果；保留查询和校验关系。
// 		tk.MustQuery(sql).Check(testkit.Rows("1"))
// 		tk.MustQuery("explain format = 'plan_tree' " + sql).Check(testkit.Rows(
// 			"IndexReader root  index:IndexRangeScan",
// 			"└─IndexRangeScan cop[tikv] table:t1, index:idx_a(a) range:[1,1], keep order:false, stats:pseudo",
// 		))
// 	})
// }
//
// 对应 Go 的 TestCTEErrNotSupportedYet：迁移 recursive CTE 错误路径。
// #[test]
// pub fn test_cte_err_not_supported_yet(/* Go args: t *testing.T */) {
// Go 这里在 cascades/non-cascades 两套 planner 下运行；只保留回调边界。
// 	testkit.RunTestUnderCascades(t, func(t *testing.T, tk *testkit.TestKit, cascades, caller string) {
// MustExec 表示测试必须成功执行 SQL。
// 		tk.MustExec("use test")
// 		tk.MustExec(`
// CREATE TABLE pub_branch (
//   id int(5) NOT NULL,
//   code varchar(12) NOT NULL,
//   type_id int(3) DEFAULT NULL,
//   name varchar(64) NOT NULL,
//   short_name varchar(32) DEFAULT NULL,
//   organ_code varchar(15) DEFAULT NULL,
//   parent_code varchar(12) DEFAULT NULL,
//   organ_layer tinyint(1) NOT NULL,
//   inputcode1 varchar(12) DEFAULT NULL,
//   inputcode2 varchar(12) DEFAULT NULL,
//   state tinyint(1) NOT NULL,
//   modify_empid int(9) NOT NULL,
//   modify_time datetime NOT NULL,
//   organ_level int(9) DEFAULT NULL,
//   address varchar(256) DEFAULT NULL,
//   db_user varchar(32) DEFAULT NULL,
//   db_password varchar(64) DEFAULT NULL,
//   org_no int(3) DEFAULT NULL,
//   ord int(5) DEFAULT NULL,
//   org_code_mpa varchar(10) DEFAULT NULL,
//   org_code_gb varchar(30) DEFAULT NULL,
//   wdchis_id int(5) DEFAULT NULL,
//   medins_code varchar(32) DEFAULT NULL,
//   PRIMARY KEY (id),
//   UNIQUE KEY pub_barnch_unique (code),
//   KEY idx_pub_branch_parent (parent_code)
// ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
// `)
// 		tk.MustExec(`
// CREATE VIEW udc_branch_test (
//   branch_id,
//   his_branch_id,
//   branch_code,
//   branch_name,
//   pid,
//   his_pid,
//   short_name,
//   inputcode1,
//   inputcode2,
//   org_no,
//   org_code,
//   org_level,
//   org_layer,
//   address,
//   state,
//   modify_by,
//   modify_time,
//   remark
// )
// AS
// SELECT a.id AS branch_id, a.id AS his_branch_id, a.code AS branch_code, a.name AS branch_name
//   , a.id + 1000000 AS pid, id AS his_pid, a.short_name AS short_name
//   , a.inputcode1 AS inputcode1, a.inputcode2 AS inputcode2, a.id AS org_no, a.code AS org_code, a.organ_level AS org_level
//   , a.organ_layer AS org_layer, a.address AS address, a.state AS state, a.modify_empid AS modify_by, a.modify_time AS modify_time
//   , NULL AS remark
// FROM pub_branch a
// WHERE organ_layer = 4
// UNION ALL
// SELECT a.id + 1000000 AS branch_id, a.id AS his_branch_id, a.code AS branch_code
//   , CONCAT(a.name, _UTF8MB4 '(中心)') AS branch_name
//   , (
//     SELECT id AS id
//     FROM pub_branch a
//     WHERE organ_layer = 2
//       AND state = 1
//     LIMIT 1
//   ) AS pid, id AS his_pid, a.short_name AS short_name, a.inputcode1 AS inputcode1, a.inputcode2 AS inputcode2
//   , a.id AS org_no, a.code AS org_code, a.organ_level AS org_level, a.organ_layer AS org_layer, a.address AS address
//   , a.state AS state, 1 AS modify_by, a.modify_time AS modify_time, NULL AS remark
// FROM pub_branch a
// WHERE organ_layer = 4
// UNION ALL
// SELECT a.id AS branch_id, a.id AS his_branch_id, a.code AS branch_code, a.name AS branch_name, NULL AS pid
//   , id AS his_pid, a.short_name AS short_name, a.inputcode1 AS inputcode1, a.inputcode2 AS inputcode2, a.id AS org_no
//   , a.code AS org_code, a.organ_level AS org_level, a.organ_layer AS org_layer, a.address AS address, a.state AS state
//   , a.modify_empid AS modify_by, a.modify_time AS modify_time, NULL AS remark
// FROM pub_branch a
// WHERE organ_layer = 2;
// `)
// 		tk.MustExec(`
// CREATE TABLE udc_branch_temp (
//   branch_id int(11) NOT NULL AUTO_INCREMENT COMMENT '',
//   his_branch_id varchar(20) DEFAULT NULL COMMENT '',
//   branch_code varchar(20) DEFAULT NULL COMMENT '',
//   branch_name varchar(64) NOT NULL COMMENT '',
//   pid int(11) DEFAULT NULL COMMENT '',
//   his_pid varchar(20) DEFAULT NULL COMMENT '',
//   short_name varchar(64) DEFAULT NULL COMMENT '',
//   inputcode1 varchar(12) DEFAULT NULL COMMENT '辅码1',
//   inputcode2 varchar(12) DEFAULT NULL COMMENT '辅码2',
//   org_no int(11) DEFAULT NULL COMMENT '',
//   org_code varchar(20) DEFAULT NULL COMMENT ',',
//   org_level tinyint(4) DEFAULT NULL COMMENT '',
//   org_layer tinyint(4) DEFAULT NULL COMMENT '',
//   address varchar(255) DEFAULT NULL COMMENT '机构地址',
//   state tinyint(4) NOT NULL DEFAULT '1' COMMENT '',
//   modify_by int(11) NOT NULL COMMENT '',
//   modify_time datetime DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '修改时间',
//   remark varchar(255) DEFAULT NULL COMMENT '备注',
//   PRIMARY KEY (branch_id) /*T![clustered_index] CLUSTERED */
// ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin AUTO_INCREMENT=1030102 COMMENT='';
// `)
// 		tk.MustGetErrCode(`
// SELECT res.*
// FROM (
//     (
//         WITH RECURSIVE d AS (
//             SELECT ub.*
//             FROM udc_branch_test ub
//             WHERE ub.branch_id = 1000102
//             UNION ALL
//             SELECT ub1.*
//             FROM udc_branch_test ub1
//             INNER JOIN d ON d.branch_id = ub1.pid
//         )
//         SELECT d.*
//         FROM d
//     )
// ) AS res
// WHERE res.state != 2
// ORDER BY res.branch_id;
// `, errno.ErrNotSupportedYet)
// 	})
// }
// */
use crate::main_test::load_named_sql_cases;
use astersql_parser::{NormalizeDigest, NormalizeDigestForBinding, NormalizeKeepHint, Parser};

#[derive(Debug, PartialEq, Eq)]
enum PlanShapeMismatch {
    RowCount {
        actual: usize,
        expected: usize,
    },
    RowWidth {
        row: usize,
        actual: usize,
        expected: usize,
    },
}

/// Go `getPlanRows`: tabs become one space and every newline is retained as a
/// row boundary, including the trailing empty row produced by `Split`.
fn get_plan_rows(plan: &str) -> Vec<String> {
    plan.replace('\t', " ")
        .split('\n')
        .map(str::to_owned)
        .collect()
}

/// Go `compareStringSlice` deliberately compares plan shape rather than text:
/// first row count, then each rendered row width.
fn compare_string_slice(actual: &[&str], expected: &[&str]) -> Result<(), PlanShapeMismatch> {
    if actual.len() != expected.len() {
        return Err(PlanShapeMismatch::RowCount {
            actual: actual.len(),
            expected: expected.len(),
        });
    }
    for (row, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        if actual.len() != expected.len() {
            return Err(PlanShapeMismatch::RowWidth {
                row,
                actual: actual.len(),
                expected: expected.len(),
            });
        }
    }
    Ok(())
}

/// 验证 NormalizeDigest 忽略 IN 列表与字符串字面量差异，但保持语句形状与 digest 一致。
#[test]
fn normalized_plan_digest_ignores_literals_but_keeps_shape() {
    // 两组字面量不同、结构相同的 SQL，规范化文本与 digest 字节应相等。
    let (left, left_digest) = NormalizeDigest("select * from t where a in (1,2,3) and b='x'");
    let (right, right_digest) = NormalizeDigest("select * from t where a in (7,8,9) and b='y'");
    assert_eq!(left, right);
    assert_eq!(left_digest.Bytes(), right_digest.Bytes());

    let queries = [
        "select * from t where a in (1, 2)",
        "select * from t where a in (1, 2, 3)",
        "select * from t where a in (1, 2, 3, 4, 5, 6, 7)",
    ];
    let (_, first_digest) = NormalizeDigest(queries[0]);
    for query in &queries[1..] {
        let (_, digest) = NormalizeDigest(query);
        assert_eq!(first_digest.Bytes(), digest.Bytes(), "{query}");
    }
}

/// 验证 binding 归一化去掉 hint，而 NormalizeKeepHint 保留 use_index。
#[test]
fn binding_normalization_and_hint_preservation_are_canonical() {
    // Binding 路径用于 SQL Binding 匹配；KeepHint 路径用于保留优化器提示文本。
    let sql = "select /*+ use_index(t, idx_a) */ * from t where a=42";
    let (binding, digest) = NormalizeDigestForBinding(sql);
    assert!(!binding.contains("use_index"));
    assert!(binding.contains("select"));
    assert_eq!(digest.Bytes().len(), 32);
    assert!(NormalizeKeepHint(sql).contains("use_index"));
}

/// Connect every Go plan-test name to its standard and Cascades fixture SQL.
#[test]
fn plan_suite_cases_are_loaded_and_parseable() {
    let names = [
        "TestNormalizedPlan",
        "TestNormalizedPlanForNextGen",
        "TestPreferRangeScan",
        "TestNormalizedPlanForDiffStore",
        "TestTiFlashLateMaterialization",
        "TestInvertedIndex",
    ];
    for cascades in [false, true] {
        for name in names {
            let cases = load_named_sql_cases("plan_normalized_suite", &[name], cascades);
            assert!(
                !cases.is_empty(),
                "missing plan fixture {name}, cascades={cascades}"
            );
            for sql in cases {
                Parser::default()
                    .Parse(&sql, "", "")
                    .unwrap_or_else(|error| panic!("plan fixture {name}/{sql:?}: {error}"));
            }
        }
    }
}

#[test]
fn plan_rows_match_go_tab_and_newline_semantics() {
    assert_eq!(get_plan_rows("a\tb\nc\t"), ["a b", "c "]);
    assert_eq!(get_plan_rows(""), [""]);
}

#[test]
fn plan_row_shape_comparison_matches_go_contract() {
    assert!(compare_string_slice(&["ab", " c"], &["xy", " z"]).is_ok());
    assert_eq!(
        compare_string_slice(&["ab"], &["x"]).unwrap_err(),
        PlanShapeMismatch::RowWidth {
            row: 0,
            actual: 2,
            expected: 1,
        }
    );
    assert_eq!(
        compare_string_slice(&["a"], &["a", "b"]).unwrap_err(),
        PlanShapeMismatch::RowCount {
            actual: 1,
            expected: 2,
        }
    );
}

#[test]
fn non_fixture_go_regression_sql_is_parseable() {
    let statements = [
        "select * from t where (a = 1 and b = 1) or not (a = 2 and b = 2)",
        "select (null = all (select c2 from t2)) is not unknown",
        "select count(*) from t1 left join t2 on t1.a <=> t2.a",
        "with recursive d as (select * from t union all select t.* from t inner join d on d.a=t.a) select * from d",
    ];
    for sql in statements {
        Parser::default()
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("Go plan regression SQL {sql:?}: {error}"));
    }
}
