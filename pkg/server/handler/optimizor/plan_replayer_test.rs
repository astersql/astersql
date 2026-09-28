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

// 主要外部依赖、测试框架、failpoint、goroutine/channel、压缩包与文件 IO 均以中文注释保留 Go 语义，。

// GoValue 是本中的桥接类型，用于承载未接入的 TiDB/testkit/http/sql 等对象。
// 后续真正接线时应替换为仓库内明确的 Rust 类型，而不是在本批测试这里推测依赖。
// Plan Replayer HTTP API 测试（机械迁移占位）。
//
// 覆盖 dump/load、capture、历史统计以及相关 issue 回归；
// 当前以中文注释保留 Go 测试语义，不真正拉起服务或连库。

/// Copyright 2026 AsterSQL.
pub struct GoValue;

static PLAN_REPLAYER_E2E_SERIAL: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();

// expectedFilesInReplayer 对应 Go 的同名字符串切片，列出测试期望在导出包中出现的文件。
/// expectedFilesInReplayer 对应 Go 的同名字符串切片，列出测试期望在导出包中出现的文件。
pub const EXPECTED_FILES_IN_REPLAYER: &[&str] = &[
    "config.toml",
    "debug_trace/debug_trace0.json",
    "explain.txt",
    "global_bindings.sql",
    "meta.txt",
    "schema/planreplayer.t.schema.txt",
    "schema/schema_meta.txt",
    "session_bindings.sql",
    "sql/sql0.sql",
    "sql_meta.toml",
    "stats/planreplayer.t.json",
    "statsMem/planreplayer.t.txt",
    "table_tiflash_replica.txt",
    "variables.toml",
];

// expectedFilesInReplayerForCapture 对应 Go 的同名字符串切片，列出测试期望在导出包中出现的文件。
/// expectedFilesInReplayerForCapture 对应 Go 的同名字符串切片，列出测试期望在导出包中出现的文件。
pub const EXPECTED_FILES_IN_REPLAYER_FOR_CAPTURE: &[&str] = &[
    "config.toml",
    "debug_trace/debug_trace0.json",
    "explain/sql.txt",
    "global_bindings.sql",
    "meta.txt",
    "schema/planreplayer.t.schema.txt",
    "schema/schema_meta.txt",
    "session_bindings.sql",
    "sql/sql0.sql",
    "sql_meta.toml",
    "stats/planreplayer.t.json",
    "statsMem/planreplayer.t.txt",
    "table_tiflash_replica.txt",
    "variables.toml",
];

// requirePlanReplayerFileTokenFromRows 对应 Go 函数 `func requirePlanReplayerFileTokenFromRows(t *testing.T, rows *sql.Rows) string {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// requirePlanReplayerFileTokenFromRows 对应 Go 函数 `func requirePlanReplayerFileTokenFromRows(t *testing.T, rows *sql.Rows...
pub fn require_plan_replayer_file_token_from_rows() {
    // Go 原始签名: func requirePlanReplayerFileTokenFromRows(t *testing.T, rows *sql.Rows) string {
    // 断言: require.True(t, rows.Next(), "unexpected data")
    // 迁移语句: var item, filename string
    // 错误处理: require.NoError(t, rows.Scan(&item, &filename))
    // 断言: require.Equal(t, "File token", item)
    // 断言: require.NotEmpty(t, filename)
    // 错误处理: require.NoError(t, rows.Close())
    // 返回值: return filename
}

// requirePlanReplayerFileTokenFromResult 对应 Go 函数 `func requirePlanReplayerFileTokenFromResult(t *testing.T, rows [][]any) string {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// requirePlanReplayerFileTokenFromResult 对应 Go 函数 `func requirePlanReplayerFileTokenFromResult(t *testing.T, rows [][]a...
pub fn require_plan_replayer_file_token_from_result() {
    // Go 原始签名: func requirePlanReplayerFileTokenFromResult(t *testing.T, rows [][]any) string {
    // 断言: require.Len(t, rows, 1)
    // 断言: require.Len(t, rows[0], 2)
    // 断言: require.Equal(t, "File token", rows[0][0])
    // 状态准备: filename, ok := rows[0][1].(string)
    // 断言: require.True(t, ok)
    // 断言: require.NotEmpty(t, filename)
    // 返回值: return filename
}

// requireSingleStringFromRows 对应 Go 函数 `func requireSingleStringFromRows(t *testing.T, rows *sql.Rows) string {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// requireSingleStringFromRows 对应 Go 函数 `func requireSingleStringFromRows(t *testing.T, rows *sql.Rows) string {`。
pub fn require_single_string_from_rows() {
    // Go 原始签名: func requireSingleStringFromRows(t *testing.T, rows *sql.Rows) string {
    // 断言: require.True(t, rows.Next(), "unexpected data")
    // 迁移语句: var value string
    // 错误处理: require.NoError(t, rows.Scan(&value))
    // 错误处理: require.NoError(t, rows.Close())
    // 返回值: return value
}

// prepareServerAndClientForTest 对应 Go 函数 `func prepareServerAndClientForTest(t *testing.T, store kv.Storage, dom *domain.Domain) (srv *server.Server, client *testserverclient.TestServerClient) {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// prepareServerAndClientForTest 对应 Go 函数 `func prepareServerAndClientForTest(t *testing.T, store kv.Storage, dom *domai...
pub fn prepare_server_and_client_for_test() {
    // Go 原始签名: func prepareServerAndClientForTest(t *testing.T, store kv.Storage, dom *domain.Domain) (srv *server.Server, client *testserverclient.TestServerClient) {
    // 状态准备: driver := server.NewTiDBDriver(store)
    // 状态准备: client = testserverclient.NewTestServerClient()

    // 状态准备: cfg := util.NewTestConfig()
    // 状态准备: cfg.Port = client.Port
    // 状态准备: cfg.Status.StatusPort = client.StatusPort
    // 状态准备: cfg.Status.ReportStatus = true

    // 保留 Go 注释: // RunInGoTestChan is a global channel and will be closed after the first server starts.
    // 保留 Go 注释: // Recreate it to avoid racing on subsequent server starts in the same test binary.
    // 并发通道: server.RunInGoTestChan = make(chan struct{})
    // 状态准备: srv, err := server.NewServer(cfg, driver)
    // 迁移语句: srv.SetDomain(dom)
    // 错误处理: require.NoError(t, err)
    // 并发/异步: go func() {
    // 状态准备: err := srv.Run(nil)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 并发同步: <-server.RunInGoTestChan
    // 状态准备: client.Port = testutil.GetPortFromTCPAddr(srv.ListenAddr())
    // 状态准备: client.StatusPort = testutil.GetPortFromTCPAddr(srv.StatusListenerAddr())
    // 迁移语句: client.WaitUntilServerOnline()
    // 返回值: return
}

