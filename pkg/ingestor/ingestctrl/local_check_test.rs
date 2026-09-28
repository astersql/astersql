// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Local backend 前置检查相关单元测试。
//
// 覆盖 TiFlash 副本与 TiDB 版本兼容性检查（5.0 前 local backend 不支持冲突表），
// 以及从多 Store 配置推导统一 Region 分裂大小/键数阈值的逻辑。

// 主要类型、函数、子用例、断言、资源收尾、并发/channel、failpoint、IO 和 mock 语义均在对应位置补充中文说明，方便人工继续迁移。

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables)]

use crate::local::{CheckTiFlashVersionForTables, GetRegionSplitSizeKeys, Version};

#[test]
// TestCheckRequirementsTiFlash 对应 Go 函数/方法声明。
// Go: func TestCheckRequirementsTiFlash(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
/// 验证旧版本下源表与 TiFlash 副本冲突检测。
pub fn test_check_requirements_ti_flash() {
    let source = [
        ("test", "t1"),
        ("test", "tbl"),
        ("test1", "t"),
        ("test1", "tbl"),
    ];
    let tiflash = [("db", "tbl"), ("test", "t1"), ("test1", "tbl")];
    let error = CheckTiFlashVersionForTables(Version::new(4, 0, 2), &source, &tiflash).unwrap_err();
    assert_eq!(
        "lightning local backend doesn't support TiFlash in this TiDB version. conflict tables: [`test`.`t1`, `test1`.`tbl`]. Please add TiFlash replica after load data.",
        error.to_string()
    );
    assert!(CheckTiFlashVersionForTables(Version::new(4, 0, 5), &source, &tiflash).is_ok());
    assert!(CheckTiFlashVersionForTables(Version::new(4, 0, 2), &source, &[("db", "tbl")]).is_ok());
    // mock: db, mock, err := sqlmock.New()
    // 资源收尾: t.Cleanup(func() {
    // 断言: require.NoError(t, db.Close())
    // 流程: })
    // 断言: require.NoError(t, err)
    // context: ctx := context.Background()
    // 流程: dbMetas := []*mydump.MDDatabaseMeta{
    // 流程: {
    // 流程: Name: "test",
    // 流程: Tables: []*mydump.MDTableMeta{
    // 流程: {
    // 流程: DB: "test",
    // 流程: Name: "t1",
    // 流程: DataFiles: []mydump.FileInfo{{}},
    // 流程: },
    // 流程: {
    // 流程: DB: "test",
    // 流程: Name: "tbl",
    // 流程: DataFiles: []mydump.FileInfo{{}},
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: Name: "test1",
    // 流程: Tables: []*mydump.MDTableMeta{
    // 流程: {
    // 流程: DB: "test1",
    // 流程: Name: "t",
    // 流程: DataFiles: []mydump.FileInfo{{}},
    // 流程: },
    // 流程: {
    // 流程: DB: "test1",
    // 流程: Name: "tbl",
    // 流程: DataFiles: []mydump.FileInfo{{}},
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: }
    // 流程: checkCtx := &backend.CheckCtx{DBMetas: dbMetas}
    // mock: mock.ExpectQuery(ingestctrl.TiFlashReplicaQueryForTest).WillReturnRows(sqlmock.NewRows([]string{"db", "tbl"}).
    // 流程: AddRow("db", "tbl").
    // 流程: AddRow("test", "t1").
    // 流程: AddRow("test1", "tbl"))
    // mock: mock.ExpectClose()
    // 流程: err = ingestctrl.CheckTiFlashVersionForTest(ctx, db, checkCtx, *semver.New("4.0.2"))
    // 断言: require.Regexp(t, "^lightning local backend doesn't support TiFlash in this TiDB version. conflict tables: \\[`test`.`t1`, `test1`.`tbl`\\]", err.Error())
}

#[test]
// TestGetRegionSplitSizeKeys 对应 Go 函数/方法声明。
// Go: func TestGetRegionSplitSizeKeys(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
/// 验证前两个 Store 查询失败后采用第三个 Store 的分裂配置。
pub fn test_get_region_split_size_keys() {
    assert_eq!(
        (1, 2),
        GetRegionSplitSizeKeys(&[(0, 0), (0, 0), (1, 2)], 96, 960)
    );
    // PD/TiKV region: allStores := []*metapb.Store{
    // 流程: {
    // 流程: Address: "172.16.102.1:20160",
    // 流程: StatusAddress: "0.0.0.0:20180",
    // 流程: },
    // 流程: {
    // 流程: Address: "172.16.102.2:20160",
    // 流程: StatusAddress: "0.0.0.0:20180",
    // 流程: },
    // 流程: {
    // 流程: Address: "172.16.102.3:20160",
    // 流程: StatusAddress: "0.0.0.0:20180",
    // 流程: },
    // 流程: }
    // context: ctx, cancel := context.WithCancel(context.Background())
    // 资源收尾: defer cancel()
    // PD/TiKV region: cli := split.NewFakePDClient(allStores, false, nil)
    // 资源收尾: defer func() {
    // PD/TiKV region: ingestctrl.SetGetSplitConfFromStoreFunc(ingestctrl.GetSplitConfFromStore)
    // 流程: }()
    // context: ingestctrl.SetGetSplitConfFromStoreFunc(func(ctx context.Context, host string, tls *common.TLS) (int64, int64, error) {
    // 控制流: if strings.Contains(host, "172.16.102.3:20180") {
    // 返回语义: return int64(1), int64(2), nil
    // 流程: }
    // 返回语义: return 0, 0, errors.New("invalid connection")
    // 流程: })
    // PD/TiKV region: splitSize, splitKeys, err := ingestctrl.GetRegionSplitSizeKeys(ctx, cli, nil)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, int64(1), splitSize)
    // 断言: require.Equal(t, int64(2), splitKeys)
}
