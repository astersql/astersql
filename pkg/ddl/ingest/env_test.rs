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

// ingest 环境（`env`）模块的单元测试。
//
// 覆盖全局临时目录初始化、按任务 ID 生成子目录名、
// 以及识别活跃任务与过期临时目录等逻辑。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::backend_mgr::{decode_backend_tag, encode_backend_tag};
use crate::env::{
    generate_ingest_temp_data_dir, ingest_temp_data_dir, init_global_lightning_env,
    processing_job_ids, stale_temp_directories,
};

/// 验证临时路径与 backend 目录命名规则一致（含 `-dup` 查重后缀）。
#[test]
fn ingest_paths_use_the_canonical_backend_directory_names() {
    let root = PathBuf::from("/tmp/aster-ingest-env-test");
    assert!(init_global_lightning_env(root.clone()));
    assert_eq!(ingest_temp_data_dir(), Some(root.clone()));
    assert_eq!(
        generate_ingest_temp_data_dir(42, false).unwrap(),
        root.join("42")
    );
    assert_eq!(
        generate_ingest_temp_data_dir(42, true).unwrap(),
        root.join("42-dup")
    );

    // Go cleanup only decodes regular backend tags. Duplicate-check tags are
    // removed alongside their regular backend after that job is classified.
    for (job_id, duplicate, encoded) in [
        (0, false, "0"),
        (42, false, "42"),
        (42, true, "42-dup"),
        (-7, false, "-7"),
        (-7, true, "-7-dup"),
    ] {
        assert_eq!(encode_backend_tag(job_id, duplicate), encoded);
        if duplicate {
            assert!(decode_backend_tag(encoded).is_err());
        } else {
            assert_eq!(decode_backend_tag(encoded), Ok(job_id));
        }
    }
    for invalid in ["", "dup", "42-", "42-dup-dup", " 42", "42 "] {
        assert!(decode_backend_tag(invalid).is_err(), "{invalid:?}");
    }
}

/// Go cleanup classifies jobs using regular directories, then removes both the
/// regular and duplicate-check directories for every stale job.
#[test]
fn active_jobs_preserve_regular_and_duplicate_directories() {
    let names = vec![
        "100".to_owned(),
        "100-dup".to_owned(),
        "101".to_owned(),
        "101-dup".to_owned(),
        "-7".to_owned(),
        "-7-dup".to_owned(),
        "not-a-job".to_owned(),
        "101-dup-dup".to_owned(),
    ];
    let active = BTreeSet::from([-7, 100]);

    assert_eq!(processing_job_ids(names.clone(), &active), vec![-7, 100]);
    assert_eq!(
        stale_temp_directories(Path::new("/tmp/ingest"), names, &active),
        vec![
            PathBuf::from("/tmp/ingest/101"),
            PathBuf::from("/tmp/ingest/101-dup"),
        ]
    );
}

/// 无法解码的目录名既不算活跃任务，也不作为清理候选。
#[test]
fn invalid_directory_names_are_neither_active_nor_removal_candidates() {
    let names = vec![
        String::new(),
        "not-a-job".to_owned(),
        "1-dup-dup".to_owned(),
        " 1".to_owned(),
    ];
    let active = BTreeSet::from([1]);
    assert!(processing_job_ids(names.clone(), &active).is_empty());
    assert!(stale_temp_directories(Path::new("/tmp"), names, &active).is_empty());
}