#[test]
// TestDumpPlanReplayerAPI 对应 Go 函数 `func TestDumpPlanReplayerAPI(t *testing.T) {`。
// 这是 HTTP/SQL 测试：保留 setup、请求、断言和清理顺序，但不会启动服务器或连接数据库。
/// TestDumpPlanReplayerAPI 对应 Go 函数 `func TestDumpPlanReplayerAPI(t *testing.T) {`。
pub fn test_dump_plan_replayer_api() {
    assert_real_dump_and_http_download_round_trip();
    // Go 原始签名: func TestDumpPlanReplayerAPI(t *testing.T) {
    // 状态准备: origin := config.GetGlobalConfig().TempDir
    // 资源收尾: defer func() {
    // 状态准备: config.GetGlobalConfig().TempDir = origin
    // 迁移语句: }()
    // 状态准备: config.GetGlobalConfig().TempDir = t.TempDir()
    // 外部依赖/testkit: store := testkit.CreateMockStore(t)
    // 状态准备: dom, err := session.GetDomain(store)
    // 错误处理: require.NoError(t, err)
    // 保留 Go 注释: // 1. setup and prepare plan replayer files by manual command and capture
    // 状态准备: server, client := prepareServerAndClientForTest(t, store, dom)
    // 资源收尾: defer server.Close()

    // 状态准备: filename, fileNameFromCapture := prepareData4PlanReplayer(t, client, dom)
    // 资源收尾: defer os.RemoveAll(replayer.GetPlanReplayerDirName())

    // 保留 Go 注释: // 2. check the contents of the plan replayer zip files.

    // 迁移语句: var filesInReplayer []string
    // 压缩包读取: collectFileNameAndAssertFileSize := func(f *zip.File) {
    // 保留 Go 注释: // collect file name
    // 状态准备: filesInReplayer = append(filesInReplayer, f.Name)
    // 保留 Go 注释: // except for {global,session}_bindings.sql and table_tiflash_replica.txt, the file should not be empty
    // 关键分支: if !strings.Contains(f.Name, "table_tiflash_replica.txt") &&
    // 迁移语句: !strings.Contains(f.Name, "bindings.sql") &&
    // 迁移语句: !strings.Contains(f.Name, "trace") {
    // 迁移语句: require.NotZero(t, f.UncompressedSize64, f.Name)
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // 2-1. check the plan replayer file from manual command
    // HTTP 请求: resp0, err := client.FetchStatus(filepath.Join("/plan_replayer/dump/", filename))
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp0.Body.Close())
    // 迁移语句: }()
    // IO 读取写入: body, err := io.ReadAll(resp0.Body)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: forEachFileInZipBytes(t, body, collectFileNameAndAssertFileSize)
    // 排序/结果归一: slices.Sort(filesInReplayer)
    // 断言: require.Equal(t, expectedFilesInReplayer, filesInReplayer)

    // 保留 Go 注释: // 2-2. check the plan replayer file from capture
    // HTTP 请求: resp1, err := client.FetchStatus(filepath.Join("/plan_replayer/dump/", fileNameFromCapture))
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp1.Body.Close())
    // 迁移语句: }()
    // IO 读取写入: body, err = io.ReadAll(resp1.Body)
    // 错误处理: require.NoError(t, err)
    // 状态准备: filesInReplayer = filesInReplayer[:0]
    // 迁移语句: forEachFileInZipBytes(t, body, collectFileNameAndAssertFileSize)
    // 排序/结果归一: slices.Sort(filesInReplayer)
    // 断言: require.Equal(t, expectedFilesInReplayerForCapture, filesInReplayer)

    // 保留 Go 注释: // 3. check plan replayer load

    // 保留 Go 注释: // 3-1. write the plan replayer file from manual command to a file
    // 状态准备: path := t.TempDir()
    // 状态准备: path = filepath.Join(path, "plan_replayer.zip")
    // 文件系统: fp, err := os.Create(path)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, fp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, fp.Close())
    // 迁移语句: }()

    // IO 读取写入: _, err = io.Copy(fp, bytes.NewReader(body))
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, fp.Sync())

    // 保留 Go 注释: // 3-2. connect to tidb and use PLAN REPLAYER LOAD to load this file
    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN(func(config *mysql.Config) {
    // 状态准备: config.AllowAllFiles = true
    // 迁移语句: }))
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)
    // 状态准备: autoAnalyzeRows := tk.MustQuery("select @@global.tidb_enable_auto_analyze")
    // 断言: require.True(t, autoAnalyzeRows.Next(), "unexpected data")
    // 迁移语句: var originAutoAnalyze string
    // 错误处理: require.NoError(t, autoAnalyzeRows.Scan(&originAutoAnalyze))
    // 错误处理: require.NoError(t, autoAnalyzeRows.Close())
    // 资源收尾: defer tk.MustExec(fmt.Sprintf("set @@global.tidb_enable_auto_analyze = '%s'", originAutoAnalyze))
    // 状态准备: tk.MustExec("set @@global.tidb_enable_auto_analyze = ON")

    // 迁移语句: tk.MustExec("use planReplayer")
    // 迁移语句: tk.MustExec("drop table planReplayer.t")
    // 格式化参数: tk.MustExec(fmt.Sprintf(`plan replayer load "%s"`, path))

    // 状态准备: warnRows := tk.MustQuery("show warnings")
    // 状态准备: foundAutoAnalyzeWarning := false
    // 状态准备: warnMessages := make([]string, 0)
    // 循环遍历: for warnRows.Next() {
    // 迁移语句: var level, msg string
    // 迁移语句: var code int64
    // 错误处理: require.NoError(t, warnRows.Scan(&level, &code, &msg))
    // 状态准备: warnMessages = append(warnMessages, msg)
    // 关键分支: if strings.Contains(msg, "tidb_enable_auto_analyze=OFF") {
    // 状态准备: foundAutoAnalyzeWarning = true
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 错误处理: require.NoError(t, warnRows.Close())
    // 断言: require.True(t, foundAutoAnalyzeWarning, "warnings: %v", warnMessages)

    // 状态准备: autoAnalyzeRows = tk.MustQuery("select @@global.tidb_enable_auto_analyze")
    // 断言: require.True(t, autoAnalyzeRows.Next(), "unexpected data")
    // 迁移语句: var autoAnalyzeValue int64
    // 错误处理: require.NoError(t, autoAnalyzeRows.Scan(&autoAnalyzeValue))
    // 断言: require.Equal(t, int64(0), autoAnalyzeValue)
    // 错误处理: require.NoError(t, autoAnalyzeRows.Close())

    // 保留 Go 注释: // 3-3. assert that the count and modify count in the stats is as expected
    // 状态准备: rows := tk.MustQuery(`show stats_meta where table_name="t"`)
    // 断言: require.True(t, rows.Next(), "unexpected data")
    // 迁移语句: var dbName, tableName string
    // 迁移语句: var modifyCount, count int64
    // 迁移语句: var other any
    // 状态准备: err = rows.Scan(&dbName, &tableName, &other, &other, &modifyCount, &count, &other)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, "planReplayer", dbName)
    // 断言: require.Equal(t, "t", tableName)
    // 断言: require.Equal(t, int64(4), modifyCount)
    // 断言: require.Equal(t, int64(8), count)

    // 保留 Go 注释: // Extra. check the plan replayer file not exists
    // HTTP 请求: resp2, err := client.FetchStatus(filepath.Join("/plan_replayer/dump/", filename+"a"))
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp2.Body.Close())
    // 迁移语句: }()
    // IO 读取写入: body, err = io.ReadAll(resp2.Body)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Contains(t, string(body), "can't find dump file")
}

#[test]
// TestPlanReplayerLoadWithSemicolonInColumnComment 对应 Go 函数 `func TestPlanReplayerLoadWithSemicolonInColumnComment(t *testing.T) {`。
// 这是 HTTP/SQL 测试：保留 setup、请求、断言和清理顺序，但不会启动服务器或连接数据库。
/// TestPlanReplayerLoadWithSemicolonInColumnComment 对应 Go 函数 `func TestPlanReplayerLoadWithSemicolonInColumnComment(t *t...
pub fn test_plan_replayer_load_with_semicolon_in_column_comment() {
    assert_dump_and_load_preserves_semicolon_comment();
    // Go 原始签名: func TestPlanReplayerLoadWithSemicolonInColumnComment(t *testing.T) {
    // 状态准备: origin := config.GetGlobalConfig().TempDir
    // 资源收尾: defer func() {
    // 状态准备: config.GetGlobalConfig().TempDir = origin
    // 迁移语句: }()
    // 状态准备: config.GetGlobalConfig().TempDir = t.TempDir()
    // 外部依赖/testkit: store := testkit.CreateMockStore(t)
    // 状态准备: dom, err := session.GetDomain(store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: server, client := prepareServerAndClientForTest(t, store, dom)
    // 资源收尾: defer server.Close()

    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN())
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)
    // 迁移语句: tk.MustExec("create database planReplayerSemicolon")
    // 迁移语句: tk.MustExec("use planReplayerSemicolon")
    // 迁移语句: tk.MustExec("create table t(k1 int, k2 int comment 'xx;xxx')")
    // 迁移语句: tk.MustExec("analyze table t")
    // 状态准备: rows := tk.MustQuery("plan replayer dump explain select * from t")
    // 状态准备: filename := requirePlanReplayerFileTokenFromRows(t, rows)

    // HTTP 请求: resp, err := client.FetchStatus(filepath.Join("/plan_replayer/dump/", filename))
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: }()
    // IO 读取写入: body, err := io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)

    // 状态准备: path := t.TempDir()
    // 状态准备: path = filepath.Join(path, "plan_replayer.zip")
    // 文件系统: fp, err := os.Create(path)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, fp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, fp.Close())
    // 错误处理: require.NoError(t, os.Remove(path))
    // 迁移语句: }()
    // IO 读取写入: _, err = io.Copy(fp, bytes.NewReader(body))
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, fp.Sync())

    // 外部依赖/数据库: db2, err := sql.Open("mysql", client.GetDSN(func(config *mysql.Config) {
    // 状态准备: config.AllowAllFiles = true
    // 迁移语句: }))
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 资源收尾: defer func() {
    // 状态准备: err := db2.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk2 := testkit.NewDBTestKit(t, db2)
    // 迁移语句: tk2.MustExec("use planReplayerSemicolon")
    // 状态准备: tk2.MustExec(`SET FOREIGN_KEY_CHECKS = 0;`)
    // 迁移语句: tk2.MustExec("drop table planReplayerSemicolon.t")
    // 状态准备: tk2.MustExec(`SET FOREIGN_KEY_CHECKS = 1;`)
    // 格式化参数: tk2.MustExec(fmt.Sprintf(`plan replayer load "%s"`, path))
    // 迁移语句: tk2.MustExec("use planReplayerSemicolon")
    // 状态准备: rows = tk2.MustQuery("show create table t")
    // 断言: require.True(t, rows.Next(), "unexpected data")
}

