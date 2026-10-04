// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage::TaskCleanupInfo;
use astersql_objstore::storage::{Context, Storage, WalkOption};
use astersql_util_logutil::log::{BgLogger, LogField, LogLevel};
use serde::Serialize;

const STORAGE_DIR: &str = "conflicted-rows";
const STORAGE_PREFIX: &str = "conflicted-rows/";
pub(crate) const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_TASK_IDS_PER_FLUSH: usize = 128;
const MAX_OBJECTS_PER_FLUSH: usize = 1000;
const MAX_LOGGED_SAMPLES: usize = 16;

pub trait TaskInfoGetter {
    fn GetTaskCleanupInfoByIDs(
        &self,
        ctx: &Context,
        task_ids: &[i64],
    ) -> anyhow::Result<HashMap<i64, TaskCleanupInfo>>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CountWithSamples {
    #[serde(rename = "count", skip_serializing_if = "is_zero")]
    pub Count: i64,
    #[serde(rename = "samples", skip_serializing_if = "Vec::is_empty")]
    pub Samples: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CleanupStats {
    #[serde(rename = "deleted-files", skip_serializing_if = "is_zero")]
    pub DeletedFiles: i64,
    #[serde(rename = "missing-tasks", skip_serializing_if = "is_empty_count")]
    pub MissingTasks: CountWithSamples,
    #[serde(rename = "missing-task-files", skip_serializing_if = "is_empty_count")]
    pub MissingTaskFiles: CountWithSamples,
    #[serde(
        rename = "non-import-into-task-files",
        skip_serializing_if = "is_empty_count"
    )]
    pub NonImportIntoTaskFiles: CountWithSamples,
    #[serde(
        rename = "unparsed-task-id-files",
        skip_serializing_if = "is_empty_count"
    )]
    pub UnparsedTaskIDFiles: CountWithSamples,
    #[serde(rename = "failures", skip_serializing_if = "is_zero")]
    pub Failures: i64,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

fn is_empty_count(value: &CountWithSamples) -> bool {
    value.Count == 0 && value.Samples.is_empty()
}

fn log_cleanup_stats(stats: &CleanupStats) {
    let encoded = serde_json::to_string(stats).unwrap_or_else(|_| "{}".to_owned());
    BgLogger().log(
        LogLevel::Info,
        "finished conflict-row file cleanup",
        [LogField::String("stats".to_owned(), encoded)],
    );
}

#[derive(Debug)]
pub struct CleanupFailure {
    pub Stats: CleanupStats,
    pub Source: anyhow::Error,
}

impl std::fmt::Display for CleanupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.Source.fmt(formatter)
    }
}

impl std::error::Error for CleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.Source.as_ref())
    }
}

fn record_count_with_samples(
    target: &mut CountWithSamples,
    samples: impl IntoIterator<Item = String>,
) {
    let samples = samples.into_iter().collect::<Vec<_>>();
    target.Count += samples.len() as i64;
    let remaining = MAX_LOGGED_SAMPLES.saturating_sub(target.Samples.len());
    target.Samples.extend(samples.into_iter().take(remaining));
}

fn merge_count_with_samples(target: &mut CountWithSamples, completed: CountWithSamples) {
    target.Count += completed.Count;
    let remaining = MAX_LOGGED_SAMPLES.saturating_sub(target.Samples.len());
    target
        .Samples
        .extend(completed.Samples.into_iter().take(remaining));
}

impl CleanupStats {
    fn merge_completed_flush(&mut self, completed: CleanupStats) {
        self.DeletedFiles += completed.DeletedFiles;
        merge_count_with_samples(&mut self.MissingTasks, completed.MissingTasks);
        merge_count_with_samples(&mut self.MissingTaskFiles, completed.MissingTaskFiles);
        merge_count_with_samples(
            &mut self.NonImportIntoTaskFiles,
            completed.NonImportIntoTaskFiles,
        );
        merge_count_with_samples(&mut self.UnparsedTaskIDFiles, completed.UnparsedTaskIDFiles);
    }
}

pub fn NewFileNamePrefix(task_id: i64, subtask_id: i64) -> String {
    NewFileNamePrefixWithUUID(task_id, subtask_id, &uuid::Uuid::new_v4().to_string())
}

pub fn NewFileNamePrefixWithUUID(task_id: i64, subtask_id: i64, uuid: &str) -> String {
    format!("{STORAGE_DIR}/{task_id}/{subtask_id}-{uuid}")
}

pub(crate) fn parse_task_id(name: &str) -> Option<i64> {
    let relative = name.strip_prefix(STORAGE_PREFIX)?;
    let (component, descendant) = relative.split_once('/')?;
    if component.is_empty()
        || descendant.trim_matches('/').is_empty()
        || !component.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    component.parse::<i64>().ok().filter(|task_id| *task_id > 0)
}

