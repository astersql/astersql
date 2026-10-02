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

//! Go-compatible external DXF index metadata. JSON byte slices remain base64;
//! large fields live in the object store and the durable SQL row holds a pointer.
use astersql_ingestor_globalsort as sort;
use base64::Engine;
use serde::{Deserialize, Serialize};

pub(super) fn encode(key: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(key)
}
pub(super) fn decode(key: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|error| error.to_string())
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct FilesStat {
    #[serde(default, rename = "min-key")]
    pub min: Option<String>,
    #[serde(default, rename = "max-key")]
    pub max: Option<String>,
    #[serde(default, deserialize_with = "null_vec")]
    pub filenames: Vec<[String; 2]>,
    #[serde(default, rename = "max-overlapping-num")]
    pub overlapping: i64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Conflict {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub count: u64,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_vec"
    )]
    pub files: Vec<String>,
}
fn null_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct SortedMeta {
    #[serde(default, rename = "start-key")]
    pub start: Option<String>,
    #[serde(default, rename = "end-key")]
    pub end: Option<String>,
    #[serde(default, rename = "total-kv-size")]
    pub size: u64,
    #[serde(default, rename = "total-kv-cnt")]
    pub count: u64,
    #[serde(
        default,
        rename = "multiple-files-stats",
        deserialize_with = "null_vec"
    )]
    pub files: Vec<FilesStat>,
    #[serde(default, rename = "conflict-info")]
    pub conflict: Conflict,
}
impl SortedMeta {
    pub(super) fn merge(&mut self, other: &Self) -> Result<(), String> {
        if other.start.as_deref().unwrap_or("").is_empty()
            && other.end.as_deref().unwrap_or("").is_empty()
        {
            return Ok(());
        }
        if self.start.as_deref().unwrap_or("").is_empty()
            && self.end.as_deref().unwrap_or("").is_empty()
        {
            *self = other.clone();
            return Ok(());
        }
        // Base64 lexical order differs from byte order. Compare decoded keys.
        for (current, next, minimum) in [
            (&mut self.start, &other.start, true),
            (&mut self.end, &other.end, false),
        ] {
            let left = decode(current.as_deref().unwrap_or(""))?;
            let right = decode(next.as_deref().unwrap_or(""))?;
            if if minimum { right < left } else { right > left } {
                *current = next.clone();
            }
        }
        self.size = self.size.wrapping_add(other.size);
        self.count = self.count.wrapping_add(other.count);
        self.files.extend(other.files.iter().cloned());
        self.conflict.count = self.conflict.count.wrapping_add(other.conflict.count);
        self.conflict
            .files
            .extend(other.conflict.files.iter().cloned());
        Ok(())
    }
    pub(super) fn from_simple(
        summary: &astersql_ingestor_simplesst::writer::WriterSummary,
    ) -> Self {
        if summary.Min.is_empty() && summary.Max.is_empty() {
            return Self::default();
        }
        let mut end = summary.Max.clone();
        end.push(0);
        Self {
            start: Some(encode(&summary.Min)),
            end: Some(encode(&end)),
            size: summary.TotalSize,
            count: summary.TotalCnt,
            files: summary
                .MultipleFilesStats
                .iter()
                .map(|files| FilesStat {
                    min: Some(encode(&files.MinKey)),
                    max: Some(encode(&files.MaxKey)),
                    filenames: files.Filenames.clone(),
                    overlapping: files.MaxOverlappingNum,
                })
                .collect(),
            conflict: Conflict {
                count: summary.ConflictInfo.Count,
                files: summary.ConflictInfo.Files.clone(),
            },
        }
    }
    pub(super) fn from_merge(summary: &sort::WriterSummary) -> Self {
        if summary.min.is_empty() && summary.max.is_empty() {
            return Self::default();
        }
        let mut end = summary.max.clone();
        end.push(0);
        Self {
            start: Some(encode(&summary.min)),
            end: Some(encode(&end)),
            size: summary.total_size,
            count: summary.total_count,
            files: summary
                .multiple_files_stats
                .iter()
                .map(|files| FilesStat {
                    min: Some(encode(&summary.min)),
                    max: Some(encode(&summary.max)),
                    filenames: files
                        .filenames
                        .iter()
                        .map(|file| [file.data_file.clone(), file.stat_file.clone()])
                        .collect(),
                    overlapping: 1,
                })
                .collect(),
            conflict: Conflict {
                count: summary.conflict_info.count,
                files: summary.conflict_info.files.clone(),
            },
        }
    }
    pub(super) fn sort_files(&self) -> Vec<sort::MultipleFilesStat> {
        self.files
            .iter()
            .map(|files| sort::MultipleFilesStat {
                filenames: files
                    .filenames
                    .iter()
                    .map(|file| sort::FilePair {
                        data_file: file[0].clone(),
                        stat_file: file[1].clone(),
                        properties: vec![],
                    })
                    .collect(),
            })
            .collect()
    }
}
/// Fields marked external:true by Go BackfillSubTaskMeta. Internal row bounds,
/// physical table and TS stay in SQL and survive pointer metadata merging.
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct ExternalFields {
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_vec"
    )]
    pub range_job_keys: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_vec"
    )]
    pub range_split_keys: Vec<String>,
    #[serde(
        default,
        rename = "data-files",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_vec"
    )]
    pub data_files: Vec<String>,
    #[serde(
        default,
        rename = "stat-files",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_vec"
    )]
    pub stat_files: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_vec"
    )]
    pub meta_groups: Vec<SortedMeta>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_vec"
    )]
    pub ele_ids: Vec<i64>,
    #[serde(flatten)]
    pub legacy: SortedMeta,
}
pub(super) fn read(store: &dyn sort::Storage, bytes: &[u8]) -> Result<serde_json::Value, String> {
    let mut internal: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let path = internal
        .get("ExternalPath")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if !path.is_empty() {
        let external: serde_json::Value =
            serde_json::from_slice(&store.read(path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        let fields = external
            .as_object()
            .ok_or("external backfill metadata must be an object")?;
        let row = internal
            .as_object_mut()
            .ok_or("internal backfill metadata must be an object")?;
        row.extend(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    Ok(internal)
}
pub(super) fn write(
    store: &dyn sort::Storage,
    internal: &mut serde_json::Value,
    fields: &ExternalFields,
    path: String,
) -> Result<Vec<u8>, String> {
    store
        .write(
            &path,
            serde_json::to_vec(fields).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    internal
        .as_object_mut()
        .ok_or("internal backfill metadata must be an object")?
        .insert("ExternalPath".into(), path.into());
    serde_json::to_vec(internal).map_err(|error| error.to_string())
}