// prepareData4PlanReplayer 对应 Go 函数 `func prepareData4PlanReplayer(t *testing.T, client *testserverclient.TestServerClient, dom *domain.Domain) (string, string) {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// prepareData4PlanReplayer 对应 Go 函数 `func prepareData4PlanReplayer(t *testing.T, client *testserverclient.TestServerCli...
pub fn prepare_data4_plan_replayer() {
    // Go 原始签名: func prepareData4PlanReplayer(t *testing.T, client *testserverclient.TestServerClient, dom *domain.Domain) (string, string) {
    // 状态准备: h := dom.StatsHandle()
    // 状态准备: replayerHandle := dom.GetPlanReplayerHandle()
    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN())
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)

    // 迁移语句: tk.MustExec("create database planReplayer")
    // 迁移语句: tk.MustExec("use planReplayer")
    // 迁移语句: tk.MustExec("create table t(a int)")
    // 迁移语句: tk.MustExec("CREATE TABLE authors (id INT PRIMARY KEY AUTO_INCREMENT,name VARCHAR(100) NOT NULL,email VARCHAR(100) UNIQUE NOT NULL);")
    // 迁移语句: tk.MustExec("CREATE TABLE books (id INT PRIMARY KEY AUTO_INCREMENT,title VARCHAR(200) NOT NULL,publication_date DATE NOT NULL,author_id INT,FOREIGN...
    // 迁移语句: tk.MustExec("create table tt(a int, b varchar(10)) PARTITION BY HASH(a) PARTITIONS 4;")
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec("insert into t values(1), (2), (3), (4)")
    // 迁移语句: tk.MustExec("flush stats_delta *.*")
    // 迁移语句: tk.MustExec("analyze table t")
    // 迁移语句: tk.MustExec("insert into t values(5), (6), (7), (8)")
    // 迁移语句: tk.MustExec("flush stats_delta *.*")
    // 迁移语句: tk.MustExec("INSERT INTO tt (a, b) VALUES (1, 'str1'), (2, 'str2'), (3, 'str3'), (4, 'str4'),(5, 'str5'), (6, 'str6'), (7, 'str7'), (8, 'str8'),(9,...
    // 迁移语句: tk.MustExec("flush stats_delta *.*")
    // 迁移语句: tk.MustExec("analyze table tt")
    // 状态准备: rows := tk.MustQuery("plan replayer dump explain select * from t")
    // 状态准备: filename := requirePlanReplayerFileTokenFromRows(t, rows)
    // 状态准备: rows = tk.MustQuery("select @@tidb_last_plan_replayer_token")
    // 状态准备: filename2 := requireSingleStringFromRows(t, rows)
    // 断言: require.Equal(t, filename, filename2)

    // 迁移语句: tk.MustExec("plan replayer capture 'e5796985ccafe2f71126ed6c0ac939ffa015a8c0744a24b7aee6d587103fd2f7' '*'")
    // 迁移语句: tk.MustQuery("select * from t")
    // 状态准备: task := replayerHandle.DrainTask()
    // 断言: require.NotNil(t, task)
    // 状态准备: worker := replayerHandle.GetWorker()
    // 断言: require.True(t, worker.HandleTask(task))
    // 状态准备: rows = tk.MustQuery("select token from mysql.plan_replayer_status where length(sql_digest) > 0")
    // 断言: require.True(t, rows.Next(), "unexpected data")
    // 迁移语句: var filename3 string
    // 错误处理: require.NoError(t, rows.Scan(&filename3))
    // 错误处理: require.NoError(t, rows.Close())

    // 返回值: return filename, filename3
}

#[test]
// TestPlanReplayerWithMultiForeignKey 对应 Go 函数 `func TestPlanReplayerWithMultiForeignKey(t *testing.T) {`。
// 这是 HTTP/SQL 测试：保留 setup、请求、断言和清理顺序，但不会启动服务器或连接数据库。
/// TestPlanReplayerWithMultiForeignKey 对应 Go 函数 `func TestPlanReplayerWithMultiForeignKey(t *testing.T) {`。
pub fn test_plan_replayer_with_multi_foreign_key() {
    assert_dump_follows_recursive_foreign_keys();
    // Go 原始签名: func TestPlanReplayerWithMultiForeignKey(t *testing.T) {
    // 状态准备: origin := config.GetGlobalConfig().TempDir
    // 资源收尾: defer func() {
    // 状态准备: config.GetGlobalConfig().TempDir = origin
    // 迁移语句: }()
    // 状态准备: config.GetGlobalConfig().TempDir = t.TempDir()
    // 外部依赖/testkit: store := testkit.CreateMockStore(t)
    // 状态准备: dom, err := session.GetDomain(store)
    // 错误处理: require.NoError(t, err)
    // 保留 Go 注释: // 1. setup and prepare plan replayer files by manual command and capture
    // 状态准备: server, client := prepareServerAndClientForTest(t, store, dom)
    // 资源收尾: defer server.Close()

    // 状态准备: filename := prepareData4Issue56458(t, client, dom)
    // 资源收尾: defer os.RemoveAll(replayer.GetPlanReplayerDirName())

    // 保留 Go 注释: // 2. check the contents of the plan replayer zip files.
    // 迁移语句: var filesInReplayer []string
    // 压缩包读取: collectFileNameAndAssertFileSize := func(f *zip.File) {
    // 保留 Go 注释: // collect file name
    // 状态准备: filesInReplayer = append(filesInReplayer, f.Name)
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // 2-1. check the plan replayer file from manual command
    // HTTP 请求: resp0, err := client.FetchStatus(filepath.Join("/plan_replayer/dump/", filename))
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp0.Body.Close())
    // 迁移语句: }()
    // IO 读取写入: body, err := io.ReadAll(resp0.Body)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: forEachFileInZipBytes(t, body, collectFileNameAndAssertFileSize)
    // 排序/结果归一: slices.Sort(filesInReplayer)
    // 断言: require.Equal(t, []string{
    // 迁移语句: "config.toml",
    // 迁移语句: "debug_trace/debug_trace0.json",
    // 迁移语句: "explain.txt",
    // 迁移语句: "global_bindings.sql",
    // 迁移语句: "meta.txt",
    // 迁移语句: "schema/planreplayer.a.schema.txt",
    // 迁移语句: "schema/planreplayer.b.schema.txt",
    // 迁移语句: "schema/planreplayer.c.schema.txt",
    // 迁移语句: "schema/planreplayer.t.schema.txt",
    // 迁移语句: "schema/planreplayer.v.schema.txt",
    // 迁移语句: "schema/planreplayer2.t.schema.txt",
    // 迁移语句: "schema/schema_meta.txt",
    // 迁移语句: "session_bindings.sql",
    // 迁移语句: "sql/sql0.sql",
    // 迁移语句: "sql_meta.toml",
    // 迁移语句: "stats/planreplayer.a.json",
    // 迁移语句: "stats/planreplayer.b.json",
    // 迁移语句: "stats/planreplayer.c.json",
    // 迁移语句: "stats/planreplayer.t.json",
    // 迁移语句: "stats/planreplayer.v.json",
    // 迁移语句: "stats/planreplayer2.t.json",
    // 迁移语句: "statsMem/planreplayer.a.txt",
    // 迁移语句: "statsMem/planreplayer.b.txt",
    // 迁移语句: "statsMem/planreplayer.c.txt",
    // 迁移语句: "statsMem/planreplayer.t.txt",
    // 迁移语句: "statsMem/planreplayer.v.txt",
    // 迁移语句: "statsMem/planreplayer2.t.txt",
    // 迁移语句: "table_tiflash_replica.txt",
    // 迁移语句: "variables.toml",
    // 迁移语句: }, filesInReplayer)

    // 保留 Go 注释: // 3. check plan replayer load
    // 保留 Go 注释: // 3-1. write the plan replayer file from manual command to a file
    // 状态准备: path := t.TempDir()
    // 状态准备: path = filepath.Join(path, "plan_replayer.zip")
    // 文件系统: fp, err := os.Create(path)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, fp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, fp.Close())
    // 错误处理: require.NoError(t, os.Remove(path))
    // 迁移语句: }()

    // IO 读取写入: _, err = io.Copy(fp, bytes.NewReader(body))
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, fp.Sync())

    // 保留 Go 注释: // 3-2. connect to tidb and use PLAN REPLAYER LOAD to load this file
    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN(func(config *mysql.Config) {
    // 状态准备: config.AllowAllFiles = true
    // 迁移语句: }))
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 迁移语句: db.SetMaxOpenConns(1)
    // 迁移语句: db.SetMaxIdleConns(1)
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)
    // 迁移语句: tk.MustExec("use planReplayer")
    // 状态准备: tk.MustExec(`SET FOREIGN_KEY_CHECKS = 0;`)
    // 迁移语句: tk.MustExec("drop table planReplayer.t")
    // 迁移语句: tk.MustExec("drop table planReplayer2.t")
    // 迁移语句: tk.MustExec("drop table planReplayer.v")
    // 迁移语句: tk.MustExec("drop table planReplayer.a")
    // 迁移语句: tk.MustExec("drop table planReplayer.b")
    // 迁移语句: tk.MustExec("drop table planReplayer.c")
    // 状态准备: tk.MustExec(`SET FOREIGN_KEY_CHECKS = 1;`)
    // 格式化参数: tk.MustExec(fmt.Sprintf(`plan replayer load "%s"`, path))
    // 迁移语句: tk.MustExec("use planReplayer")
    // 状态准备: tk.MustExec("set @@tidb_use_plan_baselines = 1")

    // 状态准备: rows := tk.MustQuery("select @@global.tidb_mem_quota_binding_cache")
    // 断言: require.True(t, rows.Next(), "unexpected data")
    // 迁移语句: var originBindingCacheQuota int64
    // 错误处理: require.NoError(t, rows.Scan(&originBindingCacheQuota))
    // 错误处理: require.NoError(t, rows.Close())
    // 状态准备: tk.MustExec("set global tidb_mem_quota_binding_cache = 268435456") // 256MB
    // 资源收尾: defer tk.MustExec(fmt.Sprintf("set global tidb_mem_quota_binding_cache = %d", originBindingCacheQuota))

    // 迁移语句: tk.MustExec("admin reload bindings")
    // 保留 Go 注释: // 3-3. check whether binding takes effect
    // 迁移语句: require.Eventually(t, func() bool {
    // 迁移语句: tk.MustExec(`select a, b from t where a in (1, 2, 3)`)
    // 状态准备: rows := tk.MustQuery("select @@last_plan_from_binding")
    // 关键分支: if !rows.Next() {
    // 状态准备: _ = rows.Close()
    // 返回值: return false
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: var count int64
    // 状态准备: err := rows.Scan(&count)
    // 状态准备: _ = rows.Close()
    // 返回值: return err == nil && count == int64(1)
    // 时间相关: }, 10*time.Second, 100*time.Millisecond)
}

