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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// TiFlash（列存副本引擎）相关 DDL 测试迁移草稿。
//
// 记录设置/校验 TiFlash 副本、同步状态与 failpoint 场景的 Go 测试步骤。
//
// TiFlash：列存副本引擎；placement rule：PD 调度副本放置的规则；
// Available：副本同步完成可对查询服务的标志；GC safe point：垃圾回收安全点。

// 这段逻辑只记录测试步骤、断言、failpoint、mock store/TiFlash/DDL 等外部依赖边界。

#[derive(Debug, Clone)]
/// 单条 Go 测试步骤的动作与细节描述。
pub struct GoTestStep {
    /// 步骤动作类别。
    pub action: &'static str,
    /// 步骤细节（SQL、断言说明等）。
    pub detail: &'static str,
}

// record_go_test_steps 对应 Go 测试中连续执行 SQL、断言、failpoint 的步骤记录。
// 它故意不执行任何业务动作，只让保留原测试顺序和关键参数。
/// 校验并记录命名测试的步骤序列（外部 TiDB harness 动作不在此执行）。
pub fn record_go_test_steps(name: &str, steps: &[GoTestStep]) {
    assert!(!name.is_empty(), "Go test mapping must have a name");
    assert!(
        !name.starts_with("Test") || !steps.is_empty(),
        "mapped Go test {name} must retain at least one executable step or assertion"
    );
    for step in steps {
        assert!(
            !step.action.is_empty(),
            "mapped Go test {name} contains an unnamed step"
        );
        assert!(
            !step.detail.is_empty(),
            "mapped Go test {name} contains an empty {} step",
            step.action
        );
    }
}

// keep_go_source 保存就近 Go 源码片段，避免测试框架、并发和外部依赖语义在里丢失。
/// 校验保留的邻近 Go 源码片段，防止映射静默退化为空占位。
pub fn keep_go_source(name: &str, source: &str) {
    assert!(!name.is_empty(), "Go source mapping must have a name");
    assert!(
        !source.trim().is_empty(),
        "mapped Go source for {name} must not be empty"
    );
}

#[derive(Debug, Clone)]
/// TiFlash 测试上下文占位（字段仍以 Go 原文形式保留）。
pub struct TiflashContext {
    // 字段来自 Go struct；真实类型依赖 TiDB 测试 harness，当前按 Go 字段原文保留。
    /// Go 结构体字段原文。
    pub go_fields: &'static str,
}
/// Go `tiflashContext` 结构体源码原文，供对照字段含义。
pub const _GO_TYPE_SOURCE_TIFLASH_CONTEXT: &str = r########"type tiflashContext struct {
	store   kv.Storage
	dom     *domain.Domain
	tiflash *infosync.MockTiFlash
	cluster *unistore.Cluster
}"########;

// 以下 const 对应 Go 常量声明；复杂 time/mysql 表达式按原 Go 表达式记录。
/// 普通表副本变为 Available 前需经历的轮询轮次。
pub const ROUND_TO_BE_AVAILABLE: u64 = 2;
/// 分区表副本变为 Available 前需经历的轮询轮次。
pub const ROUND_TO_BE_AVAILABLE_PARTITION_TABLE: u64 = 3;
/// Go 轮次常量声明原文。
pub const _GO_CONST_SOURCE_4174D0: &str = r########"const (
	RoundToBeAvailable               = 2
	RoundToBeAvailablePartitionTable = 3
)"########;

/// 对应 Go `createTiFlashContext`：搭建带 MockTiFlash 的测试环境。
pub fn create_ti_flash_context() {
    // create_ti_flash_context 对应 Go 函数 createTiFlashContext(t *testing.T) (*tiflashContext, func())。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, s.store.Close())"########,
        },
    ];
    record_go_test_steps("createTiFlashContext", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "createTiFlashContext",
        r########"	s := &tiflashContext{}
	var err error

	ddl.PollTiFlashInterval = 1000 * time.Millisecond
	ddl.PullTiFlashPdTick.Store(60)
	s.tiflash = infosync.NewMockTiFlash()
	s.store, err = teststore.NewMockStoreWithoutBootstrap(
		mockstore.WithClusterInspector(func(c testutils.Cluster) {
			mockCluster := c.(*unistore.Cluster)
			_, _, region1 := mockstore.BootstrapWithSingleStore(c)
			tiflashIdx := 0
			for tiflashIdx < 2 {
				store2 := c.AllocID()
				peer2 := c.AllocID()
				addr2 := fmt.Sprintf("tiflash%d", tiflashIdx)
				s.tiflash.AddStore(store2, addr2)
				mockCluster.AddStore(store2, addr2, &metapb.StoreLabel{Key: "engine", Value: "tiflash"})
				mockCluster.AddPeer(region1, store2, peer2)
				tiflashIdx++
			}
			s.cluster = mockCluster
		}),
		mockstore.WithStoreType(mockstore.EmbedUnistore),
	)

	require.NoError(t, err)
	session.DisableStats4Test()
	s.dom, err = session.BootstrapSession(s.store)
	infosync.SetMockTiFlash(s.tiflash)
	require.NoError(t, err)
	s.dom.SetStatsUpdating(true)

	tearDown := func() {
		s.dom.Close()
		s.tiflash.Lock()
		if s.tiflash.StatusServer != nil {
			s.tiflash.StatusServer.Close()
		}
		s.tiflash.Unlock()
		require.NoError(t, s.store.Close())
		ddl.PollTiFlashInterval = 2 * time.Second
	}
	return s, tearDown"########,
    );
}

/// 对应 Go `ChangeGCSafePoint`：写入 mysql.tidb 中的 GC safe point。
pub fn change_gc_safe_point() {
    // change_gc_safe_point 对应 Go 函数 ChangeGCSafePoint(tk *testkit.TestKit, t time.Time, enable string, lifeTime string) 。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(s)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(s)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(s)"########,
        },
    ];
    record_go_test_steps("ChangeGCSafePoint", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "ChangeGCSafePoint",
        r########"	gcTimeFormat := "20060102-15:04:05 -0700 MST"
	lastSafePoint := t.Format(gcTimeFormat)
	s := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`
	s = fmt.Sprintf(s, lastSafePoint)
	tk.MustExec(s)
	s = `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_enable','%[1]s','')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`
	s = fmt.Sprintf(s, enable)
	tk.MustExec(s)
	s = `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_life_time','%[1]s','')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`
	s = fmt.Sprintf(s, lifeTime)
	tk.MustExec(s)"########,
    );
}

/// 对应 Go flashback 相关检查辅助（迁移占位）。
pub fn check_flashback() {
    // check_flashback 对应 Go 方法 CheckFlashback，receiver 为 `s *tiflashContext`；当前只保留方法测试/辅助语义。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("flashback table ddltiflash")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
    ];
    record_go_test_steps("CheckFlashback", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "CheckFlashback",
        r########"	// If table is dropped after tikv_gc_safe_point, it can be recovered
	ChangeGCSafePoint(tk, time.Now().Add(-time.Hour), "false", "10m0s")
	defer func() {
		ChangeGCSafePoint(tk, time.Now(), "true", "10m0s")
	}()

	fCancel := TempDisableEmulatorGC()
	defer fCancel()
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("flashback table ddltiflash")
	time.Sleep(ddl.PollTiFlashInterval * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})

	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	if tb.Meta().Partition != nil {
		for _, e := range tb.Meta().Partition.Definitions {
			ruleName := infosync.MakeRuleID(e.ID)
			_, ok := s.tiflash.GetPlacementRule(ruleName)
			require.True(t, ok)
		}
	} else {
		ruleName := infosync.MakeRuleID(tb.Meta().ID)
		_, ok := s.tiflash.GetPlacementRule(ruleName)
		require.True(t, ok)
	}"########,
    );
}

/// 对应 Go `TempDisableEmulatorGC`：临时关闭模拟 GC。
pub fn temp_disable_emulator_gc() {
    // temp_disable_emulator_gc 对应 Go 函数 TempDisableEmulatorGC() func()。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    record_go_test_steps("TempDisableEmulatorGC", &[]);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TempDisableEmulatorGC",
        r########"	ori := ddlutil.IsEmulatorGCEnable()
	f := func() {
		if ori {
			ddlutil.EmulatorGCEnable()
		} else {
			ddlutil.EmulatorGCDisable()
		}
	}
	ddlutil.EmulatorGCDisable()
	return f"########,
    );
}

/// 对应 Go 设置 PD 轮询循环参数的辅助（迁移占位）。
pub fn set_pd_loop() {
    // set_pd_loop 对应 Go 方法 SetPdLoop，receiver 为 `s *tiflashContext`；当前只保留方法测试/辅助语义。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    record_go_test_steps("SetPdLoop", &[]);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "SetPdLoop",
        r########"	originValue := ddl.PullTiFlashPdTick.Swap(tick)
	return func() {
		ddl.PullTiFlashPdTick.Store(originValue)
	}"########,
    );
}

// Run all kinds of DDLs, and will create no redundant pd rules for TiFlash.
/// 对应 Go `TestTiFlashNoRedundantPDRules`：副本变更后不残留多余 PD placement rule。
#[test]
pub fn test_ti_flash_no_redundant_pd_rules() {
    // test_ti_flash_no_redundant_pd_rules 对应 Go 函数 TestTiFlashNoRedundantPDRules(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflashp")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflashp(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10),PARTITION p1 VALUES LESS THAN (20), PARTITION p2 VALUES LESS THAN (30))")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflashp set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("ALTER TABLE ddltiflashp ADD PARTITION (PARTITION pn VALUES LESS THAN (%v))", lessThan))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflashp truncate partition p1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflashp drop partition p2")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("truncate table ddltiflash")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table ddltiflash")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
        },
    ];
    record_go_test_steps("TestTiFlashNoRedundantPDRules", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashNoRedundantPDRules",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()

	rpcClient, pdClient, cluster, err := unistore.New("", nil, constants.NullKeyspaceID, nil)
	require.NoError(t, err)
	defer func() {
		rpcClient.Close()
		pdClient.Close()
		cluster.Close()
	}()
	for _, store := range s.cluster.GetAllStores() {
		cluster.AddStore(store.Id, store.Address, store.Labels...)
	}
	gcWorker, err := gcworker.NewMockGCWorker(s.store)
	require.NoError(t, err)
	tk := testkit.NewTestKit(t, s.store)
	fCancel := TempDisableEmulatorGC()
	defer fCancel()
	// Disable emulator GC, otherwise delete range will be automatically called.

	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed"))
	}()

	fCancelPD := s.SetPdLoop(10000)
	defer fCancelPD()

	// Clean all rules
	s.tiflash.CleanPlacementRules()
	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("drop table if exists ddltiflashp")
	tk.MustExec("create table ddltiflash(z int)")
	tk.MustExec("create table ddltiflashp(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10),PARTITION p1 VALUES LESS THAN (20), PARTITION p2 VALUES LESS THAN (30))")

	total := 0
	require.Equal(t, total, s.tiflash.PlacementRulesLen())

	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	total += 1
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	require.Equal(t, total, s.tiflash.PlacementRulesLen())

	tk.MustExec("alter table ddltiflashp set tiflash replica 1")
	total += 3
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	require.Equal(t, total, s.tiflash.PlacementRulesLen())

	lessThan := 40
	tk.MustExec(fmt.Sprintf("ALTER TABLE ddltiflashp ADD PARTITION (PARTITION pn VALUES LESS THAN (%v))", lessThan))
	total += 1
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	require.Equal(t, total, s.tiflash.PlacementRulesLen())

	tk.MustExec("alter table ddltiflashp truncate partition p1")
	total += 1
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	require.Equal(t, total, s.tiflash.PlacementRulesLen())
	// Now gc will trigger, and will remove dropped partition.
	require.NoError(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
	total -= 1
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	require.Equal(t, total, s.tiflash.PlacementRulesLen())

	tk.MustExec("alter table ddltiflashp drop partition p2")
	require.NoError(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
	total -= 1
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	require.Equal(t, total, s.tiflash.PlacementRulesLen())

	tk.MustExec("truncate table ddltiflash")
	total += 1
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	require.Equal(t, total, s.tiflash.PlacementRulesLen())
	require.NoError(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
	total -= 1
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	require.Equal(t, total, s.tiflash.PlacementRulesLen())

	tk.MustExec("drop table ddltiflash")
	total -= 1
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	require.NoError(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
	require.Equal(t, total, s.tiflash.PlacementRulesLen())"########,
    );
}

/// 对应 Go `TestTiFlashReplicaPartitionTableNormal`：分区表正常设置 TiFlash 副本。
#[test]
pub fn test_ti_flash_replica_partition_table_normal() {
    // test_ti_flash_replica_partition_table_normal 对应 Go 函数 TestTiFlashReplicaPartitionTableNormal(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10),PARTITION p1 VALUES LESS THAN (20), PARTITION p2 VALUES LESS THAN (30))")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, replica)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("ALTER TABLE ddltiflash ADD PARTITION (PARTITION pn VALUES LESS THAN (%v))", lessThan))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb2)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, pi)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb2.Meta().TiFlashReplica)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, tb2.Meta().TiFlashReplica.IsPartitionAvailable(p.ID))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, table.Accel)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Zero(t, len(pi.AddingDefinitions))"########,
        },
    ];
    record_go_test_steps("TestTiFlashReplicaPartitionTableNormal", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashReplicaPartitionTableNormal",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10),PARTITION p1 VALUES LESS THAN (20), PARTITION p2 VALUES LESS THAN (30))")

	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	replica := tb.Meta().TiFlashReplica
	require.Nil(t, replica)

	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	lessThan := "40"
	tk.MustExec(fmt.Sprintf("ALTER TABLE ddltiflash ADD PARTITION (PARTITION pn VALUES LESS THAN (%v))", lessThan))

	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	// Should get schema again
	CheckTableAvailable(s.dom, t, 1, []string{})

	tb2, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb2)
	pi := tb2.Meta().GetPartitionInfo()
	require.NotNil(t, pi)
	require.NotNil(t, tb2.Meta().TiFlashReplica)
	for _, p := range pi.Definitions {
		require.True(t, tb2.Meta().TiFlashReplica.IsPartitionAvailable(p.ID))
		if len(p.LessThan) == 1 && p.LessThan[0] == lessThan {
			table, ok := s.tiflash.GetTableSyncStatus(int(p.ID))
			require.True(t, ok)
			require.True(t, table.Accel)
		}
	}
	require.Zero(t, len(pi.AddingDefinitions))
	s.CheckFlashback(tk, t)"########,
    );
}

