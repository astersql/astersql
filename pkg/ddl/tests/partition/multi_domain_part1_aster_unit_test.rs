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

// 多 Domain 分区 DDL 可见性测试（第 1 部分）。
//
// Domain 是绑定同一 store 的 schema/统计信息域。多个 `TestKit` 共享 store 时，
// 一方提交的分区 DDL/DML 应反映到另一方的 catalog 与 pending stats delta。
// 同时覆盖分区 SchemaState（WriteOnly / DeleteOnly / Public 等）下
// `IDsInDDLToIgnore` 的可见性集合。

use astersql_meta_model::{
    ActionAddTablePartition, ActionDropTablePartition, ActionTruncateTablePartition,
    PartitionDefinition, PartitionInfo, StateDeleteOnly, StateDeleteReorganization, StatePublic,
    StateWriteOnly,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use std::thread;

/// 构造仅含 ID/名称的分区定义，供状态机用例复用。
fn definition(id: i64, name: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        ..Default::default()
    }
}

/// 验证 owner 建表/插入后，observer 共享 Domain 的 catalog 与 pending stats 一致。
#[test]
fn clients_share_real_partition_ddl_catalog_updates() {
    let store = CreateAnalyzeStatsStore();
    let mut owner = TestKit::new(store.clone());
    let observer = TestKit::new(store);
    owner.MustExec(
        "create table multi_t(a int) partition by hash(a) partitions 2",
        Vec::new(),
    );
    owner.MustExec("insert into multi_t values (1),(2),(3)", Vec::new());
    let context = observer.AnalyzeStatsContext().expect("shared domain");
    let catalog = context.catalog();
    let partition = catalog
        .get(&("test".to_owned(), "multi_t".to_owned()))
        .unwrap()
        .1
        .GetPartitionInfo()
        .unwrap();
    // 每个发生写入的物理分区都应出现在 pending_stats_delta_ids 中。
    let mut partition_ids = partition
        .Definitions
        .iter()
        .map(|definition| definition.ID)
        .collect::<Vec<_>>();
    let mut pending_ids = context.pending_stats_delta_ids();
    partition_ids.sort_unstable();
    pending_ids.sort_unstable();
    assert_eq!(pending_ids, partition_ids);
    assert_eq!(partition.Definitions.len(), 2);
}

/// 验证并发读线程能观察到已提交分区行对应的物理 ID 与 catalog 版本。
#[test]
fn concurrent_read_observes_committed_partition_rows() {
    let store = CreateAnalyzeStatsStore();
    let mut writer = TestKit::new(store.clone());
    writer.MustExec(
        "create table concurrent_t(a int) partition by hash(a) partitions 2",
        Vec::new(),
    );
    writer.MustExec("insert into concurrent_t values (7),(8)", Vec::new());
    // 独立线程打开共享 store 的 reader Domain，核对 pending 与分区 ID。
    let reader = thread::spawn(move || {
        let testkit = TestKit::new(store);
        let context = testkit.AnalyzeStatsContext().expect("reader domain");
        let catalog = context.catalog();
        let partition_ids = catalog
            .get(&("test".to_owned(), "concurrent_t".to_owned()))
            .expect("shared table")
            .1
            .GetPartitionInfo()
            .unwrap()
            .Definitions
            .iter()
            .map(|definition| definition.ID)
            .collect::<Vec<_>>();
        (
            context.pending_stats_delta_ids(),
            partition_ids,
            context.catalog_version(),
        )
    });
    let (mut pending_ids, mut partition_ids, catalog_version) =
        reader.join().expect("reader domain");
    pending_ids.sort_unstable();
    partition_ids.sort_unstable();
    assert_eq!(pending_ids, partition_ids);
    assert!(catalog_version > 0);
}

/// 验证 add/drop/truncate 分区在不同 SchemaState 下 `IDsInDDLToIgnore` 的集合。
///
/// SchemaState 是 DDL Job 推进中元数据对象所处阶段；忽略集合用于查询侧跳过
/// 尚不可见或正在删除的物理分区 ID。
#[test]
fn partition_state_machine_exposes_go_visibility_sets() {
    let old = definition(1, "p0");
    let new = definition(2, "p1");
    let mut partition = PartitionInfo {
        Definitions: vec![old.clone(), new.clone()],
        AddingDefinitions: vec![new.clone()],
        DroppingDefinitions: vec![old.clone()],
        NewPartitionIDs: vec![2],
        ..Default::default()
    };

    partition.DDLAction = ActionAddTablePartition;
    partition.DDLState = StateWriteOnly;
    assert_eq!(partition.IDsInDDLToIgnore(), vec![2]);

    partition.DDLAction = ActionDropTablePartition;
    assert_eq!(partition.IDsInDDLToIgnore(), vec![1]);
    assert!(partition.CanHaveOverlappingDroppingPartition());

    partition.DDLAction = ActionTruncateTablePartition;
    assert_eq!(partition.IDsInDDLToIgnore(), vec![2]);
    partition.DDLState = StateDeleteOnly;
    assert_eq!(partition.IDsInDDLToIgnore(), vec![1]);
    partition.DDLState = StateDeleteReorganization;
    assert_eq!(partition.IDsInDDLToIgnore(), vec![1]);

    // 按物理 ID 设置/覆盖读取分区状态，覆盖 Public 收尾路径且不追加重复条目。
    partition.SetStateByID(1, StateWriteOnly);
    assert_eq!(partition.GetStateByID(1), StateWriteOnly);
    partition.SetStateByID(1, StatePublic);
    assert_eq!(partition.GetStateByID(1), StatePublic);
    assert_eq!(partition.States.len(), 1);
}