#[test]
// TestIssue43192 对应 Go 函数 `func TestIssue43192(t *testing.T) {`。
// 这是 HTTP/SQL 测试：保留 setup、请求、断言和清理顺序，但不会启动服务器或连接数据库。
/// TestIssue43192 对应 Go 函数 `func TestIssue43192(t *testing.T) {`。
pub fn test_issue43192() {
    assert_binding_is_exported_as_replayable_sql();
    // Go 原始签名: func TestIssue43192(t *testing.T) {
    // 状态准备: origin := config.GetGlobalConfig().TempDir
    // 资源收尾: defer func() {
    // 状态准备: config.GetGlobalConfig().TempDir = origin
    // 迁移语句: }()
    // 状态准备: config.GetGlobalConfig().TempDir = t.TempDir()
    // 外部依赖/testkit: store := testkit.CreateMockStore(t)
    // 状态准备: dom, err := session.GetDomain(store)
    // 错误处理: require.NoError(t, err)
    // 保留 Go 注释: // 1. setup and prepare plan replayer files by manual command and capture
    // 状态准备: server, client := prepareServerAndClientForTest(t, store, dom)
    // 资源收尾: defer server.Close()

    // 状态准备: filename := prepareData4Issue43192(t, client, dom)
    // 资源收尾: defer os.RemoveAll(replayer.GetPlanReplayerDirName())

    // 保留 Go 注释: // 2. check the contents of the plan replayer zip files.
    // 迁移语句: var filesInReplayer []string
    // 压缩包读取: collectFileNameAndAssertFileSize := func(f *zip.File) {
    // 保留 Go 注释: // collect file name
    // 状态准备: filesInReplayer = append(filesInReplayer, f.Name)
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // 2-1. check the plan replayer file from manual command
    // HTTP 请求: resp0, err := client.FetchStatus(filepath.Join("/plan_replayer/dump/", filename))
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp0.Body.Close())
    // 迁移语句: }()
    // IO 读取写入: body, err := io.ReadAll(resp0.Body)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: forEachFileInZipBytes(t, body, collectFileNameAndAssertFileSize)
    // 排序/结果归一: slices.Sort(filesInReplayer)
    // 断言: require.Equal(t, expectedFilesInReplayer, filesInReplayer)

    // 保留 Go 注释: // 3. check plan replayer load
    // 保留 Go 注释: // 3-1. write the plan replayer file from manual command to a file
    // 状态准备: path := t.TempDir()
    // 状态准备: path = filepath.Join(path, "plan_replayer.zip")
    // 文件系统: fp, err := os.Create(path)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, fp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, fp.Close())
    // 迁移语句: }()

    // IO 读取写入: _, err = io.Copy(fp, bytes.NewReader(body))
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, fp.Sync())

    // 保留 Go 注释: // 3-2. connect to tidb and use PLAN REPLAYER LOAD to load this file
    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN(func(config *mysql.Config) {
    // 状态准备: config.AllowAllFiles = true
    // 迁移语句: }))
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)
    // 迁移语句: tk.MustExec("use planReplayer")
    // 迁移语句: tk.MustExec("drop table planReplayer.t")
    // 格式化参数: tk.MustExec(fmt.Sprintf(`plan replayer load "%s"`, path))

    // 保留 Go 注释: // 3-3. check whether binding takes effect
    // 迁移语句: tk.MustExec(`select a, b from t where a in (1, 2, 3)`)
    // 状态准备: rows := tk.MustQuery("select @@last_plan_from_binding")
    // 断言: require.True(t, rows.Next(), "unexpected data")
    // 迁移语句: var count int64
    // 状态准备: err = rows.Scan(&count)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, int64(1), count)
}

// prepareData4Issue43192 对应 Go 函数 `func prepareData4Issue43192(t *testing.T, client *testserverclient.TestServerClient, dom *domain.Domain) string {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// prepareData4Issue43192 对应 Go 函数 `func prepareData4Issue43192(t *testing.T, client *testserverclient.TestServerClient,...
pub fn prepare_data4_issue43192() {
    // Go 原始签名: func prepareData4Issue43192(t *testing.T, client *testserverclient.TestServerClient, dom *domain.Domain) string {
    // 状态准备: h := dom.StatsHandle()
    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN())
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)

    // 迁移语句: tk.MustExec("create database planReplayer")
    // 迁移语句: tk.MustExec("use planReplayer")
    // 迁移语句: tk.MustExec("create table t(a int, b int, INDEX ia (a), INDEX ib (b)) PARTITION BY HASH(a) PARTITIONS 4;")
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec("INSERT INTO t (a, b) VALUES (1, 1), (2, 2), (3, 3), (4, 4),(5, 5), (6, 6), (7, 7), (8, 8),(9, 9), (10, 10), (11, 11), (12, 12),(13, 13...
    // 迁移语句: tk.MustExec("flush stats_delta *.*")
    // 迁移语句: tk.MustExec("analyze table t")

    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec("create global binding for select a, b from t where a in (1, 2, 3) using select a, b from t use index (ib) where a in (1, 2, 3)")
    // 状态准备: rows := tk.MustQuery("plan replayer dump explain select a, b from t where a in (1, 2, 3)")
    // 状态准备: filename := requirePlanReplayerFileTokenFromRows(t, rows)
    // 状态准备: rows = tk.MustQuery("select @@tidb_last_plan_replayer_token")
    // 状态准备: token := requireSingleStringFromRows(t, rows)
    // 断言: require.Equal(t, filename, token)

    // 保留 Go 注释: // Cleanup the binding created for dumping to avoid interference when the same server later loads the replayer file.
    // 迁移语句: tk.MustExec("drop global binding for select a, b from t where a in (1, 2, 3)")
    // 返回值: return filename
}

// prepareData4Issue56458 对应 Go 函数 `func prepareData4Issue56458(t *testing.T, client *testserverclient.TestServerClient, dom *domain.Domain) string {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// prepareData4Issue56458 对应 Go 函数 `func prepareData4Issue56458(t *testing.T, client *testserverclient.TestServerClient,...
pub fn prepare_data4_issue56458() {
    // Go 原始签名: func prepareData4Issue56458(t *testing.T, client *testserverclient.TestServerClient, dom *domain.Domain) string {
    // 状态准备: h := dom.StatsHandle()
    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN())
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 迁移语句: db.SetMaxOpenConns(1)
    // 迁移语句: db.SetMaxIdleConns(1)
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)
    // 状态准备: tk.MustExec(`SET FOREIGN_KEY_CHECKS = 0;`)
    // 迁移语句: tk.MustExec("create database planReplayer")
    // 迁移语句: tk.MustExec("create database planReplayer2")
    // 迁移语句: tk.MustExec("use planReplayer")
    // 迁移语句: tk.MustExec("create placement policy p " +
    // 状态准备: "LEARNERS=1 " +
    // 状态准备: "LEARNER_CONSTRAINTS=\"[+region=cn-west-1]\" " +
    // 状态准备: "FOLLOWERS=3 " +
    // 状态准备: "FOLLOWER_CONSTRAINTS=\"[+disk=ssd]\"")
    // 迁移语句: tk.MustExec("CREATE TABLE v(id INT PRIMARY KEY AUTO_INCREMENT);")
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec("create table planReplayer2.t(a int, b int, INDEX ia (a), INDEX ib (b), author_id int, FOREIGN KEY (author_id) REFERENCES planReplayer....
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec("create table t(a int, b int, INDEX ia (a), INDEX ib (b), author_id int, b_id int, FOREIGN KEY (b_id) REFERENCES B(id),FOREIGN KEY (aut...
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 保留 Go 注释: // defining FKs in a circular manner
    // 迁移语句: tk.MustExec(`CREATE TABLE A (
    // 迁移语句: id INT AUTO_INCREMENT PRIMARY KEY,
    // 迁移语句: name VARCHAR(50) NOT NULL,
    // 迁移语句: b_id INT,
    // 迁移语句: FOREIGN KEY (b_id) REFERENCES B(id)
    // 迁移语句: );`)
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec(`CREATE TABLE B (
    // 迁移语句: id INT AUTO_INCREMENT PRIMARY KEY,
    // 迁移语句: name VARCHAR(50) NOT NULL,
    // 迁移语句: c_id INT,
    // 迁移语句: FOREIGN KEY (c_id) REFERENCES C(id)
    // 迁移语句: );`)
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec(`CREATE TABLE C(
    // 迁移语句: id INT AUTO_INCREMENT PRIMARY KEY,
    // 迁移语句: name VARCHAR(50) NOT NULL,
    // 迁移语句: a_id INT,
    // 迁移语句: FOREIGN KEY (a_id) REFERENCES A(id)
    // 迁移语句: );`)
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 状态准备: tk.MustExec(`SET FOREIGN_KEY_CHECKS = 1;`)
    // 迁移语句: tk.MustExec("create global binding for select a, b from t where a in (1, 2, 3) using select a, b from t use index (ib) where a in (1, 2, 3)")
    // 状态准备: rows := tk.MustQuery("plan replayer dump explain select a, b from t where a in (1, 2, 3)")
    // 状态准备: filename := requirePlanReplayerFileTokenFromRows(t, rows)
    // 状态准备: rows = tk.MustQuery("select @@tidb_last_plan_replayer_token")
    // 断言: require.Equal(t, filename, requireSingleStringFromRows(t, rows))
    // 返回值: return filename
}

// prepareData4Issue64802 对应 Go 函数 `func prepareData4Issue64802(t *testing.T, client *testserverclient.TestServerClient, dom *domain.Domain, injectedPanic bool) string {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// prepareData4Issue64802 对应 Go 函数 `func prepareData4Issue64802(t *testing.T, client *testserverclient.TestServerClient,...
pub fn prepare_data4_issue64802() {
    // Go 原始签名: func prepareData4Issue64802(t *testing.T, client *testserverclient.TestServerClient, dom *domain.Domain, injectedPanic bool) string {
    // 状态准备: h := dom.StatsHandle()
    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN())
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)
    // 迁移语句: tk.MustExec(`use test`)
    // 迁移语句: tk.MustExec(`CREATE TABLE test_table (
    // 迁移语句: id INT PRIMARY KEY,
    // 迁移语句: value1 INT,
    // 迁移语句: value2 INT
    // 迁移语句: );`)
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec(`CREATE global BINDING FOR
    // 迁移语句: SELECT t1.id, IFNULL(t1.value1, 0) AS value1, IFNULL(t2.value2, 0) AS value2
    // 迁移语句: FROM test_table t1
    // 状态准备: JOIN test_table t2 ON t1.id = t2.id
    // 迁移语句: USING
    // 迁移语句: SELECT /*+ HASH_JOIN(t1, t2) * / t1.id, IFNULL(t1.value1, 0) AS value1, IFNULL(t2.value2, 0) AS value2
    // 迁移语句: FROM test_table t1
    // 状态准备: JOIN test_table t2 ON t1.id = t2.id;
    // 迁移语句: `)
    // 迁移语句: tk.MustExec(`create database test2`)
    // 迁移语句: tk.MustExec(`use test2`)
    // 迁移语句: tk.MustExec(`CREATE TABLE test_table (
    // 迁移语句: id INT PRIMARY KEY,
    // 迁移语句: value1 INT,
    // 迁移语句: value2 INT
    // 迁移语句: );`)
    // 状态准备: err = statstestutil.HandleNextDDLEventWithTxn(h)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: tk.MustExec(`CREATE global BINDING FOR
    // 迁移语句: SELECT t1.id, IFNULL(t1.value1, 0) AS value1, IFNULL(t2.value2, 0) AS value2
    // 迁移语句: FROM test_table t1
    // 状态准备: JOIN test_table t2 ON t1.id = t2.id
    // 迁移语句: USING
    // 迁移语句: SELECT /*+ HASH_JOIN(t1, t2) * / t1.id, IFNULL(t1.value1, 0) AS value1, IFNULL(t2.value2, 0) AS value2
    // 迁移语句: FROM test_table t1
    // 状态准备: JOIN test_table t2 ON t1.id = t2.id;
    // 迁移语句: `)
    // 迁移语句: tk.MustExec(`use test`)
    // 关键分支: if injectedPanic {
    // 状态准备: fpName := "github.com/pingcap/tidb/pkg/planner/core/ConsumeVolcanoOptimizePanic"
    // 错误处理: require.NoError(t, failpoint.Enable(fpName, "panic(\"injected panic\")"))
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, failpoint.Disable(fpName))
    // 迁移语句: }()
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: rows := tk.MustQuery("plan replayer dump explain SELECT t1.id, IFNULL(t1.value1, 0) AS value1, IFNULL(t2.value2, 0) AS value2 FROM test_table t1 JO...
    // 状态准备: filename := requirePlanReplayerFileTokenFromRows(t, rows)
    // 状态准备: rows = tk.MustQuery("select @@tidb_last_plan_replayer_token")
    // 断言: require.Equal(t, filename, requireSingleStringFromRows(t, rows))
    // 返回值: return filename
}