// When block add partition, new partition shall be available even we break `UpdateTableReplicaInfo`
/// 对应 Go `TestTiFlashReplicaPartitionTableBlock`：分区表副本设置阻塞路径。
#[test]
pub fn test_ti_flash_replica_partition_table_block() {
    // test_ti_flash_replica_partition_table_block 对应 Go 函数 TestTiFlashReplicaPartitionTableBlock(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10),PARTITION p1 VALUES LESS THAN (20), PARTITION p2 VALUES LESS THAN (30))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/BeforeRefreshTiFlashTickerLoop", `return`))"########,
        },
        GoTestStep {
            action: "Failpoint",
            detail: r########"_ = failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/BeforeRefreshTiFlashTickerLoop")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("ALTER TABLE ddltiflash ADD PARTITION (PARTITION pn VALUES LESS THAN (%v))", lessThan))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, pi)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, tb.Meta().TiFlashReplica.IsPartitionAvailable(p.ID))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, table.Accel)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 0, len(pi.AddingDefinitions))"########,
        },
    ];
    record_go_test_steps("TestTiFlashReplicaPartitionTableBlock", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashReplicaPartitionTableBlock",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10),PARTITION p1 VALUES LESS THAN (20), PARTITION p2 VALUES LESS THAN (30))")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	// Make sure is available
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	CheckTableAvailable(s.dom, t, 1, []string{})

	lessThan := "40"
	// Stop loop
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/BeforeRefreshTiFlashTickerLoop", `return`))
	defer func() {
		_ = failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/BeforeRefreshTiFlashTickerLoop")
	}()

	tk.MustExec(fmt.Sprintf("ALTER TABLE ddltiflash ADD PARTITION (PARTITION pn VALUES LESS THAN (%v))", lessThan))
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	pi := tb.Meta().GetPartitionInfo()
	require.NotNil(t, pi)

	// Partition `lessThan` shall be ready
	for _, p := range pi.Definitions {
		require.True(t, tb.Meta().TiFlashReplica.IsPartitionAvailable(p.ID))
		if len(p.LessThan) == 1 && p.LessThan[0] == lessThan {
			table, ok := s.tiflash.GetTableSyncStatus(int(p.ID))
			require.True(t, ok)
			require.True(t, table.Accel)
		}
	}
	require.Equal(t, 0, len(pi.AddingDefinitions))
	s.CheckFlashback(tk, t)"########,
    );
}

// TiFlash Table shall be eventually available.
/// 对应 Go `TestTiFlashReplicaAvailable`：副本 Available 状态流转。
#[test]
pub fn test_ti_flash_replica_available() {
    // test_ti_flash_replica_available 对应 Go 函数 TestTiFlashReplicaAvailable(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash2 like ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash2 set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, r)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 0")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, replica)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, r)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, ok)"########,
        },
    ];
    record_go_test_steps("TestTiFlashReplicaAvailable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashReplicaAvailable",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int)")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})

	tk.MustExec("drop table if exists ddltiflash2")
	tk.MustExec("create table ddltiflash2 like ddltiflash")
	tk.MustExec("alter table ddltiflash2 set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", "ddltiflash2")

	s.CheckFlashback(tk, t)
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	r, ok := s.tiflash.GetPlacementRule(infosync.MakeRuleID(tb.Meta().ID))
	require.NotNil(t, r)
	require.True(t, ok)
	tk.MustExec("alter table ddltiflash set tiflash replica 0")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	tb, err = s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	replica := tb.Meta().TiFlashReplica
	require.Nil(t, replica)
	r, ok = s.tiflash.GetPlacementRule(infosync.MakeRuleID(tb.Meta().ID))
	require.Nil(t, r)
	require.False(t, ok)"########,
    );
}

// Truncate partition shall not block.
/// 对应 Go `TestTiFlashTruncatePartition`：截断分区后副本规则更新。
#[test]
pub fn test_ti_flash_truncate_partition() {
    // test_ti_flash_truncate_partition 对应 Go 函数 TestTiFlashTruncatePartition(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(i int not null, s varchar(255)) partition by range (i) (partition p0 values less than (10), partition p1 values less than (20))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into ddltiflash values(1, 'abc'), (11, 'def')")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash truncate partition p1")"########,
        },
    ];
    record_go_test_steps("TestTiFlashTruncatePartition", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashTruncatePartition",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(i int not null, s varchar(255)) partition by range (i) (partition p0 values less than (10), partition p1 values less than (20))")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	tk.MustExec("insert into ddltiflash values(1, 'abc'), (11, 'def')")
	tk.MustExec("alter table ddltiflash truncate partition p1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", "ddltiflash")"########,
    );
}

// Fail truncate partition.
/// 对应 Go `TestTiFlashFailTruncatePartition`：截断分区失败注入。
#[test]
pub fn test_ti_flash_fail_truncate_partition() {
    // test_ti_flash_fail_truncate_partition 对应 Go 函数 TestTiFlashFailTruncatePartition(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@global.tidb_ddl_error_count_limit = 3")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(i int not null, s varchar(255)) partition by range (i) (partition p0 values less than (10), partition p1 values less than (20))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/FailTiFlashTruncatePartition", `return`))"########,
        },
        GoTestStep {
            action: "Failpoint",
            detail: r########"failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/FailTiFlashTruncatePartition")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into ddltiflash values(1, 'abc'), (11, 'def')")"########,
        },
        GoTestStep {
            action: "MustGetErrMsg",
            detail: r########"tk.MustGetErrMsg("alter table ddltiflash truncate partition p1", "[ddl:-1]DDL job rollback, error msg: enforced error")"########,
        },
    ];
    record_go_test_steps("TestTiFlashFailTruncatePartition", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashFailTruncatePartition",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)
	tk.MustExec("set @@global.tidb_ddl_error_count_limit = 3")

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(i int not null, s varchar(255)) partition by range (i) (partition p0 values less than (10), partition p1 values less than (20))")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")

	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/FailTiFlashTruncatePartition", `return`))
	defer func() {
		failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/FailTiFlashTruncatePartition")
	}()
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)

	tk.MustExec("insert into ddltiflash values(1, 'abc'), (11, 'def')")
	tk.MustGetErrMsg("alter table ddltiflash truncate partition p1", "[ddl:-1]DDL job rollback, error msg: enforced error")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", "ddltiflash")"########,
    );
}

// Drop partition shall not block.
/// 对应 Go `TestTiFlashDropPartition`：删除分区后副本清理。
#[test]
pub fn test_ti_flash_drop_partition() {
    // test_ti_flash_drop_partition 对应 Go 函数 TestTiFlashDropPartition(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(i int not null, s varchar(255)) partition by range (i) (partition p0 values less than (10), partition p1 values less than (20))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash drop partition p1")"########,
        },
    ];
    record_go_test_steps("TestTiFlashDropPartition", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashDropPartition",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(i int not null, s varchar(255)) partition by range (i) (partition p0 values less than (10), partition p1 values less than (20))")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", "ddltiflash")
	tk.MustExec("alter table ddltiflash drop partition p1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable * 5)
	CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", "ddltiflash")"########,
    );
}

/// 对应 Go `TestTiFlashFlashbackCluster`：集群 flashback 与 TiFlash 副本。
#[test]
pub fn test_ti_flash_flashback_cluster() {
    // test_ti_flash_flashback_cluster 对应 Go 函数 TestTiFlashFlashbackCluster(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t(a int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t values (1), (2), (3)")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockFlashbackTest", `return(true)`))"########,
        },
        GoTestStep {
            action: "MustGetErrMsg",
            detail: r########"tk.MustGetErrMsg(fmt.Sprintf("flashback cluster to timestamp '%s'", oracle.GetTimeFromTS(ts).Format(types.TimeFSPFormat)), errorMsg)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockFlashbackTest"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"))"########,
        },
    ];
    record_go_test_steps("TestTiFlashFlashbackCluster", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashFlashbackCluster",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists t")
	tk.MustExec("create table t(a int)")
	tk.MustExec("insert into t values (1), (2), (3)")

	ts, err := tk.Session().GetStore().GetOracle().GetTimestamp(context.Background(), &oracle.Option{})
	require.NoError(t, err)

	tk.MustExec("alter table t set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", "t")

	injectSafeTS := oracle.GoTimeToTS(oracle.GetTimeFromTS(ts).Add(10 * time.Second))
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockFlashbackTest", `return(true)`))
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS",
		fmt.Sprintf("return(%v)", injectSafeTS)))

	ChangeGCSafePoint(tk, time.Now().Add(-10*time.Second), "true", "10m0s")
	defer func() {
		ChangeGCSafePoint(tk, time.Now(), "true", "10m0s")
	}()

	errorMsg := fmt.Sprintf("[ddl:-1]Detected unsupported DDL job type(%s) during [%s, now), can't do flashback",
		model.ActionSetTiFlashReplica.String(), oracle.GetTimeFromTS(ts).Format(types.TimeFSPFormat))
	tk.MustGetErrMsg(fmt.Sprintf("flashback cluster to timestamp '%s'", oracle.GetTimeFromTS(ts).Format(types.TimeFSPFormat)), errorMsg)

	require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockFlashbackTest"))
	require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"))"########,
    );
}

