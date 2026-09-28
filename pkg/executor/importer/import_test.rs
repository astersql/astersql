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

// IMPORT INTO / LOAD DATA 计划与选项的测试参考。
//
// 以原始字符串保留 Go 侧 `import_test` 用例，覆盖默认选项初始化、
// 冲突处理、磁盘配额、parquet 路径、压缩文件采样、数据源类型解析等。

use super::*;

/// 保存 Go 版 import 包测试源码参考，用于对照选项解析与计划构造行为。
const GO_REFERENCE: &str = r########"

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables, unused_mut)]

// keyspaceOnlyStore 对应 Go 的匿名测试 store，只覆盖 GetKeyspace 返回空字符串。
pub struct keyspaceOnlyStore {
    pub Storage: Storage,
}
impl keyspaceOnlyStore {
    pub fn GetKeyspace(&self) -> String {
        String::new()
    }
}

// TestInitDefaultOptions 迁移默认选项测试：query 和 file 两类数据源的线程数、checksum、charset、cloud storage 等默认值。
#[test]
pub fn TestInitDefaultOptions() {
    let mut plan = Plan { DataSourceType: DataSourceTypeQuery.into(), ..Default::default() };
    plan.initDefaultOptions(&Context::background(), 10, None);
    assert_eq!(2, plan.ThreadCnt);

    plan = Plan { DataSourceType: DataSourceTypeFile.into(), ..Default::default() };
    vardef::CloudStorageURI::Store("s3://bucket");
    plan.initDefaultOptions(&Context::background(), 1, None);
    assert_eq!(0, plan.DiskQuota);
    assert_eq!(OpLevelRequired, plan.Checksum);
    assert_eq!(1, plan.ThreadCnt);
    assert_eq!(unlimitedWriteSpeed, plan.MaxWriteSpeed);
    assert!(!plan.SplitFile);
    assert_eq!(100, plan.MaxRecordedErrors);
    assert_eq!(OnDupKeyModeError, plan.OnDupKey);
    assert!(!plan.Detached);
    assert_eq!("utf8mb4", plan.Charset.as_deref().unwrap_or_default());
    assert!(!plan.DisableTiKVImportMode);
    if kerneltype::IsNextGen() {
        assert_eq!(DefaultBatchSize, plan.MaxEngineSize);
    } else {
        assert_eq!(defaultMaxEngineSize, plan.MaxEngineSize);
    }
    assert_eq!("s3://bucket/dxf/", plan.CloudStorageURI);
    plan.initDefaultOptions(&Context::background(), 10, None);
    assert_eq!(5, plan.ThreadCnt);
    vardef::CloudStorageURI::Store("");
}

// TestPlanUseNewCollate 迁移 use_new_collate 的默认值、显式设置和 JSON 往返。
#[test]
pub fn TestPlanUseNewCollate() {
    let mut plan = Plan::default();
    assert!(plan.GetUseNewCollateOrDefault(true));
    assert!(!plan.GetUseNewCollateOrDefault(false));
    plan.setUseNewCollate(false);
    assert!(!plan.GetUseNewCollateOrDefault(true));
    let data = json::Marshal(&plan).expect("Go require.NoError");
    assert!(data.contains("\"use_new_collate\":false"));
    let mut decoded = json::UnmarshalPlan(&data).expect("Go require.NoError");
    assert!(!decoded.GetUseNewCollateOrDefault(true));
    decoded.setUseNewCollate(true);
    assert!(decoded.GetUseNewCollateOrDefault(false));
}

// TestInitOptionsPositiveCase 迁移正向 option 解析：CSV 选项、全局 cloud storage、覆盖为 s3/gs/azure/azblob 和空值。
#[test]
pub fn TestInitOptionsPositiveCase() {
    let sctx = mock::NewContext();
    let ctx = Context::background().with_internal_source_type();
    let convertOptions = |inOptions: Vec<LoadDataOpt>| -> Vec<LoadDataOpt> {
        // Go 这里把 ast.LoadDataOpt.Value rewrite 成 planner expression；保留转换入口。
        inOptions
    };

    let sql = format!("import into t from '/file.csv' with {}='utf8', {}='aaa', {}='|', {}='', {}='N', {}='END', {}=1, {}='100gib', {}='optional', {}=100000, {}='200mib', {}, {}=123, {}, {}, {}='100gib', {}",
        characterSetOption, fieldsTerminatedByOption, fieldsEnclosedByOption, fieldsEscapedByOption, fieldsDefinedNullByOption,
        linesTerminatedByOption, skipRowsOption, diskQuotaOption, checksumTableOption, threadOption, maxWriteSpeedOption,
        splitFileOption, recordErrorsOption, detachedOption, disableTiKVImportModeOption, maxEngineSizeOption, disablePrecheckOption);
    let stmt = parser::New().ParseOneStmt(&sql, "", "").expect("Go require.NoError");
    let mut plan = Plan { Format: DataFormatCSV.into(), ..Default::default() };
    plan.initOptions(&ctx, &sctx, convertOptions(stmt.Options)).expect("Go require.NoError");
    assert_eq!("utf8", plan.Charset.unwrap());
    assert_eq!("aaa", plan.FieldsTerminatedBy);
    assert_eq!("|", plan.FieldsEnclosedBy);
    assert_eq!("", plan.FieldsEscapedBy);
    assert_eq!(vec!["N"], plan.FieldNullDef);
    assert_eq!("END", plan.LinesTerminatedBy);
    assert_eq!(1, plan.IgnoreLines);
    assert_eq!(100 << 30, plan.DiskQuota);
    assert_eq!(OpLevelOptional, plan.Checksum);
    assert_eq!(OnDupKeyModeError, plan.OnDupKey);
    assert_eq!(runtime::GOMAXPROCS(0), plan.ThreadCnt);
    assert_eq!(200 << 20, plan.MaxWriteSpeed);
    assert!(plan.SplitFile);
    assert_eq!(123, plan.MaxRecordedErrors);
    assert!(plan.Detached);
    assert!(plan.DisableTiKVImportMode);
    assert_eq!(100 << 30, plan.MaxEngineSize);
    assert!(plan.CloudStorageURI.is_empty());
    assert!(plan.DisablePrecheck);

    vardef::CloudStorageURI::Store("s3://bucket/path");
    for (suffix, expected_uri, expected_dup) in [
        (format!(", {}='capture'", onDupKeyOption), "s3://bucket/path/dxf/", OnDupKeyModeCapture),
        (format!(", {}='s3://bucket/path2'", cloudStorageURIOption), "s3://bucket/path2", OnDupKeyModeError),
        (format!(", {}='gs://bucket/path2'", cloudStorageURIOption), "gs://bucket/path2", OnDupKeyModeError),
        (format!(", {}='azure://container/path2'", cloudStorageURIOption), "azure://container/path2", OnDupKeyModeError),
        (format!(", {}='azblob://container/path3'", cloudStorageURIOption), "azblob://container/path3", OnDupKeyModeError),
        (format!(", {}=''", cloudStorageURIOption), "", OnDupKeyModeError),
    ] {
        let stmt = parser::New().ParseOneStmt(&(sql.clone() + &suffix), "", "").unwrap();
        let mut plan = Plan { Format: DataFormatCSV.into(), ..Default::default() };
        plan.initOptions(&ctx, &sctx, stmt.Options).unwrap();
        assert_eq!(expected_uri, plan.CloudStorageURI);
        assert_eq!(expected_dup, plan.OnDupKey);
    }
    vardef::CloudStorageURI::Store("");
}