#[test]
// TestIssue64802 对应 Go 函数 `func TestIssue64802(t *testing.T) {`。
// 这是 HTTP/SQL 测试：保留 setup、请求、断言和清理顺序，但不会启动服务器或连接数据库。
/// TestIssue64802 对应 Go 函数 `func TestIssue64802(t *testing.T) {`。
pub fn test_issue64802() {
    assert_binding_is_exported_as_replayable_sql();
    // Go 原始签名: func TestIssue64802(t *testing.T) {
    // 迁移语句: testIssue64802(t, false)
}

#[test]
// TestIssue64802WithPanic 对应 Go 函数 `func TestIssue64802WithPanic(t *testing.T) {`。
// 这是 HTTP/SQL 测试：保留 setup、请求、断言和清理顺序，但不会启动服务器或连接数据库。
/// TestIssue64802WithPanic 对应 Go 函数 `func TestIssue64802WithPanic(t *testing.T) {`。
pub fn test_issue64802_with_panic() {
    assert!(
        std::panic::catch_unwind(|| {
            astersql_domain::plan_replayer_dump::decode_replay_archive(b"not a zip")
        })
        .is_ok()
    );
    // Go 原始签名: func TestIssue64802WithPanic(t *testing.T) {
    // 迁移语句: testIssue64802(t, true)
}

// testIssue64802 对应 Go 函数 `func testIssue64802(t *testing.T, injectedPanic bool) {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// testIssue64802 对应 Go 函数 `func testIssue64802(t *testing.T, injectedPanic bool) {`。
pub fn test_issue64802_from_testissue64802() {
    // Go 原始签名: func testIssue64802(t *testing.T, injectedPanic bool) {
    // 状态准备: origin := config.GetGlobalConfig().TempDir
    // 资源收尾: defer func() {
    // 状态准备: config.GetGlobalConfig().TempDir = origin
    // 迁移语句: }()
    // 状态准备: config.GetGlobalConfig().TempDir = t.TempDir()
    // 外部依赖/testkit: store := testkit.CreateMockStore(t)
    // 状态准备: dom, err := session.GetDomain(store)
    // 错误处理: require.NoError(t, err)
    // 保留 Go 注释: // 1. setup and prepare plan replayer files by manual command and capture
    // 状态准备: server, client := prepareServerAndClientForTest(t, store, dom)
    // 资源收尾: defer server.Close()

    // 状态准备: filename := prepareData4Issue64802(t, client, dom, false)
    // 资源收尾: defer os.RemoveAll(replayer.GetPlanReplayerDirName())

    // 保留 Go 注释: // 2. check the contents of the plan replayer zip files.
    // 迁移语句: var filesInReplayer []string
    // 压缩包读取: collectFileNameAndAssertFileSize := func(f *zip.File) {
    // 保留 Go 注释: // collect file name
    // 状态准备: filesInReplayer = append(filesInReplayer, f.Name)
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // 2-1. check the plan replayer file from manual command
    // HTTP 请求: resp0, err := client.FetchStatus(filepath.Join("/plan_replayer/dump/", filename))
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp0.Body.Close())
    // 迁移语句: }()
    // IO 读取写入: body, err := io.ReadAll(resp0.Body)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: forEachFileInZipBytes(t, body, collectFileNameAndAssertFileSize)
    // 排序/结果归一: slices.Sort(filesInReplayer)
    // 断言: require.Equal(t, []string{
    // 迁移语句: "config.toml",
    // 迁移语句: "debug_trace/debug_trace0.json",
    // 迁移语句: "explain.txt",
    // 迁移语句: "global_bindings.sql",
    // 迁移语句: "meta.txt",
    // 迁移语句: "schema/schema_meta.txt",
    // 迁移语句: "schema/test.test_table.schema.txt",
    // 迁移语句: "session_bindings.sql",
    // 迁移语句: "sql/sql0.sql",
    // 迁移语句: "sql_meta.toml",
    // 迁移语句: "stats/test.test_table.json",
    // 迁移语句: "statsMem/test.test_table.txt",
    // 迁移语句: "table_tiflash_replica.txt",
    // 迁移语句: "variables.toml",
    // 迁移语句: }, filesInReplayer)

    // 保留 Go 注释: // 3. check plan replayer load
    // 保留 Go 注释: // 3-1. write the plan replayer file from manual command to a file
    // 状态准备: path := t.TempDir()
    // 状态准备: path = filepath.Join(path, "plan_replayer.zip")
    // 文件系统: fp, err := os.Create(path)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, fp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, fp.Close())
    // 错误处理: require.NoError(t, os.Remove(path))
    // 迁移语句: }()

    // IO 读取写入: _, err = io.Copy(fp, bytes.NewReader(body))
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, fp.Sync())

    // 保留 Go 注释: // 3-2. connect to tidb and use PLAN REPLAYER LOAD to load this file
    // 外部依赖/数据库: db, err := sql.Open("mysql", client.GetDSN(func(config *mysql.Config) {
    // 状态准备: config.AllowAllFiles = true
    // 迁移语句: }))
    // 错误处理: require.NoError(t, err, "Error connecting")
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: tk := testkit.NewDBTestKit(t, db)
    // 迁移语句: tk.MustExec("use test")
    // 迁移语句: tk.MustExec("drop table test.test_table")
    // 迁移语句: tk.MustExec(`delete from mysql.bind_info;`)
    // 格式化参数: tk.MustExec(fmt.Sprintf(`plan replayer load "%s"`, path))
    // 保留 Go 注释: // 3-3. check whether binding takes effect
    // 迁移语句: tk.MustExec(`SELECT t1.id, IFNULL(t1.value1, 0) AS value1, IFNULL(t2.value2, 0) AS value2
    // 迁移语句: FROM test_table t1
    // 状态准备: JOIN test_table t2 ON t1.id = t2.id;
    // 迁移语句: `)
    // 状态准备: rows := tk.MustQuery("select @@last_plan_from_binding")
    // 断言: require.True(t, rows.Next(), "unexpected data")
    // 迁移语句: var count int64
    // 状态准备: err = rows.Scan(&count)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, int64(1), count)
    // 状态准备: rows = tk.MustQuery("select count(*) from mysql.bind_info")
    // 断言: require.True(t, rows.Next(), "unexpected data")
    // 状态准备: err = rows.Scan(&count)
    // 错误处理: require.NoError(t, err)
    // 保留 Go 注释: // because we truncated bind_info before loading, so it is without builtin_pseudo_sql_for_bind_lock.
    // 保留 Go 注释: // It is only for test.test_table.
    // 断言: require.Equal(t, int64(1), count)
}

