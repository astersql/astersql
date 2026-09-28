// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DDL job comments 单元测试：核对 analyze / reorg / DXF / cloud 注释语义。

use crate::show_ddl_jobs::{
    AnalyzeState, DDLAction, DDLJob, DDLRuntimeConfig, ReorgMeta, ReorgType, SubJob,
    showCommentsFromJob, showCommentsFromSubjob,
};

fn runtime_config(next_gen: bool) -> DDLRuntimeConfig {
    DDLRuntimeConfig {
        next_gen,
        default_reorg_worker_count: 4,
        default_reorg_batch_size: 256,
        default_reorg_max_write_speed: 0,
    }
}

fn add_index_job() -> DDLJob {
    DDLJob {
        id: 0,
        schema_name: String::new(),
        table_name: String::new(),
        schema_id: 0,
        table_id: 0,
        row_count: 0,
        start_ts: 0,
        real_start_ts: 0,
        action: DDLAction {
            name: "add index".into(),
            is_add_index: true,
            ..DDLAction::default()
        },
        schema_state: String::new(),
        state: String::new(),
        query: String::new(),
        binlog_info: None,
        rename_new_table_name: None,
        reorg_meta: None,
        sub_jobs: Vec::new(),
        may_need_reorg: true,
    }
}

fn reorg_meta(reorg_type: ReorgType) -> ReorgMeta {
    ReorgMeta {
        analyze_state: AnalyzeState::None,
        reorg_type,
        is_dist_reorg: false,
        use_cloud_storage: false,
        concurrency: 4,
        batch_size: 256,
        max_write_speed: 0,
        target_scope: String::new(),
        max_node_count: 0,
    }
}

/// Port of Go TestShowCommentsFromJob: preserve labels, ordering, and default suppression.
#[test]
fn ddl_job_comments_match_go_reorg_label_matrix() {
    let config = runtime_config(false);
    let mut job = add_index_job();
    assert_eq!(showCommentsFromJob(&job, &config), "");

    job.reorg_meta = Some(reorg_meta(ReorgType::Txn("txn".into())));
    assert_eq!(showCommentsFromJob(&job, &config), "txn");

    job.reorg_meta.as_mut().unwrap().is_dist_reorg = true;
    assert_eq!(showCommentsFromJob(&job, &config), "txn");

    job.reorg_meta.as_mut().unwrap().reorg_type = ReorgType::TxnMerge("txn-merge".into());
    assert_eq!(showCommentsFromJob(&job, &config), "txn-merge");

    let meta = job.reorg_meta.as_mut().unwrap();
    meta.reorg_type = ReorgType::Ingest("ingest".into());
    assert_eq!(showCommentsFromJob(&job, &config), "ingest, DXF");

    job.reorg_meta.as_mut().unwrap().use_cloud_storage = true;
    assert_eq!(showCommentsFromJob(&job, &config), "ingest, DXF, cloud");

    job.reorg_meta.as_mut().unwrap().max_node_count = 5;
    assert_eq!(
        showCommentsFromJob(&job, &config),
        "ingest, DXF, cloud, max_node_count=5"
    );

    let meta = job.reorg_meta.as_mut().unwrap();
    meta.max_node_count = 0;
    meta.concurrency = 8;
    meta.batch_size = 1024;
    meta.max_write_speed = 1024 * 1024;
    assert_eq!(
        showCommentsFromJob(&job, &config),
        "ingest, DXF, cloud, thread=8, batch_size=1024, max_write_speed=1048576"
    );

    let meta = job.reorg_meta.as_mut().unwrap();
    meta.concurrency = config.default_reorg_worker_count;
    meta.batch_size = config.default_reorg_batch_size;
    meta.max_write_speed = config.default_reorg_max_write_speed;
    assert_eq!(showCommentsFromJob(&job, &config), "ingest, DXF, cloud");

    job.reorg_meta.as_mut().unwrap().target_scope = "background".into();
    assert_eq!(
        showCommentsFromJob(&job, &config),
        "ingest, DXF, cloud, service_scope=background"
    );
}

#[test]
fn ddl_job_comments_preserve_analyze_and_next_gen_semantics() {
    let mut job = add_index_job();
    let mut meta = reorg_meta(ReorgType::Ingest("ingest".into()));
    meta.analyze_state = AnalyzeState::Running;
    meta.is_dist_reorg = true;
    meta.use_cloud_storage = true;
    job.reorg_meta = Some(meta);

    assert_eq!(
        showCommentsFromJob(&job, &runtime_config(false)),
        "analyzing, ingest, DXF, cloud"
    );
    assert_eq!(
        showCommentsFromJob(&job, &runtime_config(true)),
        "analyzing"
    );
}

/// 验证子作业注释在 DXF+cloud、仅 ingest、以及 next_gen 关闭注释三种场景下的文案。
#[test]
fn ddl_subjob_comments_preserve_ingest_dxf_cloud_semantics() {
    let sub_job = SubJob {
        action: DDLAction {
            name: "add index".into(),
            is_add_index: true,
            ..DDLAction::default()
        },
        schema_state: "write reorganization".into(),
        row_count: 42,
        real_start_ts: 10,
        state: "running".into(),
        // Ingest：索引回填走外部摄取路径（相对事务内回填）。
        reorg_type: ReorgType::Ingest("ingest".into()),
    };
    let config = runtime_config(false);
    let mut no_reorg = sub_job.clone();
    no_reorg.reorg_type = ReorgType::None;
    assert_eq!(showCommentsFromSubjob(&no_reorg, false, false, &config), "");
    assert_eq!(
        showCommentsFromSubjob(&sub_job, true, false, &config),
        "ingest, DXF"
    );
    assert_eq!(
        showCommentsFromSubjob(&sub_job, true, true, &config),
        "ingest, DXF, cloud"
    );
    assert_eq!(
        showCommentsFromSubjob(&sub_job, false, true, &config),
        "ingest"
    );
    assert_eq!(
        showCommentsFromSubjob(
            &sub_job,
            true,
            true,
            &DDLRuntimeConfig {
                // next_gen 架构下子作业不再展示这些运维标签。
                next_gen: true,
                ..config
            }
        ),
        ""
    );
}