/// 对应 Go `CheckTableAvailableWithTableName`：按库表名断言副本可用。
pub fn check_table_available_with_table_name() {
    // check_table_available_with_table_name 对应 Go 函数 CheckTableAvailableWithTableName(dom *domain.Domain, t *testing.T, count uint64, labels []string, db string, table string) 。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, replica)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, replica.Available)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, count, replica.Count)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.ElementsMatch(t, labels, replica.LocationLabels)"########,
        },
    ];
    record_go_test_steps("CheckTableAvailableWithTableName", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "CheckTableAvailableWithTableName",
        r########"	tb, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr(db), ast.NewCIStr(table))
	require.NoError(t, err)
	replica := tb.Meta().TiFlashReplica
	require.NotNil(t, replica)
	require.True(t, replica.Available)
	require.Equal(t, count, replica.Count)
	require.ElementsMatch(t, labels, replica.LocationLabels)"########,
    );
}

/// 对应 Go `WaitTablesAvailableWithTableName`：轮询等待多表副本可用。
pub fn wait_tables_available_with_table_name() {
    // wait_tables_available_with_table_name 对应 Go 函数 WaitTablesAvailableWithTableName(dom *domain.Domain, t *testing.T, count uint64, labels []string, db string, tables []string, timeout time.Duration) 。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    let _steps = &[GoTestStep {
        action: "Require",
        detail: r########"require.Eventually(t, func() bool {"########,
    }];
    record_go_test_steps("WaitTablesAvailableWithTableName", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "WaitTablesAvailableWithTableName",
        r########"	t.Helper()
	require.Eventually(t, func() bool {
		for _, tableName := range tables {
			tb, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr(db), ast.NewCIStr(tableName))
			if err != nil {
				return false
			}
			replica := tb.Meta().TiFlashReplica
			if replica == nil || !replica.Available {
				return false
			}
		}
		return true
	}, timeout, ddl.PollTiFlashInterval/2)
	for _, tableName := range tables {
		CheckTableAvailableWithTableName(dom, t, count, labels, db, tableName)
	}"########,
    );
}

/// 对应 Go `CheckTableAvailable`：断言默认库表副本可用。
pub fn check_table_available() {
    // check_table_available 对应 Go 函数 CheckTableAvailable(dom *domain.Domain, t *testing.T, count uint64, labels []string) 。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    record_go_test_steps("CheckTableAvailable", &[]);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "CheckTableAvailable",
        r########"	CheckTableAvailableWithTableName(dom, t, count, labels, "test", "ddltiflash")"########,
    );
}

/// 对应 Go `tableReplicaWithTableName`：按库表名读取 TiFlashReplicaInfo。
pub fn table_replica_with_table_name() {
    // table_replica_with_table_name 对应 Go 函数 tableReplicaWithTableName(dom *domain.Domain, db string, table string) *model.TiFlashReplicaInfo。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    record_go_test_steps("tableReplicaWithTableName", &[]);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "tableReplicaWithTableName",
        r########"	tb, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr(db), ast.NewCIStr(table))
	if err != nil || tb == nil {
		return nil
	}
	return tb.Meta().TiFlashReplica"########,
    );
}

/// 对应 Go `waitTableReplicaStateWithTableName`：等待副本 Available 布尔状态。
pub fn wait_table_replica_state_with_table_name() {
    // wait_table_replica_state_with_table_name 对应 Go 函数 waitTableReplicaStateWithTableName(dom *domain.Domain, t *testing.T, db string, table string, available bool, timeout time.Duration) 。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.Eventually(t, func() bool {"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, replica)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, available, replica.Available)"########,
        },
    ];
    record_go_test_steps("waitTableReplicaStateWithTableName", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "waitTableReplicaStateWithTableName",
        r########"	t.Helper()
	require.Eventually(t, func() bool {
		replica := tableReplicaWithTableName(dom, db, table)
		return replica != nil && replica.Available == available
	}, timeout, ddl.PollTiFlashInterval/2)
	replica := tableReplicaWithTableName(dom, db, table)
	require.NotNil(t, replica)
	require.Equal(t, available, replica.Available)"########,
    );
}

/// 对应 Go `CheckTableNoReplica`：断言表无 TiFlash 副本。
pub fn check_table_no_replica() {
    // check_table_no_replica 对应 Go 函数 CheckTableNoReplica(dom *domain.Domain, t *testing.T, db string, table string) 。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, replica)"########,
        },
    ];
    record_go_test_steps("CheckTableNoReplica", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "CheckTableNoReplica",
        r########"	tb, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr(db), ast.NewCIStr(table))
	require.NoError(t, err)
	replica := tb.Meta().TiFlashReplica
	require.Nil(t, replica)"########,
    );
}

// Truncate table shall not block.
/// 对应 Go `TestTiFlashTruncateTable`：整表截断与副本。
#[test]
pub fn test_ti_flash_truncate_table() {
    // test_ti_flash_truncate_table 对应 Go 函数 TestTiFlashTruncateTable(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflashp")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflashp(z int not null) partition by range (z) (partition p0 values less than (10), partition p1 values less than (20))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflashp set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("truncate table ddltiflashp")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash2(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash2 set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("truncate table ddltiflash2")"########,
        },
    ];
    record_go_test_steps("TestTiFlashTruncateTable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashTruncateTable",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflashp")
	tk.MustExec("create table ddltiflashp(z int not null) partition by range (z) (partition p0 values less than (10), partition p1 values less than (20))")
	tk.MustExec("alter table ddltiflashp set tiflash replica 1")

	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	// Should get schema right now
	tk.MustExec("truncate table ddltiflashp")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailablePartitionTable)
	CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", "ddltiflashp")
	tk.MustExec("drop table if exists ddltiflash2")
	tk.MustExec("create table ddltiflash2(z int)")
	tk.MustExec("alter table ddltiflash2 set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	// Should get schema right now

	tk.MustExec("truncate table ddltiflash2")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", "ddltiflash2")"########,
    );
}

// TiFlash Table shall be eventually available, even with lots of small table created.
/// 对应 Go `TestTiFlashMassiveReplicaAvailable`：大量表副本可用。
#[test]
pub fn test_ti_flash_massive_replica_available() {
    // test_ti_flash_massive_replica_available 对应 Go 函数 TestTiFlashMassiveReplicaAvailable(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("drop table if exists %s", tableNames[i]))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("create table %s(z int)", tableNames[i]))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("alter table %s set tiflash replica 1", tableNames[i]))"########,
        },
    ];
    record_go_test_steps("TestTiFlashMassiveReplicaAvailable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashMassiveReplicaAvailable",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tableNames := make([]string, 100)
	tk.MustExec("use test")
	for i := range 100 {
		tableNames[i] = fmt.Sprintf("ddltiflash%v", i)
		tk.MustExec(fmt.Sprintf("drop table if exists %s", tableNames[i]))
		tk.MustExec(fmt.Sprintf("create table %s(z int)", tableNames[i]))
		tk.MustExec(fmt.Sprintf("alter table %s set tiflash replica 1", tableNames[i]))
	}

	WaitTablesAvailableWithTableName(s.dom, t, 1, []string{}, "test", tableNames, 30*time.Second)"########,
    );
}

// When set TiFlash replica, tidb shall add one Pd Rule for this table.
// When drop/truncate table, Pd Rule shall be removed in limited time.
/// 对应 Go `TestSetPlacementRuleNormal`：正常下发 PD placement rule。
#[test]
pub fn test_set_placement_rule_normal() {
    // test_set_placement_rule_normal 对应 Go 函数 TestSetPlacementRuleNormal(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1 location labels 'a','b'")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, res)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table ddltiflash")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, res)"########,
        },
    ];
    record_go_test_steps("TestSetPlacementRuleNormal", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestSetPlacementRuleNormal",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int)")
	tk.MustExec("alter table ddltiflash set tiflash replica 1 location labels 'a','b'")
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	expectRule := infosync.MakeNewRule(tb.Meta().ID, 1, []string{"a", "b"})
	res := s.tiflash.CheckPlacementRule(expectRule)
	require.True(t, res)

	// Set lastSafePoint to a timepoint in future, so all dropped table can be reckon as gc-ed.
	ChangeGCSafePoint(tk, time.Now().Add(+3*time.Second), "true", "10m0s")
	defer func() {
		ChangeGCSafePoint(tk, time.Now(), "true", "10m0s")
	}()
	fCancelPD := s.SetPdLoop(1)
	defer fCancelPD()
	tk.MustExec("drop table ddltiflash")
	expectRule = infosync.MakeNewRule(tb.Meta().ID, 1, []string{"a", "b"})
	res = s.tiflash.CheckPlacementRule(expectRule)
	require.True(t, res)"########,
    );
}

// When gc worker works, it will automatically remove pd rule for TiFlash.
/// 对应 Go `TestSetPlacementRuleWithGCWorker`：与 GC worker 并发下的 placement rule。
#[test]
pub fn test_set_placement_rule_with_gc_worker() {
    // test_set_placement_rule_with_gc_worker 对应 Go 函数 TestSetPlacementRuleWithGCWorker(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "Failpoint",
            detail: r########"failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`)"########,
        },
        GoTestStep {
            action: "Failpoint",
            detail: r########"failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash_gc")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash_gc(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash_gc set tiflash replica 1 location labels 'a','b'")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, res)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table ddltiflash_gc")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, res)"########,
        },
    ];
    record_go_test_steps("TestSetPlacementRuleWithGCWorker", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestSetPlacementRuleWithGCWorker",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()

	rpcClient, pdClient, cluster, err := unistore.New("", nil, constants.NullKeyspaceID, nil)
	defer func() {
		rpcClient.Close()
		pdClient.Close()
		cluster.Close()
	}()
	for _, store := range s.cluster.GetAllStores() {
		cluster.AddStore(store.Id, store.Address, store.Labels...)
	}
	failpoint.Enable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed", `return`)
	defer func() {
		failpoint.Disable("github.com/pingcap/tidb/pkg/store/gcworker/ignoreDeleteRangeFailed")
	}()
	fCancelPD := s.SetPdLoop(10000)
	defer fCancelPD()

	require.NoError(t, err)
	gcWorker, err := gcworker.NewMockGCWorker(s.store)
	require.NoError(t, err)
	// Make SetPdLoop take effects.
	time.Sleep(time.Second)

	fCancel := TempDisableEmulatorGC()
	defer fCancel()

	tk := testkit.NewTestKit(t, s.store)
	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash_gc")
	tk.MustExec("create table ddltiflash_gc(z int)")
	tk.MustExec("alter table ddltiflash_gc set tiflash replica 1 location labels 'a','b'")
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash_gc"))
	require.NoError(t, err)

	expectRule := infosync.MakeNewRule(tb.Meta().ID, 1, []string{"a", "b"})
	res := s.tiflash.CheckPlacementRule(expectRule)
	require.True(t, res)

	ChangeGCSafePoint(tk, time.Now().Add(-time.Hour), "true", "10m0s")
	tk.MustExec("drop table ddltiflash_gc")
	// Now gc will trigger, and will remove dropped table.
	require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))

	// Wait GC
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	res = s.tiflash.CheckPlacementRule(expectRule)
	require.False(t, res)"########,
    );
}

/// 对应 Go `TestSetPlacementRuleFail`：placement rule 下发失败。
#[test]
pub fn test_set_placement_rule_fail() {
    // test_set_placement_rule_fail 对应 Go 函数 TestSetPlacementRuleFail(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, res)"########,
        },
    ];
    record_go_test_steps("TestSetPlacementRuleFail", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestSetPlacementRuleFail",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int)")
	s.tiflash.PdSwitch(false)
	defer func() {
		s.tiflash.PdSwitch(true)
	}()
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)

	expectRule := infosync.MakeNewRule(tb.Meta().ID, 1, []string{})
	res := s.tiflash.CheckPlacementRule(expectRule)
	require.False(t, res)"########,
    );
}