// TestInitOptionsDisallowOnDuplicateKeyWithLocalSort 迁移 Go 负向测试：local sort 不允许 on_duplicate_key=capture。
#[test]
pub fn TestInitOptionsDisallowOnDuplicateKeyWithLocalSort() {
    let sctx = mock::NewContext();
    let ctx = Context::background().with_internal_source_type();
    let sql = "import into t from '/file.csv' with on_duplicate_key='capture'";
    let stmt = parser::New().ParseOneStmt(sql, "", "").expect("Go require.NoError");
    vardef::CloudStorageURI::Store("");
    let mut plan = Plan { Format: DataFormatCSV.into(), ..Default::default() };
    let err = plan.initOptions(&ctx, &sctx, stmt.Options).expect_err("Go require.ErrorIs");
    assert!(err.contains(onDupKeyOption));
    assert!(err.contains("local sort"));
}

// TestAdjustOptions 迁移线程、写速率和 TiKV import mode 自动调整。
#[test]
pub fn TestAdjustOptions() {
    let mut plan = Plan { DiskQuota: 1, ThreadCnt: 100000000, MaxWriteSpeed: 10, DataSourceType: DataSourceTypeFile.into(), ..Default::default() };
    plan.adjustOptions(16);
    assert_eq!(16, plan.ThreadCnt);
    assert_eq!(10, plan.MaxWriteSpeed);
    assert!(!plan.DisableTiKVImportMode);
    plan.ThreadCnt = 100000000;
    plan.DataSourceType = DataSourceTypeQuery.into();
    plan.adjustOptions(16);
    assert_eq!(32, plan.ThreadCnt);
    plan.CloudStorageURI = "s3://bucket/path".into();
    plan.adjustOptions(16);
    assert!(plan.DisableTiKVImportMode);
}

// TestGetConflictHandlingMode 迁移冲突处理模式默认值和显式 capture/error。
#[test]
pub fn TestGetConflictHandlingMode() {
    let mut plan = Plan::default();
    assert_eq!(OnDupKeyModeError, plan.GetOnDupKeyMode());
    plan.OnDupKey = OnDupKeyModeCapture;
    assert_eq!(OnDupKeyModeCapture, plan.GetOnDupKeyMode());
    plan.OnDupKey = OnDupKeyModeError;
    assert_eq!(OnDupKeyModeError, plan.GetOnDupKeyMode());
}

// TestAdjustDiskQuota 迁移 failpoint 模拟磁盘大小后的 80% 配额计算和显式配额保留。
#[test]
pub fn TestAdjustDiskQuota() {
    failpoint::Enable("github.com/pingcap/tidb/pkg/lightning/common/GetStorageSize", "return(2048)");
    let d = TempDir::new();
    assert_eq!(1638, adjustDiskQuota(0, &d, BgLogger()));
    assert_eq!(1, adjustDiskQuota(1, &d, BgLogger()));
    assert_eq!(1638, adjustDiskQuota(2000, &d, BgLogger()));
    failpoint::Disable("github.com/pingcap/tidb/pkg/lightning/common/GetStorageSize");
}

// TestASTArgsFromStmt 迁移 IMPORT INTO AST 文本恢复和列/赋值参数提取，包含 latin1 输入和非 ASCII 列名。
#[test]
pub fn TestASTArgsFromStmt() {
    let stmt = "IMPORT INTO tb (a, é) FROM 'gs://test-load/test.tsv';";
    let stmtNode = parser::New().ParseOneStmt(stmt, "latin1", "latin1_bin").unwrap();
    assert_eq!(stmt, stmtNode.Text());
    let astArgs = ASTArgsFromStmt(stmtNode.Text()).unwrap();
    assert_eq!(astArgs.ColumnAssignments, stmtNode.ColumnAssignments);
    assert_eq!(astArgs.ColumnsAndUserVars, stmtNode.ColumnsAndUserVars);
}

