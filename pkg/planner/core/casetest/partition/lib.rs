// Copyright 2026 AsterSQL.

// `partition` casetest crate 入口。
//
// 聚合分区表（Partition：按表达式将表数据切分为多个物理分区）相关用例：
// list/range/hash 集成裁剪、分区裁剪器、TestMain 初始化。仅在 `cfg(test)` 下挂载子模块。

#![allow(dead_code)]

#[cfg(test)]
mod support {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreatePlannerTestStoreAndDomain;
    use astersql_testkit_testsetup::SetupForCommonTest;

    /// Create the same real mock-backed SQL session used by the Go TestKit.
    pub fn new_testkit() -> TestKit {
        SetupForCommonTest();
        let (store, _domain) = CreatePlannerTestStoreAndDomain();
        let mut test_kit = TestKit::new(store);
        // The planner fixture skips unrelated statistics bootstrap. Partition
        // casetests still run Go-aligned ANALYZE statements, so keep their real
        // option-persistence boundary available.
        test_kit.MustExec(
            "create table if not exists mysql.analyze_options (table_id bigint(64) not null,sample_num bigint(64) not null default 0,sample_rate double not null default -1,buckets bigint(64) not null default 0,topn bigint(64) not null default -1,column_choice enum('DEFAULT','ALL','PREDICATE','LIST') not null default 'DEFAULT',column_ids text(19372),primary key (table_id) clustered)",
            Vec::new(),
        );
        test_kit
    }

    /// Run a casetest once for each planner mode represented by Go's harness.
    pub fn run_test_under_cascades<F>(mut test: F)
    where
        F: FnMut(&mut TestKit, &str, &str),
    {
        for (cascades, caller) in [("off", "classic"), ("on", "cascades")] {
            let mut test_kit = new_testkit();
            test_kit.MustExec(
                &format!(
                    "set @@tidb_enable_cascades_planner = {}",
                    if cascades == "on" { "on" } else { "off" }
                ),
                Vec::new(),
            );
            test(&mut test_kit, cascades, caller);
        }
    }
}

/// list/range/hash 分区集成裁剪与 AccessObject / 谓词化简用例。
#[cfg(test)]
#[path = "integration_partition_test.rs"]
mod integration_partition_test;
/// LIST 分区集成测试。
#[cfg(test)]
#[path = "list_partition_integration_test.rs"]
mod list_partition_integration_test;
/// 对应 Go TestMain 的初始化语义。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// 分区裁剪器（partition pruner）专项测试。
#[cfg(test)]
#[path = "partition_pruner_test.rs"]
mod partition_pruner_test;