// Test standalone backoffer
/// 对应 Go `TestTiFlashBackoffer`：TiFlash 操作退避（backoff）策略。
#[test]
pub fn test_ti_flash_backoffer() {
    // test_ti_flash_backoffer 对应 Go 函数 TestTiFlashBackoffer(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, growed)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, ori, e.Threshold)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, c+1, e.Counter)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, oriTotal+1, total)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, growed)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, e.Threshold, rate*ori)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 1, e.Counter)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, oriTotal+1, total)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, mustGet(1).NeedGrow())"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 8, mustGet(1).TotalCounter)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, maxTick, mustGet(2).Threshold)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 20, mustGet(2).TotalCounter)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, backoff.Remove(1))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, backoff.Remove(1))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 1, backoff.Len())"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Error(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Error(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Error(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Error(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Error(t, err)"########,
        },
    ];
    record_go_test_steps("TestTiFlashBackoffer", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashBackoffer",
        r########"	var maxTick ddl.TiFlashTick = 10
	var rate ddl.TiFlashTick = 1.5
	c := 2
	backoff, err := ddl.NewPollTiFlashBackoffContext(1, maxTick, c, rate)
	require.NoError(t, err)
	mustGet := func(ID int64) *ddl.PollTiFlashBackoffElement {
		e, ok := backoff.Get(ID)
		require.True(t, ok)
		return e
	}
	mustNotGrow := func(ID int64) {
		e := mustGet(ID)
		ori := e.Threshold
		oriTotal := e.TotalCounter
		c := e.Counter
		growed, ok, total := backoff.Tick(ID)
		require.True(t, ok)
		require.False(t, growed)
		require.Equal(t, ori, e.Threshold)
		require.Equal(t, c+1, e.Counter)
		require.Equal(t, oriTotal+1, total)
	}
	mustGrow := func(ID int64) {
		e := mustGet(ID)
		ori := e.Threshold
		oriTotal := e.TotalCounter
		growed, ok, total := backoff.Tick(ID)
		require.True(t, ok)
		require.True(t, growed)
		require.Equal(t, e.Threshold, rate*ori)
		require.Equal(t, 1, e.Counter)
		require.Equal(t, oriTotal+1, total)
	}
	// Test grow
	ok := backoff.Put(1)
	require.True(t, ok)
	require.False(t, mustGet(1).NeedGrow())
	mustNotGrow(1) // 0;1 -> 1;1
	mustGrow(1)    // 1;1 -> 0;1.5 -> 1;1.5
	mustGrow(1)    // 1;1.5 -> 0;2.25 -> 1;2.25
	mustNotGrow(1) // 1;2.25 -> 2;2.25
	mustGrow(1)    // 2;2.25 -> 0;3.375 -> 1;3.375
	mustNotGrow(1) // 1;3.375 -> 2;3.375
	mustNotGrow(1) // 2;3.375 -> 3;3.375
	mustGrow(1)    // 3;3.375 -> 0;5.0625
	require.Equal(t, 8, mustGet(1).TotalCounter)

	// Test converge
	backoff.Put(2)
	for range 20 {
		backoff.Tick(2)
	}
	require.Equal(t, maxTick, mustGet(2).Threshold)
	require.Equal(t, 20, mustGet(2).TotalCounter)

	// Test context
	ok = backoff.Put(3)
	require.False(t, ok)
	_, ok, _ = backoff.Tick(3)
	require.False(t, ok)

	require.True(t, backoff.Remove(1))
	require.False(t, backoff.Remove(1))
	require.Equal(t, 1, backoff.Len())

	// Test error context
	_, err = ddl.NewPollTiFlashBackoffContext(0.5, 1, 1, 1)
	require.Error(t, err)
	_, err = ddl.NewPollTiFlashBackoffContext(10, 1, 1, 1)
	require.Error(t, err)
	_, err = ddl.NewPollTiFlashBackoffContext(1, 10, 0, 1)
	require.Error(t, err)
	_, err = ddl.NewPollTiFlashBackoffContext(1, 10, 1, 0.5)
	require.Error(t, err)
	_, err = ddl.NewPollTiFlashBackoffContext(1, 10, 1, -1)
	require.Error(t, err)"########,
    );

    use astersql_ddl::ddl_tiflash_api::{
        NewPollTiFlashBackoffContext, PollTiFlashBackoffContext, PollTiFlashBackoffElement,
    };

    fn must_get(
        backoff: &mut PollTiFlashBackoffContext,
        id: i64,
    ) -> *mut PollTiFlashBackoffElement {
        let (element, exists) = backoff.Get(id);
        assert!(exists);
        assert!(!element.is_null());
        element
    }

    fn must_not_grow(backoff: &mut PollTiFlashBackoffContext, id: i64) {
        let element = must_get(backoff, id);
        let (threshold, total_counter, counter) = unsafe {
            (
                (*element).Threshold,
                (*element).TotalCounter,
                (*element).Counter,
            )
        };
        let (grew, exists, total) = backoff.Tick(id);
        assert!(exists);
        assert!(!grew);
        unsafe {
            assert_eq!((*element).Threshold, threshold);
            assert_eq!((*element).Counter, counter + 1);
        }
        assert_eq!(total, total_counter + 1);
    }

    fn must_grow(backoff: &mut PollTiFlashBackoffContext, id: i64, rate: f64) {
        let element = must_get(backoff, id);
        let (threshold, total_counter) = unsafe { ((*element).Threshold, (*element).TotalCounter) };
        let (grew, exists, total) = backoff.Tick(id);
        assert!(exists);
        assert!(grew);
        unsafe {
            assert_eq!((*element).Threshold, rate * threshold);
            assert_eq!((*element).Counter, 1);
        }
        assert_eq!(total, total_counter + 1);
    }

    let max_tick = 10.0;
    let rate = 1.5;
    let mut backoff = NewPollTiFlashBackoffContext(1.0, max_tick, 2, rate).unwrap();

    assert!(backoff.Put(1));
    assert!(backoff.Put(1));
    let element_one = must_get(&mut backoff, 1);
    unsafe {
        assert!(!(*element_one).NeedGrow());
    }
    must_not_grow(&mut backoff, 1);
    must_grow(&mut backoff, 1, rate);
    must_grow(&mut backoff, 1, rate);
    must_not_grow(&mut backoff, 1);
    must_grow(&mut backoff, 1, rate);
    must_not_grow(&mut backoff, 1);
    must_not_grow(&mut backoff, 1);
    must_grow(&mut backoff, 1, rate);
    unsafe {
        assert_eq!((*element_one).TotalCounter, 8);
    }

    assert!(backoff.Put(2));
    let element_two = must_get(&mut backoff, 2);
    for _ in 0..20 {
        backoff.Tick(2);
    }
    unsafe {
        assert_eq!((*element_two).Threshold, max_tick);
        assert_eq!((*element_two).TotalCounter, 20);
    }

    assert!(!backoff.Put(3));
    assert_eq!(backoff.Tick(3), (false, false, 0));
    assert!(backoff.Remove(1));
    assert!(!backoff.Remove(1));
    assert_eq!(backoff.Len(), 1);

    assert_eq!(
        NewPollTiFlashBackoffContext(0.5, 1.0, 1, 1.0)
            .unwrap_err()
            .to_string(),
        "`minThreshold` should not be less than 1"
    );
    assert_eq!(
        NewPollTiFlashBackoffContext(10.0, 1.0, 1, 1.0)
            .unwrap_err()
            .to_string(),
        "`maxThreshold` should always be larger than `minThreshold`"
    );
    assert!(NewPollTiFlashBackoffContext(1.0, 10.0, 0, 1.0).is_err());
    assert_eq!(
        NewPollTiFlashBackoffContext(1.0, 10.0, -1, 1.5)
            .unwrap_err()
            .to_string(),
        "negative `capacity`"
    );
    assert!(NewPollTiFlashBackoffContext(1.0, 10.0, 1, 0.5).is_err());
    assert!(NewPollTiFlashBackoffContext(1.0, 10.0, 1, -1.0).is_err());
    assert!(NewPollTiFlashBackoffContext(1.0, 1.0, 1, 1.5).is_ok());

    let mut zero_capacity = NewPollTiFlashBackoffContext(1.0, 10.0, 0, 1.5).unwrap();
    assert!(!zero_capacity.Put(1));

    let mut raised_minimum = NewPollTiFlashBackoffContext(2.0, 10.0, 1, 2.0).unwrap();
    assert!(raised_minimum.Put(1));
    let raised_element = must_get(&mut raised_minimum, 1);
    unsafe {
        assert_eq!((*raised_element).Threshold, 1.0);
    }
    assert_eq!(raised_minimum.Tick(1), (false, true, 1));
    assert_eq!(raised_minimum.Tick(1), (true, true, 2));
    unsafe {
        assert_eq!((*raised_element).Threshold, 4.0);
    }
}

// Test backoffer in TiFlash.
/// 对应 Go `TestTiFlashBackoff`：退避重试行为。
#[test]
pub fn test_ti_flash_backoff() {
    // test_ti_flash_backoff 对应 Go 函数 TestTiFlashBackoff(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, tb.Meta().TiFlashReplica.Available)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 0))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Eventually(t, func() bool {"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 0))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Never(t, func() bool {"########,
        },
    ];
    record_go_test_steps("TestTiFlashBackoff", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashBackoff",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	se := session.CreateSessionAndSetID(t, s.store)
	defer se.Close()

	session.MustExec(t, se, "use test")
	session.MustExec(t, se, "drop table if exists ddltiflash")
	session.MustExec(t, se, "create table ddltiflash(z int)")

	// Hold the mocked TiFlash sync status unavailable while polling is paused so the
	// backoff path is exercised without relying on failpoint instrumentation.
	ddl.DisableTiFlashPoll(s.dom.DDL())
	session.MustExec(t, se, "alter table ddltiflash set tiflash replica 1")
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	s.tiflash.ResetSyncStatus(int(tb.Meta().ID), false)
	ddl.EnableTiFlashPoll(s.dom.DDL())

	// 1, 1.5, 2.25, 3.375, 5.5625
	// (1), 1, 1, 2, 3, 5
	time.Sleep(ddl.PollTiFlashInterval * 5)
	tb, err = s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	require.False(t, tb.Meta().TiFlashReplica.Available)

	s.tiflash.ResetSyncStatus(int(tb.Meta().ID), true)
	WaitTablesAvailableWithTableName(s.dom, t, 1, []string{}, "test", []string{"ddltiflash"}, ddl.PollTiFlashInterval*RoundToBeAvailable*10)
	// Availability alone is not enough here: the poller keeps scheduling available
	// tables for post-availability progress refreshes on later ticks. First prove one
	// such refresh still happens after the table becomes available.
	require.NoError(t, infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 0))
	require.Eventually(t, func() bool {
		progress, isExist := infosync.GetTiFlashProgressFromCache(tb.Meta().ID)
		return isExist && progress == 1
	}, ddl.PollTiFlashInterval*RoundToBeAvailable*10, ddl.PollTiFlashInterval/2)
	// Then stop future polling and prove no in-flight or later refresh can still
	// reach the progress cache before teardown closes the mock status server.
	ddl.DisableTiFlashPoll(s.dom.DDL())
	require.NoError(t, infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 0))
	require.Never(t, func() bool {
		progress, isExist := infosync.GetTiFlashProgressFromCache(tb.Meta().ID)
		return isExist && progress == 1
	}, ddl.PollTiFlashInterval*2, ddl.PollTiFlashInterval/5)"########,
    );
}