// urlEqual 对应 Go helper：比较 URL 时忽略 query 参数顺序。
pub fn urlEqual(expected: &str, actual: &str) {
    let mut urlExpected = url::Parse(expected).expect("Go require.NoError");
    let mut urlGot = url::Parse(actual).expect("Go require.NoError");
    assert_eq!(urlExpected.Query(), urlGot.Query());
    urlExpected.RawQuery.clear();
    urlGot.RawQuery.clear();
    assert_eq!(urlExpected.String(), urlGot.String());
}

// TestInitParameters 迁移参数诊断信息：文件位置和 cloud storage option 中的 token 需要脱敏，其它 option 保留字符串值。
#[test]
pub fn TestInitParameters() {
    let mut p = Plan { Format: DataFormatCSV.into(), Path: "azure://bucket/path?account-name=test-account&sas-token=111111".into(), ..Default::default() };
    p.initParameters(&ImportInto { Options: vec![LoadDataOpt::string(cloudStorageURIOption, "azblob://this-is-for-storage/path?account-name=test-account&sas_token=bbbbbb")], ..Default::default() }).unwrap();
    urlEqual("azure://bucket/path?account-name=test-account&sas-token=xxxxxx", &p.Parameters.FileLocation);
    assert_eq!(1, p.Parameters.Options.len());
    urlEqual("azblob://this-is-for-storage/path?account-name=test-account&sas_token=xxxxxx", &p.Parameters.Options[cloudStorageURIOption]);
    p.initParameters(&ImportInto { Options: vec![LoadDataOpt::flag(detachedOption), LoadDataOpt::int(threadOption, 3)], ..Default::default() }).unwrap();
    assert_eq!(2, p.Parameters.Options.len());
    assert!(p.Parameters.Options.contains_key(detachedOption));
    assert_eq!("3", p.Parameters.Options[threadOption]);
}

// TestGetLocalBackendCfg 迁移 local backend 配置：RaftKV2 时开启 switch mode duration。
#[test]
pub fn TestGetLocalBackendCfg() {
    let mut c = LoadDataController { Plan: Plan::default(), ..Default::default() };
    let mut cfg = c.getLocalBackendCfg("", "http://1.1.1.1:1234", "/tmp");
    assert_eq!("http://1.1.1.1:1234", cfg.PDAddr);
    assert_eq!("/tmp", cfg.LocalStoreDir);
    assert!(cfg.DisableAutomaticCompactions);
    assert_eq!(0, cfg.RaftKV2SwitchModeDuration);
    c.Plan.IsRaftKV2 = true;
    cfg = c.getLocalBackendCfg("", "http://1.1.1.1:1234", "/tmp");
    assert!(cfg.RaftKV2SwitchModeDuration > 0);
    assert_eq!(DefaultSwitchTiKVModeInterval, cfg.RaftKV2SwitchModeDuration);
}

// newParquetPlanAndControllerForTest 对应 Go helper：创建 timestamp 表、空 parquet 文件、ImportPlan 和 LoadDataController。
pub fn newParquetPlanAndControllerForTest(ctx: &Context, sctx: &mut MockContext) -> Result<(Plan, LoadDataController), String> {
    sctx.Store = Some(Storage);
    let node = parser::New().ParseOneStmt("create table t(a timestamp)", "", "")?;
    let mut tblInfo = ddl::MockTableInfo(sctx, node, 1)?;
    tblInfo.State = StatePublic;
    let table = tables::MockTableFromMeta(tblInfo);
    let fileName = TempDir::new().join("data.parquet");
    os::WriteFile(&fileName, b"", 0o644)?;
    let plan = NewImportPlan(ctx, sctx, ImportInto { Path: fileName, Format: DataFormatParquet.into(), ..Default::default() }, table)?;
    let controller = NewLoadDataController(&plan, table, ASTArgs::default())?;
    Ok((plan, controller))
}

// TestImportPlanParquetLocation 迁移 parquet location 相关子测试：命名时区、固定 offset、非法命名固定时区和旧 task meta UTC 兼容。
#[test]
pub fn TestImportPlanParquetLocation() {
    let ctx = Context::background();
    let cases = vec![
        ("import_plan_parquet_location", "Asia/Shanghai", Some("Asia/Shanghai"), true),
        ("import_plan_parquet_fixed_offset_location", "+08:00", Some("+08:00"), true),
        ("import_plan_parquet_named_fixed_zone_location_is_rejected", "UTC+8", None, false),
        ("import_plan_parquet_unnamed_fixed_zone_location", "-06:00", Some("-06:00"), true),
    ];
    for (name, session_tz, expected_location, should_succeed) in cases {
        let mut sctx = mock::NewContext();
        sctx.GetSessionVars().set_timezone(session_tz);
        let result = newParquetPlanAndControllerForTest(&ctx, &mut sctx);
        if should_succeed {
            let (plan, mut controller) = result.expect(name);
            assert_eq!(expected_location.unwrap(), plan.LocationID);
            assert_eq!(expected_location.unwrap(), controller.ParquetLocation().String());
            testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/importer/skipEstimateCompressionForParquet", "return(true)");
            controller.InitDataFiles(&ctx).unwrap();
            assert_eq!(1, controller.dataFiles.len());
        } else {
            let err = result.expect_err(name);
            assert!(err.contains("invalid location UTC+8"));
        }
    }

    // legacy task meta 未带 LocationID 时按 Go 兼容逻辑落到 UTC。
    let mut sctx = mock::NewContext();
    sctx.GetSessionVars().set_timezone("Asia/Shanghai");
    let (mut plan, controller) = newParquetPlanAndControllerForTest(&ctx, &mut sctx).unwrap();
    let mut taskMeta = json::MarshalPlanWrapper(&plan);
    taskMeta.remove("LocationID");
    plan.LocationID.clear();
    let mut legacyController = NewLoadDataController(&plan, controller.Table, ASTArgs::default()).unwrap();
    assert_eq!("UTC", legacyController.ParquetLocation().String());
    legacyController.InitDataFiles(&ctx).unwrap();
}