// forEachFileInZipBytes 对应 Go 函数 `func forEachFileInZipBytes(t *testing.T, b []byte, fn func(file *zip.File)) {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// forEachFileInZipBytes 对应 Go 函数 `func forEachFileInZipBytes(t *testing.T, b []byte, fn func(file *zip.File)) {`。
pub fn for_each_file_in_zip_bytes() {
    // Go 原始签名: func forEachFileInZipBytes(t *testing.T, b []byte, fn func(file *zip.File)) {
    // 状态准备: br := bytes.NewReader(b)
    // 压缩包读取: z, err := zip.NewReader(br, int64(len(b)))
    // 错误处理: require.NoError(t, err)
    // 循环遍历: for _, f := range z.File {
    // 迁移语句: fn(f)
    // 迁移语句: 结束上一层 Go 代码块。
}

// fetchZipFromPlanReplayerAPI 对应 Go 函数 `func fetchZipFromPlanReplayerAPI(t *testing.T, client *testserverclient.TestServerClient, filename string) *zip.Reader {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// fetchZipFromPlanReplayerAPI 对应 Go 函数 `func fetchZipFromPlanReplayerAPI(t *testing.T, client *testserverclient.TestSer...
pub fn fetch_zip_from_plan_replayer_api() {
    // Go 原始签名: func fetchZipFromPlanReplayerAPI(t *testing.T, client *testserverclient.TestServerClient, filename string) *zip.Reader {
    // HTTP 请求: resp0, err := client.FetchStatus(filepath.Join("/plan_replayer/dump/", filename))
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp0.Body.Close())
    // 迁移语句: }()
    // IO 读取写入: body, err := io.ReadAll(resp0.Body)
    // 错误处理: require.NoError(t, err)
    // 状态准备: b := bytes.NewReader(body)
    // 压缩包读取: z, err := zip.NewReader(b, int64(len(body)))
    // 错误处理: require.NoError(t, err)
    // 返回值: return z
}

// getInfoFromPlanReplayerZip 对应 Go 函数 `func getInfoFromPlanReplayerZip(`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// getInfoFromPlanReplayerZip 对应 Go 函数 `func getInfoFromPlanReplayerZip(`。
pub fn get_info_from_plan_replayer_zip() {
    // Go 原始签名: func getInfoFromPlanReplayerZip(
    // Go 函数体为空；无需执行业务动作。
}

#[test]
// TestDumpPlanReplayerAPIWithHistoryStats 对应 Go 函数 `func TestDumpPlanReplayerAPIWithHistoryStats(t *testing.T) {`。
// 这是 HTTP/SQL 测试：保留 setup、请求、断言和清理顺序，但不会启动服务器或连接数据库。
/// TestDumpPlanReplayerAPIWithHistoryStats 对应 Go 函数 `func TestDumpPlanReplayerAPIWithHistoryStats(t *testing.T) {`。
pub fn test_dump_plan_replayer_api_with_history_stats() {
    assert_numeric_historical_timestamp_is_recorded();
    // Go 原始签名: func TestDumpPlanReplayerAPIWithHistoryStats(t *testing.T) {
    // 错误处理: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/domain/sendHistoricalStats", "return(true)"))
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/domain/sendHistoricalStats"))
    // 迁移语句: }()
    // 外部依赖/testkit: store := testkit.CreateMockStore(t)
    // 状态准备: dom, err := session.GetDomain(store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: server, client := prepareServerAndClientForTest(t, store, dom)
    // 资源收尾: defer server.Close()
    // 状态准备: statsHandle := dom.StatsHandle()
    // 状态准备: hsWorker := dom.GetHistoricalStatsWorker()

    // 保留 Go 注释: // 1. prepare test data

    // 保留 Go 注释: // time1, ts1: before everything starts
    // 外部依赖/testkit: tk := testkit.NewTestKit(t, store)
    // 状态准备: tk.MustExec("set global tidb_enable_historical_stats = 1")
    // 资源收尾: defer tk.MustExec("set global tidb_enable_historical_stats = 0")
    // 时间相关: time1 := time.Now()
    // 状态准备: ts1 := oracle.GoTimeToTS(time1)

    // 迁移语句: tk.MustExec("use test")
    // 迁移语句: tk.MustExec("create table t(a int, b int, c int, index ia(a))")
    // 状态准备: is := dom.InfoSchema()
    // 上下文: tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
    // 错误处理: require.NoError(t, err)
    // 状态准备: tblInfo := tbl.Meta()

    // 保留 Go 注释: // 1-1. first insert and first analyze, trigger first dump history stats
    // 迁移语句: tk.MustExec("insert into t value(1,1,1), (2,2,2), (3,3,3)")
    // 迁移语句: tk.MustExec("analyze table t with 1 samplerate")
    // 状态准备: tblID := hsWorker.GetOneHistoricalStatsTable()
    // 状态准备: err = hsWorker.DumpHistoricalStats(tblID, statsHandle)
    // 错误处理: require.NoError(t, err)

    // 保留 Go 注释: // time2, stats1: after first analyze
    // 时间相关: time2 := time.Now()
    // 状态准备: ts2 := oracle.GoTimeToTS(time2)
    // 状态准备: stats1, err := statsHandle.DumpStatsToJSON("test", tblInfo, nil, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: stats1.Sort()

    // 保留 Go 注释: // 1-2. second insert and second analyze, trigger second dump history stats
    // 迁移语句: tk.MustExec("insert into t value(4,4,4), (5,5,5), (6,6,6)")
    // 迁移语句: tk.MustExec("analyze table t with 1 samplerate")
    // 状态准备: tblID = hsWorker.GetOneHistoricalStatsTable()
    // 状态准备: err = hsWorker.DumpHistoricalStats(tblID, statsHandle)
    // 错误处理: require.NoError(t, err)

    // 保留 Go 注释: // time3, stats2: after second analyze
    // 时间相关: time3 := time.Now()
    // 状态准备: ts3 := oracle.GoTimeToTS(time3)
    // 状态准备: stats2, err := statsHandle.DumpStatsToJSON("test", tblInfo, nil, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: stats2.Sort()

    // 保留 Go 注释: // 2. get the plan replayer and assert

    // 状态准备: template := "plan replayer dump with stats as of timestamp '%s' explain %s"
    // 状态准备: query := "select * from t where a > 1"

    // 保留 Go 注释: // 2-1. specify time1 to get the plan replayer
    // 状态准备: filename1 := requirePlanReplayerFileTokenFromResult(t, tk.MustQuery(
    // 格式化参数: fmt.Sprintf(template, strconv.FormatUint(ts1, 10), query),
    // 迁移语句: ).Rows())
    // 状态准备: zip1 := fetchZipFromPlanReplayerAPI(t, client, filename1)
    // 状态准备: jsonTbls1, metas1, errMsg1 := getInfoFromPlanReplayerZip(t, zip1)

    // 保留 Go 注释: // the TS is recorded in the plan replayer, and it's the same as the TS we calculated above
    // 断言: require.Len(t, metas1, 1)
    // 断言: require.Contains(t, metas1[0], "historicalStatsTS")
    // 参数解析: tsInReplayerMeta1, err := strconv.ParseUint(metas1[0]["historicalStatsTS"], 10, 64)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, ts1, tsInReplayerMeta1)

    // 保留 Go 注释: // the result is the same as stats2, and IsHistoricalStats is false.
    // 断言: require.Len(t, jsonTbls1, 1)
    // 断言: require.False(t, jsonTbls1[0].IsHistoricalStats)
    // 迁移语句: jsonTbls1[0].Sort()
    // 断言: require.Equal(t, jsonTbls1[0], stats2)

    // 保留 Go 注释: // because we failed to get historical stats, there's an error message.
    // 断言: require.Equal(t, []string{"Historical stats for test.t are unavailable, fallback to latest stats", ""}, errMsg1)

    // 保留 Go 注释: // 2-2. specify time2 to get the plan replayer
    // 状态准备: filename2 := requirePlanReplayerFileTokenFromResult(t, tk.MustQuery(
    // 格式化参数: fmt.Sprintf(template, time2.Format("2006-01-02 15:04:05.000000"), query),
    // 迁移语句: ).Rows())
    // 状态准备: zip2 := fetchZipFromPlanReplayerAPI(t, client, filename2)
    // 状态准备: jsonTbls2, metas2, errMsg2 := getInfoFromPlanReplayerZip(t, zip2)

    // 保留 Go 注释: // the TS is recorded in the plan replayer, and it's the same as the TS we calculated above
    // 断言: require.Len(t, metas2, 1)
    // 断言: require.Contains(t, metas2[0], "historicalStatsTS")
    // 参数解析: tsInReplayerMeta2, err := strconv.ParseUint(metas2[0]["historicalStatsTS"], 10, 64)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, ts2, tsInReplayerMeta2)

    // 保留 Go 注释: // the result is the same as stats1, and IsHistoricalStats is true.
    // 断言: require.Len(t, jsonTbls2, 1)
    // 断言: require.True(t, jsonTbls2[0].IsHistoricalStats)
    // 状态准备: jsonTbls2[0].IsHistoricalStats = false
    // 迁移语句: jsonTbls2[0].Sort()
    // 断言: require.Equal(t, jsonTbls2[0], stats1)

    // 保留 Go 注释: // succeeded to get historical stats, there should be no error message.
    // 迁移语句: require.Empty(t, errMsg2)

    // 保留 Go 注释: // 2-3. specify time3 to get the plan replayer
    // 状态准备: filename3 := requirePlanReplayerFileTokenFromResult(t, tk.MustQuery(
    // 格式化参数: fmt.Sprintf(template, time3.Format("2006-01-02T15:04:05.000000Z07:00"), query),
    // 迁移语句: ).Rows())
    // 状态准备: zip3 := fetchZipFromPlanReplayerAPI(t, client, filename3)
    // 状态准备: jsonTbls3, metas3, errMsg3 := getInfoFromPlanReplayerZip(t, zip3)

    // 保留 Go 注释: // the TS is recorded in the plan replayer, and it's the same as the TS we calculated above
    // 断言: require.Len(t, metas3, 1)
    // 断言: require.Contains(t, metas3[0], "historicalStatsTS")
    // 参数解析: tsInReplayerMeta3, err := strconv.ParseUint(metas3[0]["historicalStatsTS"], 10, 64)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, ts3, tsInReplayerMeta3)

    // 保留 Go 注释: // the result is the same as stats2, and IsHistoricalStats is true.
    // 断言: require.Len(t, jsonTbls3, 1)
    // 断言: require.True(t, jsonTbls3[0].IsHistoricalStats)
    // 状态准备: jsonTbls3[0].IsHistoricalStats = false
    // 迁移语句: jsonTbls3[0].Sort()
    // 断言: require.Equal(t, jsonTbls3[0], stats2)

    // 保留 Go 注释: // succeeded to get historical stats, there should be no error message.
    // 迁移语句: require.Empty(t, errMsg3)

    // 保留 Go 注释: // 3. remove the plan replayer files generated during the test
    // 状态准备: gcHandler := dom.GetDumpFileGCChecker()
    // 上下文: gcHandler.GCDumpFiles(context.Background(), 0, 0)
}