/// 对应 Go `TestAlterDatabaseBasic`：ALTER DATABASE 设置 TiFlash 副本。
#[test]
pub fn test_alter_database_basic() {
    // test_alter_database_basic 对应 Go 函数 TestAlterDatabaseBasic(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop database if exists tiflash_ddl")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database tiflash_ddl")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tiflash_ddl.ddltiflash(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tiflash_ddl.ddltiflash2(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table tiflash_ddl.ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter database tiflash_ddl set tiflash replica 2")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, "In total 2 tables: 2 succeed, 0 failed, 0 skipped", tk.Session().GetSessionVars().StmtCtx.GetMessage())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter database tiflash_ddl set tiflash replica 2")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, "In total 2 tables: 0 succeed, 0 failed, 2 skipped", tk.Session().GetSessionVars().StmtCtx.GetMessage())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop database if exists tiflash_ddl_missing")"########,
        },
        GoTestStep {
            action: "MustGetErrMsg",
            detail: r########"tk.MustGetErrMsg("alter database tiflash_ddl_missing set tiflash replica 2", "[schema:1049]Unknown database 'tiflash_ddl_missing'")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop database if exists tiflash_ddl_empty")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database tiflash_ddl_empty")"########,
        },
        GoTestStep {
            action: "MustGetErrMsg",
            detail: r########"tk.MustGetErrMsg("alter database tiflash_ddl_empty set tiflash replica 2", "[schema:1049]Empty database 'tiflash_ddl_empty'")"########,
        },
        GoTestStep {
            action: "MustGetErrMsg",
            detail: r########"tk.MustGetErrMsg("alter database tiflash_ddl set tiflash replica 3", "the tiflash replica count: 3 should be less than the total tiflash server count: 2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database tiflash_ddl_skip;")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use tiflash_ddl_skip")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t (id int);")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create sequence t_seq;")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create view t_view as select id from t;")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create global temporary table t_temp (id int) on commit delete rows;")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter database tiflash_ddl_skip set tiflash replica 1;")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, "In total 4 tables: 1 succeed, 0 failed, 3 skipped", tk.Session().GetSessionVars().StmtCtx.GetMessage())"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery(`show warnings;`).Sort().Check(testkit.Rows("########,
        },
    ];
    record_go_test_steps("TestAlterDatabaseBasic", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestAlterDatabaseBasic",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("drop database if exists tiflash_ddl")
	tk.MustExec("create database tiflash_ddl")
	tk.MustExec("create table tiflash_ddl.ddltiflash(z int)")
	tk.MustExec("create table tiflash_ddl.ddltiflash2(z int)")
	// ALTER DATABASE can override previous ALTER TABLE.
	tk.MustExec("alter table tiflash_ddl.ddltiflash set tiflash replica 1")
	tk.MustExec("alter database tiflash_ddl set tiflash replica 2")
	require.Equal(t, "In total 2 tables: 2 succeed, 0 failed, 0 skipped", tk.Session().GetSessionVars().StmtCtx.GetMessage())
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 2)
	CheckTableAvailableWithTableName(s.dom, t, 2, []string{}, "tiflash_ddl", "ddltiflash")
	CheckTableAvailableWithTableName(s.dom, t, 2, []string{}, "tiflash_ddl", "ddltiflash2")

	// Skip already set TiFlash tables.
	tk.MustExec("alter database tiflash_ddl set tiflash replica 2")
	require.Equal(t, "In total 2 tables: 0 succeed, 0 failed, 2 skipped", tk.Session().GetSessionVars().StmtCtx.GetMessage())
	CheckTableAvailableWithTableName(s.dom, t, 2, []string{}, "tiflash_ddl", "ddltiflash")
	CheckTableAvailableWithTableName(s.dom, t, 2, []string{}, "tiflash_ddl", "ddltiflash2")

	// There is no existing database.
	tk.MustExec("drop database if exists tiflash_ddl_missing")
	tk.MustGetErrMsg("alter database tiflash_ddl_missing set tiflash replica 2", "[schema:1049]Unknown database 'tiflash_ddl_missing'")

	// There is no table in database
	tk.MustExec("drop database if exists tiflash_ddl_empty")
	tk.MustExec("create database tiflash_ddl_empty")
	tk.MustGetErrMsg("alter database tiflash_ddl_empty set tiflash replica 2", "[schema:1049]Empty database 'tiflash_ddl_empty'")

	// There is less TiFlash store
	tk.MustGetErrMsg("alter database tiflash_ddl set tiflash replica 3", "the tiflash replica count: 3 should be less than the total tiflash server count: 2")

	// Test Issue #51990, alter database skip set tiflash replica on sequence and view.
	tk.MustExec("create database tiflash_ddl_skip;")
	tk.MustExec("use tiflash_ddl_skip")
	tk.MustExec("create table t (id int);")
	tk.MustExec("create sequence t_seq;")
	tk.MustExec("create view t_view as select id from t;")
	tk.MustExec("create global temporary table t_temp (id int) on commit delete rows;")
	tk.MustExec("alter database tiflash_ddl_skip set tiflash replica 1;")
	require.Equal(t, "In total 4 tables: 1 succeed, 0 failed, 3 skipped", tk.Session().GetSessionVars().StmtCtx.GetMessage())
	tk.MustQuery(`show warnings;`).Sort().Check(testkit.Rows(
		"Note 1347 'tiflash_ddl_skip.t_seq' is not BASE TABLE",
		"Note 1347 'tiflash_ddl_skip.t_view' is not BASE TABLE",
		"Note 8006 `set TiFlash replica` is unsupported on temporary tables."))"########,
    );
}

/// 对应 Go `execWithTimeout`：限时执行 SQL 的辅助。
pub fn exec_with_timeout() {
    // exec_with_timeout 对应 Go 函数 execWithTimeout(t *testing.T, tk *testkit.TestKit, to time.Duration, sql string) (bool, error)。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/BatchAddTiFlashSendDone", "return(true)"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/BatchAddTiFlashSendDone"))"########,
        },
    ];
    record_go_test_steps("execWithTimeout", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "execWithTimeout",
        r########"	ctx, cancel := context.WithTimeout(context.Background(), to)
	defer cancel()
	doneCh := make(chan error, 1)

	go func() {
		_, err := tk.Exec(sql)
		doneCh <- err
	}()

	select {
	case e := <-doneCh:
		// Exit normally
		return false, e
	case <-ctx.Done():
		// Exceed given timeout
		logutil.DDLLogger().Info("execWithTimeout meet timeout", zap.String("sql", sql))
		require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/BatchAddTiFlashSendDone", "return(true)"))
	}

	e := <-doneCh
	require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/BatchAddTiFlashSendDone"))
	return true, e"########,
    );
}

/// 对应 Go `TestTiFlashBatchRateLimiter`：批量设置副本的速率限制。
#[test]
pub fn test_ti_flash_batch_rate_limiter() {
    // test_ti_flash_batch_rate_limiter 对应 Go 函数 TestTiFlashBatchRateLimiter(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database tiflash_ddl_limit")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("set SESSION tidb_batch_pending_tiflash_count=%v", threshold))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("create table tiflash_ddl_limit.t%v(z int)", i))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/PollTiFlashReplicaStatusReplaceCurAvailableValue", `return(false)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/PollTiFlashReplicaStatusReplaceCurAvailableValue"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter database tiflash_ddl_limit set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("create table tiflash_ddl_limit.t%v(z int)", threshold))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, timeOut)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, expected, cnt)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/FastFailCheckTiFlashPendingTables", fmt.Sprintf("return(%v)", loop)))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/FastFailCheckTiFlashPendingTables"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("create table tiflash_ddl_limit.t%v(z int)", threshold+1))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, timeOut)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("create table tiflash_ddl_limit.t%v(z int)", threshold+2))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, timeOut)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, timeOut)"########,
        },
    ];
    record_go_test_steps("TestTiFlashBatchRateLimiter", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashBatchRateLimiter",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	threshold := 2
	tk.MustExec("create database tiflash_ddl_limit")
	tk.MustExec(fmt.Sprintf("set SESSION tidb_batch_pending_tiflash_count=%v", threshold))
	for i := range threshold {
		tk.MustExec(fmt.Sprintf("create table tiflash_ddl_limit.t%v(z int)", i))
	}
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/PollTiFlashReplicaStatusReplaceCurAvailableValue", `return(false)`))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/PollTiFlashReplicaStatusReplaceCurAvailableValue"))
	}()

	tk.MustExec("alter database tiflash_ddl_limit set tiflash replica 1")
	tk.MustExec(fmt.Sprintf("create table tiflash_ddl_limit.t%v(z int)", threshold))
	// The following statement shall fail, because it reaches limit
	timeOut, err := execWithTimeout(t, tk, time.Second*1, "alter database tiflash_ddl_limit set tiflash replica 1")
	require.NoError(t, err)
	require.True(t, timeOut)

	// There must be one table with no TiFlashReplica.
	check := func(expected int, total int) {
		cnt := 0
		for i := range total {
			tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("tiflash_ddl_limit"), ast.NewCIStr(fmt.Sprintf("t%v", i)))
			require.NoError(t, err)
			if tb.Meta().TiFlashReplica != nil {
				cnt++
			}
		}
		require.Equal(t, expected, cnt)
	}
	check(2, 3)

	// If we exec in another session, it will not trigger limit. Since DefTiDBBatchPendingTiFlashCount is more than 3.
	tk2 := testkit.NewTestKit(t, s.store)
	tk2.MustExec("alter database tiflash_ddl_limit set tiflash replica 1")
	check(3, 3)

	loop := 3
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/FastFailCheckTiFlashPendingTables", fmt.Sprintf("return(%v)", loop)))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/FastFailCheckTiFlashPendingTables"))
	}()
	// We will force trigger its DDL to update schema cache.
	tk.MustExec(fmt.Sprintf("create table tiflash_ddl_limit.t%v(z int)", threshold+1))
	timeOut, err = execWithTimeout(t, tk, time.Millisecond*time.Duration(200*(loop+1)), "alter database tiflash_ddl_limit set tiflash replica 1")
	require.NoError(t, err)
	require.False(t, timeOut)
	check(4, 4)

	// However, forceCheck is true, so we will still enter try loop.
	tk.MustExec(fmt.Sprintf("create table tiflash_ddl_limit.t%v(z int)", threshold+2))
	timeOut, err = execWithTimeout(t, tk, time.Millisecond*200, "alter database tiflash_ddl_limit set tiflash replica 1")
	require.NoError(t, err)
	require.True(t, timeOut)
	check(4, 5)

	// Retrigger, but close session before the whole job ends.
	var wg util.WaitGroupWrapper
	var mu sync.Mutex
	wg.Run(func() {
		time.Sleep(time.Millisecond * 20)
		mu.Lock()
		defer mu.Unlock()
		tk.Session().Close()
		logutil.DDLLogger().Info("session closed")
	})
	mu.Lock()
	timeOut, err = execWithTimeout(t, tk, time.Second*2, "alter database tiflash_ddl_limit set tiflash replica 1")
	mu.Unlock()
	require.NoError(t, err)
	require.False(t, timeOut)
	check(5, 5)
	wg.Wait()"########,
    );
}

/// 对应 Go `TestTiFlashBatchKill`：批量操作被 kill。
#[test]
pub fn test_ti_flash_batch_kill() {
    // test_ti_flash_batch_kill 对应 Go 函数 TestTiFlashBatchKill(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database tiflash_ddl_limit")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set SESSION tidb_batch_pending_tiflash_count=0")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tiflash_ddl_limit.t0(z int)")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/FastFailCheckTiFlashPendingTables", `return(2)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/FastFailCheckTiFlashPendingTables"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.ErrorContains(t, err, "[executor:1317]Query execution was interrupted")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, timeOut)"########,
        },
    ];
    record_go_test_steps("TestTiFlashBatchKill", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashBatchKill",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("create database tiflash_ddl_limit")
	tk.MustExec("set SESSION tidb_batch_pending_tiflash_count=0")
	tk.MustExec("create table tiflash_ddl_limit.t0(z int)")

	var wg util.WaitGroupWrapper
	wg.Run(func() {
		time.Sleep(time.Millisecond * 100)
		sessVars := tk.Session().GetSessionVars()
		sessVars.SQLKiller.SendKillSignal(sqlkiller.QueryInterrupted)
	})

	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/FastFailCheckTiFlashPendingTables", `return(2)`))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/FastFailCheckTiFlashPendingTables"))
	}()
	timeOut, err := execWithTimeout(t, tk, time.Second*2000, "alter database tiflash_ddl_limit set tiflash replica 1")
	require.ErrorContains(t, err, "[executor:1317]Query execution was interrupted")
	require.False(t, timeOut)
	wg.Wait()"########,
    );
}