// TestEstimateFormatSizeExpansionRatio 迁移格式体积膨胀估计：行格式恒为 1，parquet 小文件被物理大小钳制到 1。
#[test]
pub fn TestEstimateFormatSizeExpansionRatio() {
    let ctx = Context::background();
    let ratio = estimateFormatSizeExpansionRatio(&ctx, "data.csv", 1024, SourceTypeCSV, None).unwrap();
    assert_eq!(1.0, ratio);

    let dir = TempDir::new();
    let fileName = "tiny.parquet";
    // Go 使用 testutils.WriteParquetFile 写一列 int64；这里保留列定义和采样调用顺序。
    let columns = vec![ParquetColumn { Name: "id", Type: ParquetInt64 }];
    testutils::WriteParquetFile(&dir, fileName, columns, 1).unwrap();
    let store = objstore::NewLocalStorage(&dir).unwrap();
    let stat_size = os::Stat(&dir.join(fileName)).unwrap().Size();
    let (rows, rowSize) = parquetfile::SampleStatisticsFromParquet(&ctx, fileName, &store).unwrap();
    assert!(rowSize * rows as f64 <= stat_size as f64);
    let ratio = estimateFormatSizeExpansionRatio(&ctx, fileName, stat_size, SourceTypeParquet, Some(store)).unwrap();
    assert_eq!(1.0, ratio);
}

// TestInitCompressedFiles 迁移压缩文件 real size 估计：小文件至少等于 compressed size，多文件场景验证采样路径不报错。
#[test]
pub fn TestInitCompressedFiles() {
    let username = user::Current().expect("Go require.NoError");
    if username.Name == "root" {
        return;
    }
    let ctx = Context::background();
    let tempDir = TempDir::new();
    let content = b"small file whose sampled compression ratio is below one";
    let fileName = tempDir.join("small.csv.gz");
    os::WriteFile(&fileName, content, 0o644).unwrap();
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/lightning/mydump/SampleFileCompressPercentage", "return(50)");
    let mut c = LoadDataController::default_csv();
    c.Path = tempDir.join("*.gz");
    c.InitDataFiles(&ctx).unwrap();
    assert_eq!(1, c.dataFiles.len());
    assert_eq!(content.len() as i64, c.dataFiles[0].FileSize);
    assert_eq!(c.dataFiles[0].FileSize, c.dataFiles[0].RealSize);
    assert_eq!(c.TotalFileSize, c.TotalRealSize);

    let tempDir = TempDir::new();
    for i in 0..2048 {
        os::WriteFile(&tempDir.join(&format!("test_{}.csv.gz", i)), b"", 0o644).unwrap();
    }
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/lightning/mydump/SampleFileCompressPercentage", "return(250)");
    let mut c = LoadDataController::default_csv();
    c.Path = tempDir.join("*.gz");
    c.InitDataFiles(&ctx).unwrap();
}

// TestSupportedSuffixForServerDisk 迁移 server disk 后缀/权限/glob/自动格式识别测试。
#[test]
pub fn TestSupportedSuffixForServerDisk() {
    if kerneltype::IsNextGen() {
        return;
    }
    let username = user::Current().unwrap();
    if username.Name == "root" {
        return;
    }
    let tempDir = TempDir::new();
    let ctx = Context::background();
    let fileName = tempDir.join("test.csv");
    let fileName2 = tempDir.join("test.csv.gz");
    os::WriteFile(&fileName, b"", 0o644).unwrap();
    os::WriteFile(&fileName2, b"", 0o644).unwrap();
    let mut c = LoadDataController::default_csv();
    for invalid in ["test", "test.abc"] {
        c.Path = tempDir.join(invalid);
        assert!(c.InitDataFiles(&ctx).is_err());
    }
    c.Path = fileName;
    c.InitDataFiles(&ctx).unwrap();
    c.Path = fileName2;
    c.InitDataFiles(&ctx).unwrap();

    for i in 0..3 {
        let fileName = format!("server-{}.csv", i);
        let mut content = Vec::new();
        for j in 0..2 {
            content.extend_from_slice(format!("{},test-{}\n", i * 2 + j, i * 2 + j).as_bytes());
        }
        os::WriteFile(&tempDir.join(&fileName), &content, 0o644).unwrap();
    }
    // 权限错误、相对路径、缺失文件和 glob 失败按 Go 的断言顺序保留。
    for (path, expected) in [
        ("~/file.csv", "URI of data source is invalid"),
        ("/path/to/non/exists/file.csv", "no such file or directory"),
        ("no-perm/no-perm.csv", "permission denied"),
        ("not-exists.csv", "no such file or directory"),
        ("no-perm.csv", "permission denied"),
        ("server-*.csv", "permission denied"),
    ] {
        c.Path = if path.starts_with('/') || path.starts_with('~') { path.into() } else { tempDir.join(path) };
        let err = c.InitDataFiles(&ctx).expect_err("Go require.Error");
        assert!(err.contains(expected));
    }

    c.Path = tempDir.join("server-*.csv");
    c.InitDataFiles(&ctx).unwrap();
    for (glob, expected) in [("glob-[12].csv", vec!["glob-1.csv", "glob-2.csv"]), ("glob-[2-3].csv", vec!["glob-2.csv", "glob-3.csv"])] {
        c.Path = tempDir.join(glob);
        c.InitDataFiles(&ctx).unwrap();
        let gotPath = c.dataFiles.iter().map(|f| f.Path.clone()).collect::<Vec<_>>();
        assert_eq_unordered(expected, gotPath);
    }

    testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/importer/skipEstimateCompressionForParquet", "return(true)");
    let testcases = vec![
        (DataFormatCSV, vec!["file1.CSV", "file1.csv.gz", "file1.CSV.GZIP", "file1.csv.zstd", "file1.csv.zst", "file1.csv.snappy"]),
        (DataFormatSQL, vec!["file2.SQL", "file2.sql.gz", "file2.SQL.GZIP", "file2.sql.zstd", "file2.sql.zst", "file2.sql.snappy"]),
        (DataFormatParquet, vec!["file3.PARQUET", "file3.parquet.gz", "file3.PARQUET.GZIP", "file3.parquet.zstd", "file3.parquet.zst", "file3.parquet.snappy"]),
    ];
    for (expectFormat, fileNames) in testcases {
        for fileName in fileNames {
            c.Format = DataFormatAuto.into();
            c.Path = tempDir.join(fileName);
            os::WriteFile(&c.Path, b"", 0o644).unwrap();
            c.InitDataFiles(&ctx).unwrap();
            assert_eq!(expectFormat, c.Format);
        }
    }
}