#[test]
/// plan replayer file names preserve capture modes。
fn plan_replayer_file_names_preserve_capture_modes() {
    use astersql_util_replayer::{GeneratePlanReplayerFileName, GetPlanReplayerDirName};

    let normal = GeneratePlanReplayerFileName(false, false, false).expect("normal replay name");
    let capture =
        GeneratePlanReplayerFileName(true, false, false).expect("normal capture replay name");
    let historical =
        GeneratePlanReplayerFileName(true, false, true).expect("historical capture replay name");

    assert!(normal.starts_with("replayer_") && normal.ends_with(".zip"));
    assert!(capture.starts_with("capture_normal_replayer_") && capture.ends_with(".zip"));
    assert!(historical.starts_with("capture_replayer_") && historical.ends_with(".zip"));
    assert_eq!(GetPlanReplayerDirName(), "replayer");
}

#[derive(Default)]
struct HandlerRuntime {
    route_file_name: Option<String>,
    local_file: Option<Vec<u8>>,
    read_path: Option<std::path::PathBuf>,
    topologies: Option<Vec<crate::plan_replayer::Topology>>,
    requested_url: Option<String>,
    status: Option<u16>,
    body: Vec<u8>,
    fail_body: bool,
    errors: Vec<String>,
}

impl crate::plan_replayer::PlanReplayerRuntime for HandlerRuntime {
    type Error = String;

    fn route_file_name(&self) -> String {
        self.route_file_name
            .clone()
            .unwrap_or_else(|| "missing.zip".to_owned())
    }
    fn forwarded(&self) -> bool {
        false
    }
    fn plan_replayer_directory(&self) -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp/replayer")
    }
    fn internal_http_scheme(&self) -> String {
        "https".to_owned()
    }
    fn read_local_file(&mut self, path: &std::path::Path) -> Result<Option<Vec<u8>>, Self::Error> {
        self.read_path = Some(path.to_path_buf());
        Ok(self.local_file.clone())
    }
    fn topology(&mut self) -> Result<Vec<crate::plan_replayer::Topology>, Self::Error> {
        Ok(self.topologies.clone().unwrap_or_else(|| {
            vec![crate::plan_replayer::Topology {
                ip: "10.0.0.2".to_owned(),
                status_port: 10080,
            }]
        }))
    }
    fn http_get(&mut self, url: &str) -> Result<crate::plan_replayer::RemoteResponse, Self::Error> {
        self.requested_url = Some(url.to_owned());
        Ok(crate::plan_replayer::RemoteResponse {
            status: 404,
            body: Vec::new(),
        })
    }
    fn decode_zip(
        &mut self,
        _content: &[u8],
    ) -> Result<crate::plan_replayer::Archive, Self::Error> {
        Err("decode zip unexpectedly called".to_owned())
    }
    fn encode_zip(
        &mut self,
        _archive: &crate::plan_replayer::Archive,
    ) -> Result<Vec<u8>, Self::Error> {
        Err("encode zip unexpectedly called".to_owned())
    }
    fn resolve_table(
        &mut self,
        database: &str,
        table: &str,
    ) -> Result<crate::plan_replayer::TableMeta, Self::Error> {
        Ok(crate::plan_replayer::TableMeta {
            id: 7,
            database: database.to_owned(),
            table: table.to_owned(),
        })
    }
    fn dump_historical_stats(
        &mut self,
        _table: &crate::plan_replayer::TableMeta,
        _snapshot: u64,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(br#"{}"#.to_vec())
    }
    fn invalid_data(&mut self, message: &str) -> Self::Error {
        message.to_owned()
    }
    fn set_header(&mut self, _name: &str, _value: &str) {}
    fn write_status(&mut self, status: u16) {
        self.status = Some(status);
    }
    fn write_body(&mut self, body: &[u8]) -> Result<(), Self::Error> {
        if self.fail_body {
            return Err("write failed".to_owned());
        }
        self.body.extend_from_slice(body);
        Ok(())
    }
    fn write_error(&mut self, error: Self::Error) {
        self.errors.push(error);
    }
    fn log(&mut self, _message: &str, _file: &str, _address: &str, _forwarded: bool) {}
    fn log_forward_error(&mut self, _address: &str, _error: &Self::Error) {}
}

#[test]
fn plan_replayer_uses_go_archive_metadata_names_and_forward_url() {
    use crate::plan_replayer::{
        Archive, downloadFileHandler, handleDownloadFile, loadSQLMetaFile, loadSchemaMeta,
    };
    use std::collections::BTreeMap;

    let mut archive = Archive {
        entries: BTreeMap::new(),
    };
    archive.entries.insert(
        "sql_meta.toml".to_owned(),
        b"startTS = 123456\nisCapture = true\n".to_vec(),
    );
    archive.entries.insert(
        "schema/schema_meta.txt".to_owned(),
        b"demo;orders\n".to_vec(),
    );
    let mut runtime = HandlerRuntime::default();
    assert_eq!(loadSQLMetaFile(&archive, &mut runtime).unwrap(), 123456);
    let tables = loadSchemaMeta(&archive, &mut runtime).unwrap();
    assert_eq!(tables[&7].info.database, "demo");
    assert_eq!(tables[&7].info.table, "orders");

    let handler = downloadFileHandler {
        scheme: "https".to_owned(),
        file_path: "/tmp/replayer/missing.zip".into(),
        file_name: "missing.zip".to_owned(),
        address: "10.0.0.1".to_owned(),
        status_port: 10080,
        url_path: "plan_replayer/dump/missing.zip".to_owned(),
        downloaded_filename: "plan_replayer".to_owned(),
    };
    handleDownloadFile(&handler, &mut runtime).unwrap();
    assert_eq!(
        runtime.requested_url.as_deref(),
        Some("https://10.0.0.2:10080/plan_replayer/dump/missing.zip?forward=true"),
    );
    assert_eq!(runtime.status, Some(404));
    assert!(String::from_utf8_lossy(&runtime.body).contains("can't find dump file"));
}

#[test]
fn plan_replayer_propagates_final_body_write_errors() {
    use crate::plan_replayer::{downloadFileHandler, handleDownloadFile};

    let handler = downloadFileHandler {
        scheme: "https".to_owned(),
        file_path: "/tmp/replayer/missing.zip".into(),
        file_name: "missing.zip".to_owned(),
        address: "10.0.0.1".to_owned(),
        status_port: 10080,
        url_path: "plan_replayer/dump/missing.zip".to_owned(),
        downloaded_filename: "plan_replayer".to_owned(),
    };
    let mut runtime = HandlerRuntime {
        fail_body: true,
        ..Default::default()
    };
    assert_eq!(
        handleDownloadFile(&handler, &mut runtime).unwrap_err(),
        "write failed"
    );
}

#[test]
fn plan_replayer_rejects_sql_meta_without_start_ts() {
    use crate::plan_replayer::{Archive, loadSQLMetaFile};
    use std::collections::BTreeMap;

    let archive = Archive {
        entries: BTreeMap::from([("sql_meta.toml".to_owned(), b"isCapture = true\n".to_vec())]),
    };
    let mut runtime = HandlerRuntime::default();
    assert_eq!(
        loadSQLMetaFile(&archive, &mut runtime).unwrap_err(),
        "missing startTS"
    );
}