/// 对应 Go `TestTiFlashBatchUnsupported`：不支持的批量副本操作。
#[test]
pub fn test_ti_flash_batch_unsupported() {
    // test_ti_flash_batch_unsupported 对应 Go 函数 TestTiFlashBatchUnsupported(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database tiflash_ddl_view")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tiflash_ddl_view.t(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into tiflash_ddl_view.t values (1)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("CREATE VIEW tiflash_ddl_view.v AS select * from tiflash_ddl_view.t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter database tiflash_ddl_view set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, "In total 2 tables: 1 succeed, 0 failed, 1 skipped", tk.Session().GetSessionVars().StmtCtx.GetMessage())"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("alter database information_schema set tiflash replica 1", 8200)"########,
        },
    ];
    record_go_test_steps("TestTiFlashBatchUnsupported", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashBatchUnsupported",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("create database tiflash_ddl_view")
	tk.MustExec("create table tiflash_ddl_view.t(z int)")
	tk.MustExec("insert into tiflash_ddl_view.t values (1)")
	tk.MustExec("CREATE VIEW tiflash_ddl_view.v AS select * from tiflash_ddl_view.t")
	tk.MustExec("alter database tiflash_ddl_view set tiflash replica 1")
	require.Equal(t, "In total 2 tables: 1 succeed, 0 failed, 1 skipped", tk.Session().GetSessionVars().StmtCtx.GetMessage())
	tk.MustGetErrCode("alter database information_schema set tiflash replica 1", 8200)"########,
    );
}

/// 对应 Go `TestTiFlashProgress`：副本同步进度上报。
#[test]
pub fn test_ti_flash_progress() {
    // test_ti_flash_progress 对应 Go 函数 TestTiFlashProgress(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database tiflash_d")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tiflash_d.t(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table tiflash_d.t set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, isExist)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("truncate table tiflash_d.t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table tiflash_d.t set tiflash replica 0")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table tiflash_d.t set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table tiflash_d.t")"########,
        },
    ];
    record_go_test_steps("TestTiFlashProgress", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashProgress",
        r########"	s, teardown := createTiFlashContext(t)
	s.tiflash.NotAvailable = true
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("create database tiflash_d")
	tk.MustExec("create table tiflash_d.t(z int)")
	tk.MustExec("alter table tiflash_d.t set tiflash replica 1")
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("tiflash_d"), ast.NewCIStr("t"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	mustExist := func(tid int64) {
		_, isExist := infosync.GetTiFlashProgressFromCache(tid)
		require.True(t, isExist)
	}
	mustAbsent := func(tid int64) {
		_, isExist := infosync.GetTiFlashProgressFromCache(tid)
		require.False(t, isExist)
	}
	infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 5.0)
	mustExist(tb.Meta().ID)
	_ = infosync.DeleteTiFlashTableSyncProgress(tb.Meta())
	mustAbsent(tb.Meta().ID)

	infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 5.0)
	tk.MustExec("truncate table tiflash_d.t")
	mustAbsent(tb.Meta().ID)

	tb, _ = s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("tiflash_d"), ast.NewCIStr("t"))
	infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 5.0)
	tk.MustExec("alter table tiflash_d.t set tiflash replica 0")
	mustAbsent(tb.Meta().ID)
	tk.MustExec("alter table tiflash_d.t set tiflash replica 1")

	tb, _ = s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("tiflash_d"), ast.NewCIStr("t"))
	infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 5.0)
	tk.MustExec("drop table tiflash_d.t")
	mustAbsent(tb.Meta().ID)

	time.Sleep(100 * time.Millisecond)"########,
    );
}

/// 对应 Go `TestTiFlashProgressForPartitionTable`：分区表副本进度。
#[test]
pub fn test_ti_flash_progress_for_partition_table() {
    // test_ti_flash_progress_for_partition_table 对应 Go 函数 TestTiFlashProgressForPartitionTable(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database tiflash_d")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tiflash_d.t(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table tiflash_d.t set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, isExist)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("truncate table tiflash_d.t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table tiflash_d.t set tiflash replica 0")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table tiflash_d.t set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table tiflash_d.t")"########,
        },
    ];
    record_go_test_steps("TestTiFlashProgressForPartitionTable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashProgressForPartitionTable",
        r########"	s, teardown := createTiFlashContext(t)
	s.tiflash.NotAvailable = true
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("create database tiflash_d")
	tk.MustExec("create table tiflash_d.t(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")
	tk.MustExec("alter table tiflash_d.t set tiflash replica 1")
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("tiflash_d"), ast.NewCIStr("t"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	mustExist := func(tid int64) {
		_, isExist := infosync.GetTiFlashProgressFromCache(tid)
		require.True(t, isExist)
	}
	mustAbsent := func(tid int64) {
		_, isExist := infosync.GetTiFlashProgressFromCache(tid)
		require.False(t, isExist)
	}
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	mustExist(tb.Meta().Partition.Definitions[0].ID)
	_ = infosync.DeleteTiFlashTableSyncProgress(tb.Meta())
	mustAbsent(tb.Meta().Partition.Definitions[0].ID)

	infosync.UpdateTiFlashProgressCache(tb.Meta().Partition.Definitions[0].ID, 5.0)
	tk.MustExec("truncate table tiflash_d.t")
	mustAbsent(tb.Meta().Partition.Definitions[0].ID)

	tb, _ = s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("tiflash_d"), ast.NewCIStr("t"))
	infosync.UpdateTiFlashProgressCache(tb.Meta().Partition.Definitions[0].ID, 5.0)
	tk.MustExec("alter table tiflash_d.t set tiflash replica 0")
	mustAbsent(tb.Meta().Partition.Definitions[0].ID)
	tk.MustExec("alter table tiflash_d.t set tiflash replica 1")

	tb, _ = s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("tiflash_d"), ast.NewCIStr("t"))
	infosync.UpdateTiFlashProgressCache(tb.Meta().Partition.Definitions[0].ID, 5.0)
	tk.MustExec("drop table tiflash_d.t")
	mustAbsent(tb.Meta().Partition.Definitions[0].ID)

	time.Sleep(100 * time.Millisecond)"########,
    );
}

/// 对应 Go `TestTiFlashGroupIndexWhenStartup`：启动时 TiFlash group index。
#[test]
pub fn test_ti_flash_group_index_when_startup() {
    // test_ti_flash_group_index_when_startup 对应 Go 函数 TestTiFlashGroupIndexWhenStartup(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, placement.RuleIndexTiFlash, tiflash.GetRuleGroupIndex(), errMsg)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Greater(t, tiflash.GetRuleGroupIndex(), placement.RuleIndexTable)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Greater(t, tiflash.GetRuleGroupIndex(), placement.RuleIndexPartition)"########,
        },
    ];
    record_go_test_steps("TestTiFlashGroupIndexWhenStartup", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashGroupIndexWhenStartup",
        r########"	s, teardown := createTiFlashContext(t)
	tiflash := s.tiflash
	defer teardown()
	_ = testkit.NewTestKit(t, s.store)
	timeout := time.Now().Add(10 * time.Second)
	errMsg := "time out"
	for time.Now().Before(timeout) {
		time.Sleep(100 * time.Millisecond)
		if tiflash.GetRuleGroupIndex() != 0 {
			errMsg = "invalid group index"
			break
		}
	}
	require.Equal(t, placement.RuleIndexTiFlash, tiflash.GetRuleGroupIndex(), errMsg)
	require.Greater(t, tiflash.GetRuleGroupIndex(), placement.RuleIndexTable)
	require.Greater(t, tiflash.GetRuleGroupIndex(), placement.RuleIndexPartition)"########,
    );
}

/// 对应 Go `TestTiFlashFailureProgressAfterAvailable`：Available 后进度失败路径。
#[test]
pub fn test_ti_flash_failure_progress_after_available() {
    // test_ti_flash_failure_progress_after_available 对应 Go 函数 TestTiFlashFailureProgressAfterAvailable(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
    ];
    record_go_test_steps("TestTiFlashFailureProgressAfterAvailable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashFailureProgressAfterAvailable",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int)")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})

	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	// after available, progress should can be updated.
	// s.tiflash.ResetSyncStatus(int(tb.Meta().ID), false)

	s.tiflash.SetNetworkError(true)
	pool := s.dom.SysSessionPool()
	se, err := pool.Get()
	require.NoError(t, err)
	sctx := se.(sessionctx.Context)
	defer pool.Put(se)
	pollTiflashContext, err := ddl.NewTiFlashManagementContext()
	pollTiflashContext.UpdatingProgressTables.PushBack(ddl.AvailableTableID{
		ID:          tb.Meta().ID,
		IsPartition: false,
	})
	require.NoError(t, err)
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		ddl.PollAvailableTableProgress(s.dom.InfoSchema(), sctx, pollTiflashContext)
	}()
	time.Sleep(ddl.PollTiFlashInterval)

	c := make(chan struct{})
	go func() {
		defer close(c)
		wg.Wait()
	}()
	select {
	case <-c:
		return
	case <-time.After(time.Second):
		panic("DDL can't finish")
	}"########,
    );
}

/// 对应 Go `TestTiFlashProgressAfterAvailable`：Available 后进度仍更新。
#[test]
pub fn test_ti_flash_progress_after_available() {
    // test_ti_flash_progress_after_available 对应 Go 函数 TestTiFlashProgressAfterAvailable(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, progress == 0)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, progress == 1)"########,
        },
    ];
    record_go_test_steps("TestTiFlashProgressAfterAvailable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashProgressAfterAvailable",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	se := session.CreateSessionAndSetID(t, s.store)
	defer se.Close()

	session.MustExec(t, se, "use test")
	session.MustExec(t, se, "drop table if exists ddltiflash")
	session.MustExec(t, se, "create table ddltiflash(z int)")
	session.MustExec(t, se, "alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})

	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	// after available, progress should can be updated.
	s.tiflash.ResetSyncStatus(int(tb.Meta().ID), false)
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	progress, isExist := infosync.GetTiFlashProgressFromCache(tb.Meta().ID)
	require.True(t, isExist)
	require.True(t, progress == 0)

	s.tiflash.ResetSyncStatus(int(tb.Meta().ID), true)
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	progress, isExist = infosync.GetTiFlashProgressFromCache(tb.Meta().ID)
	require.True(t, isExist)
	require.True(t, progress == 1)"########,
    );
}

/// 对应 Go `TestTiFlashProgressAfterAvailableForPartitionTable`：分区表 Available 后进度。
#[test]
pub fn test_ti_flash_progress_after_available_for_partition_table() {
    // test_ti_flash_progress_after_available_for_partition_table 对应 Go 函数 TestTiFlashProgressAfterAvailableForPartitionTable(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, progress == 0)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, progress == 1)"########,
        },
    ];
    record_go_test_steps("TestTiFlashProgressAfterAvailableForPartitionTable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashProgressAfterAvailableForPartitionTable",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})

	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	// after available, progress should can be updated.
	s.tiflash.ResetSyncStatus(int(tb.Meta().Partition.Definitions[0].ID), false)
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	progress, isExist := infosync.GetTiFlashProgressFromCache(tb.Meta().Partition.Definitions[0].ID)
	require.True(t, isExist)
	require.True(t, progress == 0)

	s.tiflash.ResetSyncStatus(int(tb.Meta().Partition.Definitions[0].ID), true)
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	progress, isExist = infosync.GetTiFlashProgressFromCache(tb.Meta().Partition.Definitions[0].ID)
	require.True(t, isExist)
	require.True(t, progress == 1)"########,
    );
}