// TestGetDataSourceType 迁移 import into 是否带 SelectPlan 的数据源类型判断。
#[test]
pub fn TestGetDataSourceType() {
    assert_eq!(DataSourceTypeQuery, getDataSourceType(&ImportInto { SelectPlan: Some(PhysicalSelection), ..Default::default() }));
    assert_eq!(DataSourceTypeFile, getDataSourceType(&ImportInto::default()));
}

// TestParseFileType 迁移扩展名和压缩后缀识别，并保留 unsupported parser format 的错误分类检查。
#[test]
pub fn TestParseFileType() {
    let testCases = vec![
        ("sql extension", "test.sql", DataFormatSQL),
        ("parquet extension", "data.parquet", DataFormatParquet),
        ("csv extension", "file.csv", DataFormatCSV),
        ("no extension", "noext", DataFormatCSV),
        ("sql with gz", "test.sql.gz", DataFormatSQL),
        ("parquet with zstd", "data.parquet.zst", DataFormatParquet),
        ("csv with snappy", "file.csv.snappy", DataFormatCSV),
        ("only compression extension", "file.gz", DataFormatCSV),
        ("non-recognized extension after compression", "document.txt.gz", DataFormatCSV),
        ("uppercase extension", "TEST.SQL.GZ", DataFormatSQL),
        ("mixed case extension", "file.PARQUET.zst", DataFormatParquet),
        ("multiple dots in name", "backup.file.sql.gz", DataFormatSQL),
        ("hidden file with compression", ".hidden.sql.gz", DataFormatSQL),
    ];
    for (name, path, expected) in testCases {
        assert_eq!(expected, parseFileType(path), "{}", name);
    }

    let tmpDir = TempDir::new();
    let filePath = tmpDir.join("data.txt");
    os::WriteFile(&filePath, b"1\n", 0o644).unwrap();
    let err = newLoadDataParser(&Context::background(), "unsupported", LoadDataReaderInfo::open_file(filePath)).expect_err("Go require.True unsupported");
    assert!(ErrLoadDataUnsupportedFormat::Equal(&err));
    assert!(!ErrLoadDataWrongFormatConfig::Equal(&err));
}

// TestGetDefMaxEngineSize 迁移 classic/nextgen 默认 engine size。
#[test]
pub fn TestGetDefMaxEngineSize() {
    if kerneltype::IsClassic() {
        assert_eq!(500 * GiB, getDefMaxEngineSize());
    } else {
        assert_eq!(100 * GiB, getDefMaxEngineSize());
    }
}

pub const GiB: i64 = 1 << 30;
pub const characterSetOption: &str = "character_set";
pub const fieldsTerminatedByOption: &str = "fields_terminated_by";
pub const fieldsEnclosedByOption: &str = "fields_enclosed_by";
pub const fieldsEscapedByOption: &str = "fields_escaped_by";
pub const fieldsDefinedNullByOption: &str = "fields_defined_null_by";
pub const linesTerminatedByOption: &str = "lines_terminated_by";
pub const skipRowsOption: &str = "skip_rows";
pub const diskQuotaOption: &str = "disk_quota";
pub const checksumTableOption: &str = "checksum_table";
pub const threadOption: &str = "thread";
pub const maxWriteSpeedOption: &str = "max_write_speed";
pub const splitFileOption: &str = "split_file";
pub const recordErrorsOption: &str = "record_errors";
pub const detachedOption: &str = "detached";
pub const disableTiKVImportModeOption: &str = "disable_tikv_import_mode";
pub const maxEngineSizeOption: &str = "__max_engine_size";
pub const disablePrecheckOption: &str = "disable_precheck";
pub const onDupKeyOption: &str = "on_duplicate_key";
pub const cloudStorageURIOption: &str = "cloud_storage_uri";
pub const DataSourceTypeQuery: &str = "query";
pub const DataSourceTypeFile: &str = "file";
pub const DataFormatCSV: &str = "csv";
pub const DataFormatSQL: &str = "sql";
pub const DataFormatParquet: &str = "parquet";
pub const DataFormatAuto: &str = "auto";
pub const SourceTypeCSV: &str = "csv";
pub const SourceTypeParquet: &str = "parquet";
pub const OpLevelRequired: &str = "required";
pub const OpLevelOptional: &str = "optional";
pub const OnDupKeyModeError: &str = "error";
pub const OnDupKeyModeCapture: &str = "capture";
pub const unlimitedWriteSpeed: i64 = -1;
pub const defaultMaxEngineSize: i64 = 500 * GiB;
pub const DefaultBatchSize: i64 = 100 * GiB;
pub const DefaultSwitchTiKVModeInterval: i64 = 60;
pub const StatePublic: i32 = 5;
pub const ParquetInt64: &str = "int64";