fn should_delete(info: &TaskCleanupInfo, now: SystemTime) -> bool {
    if info.Type != proto::ImportInto {
        return false;
    }
    match info.State {
        proto::TaskStateFailed | proto::TaskStateReverted => true,
        proto::TaskStateSucceed => info
            .EndTime
            .and_then(|end_time| end_time.checked_add(RETENTION))
            .is_some_and(|expires_at| now >= expires_at),
        _ => false,
    }
}

pub fn CleanFiles(
    ctx: &Context,
    store: &dyn Storage,
    info_getter: &dyn TaskInfoGetter,
    now: SystemTime,
) -> Result<CleanupStats, CleanupFailure> {
    let mut stats = CleanupStats::default();
    let mut task_files = HashMap::<i64, Vec<String>>::with_capacity(MAX_TASK_IDS_PER_FLUSH);
    let mut unparsed_files = Vec::<String>::new();
    let mut file_count = 0usize;

    let flush = |task_files: &mut HashMap<i64, Vec<String>>,
                 unparsed_files: &mut Vec<String>,
                 file_count: &mut usize,
                 stats: &mut CleanupStats|
     -> anyhow::Result<()> {
        if *file_count == 0 {
            return Ok(());
        }
        let mut task_ids = task_files.keys().copied().collect::<Vec<_>>();
        task_ids.sort_unstable();
        let infos = if task_ids.is_empty() {
            HashMap::new()
        } else {
            info_getter.GetTaskCleanupInfoByIDs(ctx, &task_ids)?
        };

        let mut completed = CleanupStats::default();
        record_count_with_samples(
            &mut completed.UnparsedTaskIDFiles,
            unparsed_files.iter().cloned(),
        );
        let mut files_to_delete = unparsed_files.clone();
        for task_id in task_ids {
            let files = &task_files[&task_id];
            match infos.get(&task_id) {
                None => {
                    record_count_with_samples(&mut completed.MissingTasks, [task_id.to_string()]);
                    record_count_with_samples(
                        &mut completed.MissingTaskFiles,
                        files.iter().cloned(),
                    );
                    files_to_delete.extend(files.iter().cloned());
                }
                Some(info) if info.Type != proto::ImportInto => {
                    record_count_with_samples(
                        &mut completed.NonImportIntoTaskFiles,
                        files.iter().cloned(),
                    );
                    files_to_delete.extend(files.iter().cloned());
                }
                Some(info) if should_delete(info, now) => {
                    files_to_delete.extend(files.iter().cloned());
                }
                Some(_) => {}
            }
        }
        if !files_to_delete.is_empty() {
            store.DeleteFiles(ctx, &files_to_delete)?;
            completed.DeletedFiles = files_to_delete.len() as i64;
        }
        stats.merge_completed_flush(completed);
        task_files.clear();
        unparsed_files.clear();
        *file_count = 0;
        Ok(())
    };

    let walk_result = store.WalkDir(
        ctx,
        Some(&WalkOption {
            sub_dir: STORAGE_PREFIX.to_owned(),
            ..Default::default()
        }),
        &mut |name, _| {
            if let Some(task_id) = parse_task_id(name) {
                task_files.entry(task_id).or_default().push(name.to_owned());
            } else {
                unparsed_files.push(name.to_owned());
            }
            file_count += 1;
            if file_count > MAX_OBJECTS_PER_FLUSH || task_files.len() > MAX_TASK_IDS_PER_FLUSH {
                flush(
                    &mut task_files,
                    &mut unparsed_files,
                    &mut file_count,
                    &mut stats,
                )?;
            }
            Ok(())
        },
    );
    if let Err(error) = walk_result {
        stats.Failures += 1;
        log_cleanup_stats(&stats);
        return Err(CleanupFailure {
            Stats: stats,
            Source: error,
        });
    }
    if let Err(error) = flush(
        &mut task_files,
        &mut unparsed_files,
        &mut file_count,
        &mut stats,
    ) {
        stats.Failures += 1;
        log_cleanup_stats(&stats);
        return Err(CleanupFailure {
            Stats: stats,
            Source: error,
        });
    }
    log_cleanup_stats(&stats);
    Ok(stats)
}

pub fn CleanConflictRowFiles(
    ctx: &Context,
    info_getter: &dyn TaskInfoGetter,
    cloud_storage_uri: &str,
) -> anyhow::Result<()> {
    if cloud_storage_uri.is_empty() {
        return Ok(());
    }
    let store = astersql_objstore::storage::NewFromURL(ctx, cloud_storage_uri)?;
    let result = CleanFiles(ctx, store.as_ref(), info_getter, SystemTime::now());
    store.Close();
    result.map(|_| ()).map_err(|failure| failure.Source)
}