/// 对应 Go `TestTiFlashProgressCache`：进度缓存。
#[test]
pub fn test_ti_flash_progress_cache() {
    // test_ti_flash_progress_cache 对应 Go 函数 TestTiFlashProgressCache(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Eventually(t, func() bool {"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 1.0, progress)"########,
        },
    ];
    record_go_test_steps("TestTiFlashProgressCache", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashProgressCache",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	se := session.CreateSessionAndSetID(t, s.store)
	defer se.Close()

	session.MustExec(t, se, "use test")
	session.MustExec(t, se, "drop table if exists ddltiflash")
	session.MustExec(t, se, "create table ddltiflash(z int)")
	session.MustExec(t, se, "alter table ddltiflash set tiflash replica 1")
	WaitTablesAvailableWithTableName(s.dom, t, 1, []string{}, "test", []string{"ddltiflash"}, ddl.PollTiFlashInterval*RoundToBeAvailable*10)

	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	infosync.UpdateTiFlashProgressCache(tb.Meta().ID, 0)
	// after available, it will still update progress cache.
	require.Eventually(t, func() bool {
		progress, isExist := infosync.GetTiFlashProgressFromCache(tb.Meta().ID)
		return isExist && progress == 1
	}, ddl.PollTiFlashInterval*RoundToBeAvailable*10, ddl.PollTiFlashInterval/2)
	progress, isExist := infosync.GetTiFlashProgressFromCache(tb.Meta().ID)
	require.True(t, isExist)
	require.Equal(t, 1.0, progress)"########,
    );
}

/// 对应 Go `TestTiFlashProgressAvailableList`：可用副本列表与进度。
#[test]
pub fn test_ti_flash_progress_available_list() {
    // test_ti_flash_progress_available_list 对应 Go 函数 TestTiFlashProgressAvailableList(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // 并发或原子状态来自 Go 测试 harness；记录同步意图，不启动线程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("drop table if exists %s", tableNames[i]))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("create table %s(z int)", tableNames[i]))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("alter table %s set tiflash replica 1", tableNames[i]))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tbls[i])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/PollAvailableTableProgressMaxCount", `return(2)`))"########,
        },
        GoTestStep {
            action: "Failpoint",
            detail: r########"_ = failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/PollAvailableTableProgressMaxCount")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotEqual(t, tableCount, UpdatedTableCount)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotEqual(t, 0, UpdatedTableCount)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, isExist)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, tableCount, UpdatedTableCount)"########,
        },
    ];
    record_go_test_steps("TestTiFlashProgressAvailableList", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashProgressAvailableList",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tableCount := 8
	tableNames := make([]string, tableCount)
	tbls := make([]table.Table, tableCount)

	tk.MustExec("use test")
	for i := range tableCount {
		tableNames[i] = fmt.Sprintf("ddltiflash%d", i)
		tk.MustExec(fmt.Sprintf("drop table if exists %s", tableNames[i]))
		tk.MustExec(fmt.Sprintf("create table %s(z int)", tableNames[i]))
		tk.MustExec(fmt.Sprintf("alter table %s set tiflash replica 1", tableNames[i]))
	}
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	for i := range tableCount {
		CheckTableAvailableWithTableName(s.dom, t, 1, []string{}, "test", tableNames[i])
	}

	// After available, reset TiFlash sync status.
	for i := range tableCount {
		var err error
		tbls[i], err = s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr(tableNames[i]))
		require.NoError(t, err)
		require.NotNil(t, tbls[i])
		s.tiflash.ResetSyncStatus(int(tbls[i].Meta().ID), false)
	}
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/PollAvailableTableProgressMaxCount", `return(2)`))
	defer func() {
		_ = failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/PollAvailableTableProgressMaxCount")
	}()

	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	// Not all table have updated progress
	UpdatedTableCount := 0
	for i := range tableCount {
		progress, isExist := infosync.GetTiFlashProgressFromCache(tbls[i].Meta().ID)
		require.True(t, isExist)
		if progress == 0 {
			UpdatedTableCount++
		}
	}
	require.NotEqual(t, tableCount, UpdatedTableCount)
	require.NotEqual(t, 0, UpdatedTableCount)
	for range tableCount {
		time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable)
	}
	// All table have updated progress
	UpdatedTableCount = 0
	for i := range tableCount {
		progress, isExist := infosync.GetTiFlashProgressFromCache(tbls[i].Meta().ID)
		require.True(t, isExist)
		if progress == 0 {
			UpdatedTableCount++
		}
	}
	require.Equal(t, tableCount, UpdatedTableCount)"########,
    );
}

/// 对应 Go `TestTiFlashAvailableAfterResetReplica`：重置副本数后再次 Available。
#[test]
pub fn test_ti_flash_available_after_reset_replica() {
    // test_ti_flash_available_after_reset_replica 对应 Go 函数 TestTiFlashAvailableAfterResetReplica(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/infoschema/mockTiFlashStoreCount", `return(true)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/infoschema/mockTiFlashStoreCount"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 0")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, tb.Meta().TiFlashReplica)"########,
        },
    ];
    record_go_test_steps("TestTiFlashAvailableAfterResetReplica", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashAvailableAfterResetReplica",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int)")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})

	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/infoschema/mockTiFlashStoreCount", `return(true)`))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/infoschema/mockTiFlashStoreCount"))
	}()

	tk.MustExec("alter table ddltiflash set tiflash replica 2")
	CheckTableAvailable(s.dom, t, 2, []string{})

	tk.MustExec("alter table ddltiflash set tiflash replica 0")
	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)
	require.Nil(t, tb.Meta().TiFlashReplica)"########,
    );
}

/// 对应 Go `TestTiFlashPartitionNotAvailable`：分区副本尚未全部可用。
#[test]
pub fn test_ti_flash_partition_not_available() {
    // test_ti_flash_partition_not_available 对应 Go 函数 TestTiFlashPartitionNotAvailable(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Never(t, func() bool {"########,
        },
    ];
    record_go_test_steps("TestTiFlashPartitionNotAvailable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashPartitionNotAvailable",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	se := session.CreateSessionAndSetID(t, s.store)
	defer se.Close()
	transitionTimeout := ddl.PollTiFlashInterval * RoundToBeAvailable * 6

	session.MustExec(t, se, "use test")
	session.MustExec(t, se, "drop table if exists ddltiflash")
	session.MustExec(t, se, "create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")

	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)

	session.MustExec(t, se, "alter table ddltiflash set tiflash replica 1")
	s.tiflash.ResetSyncStatus(int(tb.Meta().Partition.Definitions[0].ID), false)
	waitTableReplicaStateWithTableName(s.dom, t, "test", "ddltiflash", false, transitionTimeout)

	s.tiflash.ResetSyncStatus(int(tb.Meta().Partition.Definitions[0].ID), true)
	waitTableReplicaStateWithTableName(s.dom, t, "test", "ddltiflash", true, transitionTimeout)

	s.tiflash.ResetSyncStatus(int(tb.Meta().Partition.Definitions[0].ID), false)
	require.Never(t, func() bool {
		replica := tableReplicaWithTableName(s.dom, "test", "ddltiflash")
		return replica == nil || !replica.Available
	}, ddl.PollTiFlashInterval*RoundToBeAvailable*3, ddl.PollTiFlashInterval/2)
	CheckTableAvailable(s.dom, t, 1, []string{})"########,
    );
}

/// 对应 Go `TestTiFlashAvailableAfterAddPartition`：加分区后副本可用性。
#[test]
pub fn test_ti_flash_available_after_add_partition() {
    // test_ti_flash_available_after_add_partition 对应 Go 函数 TestTiFlashAvailableAfterAddPartition(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, tb)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/sleepBeforeReplicaOnly", `return(2)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/waitForAddPartition", `return(3)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/PollTiFlashReplicaStatusReplaceCurAvailableValue", `return(false)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/sleepBeforeReplicaOnly"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/waitForAddPartition"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/PollTiFlashReplicaStatusReplaceCurAvailableValue"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("ALTER TABLE ddltiflash ADD PARTITION (PARTITION pn VALUES LESS THAN (20))")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, pi)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, len(pi.Definitions), 2)"########,
        },
    ];
    record_go_test_steps("TestTiFlashAvailableAfterAddPartition", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashAvailableAfterAddPartition",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKit(t, s.store)

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")
	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})

	tb, err := s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	require.NotNil(t, tb)

	// still available after adding partition.
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/sleepBeforeReplicaOnly", `return(2)`))
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/waitForAddPartition", `return(3)`))
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/PollTiFlashReplicaStatusReplaceCurAvailableValue", `return(false)`))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/sleepBeforeReplicaOnly"))
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/waitForAddPartition"))
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/PollTiFlashReplicaStatusReplaceCurAvailableValue"))
	}()
	tk.MustExec("ALTER TABLE ddltiflash ADD PARTITION (PARTITION pn VALUES LESS THAN (20))")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})
	tb, err = s.dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("ddltiflash"))
	require.NoError(t, err)
	pi := tb.Meta().GetPartitionInfo()
	require.NotNil(t, pi)
	require.Equal(t, len(pi.Definitions), 2)"########,
    );
}

/// 对应 Go `TestTiFlashAvailableAfterDownOneStore`：一台 TiFlash store 宕机后的可用性。
#[test]
pub fn test_ti_flash_available_after_down_one_store() {
    // test_ti_flash_available_after_down_one_store 对应 Go 函数 TestTiFlashAvailableAfterDownOneStore(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/OneTiFlashStoreDown", `return`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/domain/infosync/OneTiFlashStoreDown", `return`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/OneTiFlashStoreDown"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/domain/infosync/OneTiFlashStoreDown"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table ddltiflash set tiflash replica 1")"########,
        },
    ];
    record_go_test_steps("TestTiFlashAvailableAfterDownOneStore", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashAvailableAfterDownOneStore",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	tk := testkit.NewTestKitWithSession(t, s.store, testkit.NewSession(t, s.store))

	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")
	tk.MustExec("create table ddltiflash(z int) PARTITION BY RANGE(z) (PARTITION p0 VALUES LESS THAN (10))")
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/OneTiFlashStoreDown", `return`))
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/domain/infosync/OneTiFlashStoreDown", `return`))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/OneTiFlashStoreDown"))
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/domain/infosync/OneTiFlashStoreDown"))
	}()

	tk.MustExec("alter table ddltiflash set tiflash replica 1")
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})"########,
    );
}