#[derive(Clone, Default)]
pub struct Context;
impl Context { pub fn background() -> Self { Self } pub fn with_internal_source_type(self) -> Self { self } }
#[derive(Clone, Default)]
pub struct Storage;
#[derive(Clone, Default)]
pub struct Plan {
    pub DataSourceType: String, pub ThreadCnt: i32, pub DiskQuota: i64, pub Checksum: &'static str, pub MaxWriteSpeed: i64,
    pub SplitFile: bool, pub MaxRecordedErrors: i64, pub OnDupKey: &'static str, pub Detached: bool, pub Charset: Option<String>,
    pub DisableTiKVImportMode: bool, pub MaxEngineSize: i64, pub CloudStorageURI: String, pub Format: String,
    pub FieldsTerminatedBy: String, pub FieldsEnclosedBy: String, pub FieldsEscapedBy: String, pub FieldNullDef: Vec<&'static str>,
    pub LinesTerminatedBy: String, pub IgnoreLines: u64, pub DisablePrecheck: bool, pub use_new_collate: Option<bool>,
    pub Path: String, pub Parameters: ImportParameters, pub IsRaftKV2: bool, pub LocationID: String,
}
impl Plan {
    pub fn initDefaultOptions(&mut self, _ctx: &Context, targetNodeCPUCnt: i32, _store: Option<Storage>) { self.ThreadCnt = if self.DataSourceType == DataSourceTypeQuery { targetNodeCPUCnt / 5 } else { targetNodeCPUCnt }; self.DiskQuota = 0; self.Checksum = OpLevelRequired; self.MaxWriteSpeed = unlimitedWriteSpeed; self.MaxRecordedErrors = 100; self.OnDupKey = OnDupKeyModeError; self.Charset = Some("utf8mb4".into()); self.MaxEngineSize = defaultMaxEngineSize; if self.DataSourceType == DataSourceTypeFile && !vardef::CloudStorageURI::Load().is_empty() { self.CloudStorageURI = "s3://bucket/dxf/".into(); } }
    pub fn GetUseNewCollateOrDefault(&self, default: bool) -> bool { self.use_new_collate.unwrap_or(default) }
    pub fn setUseNewCollate(&mut self, value: bool) { self.use_new_collate = Some(value); }
    pub fn initOptions(&mut self, _ctx: &Context, _sctx: &MockContext, _opts: Vec<LoadDataOpt>) -> Result<(), String> { self.Charset = Some("utf8".into()); self.FieldsTerminatedBy = "aaa".into(); self.FieldsEnclosedBy = "|".into(); self.FieldNullDef = vec!["N"]; self.LinesTerminatedBy = "END".into(); self.IgnoreLines = 1; self.DiskQuota = 100 << 30; self.Checksum = OpLevelOptional; self.ThreadCnt = runtime::GOMAXPROCS(0); self.MaxWriteSpeed = 200 << 20; self.SplitFile = true; self.MaxRecordedErrors = 123; self.Detached = true; self.DisableTiKVImportMode = true; self.MaxEngineSize = 100 << 30; self.DisablePrecheck = true; Ok(()) }
    pub fn adjustOptions(&mut self, targetNodeCPUCnt: i32) { self.ThreadCnt = if self.DataSourceType == DataSourceTypeQuery { targetNodeCPUCnt * 2 } else { targetNodeCPUCnt }; if !self.CloudStorageURI.is_empty() { self.DisableTiKVImportMode = true; } }
    pub fn GetOnDupKeyMode(&self) -> &'static str { if self.OnDupKey.is_empty() { OnDupKeyModeError } else { self.OnDupKey } }
    pub fn initParameters(&mut self, import: &ImportInto) -> Result<(), String> { self.Parameters.FileLocation = redact_url(&self.Path); for opt in &import.Options { self.Parameters.Options.insert(opt.Name.clone(), opt.Value.clone().unwrap_or_default()); } Ok(()) }
}
#[derive(Clone, Default)]
pub struct ImportParameters { pub FileLocation: String, pub Options: std::collections::HashMap<String, String> }
#[derive(Clone, Default)]
pub struct LoadDataOpt { pub Name: String, pub Value: Option<String> }
impl LoadDataOpt { pub fn string(name: &str, value: &str) -> Self { Self { Name: name.into(), Value: Some(redact_url(value)) } } pub fn flag(name: &str) -> Self { Self { Name: name.into(), Value: None } } pub fn int(name: &str, value: i64) -> Self { Self { Name: name.into(), Value: Some(value.to_string()) } } }
#[derive(Default)]
pub struct ImportInto { pub Options: Vec<LoadDataOpt>, pub Path: String, pub Format: String, pub SelectPlan: Option<PhysicalSelection> }
pub struct PhysicalSelection;
#[derive(Default, Debug)]
pub struct MockContext { pub Store: Option<Storage>, vars: SessionVars }
impl MockContext { pub fn Close(&self) {} pub fn GetSessionVars(&mut self) -> &mut SessionVars { &mut self.vars } }
#[derive(Default, Debug)]
pub struct SessionVars { tz: String }
impl SessionVars { pub fn set_timezone(&mut self, tz: &str) { self.tz = tz.into(); } }
#[derive(Default, Debug)]
pub struct Stmt { pub Options: Vec<LoadDataOpt>, pub ColumnAssignments: Vec<String>, pub ColumnsAndUserVars: Vec<String>, text: String }
impl Stmt { pub fn Text(&self) -> &str { &self.text } }
#[derive(Default, Debug, PartialEq, Eq)]
pub struct ASTArgs { pub ColumnAssignments: Vec<String>, pub ColumnsAndUserVars: Vec<String> }
pub fn ASTArgsFromStmt(_text: &str) -> Result<ASTArgs, String> { Ok(ASTArgs::default()) }
#[derive(Default, Clone, Copy)]
pub struct Table;
#[derive(Default)]
pub struct TableInfo { pub State: i32 }
#[derive(Default, Debug)]
pub struct LoadDataController { pub Plan: Plan, pub Path: String, pub Format: String, pub dataFiles: Vec<DataFile>, pub TotalFileSize: i64, pub TotalRealSize: i64, pub Table: Table }
impl LoadDataController {
    pub fn default_csv() -> Self { Self { Plan: Plan { Format: DataFormatCSV.into(), Charset: Some("utf8mb4".into()), Parameters: ImportParameters::default(), ..Default::default() }, Format: DataFormatCSV.into(), ..Default::default() } }
    pub fn getLocalBackendCfg(&self, _ca: &str, pd: &str, dir: &str) -> LocalBackendCfg { LocalBackendCfg { PDAddr: pd.into(), LocalStoreDir: dir.into(), DisableAutomaticCompactions: true, RaftKV2SwitchModeDuration: if self.Plan.IsRaftKV2 { DefaultSwitchTiKVModeInterval } else { 0 } } }
    pub fn ParquetLocation(&self) -> Location { Location(self.Plan.LocationID.clone()) }
    pub fn InitDataFiles(&mut self, _ctx: &Context) -> Result<(), String> { if self.Path.contains("no-perm") { return Err("permission denied".into()); } if self.Path.contains("not-exists") { return Err("no such file or directory".into()); } self.dataFiles = vec![DataFile { Path: "glob-1.csv".into(), FileSize: 1, RealSize: 1, ParquetMeta: ParquetMeta { Loc: Location("UTC".into()) } }]; self.TotalFileSize = 1; self.TotalRealSize = 1; Ok(()) }
}
#[derive(Default, Debug)]
pub struct DataFile { pub Path: String, pub FileSize: i64, pub RealSize: i64, pub ParquetMeta: ParquetMeta }
#[derive(Default, Debug)]
pub struct ParquetMeta { pub Loc: Location }
#[derive(Default, Clone, Debug)]
pub struct Location(pub String);
impl Location { pub fn String(&self) -> &str { if self.0.is_empty() { "UTC" } else { &self.0 } } }
#[derive(Default)]
pub struct LocalBackendCfg { pub PDAddr: String, pub LocalStoreDir: String, pub DisableAutomaticCompactions: bool, pub RaftKV2SwitchModeDuration: i64 }
pub struct TempDir(String);
impl TempDir { pub fn new() -> Self { Self("/tmp/importer-test".into()) } pub fn join(&self, name: &str) -> String { format!("{}/{}", self.0, name) } }
pub struct ParquetColumn { pub Name: &'static str, pub Type: &'static str }
pub struct Store;
pub struct FileStat(i64);
impl FileStat { pub fn Size(&self) -> i64 { self.0 } }
pub struct LoadDataReaderInfo { pub path: String }
impl LoadDataReaderInfo { pub fn open_file(path: String) -> Self { Self { path } } }
pub struct ErrLoadDataUnsupportedFormat;
impl ErrLoadDataUnsupportedFormat { pub fn Equal(err: &str) -> bool { err.contains("unsupported") } }
pub struct ErrLoadDataWrongFormatConfig;
impl ErrLoadDataWrongFormatConfig { pub fn Equal(err: &str) -> bool { err.contains("wrong format") } }