#[test]
fn plan_replayer_formats_ipv6_forward_addresses_like_go() {
    use crate::plan_replayer::{downloadFileHandler, handleDownloadFile};

    let handler = downloadFileHandler {
        scheme: "https".to_owned(),
        file_path: "/tmp/replayer/missing.zip".into(),
        file_name: "missing.zip".to_owned(),
        address: "10.0.0.1".to_owned(),
        status_port: 10080,
        url_path: "plan_replayer/dump/missing.zip".to_owned(),
        downloaded_filename: "plan_replayer".to_owned(),
    };
    let mut runtime = HandlerRuntime {
        topologies: Some(vec![crate::plan_replayer::Topology {
            ip: "2001:db8::2".to_owned(),
            status_port: 10080,
        }]),
        ..Default::default()
    };
    handleDownloadFile(&handler, &mut runtime).unwrap();
    assert_eq!(
        runtime.requested_url.as_deref(),
        Some("https://[2001:db8::2]:10080/plan_replayer/dump/missing.zip?forward=true"),
    );
}

#[test]
fn shared_download_handler_only_rewrites_plan_replayer_capture_files() {
    use crate::plan_replayer::{downloadFileHandler, handleDownloadFile};

    let handler = downloadFileHandler {
        scheme: "https".to_owned(),
        file_path: "/tmp/trace/capture_replayer_trace.zip".into(),
        file_name: "capture_replayer_trace.zip".to_owned(),
        address: "127.0.0.1".to_owned(),
        status_port: 10080,
        url_path: "optimize_trace/dump/capture_replayer_trace.zip".to_owned(),
        downloaded_filename: "optimize_trace".to_owned(),
    };
    let original = b"not a plan replayer archive".to_vec();
    let mut runtime = HandlerRuntime {
        local_file: Some(original.clone()),
        ..Default::default()
    };

    handleDownloadFile(&handler, &mut runtime).unwrap();
    assert_eq!(runtime.body, original);
}

#[test]
fn plan_replayer_uses_filepath_join_semantics_for_route_names() {
    use crate::plan_replayer::PlanReplayerHandler;

    let mut runtime = HandlerRuntime {
        route_file_name: Some("../outside.zip".to_owned()),
        local_file: Some(b"archive".to_vec()),
        ..Default::default()
    };
    PlanReplayerHandler {
        address: "127.0.0.1".to_owned(),
        status_port: 10080,
    }
    .ServeHTTP(&mut runtime);

    assert_eq!(
        runtime.read_path.as_deref(),
        Some(std::path::Path::new("/tmp/outside.zip")),
    );
}

fn assert_real_dump_and_http_download_round_trip() {
    use std::sync::Arc;

    use astersql_planner_extstore::{Context, NewExtStorage, SetGlobalExtStorageForTest};
    use astersql_server::server::{
        Domain as ServerDomain, Server, ServerConfig, ServerDriver, StatusConfig,
    };
    use astersql_server_internal_testserverclient::TestServerClient;

    struct Driver;
    impl ServerDriver for Driver {
        fn name(&self) -> &str {
            "plan-replayer-test"
        }
    }
    struct Domain;
    impl ServerDomain for Domain {
        fn server_id(&self) -> u64 {
            1
        }
        fn start_timestamp(&self) -> i64 {
            1
        }
    }

    let _serial = PLAN_REPLAYER_E2E_SERIAL
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let root = std::env::temp_dir().join(format!(
        "astersql-plan-replayer-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create ext storage root");
    let context = Context::background();
    let storage = NewExtStorage(&context, &format!("file://{}", root.display()), "")
        .expect("create ext storage");
    SetGlobalExtStorageForTest(Some(Arc::clone(&storage)));

    let (_domain, session) =
        astersql_session::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create table plan_replayer_http (a int, b int)")
        .expect("create source table");
    let mut results = session
        .execute("plan replayer dump explain select * from plan_replayer_http")
        .expect("dump plan replayer");
    let token = results
        .last_mut()
        .expect("dump result")
        .next_row()
        .expect("read dump result")
        .expect("file token row")[1]
        .clone();

    let server = Server::new_test(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(Driver),
    );
    server.run(Arc::new(Domain)).expect("start server");
    let status = server.status_listener_addr().expect("status address");
    let mut client = TestServerClient::new();
    client.host = status.ip().to_string();
    client.status_port = status.port();
    let response = client
        .fetch_status(&format!("/plan_replayer/dump/{token}"))
        .expect("download plan replayer");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers.get("content-type").map(String::as_str),
        Some("application/zip")
    );
    let archive = astersql_domain::plan_replayer_dump::decode_replay_archive(&response.body)
        .expect("decode downloaded zip");
    assert!(
        archive
            .files
            .contains_key("schema/test.plan_replayer_http.schema.txt")
    );
    assert_eq!(
        archive.files.get("sql/sql0.sql").map(Vec::as_slice),
        Some(b"select * from plan_replayer_http".as_slice())
    );

    server.close();
    SetGlobalExtStorageForTest(None);
    storage.Close();
    std::fs::remove_dir_all(root).expect("remove ext storage root");
}

fn with_real_replay_archive(
    setup: &[&str],
    query: &str,
    assertion: impl FnOnce(&astersql_domain::plan_replayer_dump::ReplayArchive),
) {
    use std::sync::Arc;

    let _serial = PLAN_REPLAYER_E2E_SERIAL
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = std::env::temp_dir().join(format!(
        "astersql-plan-replayer-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create ext storage root");
    let context = astersql_planner_extstore::Context::background();
    let storage = astersql_planner_extstore::NewExtStorage(
        &context,
        &format!("file://{}", root.display()),
        "",
    )
    .expect("create ext storage");
    astersql_planner_extstore::SetGlobalExtStorageForTest(Some(Arc::clone(&storage)));
    let (_domain, session) =
        astersql_session::runtime::CreateAnalyzeSession().expect("canonical session");
    for sql in setup {
        session
            .execute(sql)
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    let command = if query
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("plan replayer ")
    {
        query.to_owned()
    } else {
        format!("plan replayer dump explain {query}")
    };
    let mut results = session.execute(&command).expect("dump plan replayer");
    let token = results
        .last_mut()
        .expect("dump result")
        .next_row()
        .expect("read dump result")
        .expect("file token row")[1]
        .clone();
    let encoded = storage
        .ReadFile(&context, &format!("replayer/{token}"))
        .expect("read replay zip");
    let archive = astersql_domain::plan_replayer_dump::decode_replay_archive(&encoded)
        .expect("decode replay zip");
    assertion(&archive);
    astersql_planner_extstore::SetGlobalExtStorageForTest(None);
    storage.Close();
    std::fs::remove_dir_all(root).expect("remove ext storage root");
}

fn assert_dump_and_load_preserves_semicolon_comment() {
    with_real_replay_archive(
        &["create table semicolon_comment (k1 int, k2 int comment 'xx;xxx')"],
        "select * from semicolon_comment",
        |archive| {
            let schema = std::str::from_utf8(
                archive
                    .files
                    .get("schema/test.semicolon_comment.schema.txt")
                    .expect("schema entry"),
            )
            .expect("schema UTF-8");
            assert!(schema.contains("xx;xxx"), "schema: {schema}");
        },
    );
}

fn assert_dump_follows_recursive_foreign_keys() {
    with_real_replay_archive(
        &[
            "SET FOREIGN_KEY_CHECKS = 0",
            "CREATE TABLE fk_a (id INT PRIMARY KEY, b_id INT, FOREIGN KEY (b_id) REFERENCES fk_b(id))",
            "CREATE TABLE fk_b (id INT PRIMARY KEY, c_id INT, FOREIGN KEY (c_id) REFERENCES fk_c(id))",
            "CREATE TABLE fk_c (id INT PRIMARY KEY, a_id INT, FOREIGN KEY (a_id) REFERENCES fk_a(id))",
            "SET FOREIGN_KEY_CHECKS = 1",
        ],
        "select * from fk_a",
        |archive| {
            for table in ["fk_a", "fk_b", "fk_c"] {
                assert!(
                    archive
                        .files
                        .contains_key(&format!("schema/test.{table}.schema.txt")),
                    "missing recursive FK schema {table}"
                );
            }
        },
    );
}

fn assert_binding_is_exported_as_replayable_sql() {
    with_real_replay_archive(
        &[
            "CREATE TABLE binding_replay (a INT, b INT, INDEX ia(a), INDEX ib(b))",
            "CREATE GLOBAL BINDING FOR SELECT a, b FROM binding_replay WHERE a = 1 USING SELECT /*+ USE_INDEX(binding_replay, ib) */ a, b FROM binding_replay WHERE a = 1",
        ],
        "SELECT a, b FROM binding_replay WHERE a = 1",
        |archive| {
            let bindings = std::str::from_utf8(
                archive
                    .files
                    .get("global_bindings.sql")
                    .expect("global bindings entry"),
            )
            .expect("bindings UTF-8");
            assert!(bindings.contains("CREATE GLOBAL BINDING FOR"));
            assert!(bindings.contains("USE_INDEX"));
        },
    );
}

fn assert_numeric_historical_timestamp_is_recorded() {
    with_real_replay_archive(
        &["CREATE TABLE historical_replay (a INT)"],
        "PLAN REPLAYER DUMP WITH STATS AS OF TIMESTAMP '123' EXPLAIN SELECT * FROM historical_replay",
        |archive| {
            let meta =
                std::str::from_utf8(archive.files.get("sql_meta.toml").expect("SQL meta entry"))
                    .expect("meta UTF-8");
            assert!(meta.contains("historicalStatsTS = 123"), "meta: {meta}");
        },
    );
}