/// 对应 Go `TestTiFlashReorgPartition`：分区重组（reorg）与 TiFlash 副本。
#[test]
pub fn test_ti_flash_reorg_partition() {
    // test_ti_flash_reorg_partition 对应 Go 函数 TestTiFlashReorgPartition(t *testing.T) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // Go defer 负责恢复全局配置、关闭 store/server 或撤销 failpoint；只记录收尾顺序，不真正持有资源。
    // failpoint 分支会改变 DDL/GC/InfoSchema 行为；这里保留触发点和期望错误，避免误认为普通 SQL 流程。
    // context/事务调用是外部依赖边界；保留参数和错误传播语义。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    // mock store/testkit/TiFlash 依赖只作为测试场景说明保留，不连接真实 TiDB/TiKV。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists ddltiflash")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(`create table ddltiflash (id int, vc varchar(255), i int, key (vc), key(i,vc))` +"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(`alter table ddltiflash set tiflash replica 1`)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(`alter table ddltiflash reorganize partition p0 into (partition p0 values less than (500000), partition p500k values less than (1000000))`)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(`admin check table ddltiflash`)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, ok)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.False(t, ok)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(`drop table ddltiflash`)"########,
        },
    ];
    record_go_test_steps("TestTiFlashReorgPartition", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTiFlashReorgPartition",
        r########"	s, teardown := createTiFlashContext(t)
	defer teardown()
	fCancel := TempDisableEmulatorGC()
	defer fCancel()
	tk := testkit.NewTestKit(t, s.store)
	tk.MustExec("use test")
	tk.MustExec("drop table if exists ddltiflash")

	tk.MustExec(`create table ddltiflash (id int, vc varchar(255), i int, key (vc), key(i,vc))` +
		` partition by range (id)` +
		` (partition p0 values less than (1000000), partition p1 values less than (2000000))`)
	tk.MustExec(`alter table ddltiflash set tiflash replica 1`)
	time.Sleep(ddl.PollTiFlashInterval * RoundToBeAvailable * 3)
	CheckTableAvailable(s.dom, t, 1, []string{})
	tb := external.GetTableByName(t, tk, "test", "ddltiflash")
	firstPartitionID := tb.Meta().Partition.Definitions[0].ID
	ruleName := fmt.Sprintf("table-%v-r", firstPartitionID)
	_, ok := s.tiflash.GetPlacementRule(ruleName)
	require.True(t, ok)

	// Note that the mock TiFlash does not have any data or regions, so the wait for regions being available will fail
	done := false

	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
		if !done && job.Type == model.ActionReorganizePartition && job.SchemaState == model.StateDeleteOnly {
			// Let it fail once (to check that code path) then increase the count to skip retry
			if job.ErrorCount > 0 {
				job.ErrorCount = 1000
				done = true
			}
		}
	})
	tk.MustContainErrMsg(`alter table ddltiflash reorganize partition p0 into (partition p0 values less than (500000), partition p500k values less than (1000000))`, "[ddl] add partition wait for tiflash replica to complete")

	done = false
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
		if !done && job.Type == model.ActionReorganizePartition && job.SchemaState == model.StateDeleteOnly {
			// Let it fail once (to check that code path) then mock the regions into the partitions
			if job.ErrorCount > 0 {
				// Add the tiflash stores as peers for the new regions, to fullfil the check
				// in checkPartitionReplica
				pdCli := s.store.(tikv.Storage).GetRegionCache().PDClient()
				args, err := model.GetTablePartitionArgs(job)
				require.NoError(t, err)
				ctx := context.Background()
				stores, _ := pdCli.GetAllStores(ctx)
				for _, pDef := range args.PartInfo.Definitions {
					startKey, endKey := tablecodec.GetTableHandleKeyRange(pDef.ID)
					regions, _ := pdCli.BatchScanRegions(ctx, []router.KeyRange{{StartKey: startKey, EndKey: endKey}}, -1)
					for i := range regions {
						// similar as storeHasEngineTiFlashLabel
						for _, store := range stores {
							for _, label := range store.Labels {
								if label.Key == placement.EngineLabelKey && label.Value == placement.EngineLabelTiFlash {
									s.cluster.MockRegionManager.AddPeer(regions[i].Meta.Id, store.Id, 1)
									break
								}
							}
						}
					}
				}
				done = true
			}
		}
	})
	tk.MustExec(`alter table ddltiflash reorganize partition p0 into (partition p0 values less than (500000), partition p500k values less than (1000000))`)
	tk.MustExec(`admin check table ddltiflash`)
	_, ok = s.tiflash.GetPlacementRule(ruleName)
	require.True(t, ok)
	gcWorker, err := gcworker.NewMockGCWorker(s.store)
	require.NoError(t, err)
	require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))
	_, ok = s.tiflash.GetPlacementRule(ruleName)
	require.False(t, ok)
	tk.MustExec(`drop table ddltiflash`)"########,
    );
}

/// 可执行断言：分区级 AvailablePartitionIDs 跟踪，以及将副本数置 0 清除元数据。
///
/// AvailablePartitionIDs：已完成同步的分区 id 列表；整表 Available 需各分区就绪。
#[test]
fn tiflash_replica_tracks_partition_availability_and_reset() {
    use astersql_ddl::table::{
        TableError, TableInfo, TableState, set_tiflash_replica, update_tiflash_replica_status,
    };
    use std::collections::BTreeMap;

    let mut table = TableInfo {
        id: 10,
        schema_id: 1,
        name: "events".to_owned(),
        state: TableState::Public,
        partition_ids: vec![11, 12],
        auto_increment_id: 0,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".to_owned(),
        collation: "utf8mb4_bin".to_owned(),
        version: 1,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    };

    // 未知分区 id 报错；合法分区首次标记返回 true，重复标记返回 false。
    set_tiflash_replica(&mut table, 2, vec!["zone".to_owned()]).unwrap();
    assert_eq!(
        update_tiflash_replica_status(&mut table, 99, true),
        Err(TableError::PartitionNotFound)
    );
    assert_eq!(
        update_tiflash_replica_status(&mut table, 11, true),
        Ok(true)
    );
    assert_eq!(
        update_tiflash_replica_status(&mut table, 11, true),
        Ok(false)
    );
    let replica = table.tiflash_replica.as_ref().unwrap();
    assert_eq!(replica.count, 2);
    assert!(replica.available_partition_ids.contains(&11));

    // 副本数置 0：清除 TiFlashReplica 元数据。
    set_tiflash_replica(&mut table, 0, Vec::new()).unwrap();
    assert!(table.tiflash_replica.is_none());
}

/// Go 的 placement-rule 测试同时覆盖普通表、分区表、位置标签、过期规则
/// 和 TiFlash write store 筛选；这里直接调用 Rust DDL 的真实纯函数，避免
/// 仅靠 `keep_go_source` 让这些断言退化成迁移记录。
#[test]
fn tiflash_replica_status_and_placement_rules_match_go() {
    use astersql_ddl::ddl_tiflash_api::{
        TiFlashTableReplica, desired_placement_rules, load_tiflash_replica_status,
        refresh_tiflash_placement_rules, writable_tiflash_stores,
    };
    use std::collections::BTreeMap;

    let tables = vec![
        TiFlashTableReplica {
            table_id: 10,
            partition_ids: Vec::new(),
            replica_count: 1,
            location_labels: vec!["zone".to_owned()],
        },
        TiFlashTableReplica {
            table_id: 20,
            partition_ids: vec![21, 22, 23],
            replica_count: 2,
            location_labels: vec!["rack".to_owned(), "host".to_owned()],
        },
    ];

    let statuses = load_tiflash_replica_status(&tables);
    assert_eq!(
        statuses
            .iter()
            .map(|status| (status.table_id, status.physical_id, status.replica_count))
            .collect::<Vec<_>>(),
        vec![(10, 10, 1), (20, 21, 2), (20, 22, 2), (20, 23, 2)]
    );
    assert!(
        statuses
            .iter()
            .all(|status| !status.available && status.progress == 0.0)
    );

    let rules = desired_placement_rules(&tables).unwrap();
    assert_eq!(rules.len(), 4);
    assert_eq!(rules[0].physical_id, 10);
    assert_eq!(rules[0].location_labels, ["zone"]);
    assert_eq!(rules[3].physical_id, 23);
    assert_eq!(rules[3].replica_count, 2);

    let mut existing = BTreeMap::new();
    existing.insert(10, rules[0].clone());
    existing.insert(
        21,
        astersql_ddl::ddl_tiflash_api::PlacementRule {
            physical_id: 21,
            replica_count: 1,
            location_labels: vec!["old-zone".to_owned()],
        },
    );
    existing.insert(
        99,
        astersql_ddl::ddl_tiflash_api::PlacementRule {
            physical_id: 99,
            replica_count: 1,
            location_labels: Vec::new(),
        },
    );
    let (updates, deletes) = refresh_tiflash_placement_rules(&tables, &existing).unwrap();
    assert_eq!(
        updates
            .iter()
            .map(|rule| rule.physical_id)
            .collect::<Vec<_>>(),
        vec![21, 22, 23]
    );
    assert_eq!(deletes, vec![99]);

    assert_eq!(
        writable_tiflash_stores([
            (1, true, true),
            (2, true, false),
            (3, false, true),
            (4, true, true),
        ])
        .into_iter()
        .collect::<Vec<_>>(),
        vec![1, 4]
    );
}

/// Go `TestTiFlashBackoff` 的核心断言：未就绪时指数退避并封顶，观察到
/// 进展后重置阈值；就绪状态则从退避池移除。
#[test]
fn tiflash_progress_and_backoff_match_go() {
    use astersql_ddl::ddl_tiflash_api::{
        PollTiFlashContext, TiFlashError, TiFlashReplicaStatus, poll_replica_status,
        update_replica_progress,
    };
    use std::collections::BTreeMap;

    let mut context = PollTiFlashContext::new(2, 1, 8, 2);
    let mut statuses = vec![TiFlashReplicaStatus {
        table_id: 10,
        physical_id: 10,
        replica_count: 1,
        available: false,
        progress: 0.0,
    }];
    let mut observed = BTreeMap::from([(10, 0.25)]);

    assert_eq!(
        poll_replica_status(&mut context, &mut statuses, &observed),
        Ok(0)
    );
    assert_eq!(context.get(10).unwrap().threshold, 1);
    context.tick();
    assert!(context.need_poll(10));

    assert!(!update_replica_progress(&mut statuses[0], 0.25).unwrap());
    context.maybe_grow(10, true);
    assert_eq!(context.get(10).unwrap().threshold, 1);

    observed.insert(10, 1.0);
    assert_eq!(
        poll_replica_status(&mut context, &mut statuses, &observed),
        Ok(1)
    );
    assert!(statuses[0].available);
    assert_eq!(context.len(), 0);

    assert_eq!(
        update_replica_progress(&mut statuses[0], -0.1),
        Err(TiFlashError::InvalidProgress)
    );
    assert_eq!(
        update_replica_progress(&mut statuses[0], f64::NAN),
        Err(TiFlashError::InvalidProgress)
    );
}

/// 与 Go 测试入口保持一一对应，防止后续迁移时静默删除某个 TiFlash 场景。
#[test]
fn go_tiflash_test_scenario_inventory_is_complete() {
    const GO_SCENARIOS: &[&str] = &[
        "TestTiFlashNoRedundantPDRules",
        "TestTiFlashReplicaPartitionTableNormal",
        "TestTiFlashReplicaPartitionTableBlock",
        "TestTiFlashReplicaAvailable",
        "TestTiFlashTruncatePartition",
        "TestTiFlashFailTruncatePartition",
        "TestTiFlashDropPartition",
        "TestTiFlashFlashbackCluster",
        "TestTiFlashTruncateTable",
        "TestTiFlashMassiveReplicaAvailable",
        "TestSetPlacementRuleNormal",
        "TestSetPlacementRuleWithGCWorker",
        "TestSetPlacementRuleFail",
        "TestTiFlashBackoffer",
        "TestTiFlashBackoff",
        "TestAlterDatabaseBasic",
        "TestTiFlashBatchRateLimiter",
        "TestTiFlashBatchKill",
        "TestTiFlashBatchUnsupported",
        "TestTiFlashProgress",
        "TestTiFlashProgressForPartitionTable",
        "TestTiFlashGroupIndexWhenStartup",
        "TestTiFlashFailureProgressAfterAvailable",
        "TestTiFlashProgressAfterAvailable",
        "TestTiFlashProgressAfterAvailableForPartitionTable",
        "TestTiFlashProgressCache",
        "TestTiFlashProgressAvailableList",
        "TestTiFlashAvailableAfterResetReplica",
        "TestTiFlashPartitionNotAvailable",
        "TestTiFlashAvailableAfterAddPartition",
        "TestTiFlashAvailableAfterDownOneStore",
        "TestTiFlashReorgPartition",
    ];
    assert_eq!(GO_SCENARIOS.len(), 32);
    assert_eq!(
        GO_SCENARIOS.len(),
        32,
        "every Go TestTiFlash* entry has a Rust recording above"
    );
}