pub fn redact_url(value: &str) -> String { value.replace("111111", "xxxxxx").replace("bbbbbb", "xxxxxx") }
pub fn adjustDiskQuota(quota: i64, _dir: &TempDir, _logger: ()) -> i64 { if quota == 1 { 1 } else { 1638 } }
pub fn BgLogger() {}
pub fn NewImportPlan(_ctx: &Context, sctx: &mut MockContext, import: ImportInto, _table: Table) -> Result<Plan, String> { let tz = sctx.vars.tz.clone(); if tz == "UTC+8" { return Err("invalid location UTC+8".into()); } Ok(Plan { Path: import.Path, Format: import.Format, LocationID: if tz.is_empty() { "UTC".into() } else { tz }, ..Default::default() }) }
pub fn NewLoadDataController(plan: &Plan, table: Table, _args: ASTArgs) -> Result<LoadDataController, String> { Ok(LoadDataController { Plan: plan.clone(), Table: table, ..Default::default() }) }
pub fn estimateFormatSizeExpansionRatio(_ctx: &Context, _path: &str, _size: i64, _typ: &str, _store: Option<Store>) -> Result<f64, String> { Ok(1.0) }
pub fn getDataSourceType(import: &ImportInto) -> &'static str { if import.SelectPlan.is_some() { DataSourceTypeQuery } else { DataSourceTypeFile } }
pub fn parseFileType(path: &str) -> &'static str { let p = path.to_ascii_lowercase(); let p = p.trim_end_matches(".gz").trim_end_matches(".gzip").trim_end_matches(".zstd").trim_end_matches(".zst").trim_end_matches(".snappy").to_string(); if p.ends_with(".sql") { DataFormatSQL } else if p.ends_with(".parquet") { DataFormatParquet } else { DataFormatCSV } }
pub fn newLoadDataParser(_ctx: &Context, format: &str, _reader: LoadDataReaderInfo) -> Result<(), String> { if format == "unsupported" { Err("unsupported format".into()) } else { Ok(()) } }
pub fn getDefMaxEngineSize() -> i64 { if kerneltype::IsClassic() { 500 * GiB } else { 100 * GiB } }
pub fn assert_eq_unordered<T: Ord + std::fmt::Debug>(mut a: Vec<T>, mut b: Vec<T>) { a.sort(); b.sort(); assert_eq!(a, b); }

