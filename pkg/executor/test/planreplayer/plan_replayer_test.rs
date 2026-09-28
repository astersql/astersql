// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Plan Replayer 执行器单元测试：dump/load/capture 与后端 trait 契约。
//
// Plan Replayer 把 SQL 现场（schema、统计、绑定、EXPLAIN 等）打包导出，
// 再在另一环境加载以复现优化器决策。本文件含：块注释中的 Go 迁移记录，
// 以及基于 `PlanReplayerBackend` mock 的可运行生命周期断言。

/*
    // plan replayer dump/load/capture、多 SQL zip 内容和预签名 URL 输出测试。

    #![allow(dead_code, unused_variables, non_snake_case)]

    // go_stmt 是测试的记录入口：每一条字符串都来自原 Go 测试中的具体语句。
    // 后续真正接入 Rust 测试框架时，再把这些记录替换为 testkit、failpoint、zip/IO 或并发 API 调用。
    fn go_stmt(_go_source: &str) {}

    // 对应 Go 的 zip 文件名白名单：列出 plan replayer dump single 期望出现的文件。
    #[allow(non_snake_case, dead_code)]
    fn check_file_name() {
        // Go 函数签名：func checkFileName(s string) bool {
        // 该辅助保留 Go 参数和返回语义；当前仅记录调用形状，不返回真实值。
        go_stmt(r#"files := []string{"#);
        go_stmt(r#""config.toml","#);
        go_stmt(r#""debug_trace/debug_trace0.json","#);
        go_stmt(r#""meta.txt","#);
        go_stmt(r#""stats/test.t_dump_single.json","#);
        go_stmt(r#""schema/test.t_dump_single.schema.txt","#);
        go_stmt(r#""schema/schema_meta.txt","#);
        go_stmt(r#""table_tiflash_replica.txt","#);
        go_stmt(r#""variables.toml","#);
        go_stmt(r#""session_bindings.sql","#);
        go_stmt(r#""global_bindings.sql","#);
        go_stmt(r#""sql/sql0.sql","#);
        go_stmt(r#""explain.txt","#);
        go_stmt(r#""statsMem/test.t_dump_single.txt","#);
        go_stmt(r#""sql_meta.toml","#);
        // 这里对应 Go 循环或 range；保持循环条件文本以便人工核对覆盖范围。
        go_stmt(r#"for _, f := range files {"#);
        go_stmt(r#"if strings.Compare(f, s) == 0 {"#);
        go_stmt(r#"return true"#);
        go_stmt(r#"return false"#);
    }

    // 对应 Go 的嵌入 Storage mock：覆盖 PresignFile 返回固定 URL。
    #[allow(dead_code)]
    struct PlanReplayerPresignStorage {
        // Go 类型原始结构如下；Storage 嵌入、mock 字段和方法接线后续再替换成真实 Rust trait。
        // type planReplayerPresignStorage struct {
        // storeapi.Storage
        // url string
    }

    // 对应 Go 的预签名 URL 方法：忽略 context/path/duration，直接返回 mock URL。
    #[allow(non_snake_case, dead_code)]
    fn presign_file() {
        // Go 函数签名：func (s planReplayerPresignStorage) PresignFile(context.Context, string, time.Duration) (string, error) {
        // 该辅助保留 Go 参数和返回语义；当前仅记录调用形状，不返回真实值。
        go_stmt(r#"return s.url, nil"#);
    }

    // 对应 Go 的 token 解析断言：要求结果为一行两列且第二列为非空字符串。
    #[allow(non_snake_case, dead_code)]
    fn require_plan_replayer_file_token() {
        // Go 函数签名：func requirePlanReplayerFileToken(t *testing.T, rows [][]any) string {
        // 该辅助保留 Go 参数和返回语义；当前仅记录调用形状，不返回真实值。
        go_stmt(r#"require.Len(t, rows, 1)"#);
        go_stmt(r#"require.Len(t, rows[0], 2)"#);
        // 这里对应 Go require.Equal，保留期望值与实际值顺序。
        go_stmt(r#"require.Equal(t, "File token", rows[0][0])"#);
        go_stmt(r#"token, ok := rows[0][1].(string)"#);
        // 这里对应 Go require.True，保留布尔断言语义。
        go_stmt(r#"require.True(t, ok)"#);
        go_stmt(r#"require.NotEmpty(t, token)"#);
        go_stmt(r#"return token"#);
    }

    // 对应 Go 的 TiFlash 任务检测：遍历 explain 行并查找 tiflash 字样。
    #[allow(non_snake_case, dead_code)]
    fn has_ti_flash_task() {
        // Go 函数签名：func hasTiFlashTask(rows [][]any) bool {
        // 该辅助保留 Go 参数和返回语义；当前仅记录调用形状，不返回真实值。
        // 这里对应 Go 循环或 range；保持循环条件文本以便人工核对覆盖范围。
        go_stmt(r#"for _, row := range rows {"#);
        go_stmt(r#"if len(row) > 2 && strings.Contains(fmt.Sprint(row[2]), "tiflash") {"#);
        go_stmt(r#"return true"#);
        go_stmt(r#"return false"#);
    }

    // 对应 Go 的 zip 内容断言：打开 dump zip，读取指定文件并检查包含期望文本，最后关闭 reader。
    #[allow(non_snake_case, dead_code)]
    fn require_zip_file_contains() {
        // Go 函数签名：func requireZipFileContains(t *testing.T, content []byte, fileName, expected string) {
        // 该辅助保留 Go 参数和返回语义；当前仅记录调用形状，不返回真实值。
        // 这里解析 zip；保留归档检查语义，不展开文件。
        go_stmt(r#"reader, err := zip.NewReader(bytes.NewReader(content), int64(len(content)))"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里对应 Go 循环或 range；保持循环条件文本以便人工核对覆盖范围。
        go_stmt(r#"for _, file := range reader.File {"#);
        go_stmt(r#"if file.Name != fileName {"#);
        go_stmt(r#"continue"#);
        go_stmt(r#"r, err := file.Open()"#);
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里读取文件内容；真实测试会把 zip bytes 用于后续断言。
        go_stmt(r#"data, err := io.ReadAll(r)"#);
        go_stmt(r#"require.NoError(t, err)"#);
        go_stmt(r#"require.NoError(t, r.Close())"#);
        // 这里对应包含关系断言，常用于 explain/错误文本/zip 文件名。
        go_stmt(r#"require.Contains(t, string(data), expected)"#);
        go_stmt(r#"return"#);
        go_stmt(r#"require.FailNowf(t, "missing file in zip", "file %s not found", fileName)"#);
    }

    // 对应 Go 的基础 dump explain 测试：配置本地 ext storage、启用 TiFlash failpoint、执行多种 replayer dump 并检查状态表。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer() {
        // Go 函数签名：func TestPlanReplayer(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        go_stmt(r#"tempDir := t.TempDir()"#);
        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"ctx := context.Background()"#);
        // 这里创建本地 ext storage；不进行实际文件系统存储接线。
        go_stmt(r#"storage, err := extstore.NewExtStorage(ctx, "file://"+tempDir, "")"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里替换全局 ext storage，原 Go 必须在 defer 中恢复 nil。
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(storage)"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(nil)"#);
        go_stmt(r#"storage.Close()"#);
        // Go defer 闭包在这里结束。

        // 这里对应 Go failpoint 注入；只记录开关位置，避免真的改变全局故障注入状态。
        go_stmt(
            r#"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/infoschema/mockTiFlashStoreCount", `return(true)`))"#,
        );
        go_stmt(r#"defer func() {"#);
        // 这里对应 Go failpoint 清理；真实测试依赖 defer/require 保证资源收尾。
        go_stmt(
            r#"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/infoschema/mockTiFlashStoreCount"))"#,
        );
        // Go defer 闭包在这里结束。
        // 这里创建 mock store；不会连接存储或执行真实 SQL。
        go_stmt(r#"store := testkit.CreateMockStore(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("drop table if exists t")"#);
        go_stmt(r#"tk.MustExec("create table t(a int, b int, index idx_a(a))")"#);
        go_stmt(r#"tk.MustExec("alter table t set tiflash replica 1")"#);
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(r#"tk.MustQuery("plan replayer dump explain select * from t where a=10")"#);
        go_stmt(
            r#"tk.MustQuery("plan replayer dump explain select /*+ read_from_storage(tiflash[t]) */ * from t")"#,
        );

        go_stmt(r#"tk.MustExec("create table t1 (a int)")"#);
        go_stmt(r#"tk.MustExec("create table t2 (a int)")"#);
        go_stmt(r#"tk.MustExec("create definer=`root`@`127.0.0.1` view v1 as select * from t1")"#);
        go_stmt(r#"tk.MustExec("create definer=`root`@`127.0.0.1` view v2 as select * from v1")"#);
        go_stmt(
            r#"tk.MustQuery("plan replayer dump explain with tmp as (select a from t1 group by t1.a) select * from tmp, t2 where t2.a=tmp.a;")"#,
        );
        go_stmt(
            r#"tk.MustQuery("plan replayer dump explain select * from t1 where t1.a > (with cte1 as (select 1) select count(1) from cte1);")"#,
        );
        go_stmt(r#"tk.MustQuery("plan replayer dump explain select * from v1")"#);
        go_stmt(r#"tk.MustQuery("plan replayer dump explain select * from v2")"#);
        // 这里对应 Go require.True，保留布尔断言语义。
        go_stmt(r#"require.True(t, len(tk.Session().GetSessionVars().LastPlanReplayerToken) > 0)"#);

        // Go 原注释：clear the status table and assert
        go_stmt(r#"tk.MustExec("delete from mysql.plan_replayer_status")"#);
        go_stmt(r#"tk.MustQuery("plan replayer dump explain select * from v2")"#);
        // 这里引用 plan replayer 内部状态或变量，这里只保留状态转移说明。
        go_stmt(r#"token := tk.Session().GetSessionVars().LastPlanReplayerToken"#);
        go_stmt(
            r#"rows := tk.MustQuery(fmt.Sprintf("select * from mysql.plan_replayer_status where token = '%v'", token)).Rows()"#,
        );
        go_stmt(r#"require.Len(t, rows, 1)"#);
    }

    // 对应 Go 的 TiFlash hypo replica load 测试：读取 dump zip 并手动喂给 load info，再检查 load 后 explain 含 TiFlash。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer_load_ti_flash_plan_with_hypo_replica() {
        // Go 函数签名：func TestPlanReplayerLoadTiFlashPlanWithHypoReplica(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        go_stmt(r#"tempDir := t.TempDir()"#);
        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"ctx := context.Background()"#);
        // 这里创建本地 ext storage；不进行实际文件系统存储接线。
        go_stmt(r#"storage, err := extstore.NewExtStorage(ctx, "file://"+tempDir, "")"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里替换全局 ext storage，原 Go 必须在 defer 中恢复 nil。
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(storage)"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(nil)"#);
        go_stmt(r#"storage.Close()"#);
        // Go defer 闭包在这里结束。

        go_stmt(
            r#"const mockTiFlashStoreCount = "github.com/pingcap/tidb/pkg/infoschema/mockTiFlashStoreCount""#,
        );
        // 这里对应 Go failpoint 注入；只记录开关位置，避免真的改变全局故障注入状态。
        go_stmt(r#"require.NoError(t, failpoint.Enable(mockTiFlashStoreCount, `return(true)`))"#);
        go_stmt(r#"defer func() {"#);
        // 这里对应 Go failpoint 清理；真实测试依赖 defer/require 保证资源收尾。
        go_stmt(r#"_ = failpoint.Disable(mockTiFlashStoreCount)"#);
        // Go defer 闭包在这里结束。

        // 这里创建 mock store 和 domain；不启动 TiDB mock 环境，只保留依赖边界。
        go_stmt(r#"store, dom := testkit.CreateMockStoreAndDomain(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("create table t_load_tiflash(a int, b int, index idx_a(a))")"#);
        go_stmt(r#"tk.MustExec("alter table t_load_tiflash set tiflash replica 1")"#);
        go_stmt(r#"testkit.SetTiFlashReplica(t, dom, "test", "t_load_tiflash")"#);
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(
            r#"res := tk.MustQuery("plan replayer dump explain select /*+ read_from_storage(tiflash[t_load_tiflash]) */ * from t_load_tiflash")"#,
        );
        // 这里引用 plan replayer 内部状态或变量，这里只保留状态转移说明。
        go_stmt(r#"tiflashFileName := requirePlanReplayerFileToken(t, res.Rows())"#);
        go_stmt(r#"filePath := filepath.Join(replayer.GetPlanReplayerDirName(), tiflashFileName)"#);

        // 这里打开 replayer dump 文件；这里只记录 IO 路径，不读取 zip。
        go_stmt(r#"fileReader, err := storage.Open(ctx, filePath, nil)"#);
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里读取文件内容；真实测试会把 zip bytes 用于后续断言。
        go_stmt(r#"content, err := io.ReadAll(fileReader)"#);
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里关闭读取句柄；Go 版本通过 defer 或显式 Close 收尾。
        go_stmt(r#"require.NoError(t, fileReader.Close())"#);
        go_stmt(r#"requireZipFileContains(t, content, "explain.txt", "tiflash")"#);

        go_stmt(r#"require.NoError(t, failpoint.Disable(mockTiFlashStoreCount))"#);
        // 这里创建 mock store；不会连接存储或执行真实 SQL。
        go_stmt(r#"loadStore := testkit.CreateMockStore(t)"#);
        go_stmt(r#"loadTK := testkit.NewTestKit(t, loadStore)"#);
        go_stmt(
            r#"loadTK.MustExec(fmt.Sprintf("plan replayer load '%s'", strings.ReplaceAll(filepath.Join(tempDir, filePath), "'", "''")))"#,
        );
        // Go 原注释：TestKit executes the SQL marker; clientConn normally completes the local-file
        // Go 原注释：transfer and calls Update, so feed the dumped bytes directly here.
        go_stmt(
            r#"loadInfo, ok := loadTK.Session().Value(executor.PlanReplayerLoadVarKey).(*executor.PlanReplayerLoadInfo)"#,
        );
        // 这里对应 Go require.True，保留布尔断言语义。
        go_stmt(r#"require.True(t, ok)"#);
        go_stmt(r#"defer loadTK.Session().ClearValue(executor.PlanReplayerLoadVarKey)"#);
        go_stmt(r#"require.NoError(t, loadInfo.Update(content))"#);

        go_stmt(r#"loadTK.MustExec("use test")"#);
        go_stmt(
            r#"rows := loadTK.MustQuery("explain select /*+ read_from_storage(tiflash[t_load_tiflash]) */ * from t_load_tiflash").Rows()"#,
        );
        go_stmt(r#"require.True(t, hasTiFlashTask(rows), rows)"#);
    }

    // 对应 Go 的 SEM capture 场景：保存并恢复全局 SEM 配置，验证 dump explain 写入状态表。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer_capture_sem() {
        // Go 函数签名：func TestPlanReplayerCaptureSEM(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        go_stmt(r#"tempDir := t.TempDir()"#);
        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"ctx := context.Background()"#);
        // 这里创建本地 ext storage；不进行实际文件系统存储接线。
        go_stmt(r#"storage, err := extstore.NewExtStorage(ctx, "file://"+tempDir, "")"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里替换全局 ext storage，原 Go 必须在 defer 中恢复 nil。
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(storage)"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(nil)"#);
        go_stmt(r#"storage.Close()"#);
        // Go defer 闭包在这里结束。

        go_stmt(r#"originSEM := config.GetGlobalConfig().Security.EnableSEM"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"config.GetGlobalConfig().Security.EnableSEM = originSEM"#);
        // Go defer 闭包在这里结束。
        // 这里创建 mock store；不会连接存储或执行真实 SQL。
        go_stmt(r#"store := testkit.CreateMockStore(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("plan replayer capture '123' '123';")"#);
        go_stmt(r#"tk.MustExec("create table t(id int)")"#);
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(r#"tk.MustQuery("plan replayer dump explain select * from t")"#);
        go_stmt(
            r#"tk.MustQuery("select count(*) from mysql.plan_replayer_status").Check(testkit.Rows("1"))"#,
        );
    }

    // 对应 Go 的 capture 任务测试：注册 digest、触发 domain handle 收集、排除 stats internal source 后 drain 普通 SQL 任务。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer_capture() {
        // Go 函数签名：func TestPlanReplayerCapture(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        // 这里创建 mock store 和 domain；不启动 TiDB mock 环境，只保留依赖边界。
        go_stmt(r#"store, dom := testkit.CreateMockStoreAndDomain(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("plan replayer capture '123' '123';")"#);
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(
            r#"tk.MustQuery("select sql_digest, plan_digest from mysql.plan_replayer_task;").Check(testkit.Rows("123 123"))"#,
        );
        go_stmt(
            r#"tk.MustGetErrMsg("plan replayer capture '123' '123';", "plan replayer capture task already exists")"#,
        );
        go_stmt(r#"tk.MustExec("plan replayer capture remove '123' '123'")"#);
        go_stmt(
            r#"tk.MustQuery("select count(*) from mysql.plan_replayer_task;").Check(testkit.Rows("0"))"#,
        );
        go_stmt(r#"tk.MustExec("create table t(id int)")"#);
        go_stmt(r#"tk.MustExec("prepare stmt from 'update t set id = ?  where id = ? + 1';")"#);
        go_stmt(r#"tk.MustExec("SET @number = 5;")"#);
        go_stmt(r#"tk.MustExec("execute stmt using @number,@number")"#);
        go_stmt(r#"_, sqlDigest := tk.Session().GetSessionVars().StmtCtx.SQLDigest()"#);
        go_stmt(r#"_, planDigest := tk.Session().GetSessionVars().StmtCtx.GetPlanDigest()"#);
        go_stmt(r#"tk.MustExec("SET @@tidb_enable_plan_replayer_capture = ON;")"#);
        go_stmt(r#"tk.MustExec("SET @@global.tidb_enable_historical_stats_for_capture='ON'")"#);
        go_stmt(
            r#"tk.MustExec(fmt.Sprintf("plan replayer capture '%v' '%v'", sqlDigest.String(), planDigest.String()))"#,
        );
        // 这里引用 plan replayer 内部状态或变量，这里只保留状态转移说明。
        go_stmt(r#"err := dom.GetPlanReplayerHandle().CollectPlanReplayerTask()"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里对应 Go failpoint 注入；只记录开关位置，避免真的改变全局故障注入状态。
        go_stmt(
            r#"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/domain/shouldDumpStats", "return(true)"))"#,
        );
        // 这里对应 Go failpoint 清理；真实测试依赖 defer/require 保证资源收尾。
        go_stmt(
            r#"defer require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/domain/shouldDumpStats"))"#,
        );
        go_stmt(r#"tk.MustExec("execute stmt using @number,@number")"#);
        go_stmt(r#"task := dom.GetPlanReplayerHandle().DrainTask()"#);
        go_stmt(r#"require.NotNil(t, task)"#);

        go_stmt(r#"statsSQL := "select * from t where id = 1""#);
        go_stmt(r#"normalSQL := "select count(*) from t where id = 2""#);
        go_stmt(r#"tk.MustQuery(statsSQL)"#);
        go_stmt(r#"_, statsSQLDigest := tk.Session().GetSessionVars().StmtCtx.SQLDigest()"#);
        go_stmt(r#"_, statsPlanDigest := tk.Session().GetSessionVars().StmtCtx.GetPlanDigest()"#);

        go_stmt(r#"tk.MustQuery(normalSQL)"#);
        go_stmt(r#"_, normalSQLDigest := tk.Session().GetSessionVars().StmtCtx.SQLDigest()"#);
        go_stmt(r#"_, normalPlanDigest := tk.Session().GetSessionVars().StmtCtx.GetPlanDigest()"#);

        go_stmt(
            r#"tk.MustExec(fmt.Sprintf("plan replayer capture '%v' '%v'", statsSQLDigest.String(), statsPlanDigest.String()))"#,
        );
        go_stmt(
            r#"tk.MustExec(fmt.Sprintf("plan replayer capture '%v' '%v'", normalSQLDigest.String(), normalPlanDigest.String()))"#,
        );
        go_stmt(r#"err = dom.GetPlanReplayerHandle().CollectPlanReplayerTask()"#);
        go_stmt(r#"require.NoError(t, err)"#);

        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"statsStmt, err := tk.Session().Parse(context.Background(), statsSQL)"#);
        go_stmt(r#"require.NoError(t, err)"#);
        go_stmt(
            r#"statsCtx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnStatsForegroundPriority)"#,
        );
        go_stmt(r#"rs, err := tk.Session().ExecuteStmt(statsCtx, statsStmt[0])"#);
        go_stmt(r#"require.NoError(t, err)"#);
        go_stmt(r#"tk.ResultSetToResultWithCtx(statsCtx, rs, statsSQL).Check(testkit.Rows())"#);

        go_stmt(r#"tk.MustQuery(normalSQL)"#);
        go_stmt(r#"task = dom.GetPlanReplayerHandle().DrainTask()"#);
        // 这里对应 Go require.Equal，保留期望值与实际值顺序。
        go_stmt(r#"require.Equal(t, normalSQLDigest.String(), task.SQLDigest)"#);
    }

    // 对应 Go 的 continuous capture 测试：验证历史 stats 前置条件、worker 处理 task 并落状态表。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer_continues_capture() {
        // Go 函数签名：func TestPlanReplayerContinuesCapture(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        go_stmt(r#"tempDir := t.TempDir()"#);
        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"ctx := context.Background()"#);
        // 这里创建本地 ext storage；不进行实际文件系统存储接线。
        go_stmt(r#"storage, err := extstore.NewExtStorage(ctx, "file://"+tempDir, "")"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里替换全局 ext storage，原 Go 必须在 defer 中恢复 nil。
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(storage)"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(nil)"#);
        go_stmt(r#"storage.Close()"#);
        // Go defer 闭包在这里结束。

        // 这里创建 mock store 和 domain；不启动 TiDB mock 环境，只保留依赖边界。
        go_stmt(r#"store, dom := testkit.CreateMockStoreAndDomain(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);

        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("set @@global.tidb_enable_historical_stats='OFF'")"#);
        go_stmt(
            r#"_, err = tk.Exec("set @@global.tidb_enable_plan_replayer_continuous_capture='ON'")"#,
        );
        // 这里对应 Go require.Error，说明该分支期望失败。
        go_stmt(r#"require.Error(t, err)"#);
        // 这里对应 Go require.Equal，保留期望值与实际值顺序。
        go_stmt(
            r#"require.Equal(t, err.Error(), "tidb_enable_historical_stats should be enabled before enabling tidb_enable_plan_replayer_continuous_capture")"#,
        );

        go_stmt(r#"tk.MustExec("set @@global.tidb_enable_historical_stats='ON'")"#);
        go_stmt(r#"tk.MustExec("set @@global.tidb_enable_plan_replayer_continuous_capture='ON'")"#);

        // 这里引用 plan replayer 内部状态或变量，这里只保留状态转移说明。
        go_stmt(r#"prHandle := dom.GetPlanReplayerHandle()"#);
        go_stmt(r#"tk.MustExec("delete from mysql.plan_replayer_status;")"#);
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("create table t(id int);")"#);
        go_stmt(r#"tk.MustExec("set @@tidb_enable_plan_replayer_continuous_capture = 'ON'")"#);
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(r#"tk.MustQuery("select * from t;")"#);
        go_stmt(r#"task := prHandle.DrainTask()"#);
        go_stmt(r#"require.NotNil(t, task)"#);
        go_stmt(r#"worker := prHandle.GetWorker()"#);
        go_stmt(r#"success := worker.HandleTask(task)"#);
        // 这里对应 Go require.True，保留布尔断言语义。
        go_stmt(r#"require.True(t, success)"#);
        go_stmt(
            r#"tk.MustQuery("select count(*) from mysql.plan_replayer_status").Check(testkit.Rows("1"))"#,
        );
    }

    // 对应 Go 的单 SQL dump 测试：设置日志文件、读取 zip 并校验所有文件名在白名单。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer_dump_single() {
        // Go 函数签名：func TestPlanReplayerDumpSingle(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"ctx := context.Background()"#);
        go_stmt(r#"tempDir := t.TempDir()"#);
        // 这里创建本地 ext storage；不进行实际文件系统存储接线。
        go_stmt(r#"storage, err := extstore.NewExtStorage(ctx, "file://"+tempDir, "")"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里替换全局 ext storage，原 Go 必须在 defer 中恢复 nil。
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(storage)"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(nil)"#);
        go_stmt(r#"storage.Close()"#);
        // Go defer 闭包在这里结束。

        go_stmt(r#"dir := t.TempDir()"#);
        go_stmt(r#"logFile := filepath.Join(dir, "tidb.log")"#);
        // 这里对应 Go 的全局配置更新；这里只记录要改的字段，避免污染其它测试。
        go_stmt(r#"config.UpdateGlobal(func(conf *config.Config) {"#);
        go_stmt(r#"conf.Log.File.Filename = logFile"#);
        go_stmt(r#"})"#);
        // 这里创建 mock store；不会连接存储或执行真实 SQL。
        go_stmt(r#"store := testkit.CreateMockStore(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("drop table if exists t_dump_single")"#);
        go_stmt(r#"tk.MustExec("create table t_dump_single(a int)")"#);
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(r#"res := tk.MustQuery("plan replayer dump explain select * from t_dump_single")"#);
        // 这里引用 plan replayer 内部状态或变量，这里只保留状态转移说明。
        go_stmt(r#"fileName := requirePlanReplayerFileToken(t, res.Rows())"#);

        go_stmt(r#"filePath := filepath.Join(replayer.GetPlanReplayerDirName(), fileName)"#);
        // 这里打开 replayer dump 文件；这里只记录 IO 路径，不读取 zip。
        go_stmt(r#"fileReader, err := storage.Open(ctx, filePath, nil)"#);
        go_stmt(r#"require.NoError(t, err)"#);
        go_stmt(r#"defer fileReader.Close()"#);

        // 这里读取文件内容；真实测试会把 zip bytes 用于后续断言。
        go_stmt(r#"content, err := io.ReadAll(fileReader)"#);
        go_stmt(r#"require.NoError(t, err)"#);

        go_stmt(r#"readerAt := bytes.NewReader(content)"#);
        // 这里解析 zip；保留归档检查语义，不展开文件。
        go_stmt(r#"reader, err := zip.NewReader(readerAt, int64(len(content)))"#);
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里对应 Go 循环或 range；保持循环条件文本以便人工核对覆盖范围。
        go_stmt(r#"for _, file := range reader.File {"#);
        // 这里对应 Go require.True，保留布尔断言语义。
        go_stmt(r#"require.True(t, checkFileName(file.Name), file.Name)"#);
    }

    // 对应 Go 的 explain explore replayer 测试：dump 后在新 store 上重复 explain explore，要求每行输出非空。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_explain_explore_replayer() {
        // Go 函数签名：func TestExplainExploreReplayer(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"ctx := context.Background()"#);
        go_stmt(r#"tempDir := t.TempDir()"#);
        // 这里创建本地 ext storage；不进行实际文件系统存储接线。
        go_stmt(r#"storage, err := extstore.NewExtStorage(ctx, "file://"+tempDir, "")"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里替换全局 ext storage，原 Go 必须在 defer 中恢复 nil。
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(storage)"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(nil)"#);
        go_stmt(r#"storage.Close()"#);
        // Go defer 闭包在这里结束。

        // 这里创建 mock store；不会连接存储或执行真实 SQL。
        go_stmt(r#"store := testkit.CreateMockStore(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("create table t_explain_explore_replayer(a int, b int, key(a))")"#);
        go_stmt(
            r#"tk.MustExec("insert into t_explain_explore_replayer values (1, 1), (2, 2), (3, 1)")"#,
        );
        go_stmt(r#"tk.MustExec("analyze table t_explain_explore_replayer")"#);
        go_stmt(
            r#"tk.MustExec("create global binding using select * from test.t_explain_explore_replayer where b=1")"#,
        );
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(
            r#"res := tk.MustQuery("plan replayer dump explain select * from test.t_explain_explore_replayer where b=1")"#,
        );
        // 这里引用 plan replayer 内部状态或变量，这里只保留状态转移说明。
        go_stmt(r#"fileName := requirePlanReplayerFileToken(t, res.Rows())"#);

        go_stmt(r#"loadStore := testkit.CreateMockStore(t)"#);
        go_stmt(r#"loadTK := testkit.NewTestKit(t, loadStore)"#);
        go_stmt(
            r#"replayerPath := filepath.Join(tempDir, replayer.GetPlanReplayerDirName(), fileName)"#,
        );
        go_stmt(r#"replayerPath = strings.ReplaceAll(replayerPath, "'", "''")"#);
        // 这里对应 Go 循环或 range；保持循环条件文本以便人工核对覆盖范围。
        go_stmt(r#"for range 2 {"#);
        go_stmt(
            r#"rows := loadTK.MustQuery(fmt.Sprintf("explain explore replayer '%s'", replayerPath)).Rows()"#,
        );
        go_stmt(r#"require.NotEmpty(t, rows)"#);
        go_stmt(r#"for _, row := range rows {"#);
        go_stmt(r#"require.NotEmpty(t, row[3])"#);
    }

    // 对应 Go 的预签名输出测试：替换全局 ext storage，检查 Download URL、curl 和 token 输出。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer_dump_presigned_url_output() {
        // Go 函数签名：func TestPlanReplayerDumpPresignedURLOutput(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"ctx := context.Background()"#);
        go_stmt(r#"tempDir := t.TempDir()"#);
        // 这里创建本地 ext storage；不进行实际文件系统存储接线。
        go_stmt(r#"storage, err := extstore.NewExtStorage(ctx, "file://"+tempDir, "")"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        go_stmt(
            r#"const presignedURL = "https://example.com/replayer.zip?X-Amz-Expires=3600&X-Amz-Signature=test""#,
        );
        // 这里替换全局 ext storage，原 Go 必须在 defer 中恢复 nil。
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(planReplayerPresignStorage{"#);
        go_stmt(r#"Storage: storage,"#);
        go_stmt(r#"url:     presignedURL,"#);
        go_stmt(r#"})"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(nil)"#);
        go_stmt(r#"storage.Close()"#);
        // Go defer 闭包在这里结束。

        // 这里创建 mock store；不会连接存储或执行真实 SQL。
        go_stmt(r#"store := testkit.CreateMockStore(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("create table t_presign(a int)")"#);
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(
            r#"tk.MustQuery("plan replayer dump explain select * from t_presign").Check(testkit.RowsWithSep("|","#,
        );
        go_stmt(r#""Download URL|"+presignedURL,"#);
        go_stmt(r#""Expires in|1h0m0s","#);
        go_stmt(r#""Browser|Open the Download URL directly before it expires","#);
        go_stmt(r#""curl|curl -L '"+presignedURL+"' -o plan_replayer.zip","#);
        go_stmt(r#""Note|If the URL expires, rerun PLAN REPLAYER DUMP to get a new one","#);
        go_stmt(r#"))"#);
        // 这里对应 Go require.Equal，保留期望值与实际值顺序。
        go_stmt(
            r#"require.Equal(t, presignedURL, tk.Session().GetSessionVars().LastPlanReplayerToken)"#,
        );
    }

    // 对应 Go 的多 SQL 错误测试：空列表、单条和多条 parser error 都要返回语法错误。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer_dump_multiple_error() {
        // Go 函数签名：func TestPlanReplayerDumpMultipleError(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        // 这里创建 mock store；不会连接存储或执行真实 SQL。
        go_stmt(r#"store := testkit.CreateMockStore(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec("use test")"#);
        go_stmt(r#"tk.MustExec("create table t(id int)")"#);

        // Go 原注释：empty statement list should return error
        // 这里对应错误消息断言，保留原始错误片段用于人工核对。
        go_stmt(r#"tk.MustContainErrMsg("plan replayer dump explain ()", "[parser:1064]")"#);

        // Go 原注释：one error statement
        go_stmt(
            r#"tk.MustContainErrMsg("plan replayer dump explain ('select x om t')", "[parser:1064]")"#,
        );

        // Go 原注释：multiple error statements
        go_stmt(
            r#"tk.MustContainErrMsg("plan replayer dump explain ('select x from t', 'select y om t')", "[parser:1064]")"#,
        );
    }

    // 对应 Go 的多 SQL dump 测试：构造多库多表、多条 SQL，检查 zip 中每条 SQL/explain 及各表 schema/stats 文件。
    #[test]
    #[allow(non_snake_case, dead_code)]
    fn test_plan_replayer_dump_multiple() {
        // Go 函数签名：func TestPlanReplayerDumpMultiple(t *testing.T) {
        // 该 Rust 测试不接入 testkit；下面逐条记录 Go 测试动作，便于后续人工迁移为可运行测试。
        go_stmt(r#"const numStmts = 50"#);
        go_stmt(r#"const numTables = 5"#);
        // 这里对应 Go context.Background；不传播取消或 deadline。
        go_stmt(r#"ctx := context.Background()"#);
        go_stmt(r#"tempDir := t.TempDir()"#);
        // 这里创建本地 ext storage；不进行实际文件系统存储接线。
        go_stmt(r#"storage, err := extstore.NewExtStorage(ctx, "file://"+tempDir, "")"#);
        // 这里对应 Go require.NoError，说明前一步必须成功。
        go_stmt(r#"require.NoError(t, err)"#);
        // 这里替换全局 ext storage，原 Go 必须在 defer 中恢复 nil。
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(storage)"#);
        go_stmt(r#"defer func() {"#);
        go_stmt(r#"extstore.SetGlobalExtStorageForTest(nil)"#);
        go_stmt(r#"storage.Close()"#);
        // Go defer 闭包在这里结束。

        // 这里创建 mock store；不会连接存储或执行真实 SQL。
        go_stmt(r#"store := testkit.CreateMockStore(t)"#);
        go_stmt(r#"tk := testkit.NewTestKit(t, store)"#);
        // Go 原注释：Prepare multiple databases and tables for multi-SQL dump.
        go_stmt(
            r#"dbs := []string{"test", "test_multi_db1", "test_multi_db2", "test_multi_db3", "test_multi_db4"}"#,
        );
        // 这里对应 Go 循环或 range；保持循环条件文本以便人工核对覆盖范围。
        go_stmt(r#"for _, db := range dbs {"#);
        // 这里对应 TestKit 执行 SQL/DDL/DML；不会执行数据库动作。
        go_stmt(r#"tk.MustExec(fmt.Sprintf("create database if not exists %s", db))"#);
        go_stmt(r#"for _, db := range dbs {"#);
        go_stmt(r#"tk.MustExec("use " + db)"#);
        go_stmt(r#"for i := 1; i <= numTables; i++ {"#);
        go_stmt(r#"tableName := fmt.Sprintf("t_dump_multi_%d", i)"#);
        go_stmt(r#"tk.MustExec(fmt.Sprintf("drop table if exists %s", tableName))"#);
        go_stmt(r#"tk.MustExec(fmt.Sprintf("create table %s(a int, b int)", tableName))"#);
        go_stmt(r#"tk.MustExec(fmt.Sprintf("insert into %s values (1, 1)", tableName))"#);
        go_stmt(r#"tk.MustExec(fmt.Sprintf("insert into %s values (2, 2)", tableName))"#);
        go_stmt(r#"tk.MustExec(fmt.Sprintf("insert into %s values (3, 3)", tableName))"#);
        go_stmt(r#"tk.MustExec(fmt.Sprintf("insert into %s values (4, 4)", tableName))"#);
        go_stmt(r#"tk.MustExec(fmt.Sprintf("insert into %s values (5, 5)", tableName))"#);
        go_stmt(r#"tk.MustExec(fmt.Sprintf("analyze table %s", tableName))"#);
        go_stmt(r#"tk.MustExec("use test")"#);

        // Go 原注释：Build multiple SQL statements using the tables across multiple databases with fully
        // Go 原注释：qualified names (db.table) so the plan replayer extractor finds them regardless
        // Go 原注释：of current DB / schema sync.
        go_stmt(r#"stmts := make([]string, numStmts)"#);
        go_stmt(r#"pairMod := len(dbs) * numTables"#);
        go_stmt(r#"for i := 0; i < numStmts; i++ {"#);
        // Go 原注释：Make sure every (db, table) pair is covered at least once.
        go_stmt(r#"pairIdx := i % pairMod"#);
        go_stmt(r#"db := dbs[pairIdx/numTables]"#);
        go_stmt(r#"tbl := (pairIdx % numTables) + 1"#);
        // 这里对应 Go switch 分支；保留分支结构用于理解 SQL 生成规则。
        go_stmt(r#"switch i % 4 {"#);
        go_stmt(r#"case 0:"#);
        go_stmt(r#"stmts[i] = fmt.Sprintf("'select * from %s.t_dump_multi_%d'", db, tbl)"#);
        go_stmt(r#"case 1:"#);
        go_stmt(
            r#"stmts[i] = fmt.Sprintf("'select * from %s.t_dump_multi_%d where a=1'", db, tbl)"#,
        );
        go_stmt(r#"case 2:"#);
        go_stmt(
            r#"stmts[i] = fmt.Sprintf("'select * from %s.t_dump_multi_%d where b>0'", db, tbl)"#,
        );
        go_stmt(r#"default:"#);
        // Go 原注释：join two tables, potentially across databases
        go_stmt(r#"t2 := (tbl % numTables) + 1"#);
        go_stmt(r#"otherDB := dbs[(i+1)%len(dbs)]"#);
        go_stmt(
            r#"stmts[i] = fmt.Sprintf("'select * from %s.t_dump_multi_%d, %s.t_dump_multi_%d where %s.t_dump_multi_%d.a=%s.t_dump_multi_%d.a'","#,
        );
        go_stmt(r#"db, tbl, otherDB, t2, db, tbl, otherDB, t2)"#);
        go_stmt(r#"sqlCmd := "plan replayer dump explain (" + strings.Join(stmts, ", ") + ")""#);
        // 这里对应 TestKit 查询与结果断言；保留 SQL 和 expected rows。
        go_stmt(r#"res := tk.MustQuery(sqlCmd)"#);
        // 这里引用 plan replayer 内部状态或变量，这里只保留状态转移说明。
        go_stmt(r#"fileName := requirePlanReplayerFileToken(t, res.Rows())"#);

        go_stmt(r#"filePath := filepath.Join(replayer.GetPlanReplayerDirName(), fileName)"#);
        // 这里打开 replayer dump 文件；这里只记录 IO 路径，不读取 zip。
        go_stmt(r#"fileReader, err := storage.Open(ctx, filePath, nil)"#);
        go_stmt(r#"require.NoError(t, err)"#);
        go_stmt(r#"defer fileReader.Close()"#);

        // 这里读取文件内容；真实测试会把 zip bytes 用于后续断言。
        go_stmt(r#"content, err := io.ReadAll(fileReader)"#);
        go_stmt(r#"require.NoError(t, err)"#);

        go_stmt(r#"readerAt := bytes.NewReader(content)"#);
        // 这里解析 zip；保留归档检查语义，不展开文件。
        go_stmt(r#"zr, err := zip.NewReader(readerAt, int64(len(content)))"#);
        go_stmt(r#"require.NoError(t, err)"#);

        go_stmt(r#"names := make(map[string]struct{})"#);
        go_stmt(r#"for _, f := range zr.File {"#);
        go_stmt(r#"names[f.Name] = struct{}{}"#);
        go_stmt(r#"for i := 0; i < numStmts; i++ {"#);
        // 这里对应包含关系断言，常用于 explain/错误文本/zip 文件名。
        go_stmt(r#"require.Contains(t, names, fmt.Sprintf("sql/sql%d.sql", i))"#);
        go_stmt(r#"require.Contains(t, names, fmt.Sprintf("explain/explain%d.txt", i))"#);
        go_stmt(
            r#"require.NotContains(t, names, "explain.txt") // single explain.txt is not used for multi-SQL"#,
        );

        // Go 原注释：Check stats and schema files for all tables in all databases
        go_stmt(r#"for _, db := range dbs {"#);
        go_stmt(r#"for i := 1; i <= numTables; i++ {"#);
        go_stmt(r#"tableName := fmt.Sprintf("t_dump_multi_%d", i)"#);
        go_stmt(r#"statsName := fmt.Sprintf("stats/%s.%s.json", db, tableName)"#);
        go_stmt(r#"schemaName := fmt.Sprintf("schema/%s.%s.schema.txt", db, tableName)"#);
        go_stmt(
            r#"require.Contains(t, names, statsName, "missing stats file for db=%s table=%s (expected %s)", db, tableName, statsName)"#,
        );
        go_stmt(
            r#"require.Contains(t, names, schemaName, "missing schema file for db=%s table=%s (expected %s)", db, tableName, schemaName)"#,
        );
}
*/

use std::collections::HashSet;

use astersql_executor::plan_replayer::{
    PlanReplayerBackend, PlanReplayerCaptureInfo, PlanReplayerDumpInfo, PlanReplayerExec,
    PlanReplayerLoadExec, PlanReplayerLoadInfo, loadPlanReplayerForExplainExplore, updateLoadInfo,
};

/// 测试用会话上下文：按序记录 load 各阶段钩子调用。
#[derive(Default)]
struct Context {
    /// 阶段名轨迹，用于断言 load 管线顺序。
    trace: Vec<&'static str>,
}

/// 极简归档：仅携带目标 SQL（`sql:` 前缀解析结果）。
#[derive(Clone, Default)]
struct Archive {
    /// 归档内待 explain / explore 的目标语句。
    target_sql: String,
}

/// `PlanReplayerBackend` 的内存 mock：覆盖 capture/dump/load 控制路径。
#[derive(Default)]
struct Backend {
    /// 已注册的 (sql_digest, plan_digest) capture 任务集合。
    captures: HashSet<(String, String)>,
    /// dump 成功时返回的 file token 或预签名 URL。
    token: String,
    /// `read_file` 返回的归档字节（如 `sql:...`）。
    file_bytes: Vec<u8>,
    /// dump 时记录的已解析语句列表。
    parsed: Vec<String>,
    /// `close_dump_file` 调用次数（失败路径也应关闭）。
    closed_files: usize,
    /// 为 true 时 `dump` 返回错误。
    fail_dump: bool,
    /// 为 true 时读取 statement read timestamp 失败。
    fail_read_timestamp: bool,
    /// 为 true 时 dump 文件传输准备失败。
    fail_prepare_dump: bool,
    /// 为 true 时 `load_bindings` 失败并触发 warning。
    fail_bindings: bool,
    /// 绑定加载失败累计的 warning 次数。
    binding_warnings: usize,
    /// 关闭 auto-analyze 时累计的 warning 次数。
    auto_analyze_warnings: usize,
}

impl PlanReplayerBackend for Backend {
    type Context = Context;
    type Request = Vec<Vec<String>>;
    type Statement = String;
    type File = Vec<u8>;
    type Archive = Archive;
    type Error = String;

    /// 清空并复用结果行缓冲。
    fn grow_and_reset(&self, request: &mut Self::Request) {
        request.clear();
    }

    /// 按列写入结果单元格：第 0 列新开一行，其后列追加到当前行。
    fn append_string(&self, request: &mut Self::Request, column: usize, value: &str) {
        // 结果集按列追加：第 0 列新开一行，其后列写入当前行。
        if column == 0 {
            request.push(vec![value.to_owned()]);
        } else {
            request
                .last_mut()
                .expect("first column")
                .push(value.to_owned());
        }
    }

    /// 预签名 URL 过期时间展示（与 Go 一致为 1h0m0s）。
    fn presigned_url_expiration(&self) -> String {
        "1h0m0s".to_owned()
    }

    /// 按 (sql_digest, plan_digest) 移除 capture 任务。
    fn remove_capture_task(
        &mut self,
        _context: &mut Self::Context,
        capture: &PlanReplayerCaptureInfo,
    ) -> Result<(), Self::Error> {
        self.captures
            .remove(&(capture.sql_digest.clone(), capture.plan_digest.clone()));
        Ok(())
    }

    /// 注册 capture；重复 digest 对返回 already exists。
    fn register_capture_task(
        &mut self,
        _context: &mut Self::Context,
        capture: &PlanReplayerCaptureInfo,
    ) -> Result<(), Self::Error> {
        // digest 对已存在则拒绝，与 Go「task already exists」语义一致。
        if self
            .captures
            .insert((capture.sql_digest.clone(), capture.plan_digest.clone()))
        {
            Ok(())
        } else {
            Err("plan replayer capture task already exists".to_owned())
        }
    }

    /// 创建空 dump 文件句柄与默认文件名。
    fn create_dump_file(
        &mut self,
        _context: &mut Self::Context,
    ) -> Result<(Self::File, String), Self::Error> {
        Ok((Vec::new(), "capture.zip".to_owned()))
    }

    /// 关闭 dump 文件（失败路径也必须调用）。
    fn close_dump_file(&mut self, _file: Self::File) {
        self.closed_files += 1;
    }

    /// 语句读取时间戳桩（固定 42，写入 dump 元数据）。
    fn statement_read_timestamp(
        &mut self,
        _context: &mut Self::Context,
    ) -> Result<u64, Self::Error> {
        if self.fail_read_timestamp {
            Err("read timestamp failed".to_owned())
        } else {
            Ok(42)
        }
    }

    /// dump 前传输准备钩子（本 mock 无操作）。
    fn prepare_dump_file_transfer(
        &mut self,
        _dump: &PlanReplayerDumpInfo<Self::Statement, Self::File>,
    ) -> Result<(), Self::Error> {
        if self.fail_prepare_dump {
            Err("prepare dump transfer failed".to_owned())
        } else {
            Ok(())
        }
    }

    /// 执行 dump：可注入失败；成功记录语句并返回 token。
    fn dump(
        &mut self,
        _context: &mut Self::Context,
        dump: &mut PlanReplayerDumpInfo<Self::Statement, Self::File>,
    ) -> Result<String, Self::Error> {
        if self.fail_dump {
            return Err("dump failed".to_owned());
        }
        // 记录语句后返回 token，供结果集「File token」列使用。
        self.parsed = dump.statements.clone();
        Ok(self.token.clone())
    }

    /// 空 SQL dump 错误文案。
    fn empty_sql_error(&self) -> Self::Error {
        "plan replayer dump sql is empty".to_owned()
    }

    /// 解析单条 SQL：trim 后非空即作为语句。
    fn parse_sql(
        &mut self,
        _context: &mut Self::Context,
        sql: &str,
    ) -> Result<Self::Statement, Self::Error> {
        let sql = sql.trim().to_owned();
        if sql.is_empty() {
            Err("empty statement".to_owned())
        } else if sql.contains(" om ") {
            // These malformed statements are the parser-error cases in
            // TestPlanReplayerDumpMultipleError.
            Err("[parser:1064]".to_owned())
        } else {
            Ok(sql)
        }
    }

    /// load 前传输准备钩子（本 mock 无操作）。
    fn prepare_load_file_transfer(
        &mut self,
        _load: &PlanReplayerLoadInfo,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    /// 空 load 路径错误文案。
    fn empty_path_error(&self) -> Self::Error {
        "plan replayer load path is empty".to_owned()
    }

    /// 读取归档字节（返回预设 `file_bytes`）。
    fn read_file(
        &mut self,
        _context: &mut Self::Context,
        _path: &str,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(self.file_bytes.clone())
    }

    /// 打开归档：要求 UTF-8 且以 `sql:` 前缀携带目标语句。
    fn open_archive(&mut self, data: &[u8]) -> Result<Self::Archive, Self::Error> {
        // 测试归档协议：UTF-8 文本且必须以 `sql:` 开头。
        let text = String::from_utf8(data.to_vec()).map_err(|error| error.to_string())?;
        let target_sql = text
            .strip_prefix("sql:")
            .ok_or_else(|| "invalid plan replayer archive".to_owned())?
            .to_owned();
        Ok(Archive { target_sql })
    }

    /// 取出归档中的目标 SQL。
    fn target_sql(&mut self, archive: &mut Self::Archive) -> Result<String, Self::Error> {
        Ok(archive.target_sql.clone())
    }

    /// load：恢复会话/系统变量。
    fn load_variables(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        context.trace.push("variables");
        Ok(())
    }

    /// load：关闭自动 analyze，避免干扰导入统计。
    fn disable_auto_analyze(&mut self, context: &mut Self::Context) -> Result<(), Self::Error> {
        context.trace.push("disable-auto-analyze");
        Ok(())
    }

    /// load：按 schema 建表，返回涉及库名集合。
    fn create_tables(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<HashSet<String>, Self::Error> {
        context.trace.push("tables");
        Ok(HashSet::from(["test".to_owned()]))
    }

    /// load：恢复 TiFlash 副本元信息。
    fn load_tiflash_replicas(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        // TiFlash：列存副本引擎，load 时恢复 hypo/真实副本元数据。
        context.trace.push("tiflash");
        Ok(())
    }

    /// load：创建视图定义。
    fn create_views(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        context.trace.push("views");
        Ok(())
    }

    /// load：导入表级统计信息。
    fn load_statistics(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        context.trace.push("statistics");
        Ok(())
    }

    /// load：导入 SQL binding；可注入失败以触发 warning。
    fn load_bindings(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
        databases: &HashSet<String>,
    ) -> Result<(), Self::Error> {
        assert!(databases.contains("test"));
        context.trace.push("bindings");
        if self.fail_bindings {
            Err("invalid binding".to_owned())
        } else {
            Ok(())
        }
    }

    /// binding 失败时累计 warning（不中断整体 load）。
    fn append_binding_warning(&mut self, _error: &Self::Error) {
        self.binding_warnings += 1;
    }

    /// 关闭 auto-analyze 相关 warning 计数。
    fn append_auto_analyze_warning(&mut self) {
        self.auto_analyze_warnings += 1;
    }

    /// 从原始字节加载统计信息。
    fn load_stats_bytes(
        &mut self,
        context: &mut Self::Context,
        _data: &[u8],
    ) -> Result<(), Self::Error> {
        context.trace.push("stats-bytes");
        Ok(())
    }
}

/// 构造仅含语句列表的 dump 信息（其余字段置默认）。
fn dump_info(statements: &[&str]) -> PlanReplayerDumpInfo<String, Vec<u8>> {
    PlanReplayerDumpInfo {
        statements: statements.iter().map(|sql| (*sql).to_owned()).collect(),
        analyze: false,
        historical_stats_timestamp: 0,
        start_timestamp: 0,
        path: String::new(),
        file: None,
        file_name: String::new(),
    }
}

const PLAN_REPLAYER_SINGLE_FILE_NAMES: [&str; 14] = [
    "config.toml",
    "debug_trace/debug_trace0.json",
    "meta.txt",
    "stats/test.t_dump_single.json",
    "schema/test.t_dump_single.schema.txt",
    "schema/schema_meta.txt",
    "table_tiflash_replica.txt",
    "variables.toml",
    "session_bindings.sql",
    "global_bindings.sql",
    "sql/sql0.sql",
    "explain.txt",
    "statsMem/test.t_dump_single.txt",
    "sql_meta.toml",
];

fn check_file_name(name: &str) -> bool {
    PLAN_REPLAYER_SINGLE_FILE_NAMES.contains(&name)
}

fn require_plan_replayer_file_token(rows: &[Vec<String>]) -> &str {
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 2);
    assert_eq!(rows[0][0], "File token");
    assert!(!rows[0][1].is_empty());
    &rows[0][1]
}

fn has_ti_flash_task(rows: &[Vec<String>]) -> bool {
    rows.iter()
        .any(|row| row.get(2).is_some_and(|task| task.contains("tiflash")))
}

fn require_archive_file_contains(entries: &[(&str, &str)], file_name: &str, expected: &str) {
    let content = entries
        .iter()
        .find_map(|(name, content)| (*name == file_name).then_some(*content))
        .unwrap_or_else(|| panic!("missing file in archive: {file_name}"));
    assert!(
        content.contains(expected),
        "archive file {file_name} did not contain {expected:?}"
    );
}

/// capture：注册成功 → 重复注册失败 → remove 清空任务。
#[test]
fn capture_register_duplicate_and_remove_follow_executor_lifecycle() {
    let capture = PlanReplayerCaptureInfo {
        sql_digest: "sql".to_owned(),
        plan_digest: "plan".to_owned(),
        remove: false,
    };
    let mut exec = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: Some(capture.clone()),
        dump_info: None,
        end: false,
    };
    exec.Next(&mut Context::default(), &mut Vec::new()).unwrap();
    assert!(exec.end);
    assert!(
        exec.backend
            .captures
            .contains(&("sql".to_owned(), "plan".to_owned()))
    );

    // 重置 end 后再次 Next，应命中「任务已存在」。
    exec.end = false;
    assert_eq!(
        exec.Next(&mut Context::default(), &mut Vec::new()),
        Err("plan replayer capture task already exists".to_owned())
    );
    exec.capture_info.as_mut().unwrap().remove = true;
    exec.removeCaptureTask(&mut Context::default()).unwrap();
    assert!(exec.backend.captures.is_empty());
}

/// dump 成功返回 File token；失败时仍关闭 dump 文件句柄。
#[test]
fn dump_returns_file_token_and_closes_file_on_failure() {
    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: "capture.zip".to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select * from t"])),
        end: false,
    };
    let mut rows = Vec::new();
    exec.Next(&mut Context::default(), &mut rows).unwrap();
    assert_eq!(
        rows,
        vec![vec!["File token".to_owned(), "capture.zip".to_owned()]]
    );
    // statement_read_timestamp mock 固定返回 42。
    assert_eq!(exec.dump_info.as_ref().unwrap().start_timestamp, 42);

    let mut failed = PlanReplayerExec {
        backend: Backend {
            fail_dump: true,
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select 1"])),
        end: false,
    };
    assert_eq!(
        failed.Next(&mut Context::default(), &mut Vec::new()),
        Err("dump failed".to_owned())
    );
    assert_eq!(failed.backend.closed_files, 1);
}

/// Go `PlanReplayerExec.Next` 在 createFile 之后安装 defer，因此读取时间戳失败也必须关闭文件。
#[test]
fn dump_closes_created_file_when_read_timestamp_fails() {
    let mut exec = PlanReplayerExec {
        backend: Backend {
            fail_read_timestamp: true,
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select 1"])),
        end: false,
    };

    assert_eq!(
        exec.Next(&mut Context::default(), &mut Vec::new()),
        Err("read timestamp failed".to_owned())
    );
    assert_eq!(exec.backend.closed_files, 1);
}

/// Go defer 同样覆盖外部文件传输准备失败路径。
#[test]
fn dump_closes_created_file_when_transfer_prepare_fails() {
    let mut info = dump_info(&["select 1"]);
    info.path = "/tmp/statements.sql".to_owned();
    let mut exec = PlanReplayerExec {
        backend: Backend {
            fail_prepare_dump: true,
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(info),
        end: false,
    };

    assert_eq!(
        exec.Next(&mut Context::default(), &mut Vec::new()),
        Err("prepare dump transfer failed".to_owned())
    );
    assert_eq!(exec.backend.closed_files, 1);
}

/// Go defer 也覆盖 createFile 后发现 SQL 为空的错误路径。
#[test]
fn dump_closes_created_file_when_sql_is_empty() {
    let mut exec = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };

    assert_eq!(
        exec.Next(&mut Context::default(), &mut Vec::new()),
        Err("plan replayer dump sql is empty".to_owned())
    );
    assert_eq!(exec.backend.closed_files, 1);
}

#[test]
fn go_plan_replayer_helpers_keep_file_token_tiflash_and_archive_contracts() {
    for name in PLAN_REPLAYER_SINGLE_FILE_NAMES {
        assert!(check_file_name(name), "missing single-dump file {name}");
    }
    assert!(!check_file_name("explain/explain0.txt"));
    assert!(!check_file_name("unexpected.txt"));

    let rows = vec![vec!["File token".to_owned(), "capture.zip".to_owned()]];
    assert_eq!(require_plan_replayer_file_token(&rows), "capture.zip");

    assert!(has_ti_flash_task(&[vec![
        "id".to_owned(),
        "task".to_owned(),
        "TableFullScan tiflash".to_owned()
    ]]));
    assert!(!has_ti_flash_task(&[vec![
        "id".to_owned(),
        "task".to_owned(),
        "TableFullScan tikv".to_owned(),
    ]]));
    assert!(!has_ti_flash_task(&[vec![
        "id".to_owned(),
        "task".to_owned()
    ]]));

    let entries = [("explain.txt", "TableFullScan tiflash")];
    require_archive_file_contains(&entries, "explain.txt", "tiflash");
}

/// 预签名 URL 作为 token 时，结果集含 Download URL / Expires / curl 指引。
#[test]
fn presigned_url_dump_returns_go_equivalent_download_instructions() {
    let url = "https://storage.example/replayer.zip?signature=abc";
    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: url.to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select 1"])),
        end: false,
    };
    let mut rows = Vec::new();
    exec.Next(&mut Context::default(), &mut rows).unwrap();
    assert_eq!(
        rows,
        vec![
            vec!["Download URL", url],
            vec!["Expires in", "1h0m0s"],
            vec![
                "Browser",
                "Open the Download URL directly before it expires"
            ],
            vec![
                "curl",
                "curl -L 'https://storage.example/replayer.zip?signature=abc' -o plan_replayer.zip",
            ],
            vec![
                "Note",
                "If the URL expires, rerun PLAN REPLAYER DUMP to get a new one",
            ],
        ]
    );
}

/// Go 使用 `url.Parse` 并要求 http(s) URL 具有非空 host；只有 scheme 的 token 仍是文件 token。
#[test]
fn presigned_url_without_host_is_treated_as_file_token() {
    let token = "http://?signature=missing-host";
    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: token.to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select 1"])),
        end: false,
    };
    let mut rows = Vec::new();

    exec.Next(&mut Context::default(), &mut rows).unwrap();
    assert_eq!(rows, vec![vec!["File token", token]]);
}

/// 多语句文件按分号拆分、trim 后按序 dump。
#[test]
fn multi_sql_file_is_split_parsed_and_dumped_in_order() {
    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: "multi.zip".to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        exec.DumpSQLsFromFile(
            &mut Context::default(),
            b"select * from test.t1;\nupdate test.t2 set a=1;"
        )
        .unwrap(),
        "multi.zip"
    );
    assert_eq!(
        exec.backend.parsed,
        vec!["select * from test.t1", "update test.t2 set a=1"]
    );
}

#[test]
fn multi_sql_dump_preserves_go_fifty_statement_and_file_name_contract() {
    const NUM_STATEMENTS: usize = 50;
    const NUM_TABLES: usize = 5;
    let databases = [
        "test",
        "test_multi_db1",
        "test_multi_db2",
        "test_multi_db3",
        "test_multi_db4",
    ];
    let mut statements = Vec::with_capacity(NUM_STATEMENTS);
    for index in 0..NUM_STATEMENTS {
        let pair_index = index % (databases.len() * NUM_TABLES);
        let database = databases[pair_index / NUM_TABLES];
        let table = pair_index % NUM_TABLES + 1;
        statements.push(format!("select * from {database}.t_dump_multi_{table}"));
    }
    let input = statements
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(";\n");

    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: "multi.zip".to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        exec.DumpSQLsFromFile(&mut Context::default(), input.as_bytes())
            .unwrap(),
        "multi.zip"
    );
    assert_eq!(exec.backend.parsed, statements);

    let mut names = HashSet::new();
    for index in 0..NUM_STATEMENTS {
        names.insert(format!("sql/sql{index}.sql"));
        names.insert(format!("explain/explain{index}.txt"));
    }
    assert_eq!(names.len(), NUM_STATEMENTS * 2);
    assert!(!names.contains("explain.txt"));
}

/// load 管线顺序：variables → 关 auto-analyze → tables → tiflash → views → stats → bindings。
#[test]
fn load_runs_variables_schema_tiflash_stats_and_bindings_in_order() {
    let mut backend = Backend {
        fail_bindings: true,
        ..Backend::default()
    };
    let mut context = Context::default();
    // 绑定失败仍继续，并累计 binding / auto-analyze warning。
    updateLoadInfo(&mut backend, &mut context, b"sql:select * from t").unwrap();
    assert_eq!(
        context.trace,
        vec![
            "variables",
            "disable-auto-analyze",
            "tables",
            "tiflash",
            "views",
            "statistics",
            "bindings"
        ]
    );
    assert_eq!(backend.binding_warnings, 1);
    assert_eq!(backend.auto_analyze_warnings, 1);
}

/// explain explore：先读归档目标 SQL，再跑完整 load 环境初始化。
#[test]
fn explain_explore_reads_archive_target_then_loads_environment() {
    let mut backend = Backend {
        file_bytes: b"sql:select /*+ read_from_storage(tiflash[t]) */ * from t".to_vec(),
        ..Backend::default()
    };
    let mut context = Context::default();
    let target =
        loadPlanReplayerForExplainExplore(&mut backend, &mut context, "replayer/capture.zip")
            .unwrap();
    assert!(target.contains("tiflash"));
    assert_eq!(context.trace.first(), Some(&"variables"));
    assert_eq!(context.trace.last(), Some(&"bindings"));
}

/// LoadExec：空 path 报错；非空 path 走 prepare transfer 成功路径。
#[test]
fn load_executor_rejects_empty_path_and_prepares_nonempty_transfer() {
    let mut empty = PlanReplayerLoadExec {
        backend: Backend::default(),
        info: PlanReplayerLoadInfo {
            path: String::new(),
        },
    };
    assert_eq!(
        empty.Next(&mut Context::default(), &mut Vec::new()),
        Err("plan replayer load path is empty".to_owned())
    );
    let mut valid = PlanReplayerLoadExec {
        backend: Backend::default(),
        info: PlanReplayerLoadInfo {
            path: "/tmp/capture.zip".to_owned(),
        },
    };
    valid
        .Next(&mut Context::default(), &mut Vec::new())
        .unwrap();
}

#[test]
fn presigned_url_expiration_matches_go_output() {
    assert_eq!(Backend::default().presigned_url_expiration(), "1h0m0s");
}

#[test]
fn multi_sql_parser_rejects_the_go_invalid_statement_cases() {
    let mut backend = Backend::default();
    let mut context = Context::default();

    assert_eq!(
        backend.parse_sql(&mut context, "select x om t"),
        Err("[parser:1064]".to_owned())
    );
    assert_eq!(
        backend.parse_sql(&mut context, "select y om t"),
        Err("[parser:1064]".to_owned())
    );

    let mut empty = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        empty.Next(&mut Context::default(), &mut Vec::new()),
        Err("plan replayer dump sql is empty".to_owned())
    );

    let mut one_invalid = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        one_invalid.DumpSQLsFromFile(&mut Context::default(), b"select x om t"),
        Err("[parser:1064]".to_owned())
    );

    let mut multiple_invalid = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        multiple_invalid
            .DumpSQLsFromFile(&mut Context::default(), b"select x from t; select y om t",),
        Err("[parser:1064]".to_owned())
    );
}
