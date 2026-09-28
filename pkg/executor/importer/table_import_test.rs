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

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use super::table_import::{
    ImportRuntimeConfig, calculateSubtaskCnt, getAdjustedMaxEngineSize, getRegionSplitSizeKeysWith,
    prepareSortDirPath,
};

#[test]
fn prepare_sort_dir_matches_go_filesystem_branches() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("astersql-table-import-{unique}"));
    fs::create_dir_all(&temp_dir).unwrap();
    let config = ImportRuntimeConfig {
        TempDir: temp_dir.clone(),
        Port: 4000,
        ..Default::default()
    };
    let import_dir = temp_dir.join("import-4000");

    let sort_dir = prepareSortDirPath("1", &config).unwrap();
    assert_eq!(import_dir.join("1"), sort_dir);
    assert!(import_dir.is_dir());
    assert!(!sort_dir.exists());

    fs::remove_dir(&import_dir).unwrap();
    fs::write(&import_dir, b"occupied").unwrap();
    let sort_dir = prepareSortDirPath("2", &config).unwrap();
    assert_eq!(import_dir.join("2"), sort_dir);
    assert!(import_dir.is_dir());

    let sort_dir = prepareSortDirPath("3", &config).unwrap();
    fs::create_dir(&sort_dir).unwrap();
    fs::write(sort_dir.join("stale"), b"stale").unwrap();
    assert_eq!(sort_dir, prepareSortDirPath("3", &config).unwrap());
    assert!(!sort_dir.exists());
    fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn subtask_count_matches_go_table() {
    let local = [
        (1, 500, 0, 1),
        (499, 500, 1, 1),
        (500, 500, 2, 1),
        (749, 500, 3, 1),
        (750, 500, 4, 2),
        (1249, 500, 5, 2),
        (1250, 500, 6, 3),
        (100, 30, 7, 3),
    ];
    for (total, maximum, nodes, expected) in local {
        assert_eq!(expected, calculateSubtaskCnt(total, maximum, false, nodes));
    }
    let global = [
        (1, 500, 0, 1),
        (499, 500, 1, 1),
        (500, 500, 2, 2),
        (749, 500, 3, 3),
        (750, 500, 4, 4),
        (1249, 500, 5, 5),
        (1250, 500, 6, 6),
        (100, 30, 2, 4),
        (400, 99, 3, 6),
        (500, 100, 5, 5),
        (500, 200, 5, 5),
    ];
    for (total, maximum, nodes, expected) in global {
        assert_eq!(expected, calculateSubtaskCnt(total, maximum, true, nodes));
    }
}

#[test]
fn adjusted_engine_size_matches_go_table_and_real_size_regression() {
    let cases = [
        (1, 500, 0, false, 1),
        (499, 500, 1, false, 499),
        (500, 500, 2, false, 500),
        (749, 500, 3, false, 749),
        (750, 500, 4, false, 375),
        (1249, 500, 5, false, 625),
        (1250, 500, 6, false, 417),
        (100, 30, 7, false, 34),
        (500, 500, 2, true, 250),
        (749, 500, 3, true, 250),
        (750, 500, 4, true, 188),
        (1249, 500, 5, true, 250),
        (1250, 500, 6, true, 209),
        (100, 30, 2, true, 25),
        (400, 99, 3, true, 67),
        (500, 100, 5, true, 100),
        (500, 200, 5, true, 100),
        (500, 100, 1, true, 100),
    ];
    for (total, maximum, nodes, global, expected) in cases {
        assert_eq!(
            expected,
            getAdjustedMaxEngineSize(total, maximum, global, nodes)
        );
    }
    assert_eq!(3, calculateSubtaskCnt(1500, 500, false, 0));
    assert_eq!(500, getAdjustedMaxEngineSize(1500, 500, false, 0));
    assert_eq!(1, calculateSubtaskCnt(100, 500, false, 0));
    assert_eq!(100, getAdjustedMaxEngineSize(100, 500, false, 0));
}

#[test]
fn region_split_size_keys_propagates_service_errors() {
    let error = getRegionSplitSizeKeysWith(|| Err("mock error".to_owned())).unwrap_err();
    assert_eq!("mock error", error);

    let error =
        getRegionSplitSizeKeysWith(|| Err("get region split size and keys failed".to_owned()))
            .unwrap_err();
    assert_eq!("get region split size and keys failed", error);
}