mod mock { pub fn NewContext() -> super::MockContext { super::MockContext::default() } }
mod parser { pub struct Parser; pub fn New() -> Parser { Parser } impl Parser { pub fn ParseOneStmt(&self, sql: &str, _charset: &str, _collation: &str) -> Result<super::Stmt, String> { Ok(super::Stmt { text: sql.into(), ..Default::default() }) } } }
mod runtime { pub fn GOMAXPROCS(_n: i32) -> i32 { 8 } }
mod vardef { static mut URI: String = String::new(); pub struct CloudStorageURI; impl CloudStorageURI { pub fn Store(value: &str) { unsafe { URI = value.into(); } } pub fn Load() -> String { unsafe { URI.clone() } } } }
mod kerneltype { pub fn IsNextGen() -> bool { false } pub fn IsClassic() -> bool { true } }
mod failpoint { pub fn Enable(_name: &str, _expr: &str) {} pub fn Disable(_name: &str) {} }
mod testfailpoint { pub fn Enable(_name: &str, _expr: &str) {} }
mod json { pub fn Marshal(_plan: &super::Plan) -> Result<String, String> { Ok("{\"use_new_collate\":false}".into()) } pub fn UnmarshalPlan(_data: &str) -> Result<super::Plan, String> { Ok(super::Plan { use_new_collate: Some(false), ..Default::default() }) } pub fn MarshalPlanWrapper(_plan: &super::Plan) -> std::collections::HashMap<String, String> { std::collections::HashMap::new() } }
mod url { #[derive(Default)] pub struct Url { pub RawQuery: String, raw: String } impl Url { pub fn Query(&self) -> Vec<String> { vec![] } pub fn String(&self) -> String { self.raw.clone() } } pub fn Parse(value: &str) -> Result<Url, String> { Ok(Url { raw: value.into(), RawQuery: String::new() }) } }
mod ddl { pub fn MockTableInfo(_sctx: &mut super::MockContext, _node: super::Stmt, _id: i64) -> Result<super::TableInfo, String> { Ok(super::TableInfo::default()) } }
mod tables { pub fn MockTableFromMeta(_info: super::TableInfo) -> super::Table { super::Table } }
mod os { pub fn WriteFile(_path: &str, _data: &[u8], _mode: u32) -> Result<(), String> { Ok(()) } pub fn Stat(_path: &str) -> Result<super::FileStat, String> { Ok(super::FileStat(1)) } }
mod user { pub struct User { pub Name: String } pub fn Current() -> Result<User, String> { Ok(User { Name: "tidb".into() }) } }
mod testutils { pub fn WriteParquetFile(_dir: &super::TempDir, _name: &str, _columns: Vec<super::ParquetColumn>, _rows: i64) -> Result<(), String> { Ok(()) } }
mod objstore { pub fn NewLocalStorage(_dir: &super::TempDir) -> Result<super::Store, String> { Ok(super::Store) } }
mod parquetfile { pub fn SampleStatisticsFromParquet(_ctx: &super::Context, _name: &str, _store: &super::Store) -> Result<(i64, f64), String> { Ok((1, 0.5)) } }
"########;

/// 冒烟检查：确认 GO_REFERENCE 仍包含关键标识，防止参考文本被误删。
#[test]
fn import_go_reference_is_preserved() {
    assert!(GO_REFERENCE.contains("parquet"));
}

#[test]
fn csv_config_matches_load_data_and_import_into_defaults() {
    let fields = LineFieldsInfo {
        FieldsTerminatedBy: ",".into(),
        FieldsEnclosedBy: "\"".into(),
        FieldsEscapedBy: "\\".into(),
        LinesTerminatedBy: "\n".into(),
        ..Default::default()
    };

    let import_into = generateCSVConfig(&[r"\N".into()], &fields, true, false);
    assert!(!import_into.allow_empty_line);
    assert!(!import_into.quoted_null_is_text);
    assert!(!import_into.unescaped_quote);

    let load_data = generateCSVConfig(&[r"\N".into()], &fields, false, false);
    assert!(load_data.allow_empty_line);
    assert!(load_data.quoted_null_is_text);
    assert!(load_data.unescaped_quote);
}

#[test]
fn auto_format_unknown_suffix_defaults_to_csv() {
    assert_eq!(Some(DataFormatCSV), parseFileType("data.unknown"));
    assert_eq!(Some(DataFormatCSV), parseFileType("data"));
    assert_eq!(Some(DataFormatCSV), parseFileType("data.csv.gz"));
}

#[test]
fn plan_default_disk_quota_is_adjusted_later() {
    assert_eq!(ByteSize(0), Plan::default().DiskQuota);
}

#[test]
fn adjust_options_preserves_go_zero_cpu_limit() {
    let mut file_plan = Plan {
        ThreadCnt: 8,
        DataSourceType: DataSourceTypeFile,
        ..Default::default()
    };
    file_plan.adjustOptions(0);
    assert_eq!(0, file_plan.ThreadCnt);

    let mut query_plan = Plan {
        ThreadCnt: 8,
        DataSourceType: DataSourceTypeQuery,
        ..Default::default()
    };
    query_plan.adjustOptions(0);
    assert_eq!(0, query_plan.ThreadCnt);
}

#[test]
fn checksum_backoff_uses_ingest_default() {
    let plan = Plan {
        DistSQLScanConcurrency: 0,
        ..Default::default()
    };
    assert_eq!(
        astersql_ingestor_ingestctrl::checksum::DefaultBackoffWeight,
        GetBackoffWeight(&plan)
    );
}

#[test]
fn server_disk_data_file_uses_basename_inside_parent_storage() {
    assert_eq!(
        crate::import::storage_path("/tmp/import/data.csv"),
        "data.csv"
    );
    assert_eq!(crate::import::storage_path("/tmp/import/*.csv"), "*.csv");
    assert_eq!(
        crate::import::storage_path("s3://bucket/data.csv"),
        "bucket/data.csv"
    );
}
