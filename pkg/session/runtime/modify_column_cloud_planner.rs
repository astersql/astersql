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

//! Durable global-sort plans consume the actual preceding subtasks' object metadata.
use super::super::{modify_column_cloud_meta as wire, modify_column_cloud_store::CloudStore};
use super::*;
use astersql_ingestor_globalsort as sort;
use astersql_ingestor_simplesst::writer as simple;

fn groups(
    handle: &dyn scheduler::TaskHandle,
    store: &dyn sort::Storage,
    task: i64,
    steps: &[i64],
) -> Result<(Vec<wire::SortedMeta>, Vec<i64>), String> {
    for &step in steps {
        let metas = handle
            .previous_subtask_metas(task, step)
            .map_err(|e| e.to_string())?;
        if metas.is_empty() {
            continue;
        }
        let mut groups = Vec::<wire::SortedMeta>::new();
        let mut ids = Vec::new();
        for (position, bytes) in metas.iter().enumerate() {
            let internal = wire::read(store, bytes)?;
            let mut fields: wire::ExternalFields =
                serde_json::from_value(internal).map_err(|e| e.to_string())?;
            if fields.meta_groups.is_empty() {
                fields.meta_groups.push(fields.legacy);
            }
            if position == 0 {
                groups.resize_with(fields.meta_groups.len(), Default::default);
                ids = fields.ele_ids;
            }
            if fields.meta_groups.len() > groups.len() {
                return Err("subtask metadata group count exceeds the first subtask".into());
            }
            for (group, next) in groups.iter_mut().zip(&fields.meta_groups) {
                group.merge(next)?;
            }
        }
        return Ok((groups, ids));
    }
    Ok((vec![], vec![]))
}
impl Planner {
    pub(super) fn cloud_plans(
        &self,
        handle: &dyn scheduler::TaskHandle,
        task: &mut scheduler::Task,
        node_count: usize,
        next_step: i64,
    ) -> Result<Vec<Vec<u8>>, String> {
        use astersql_dxf_framework_proto::{
            BackfillStepMergeSort as MERGE, BackfillStepReadIndex as READ,
            BackfillStepWriteAndIngest as INGEST,
        };
        let store = CloudStore::open(&self.cloud_storage_uri, Arc::new(AtomicBool::new(false)))?;
        let mut result = Vec::new();
        let mut publish =
            |fields: wire::ExternalFields, ts: u64, step: &str| -> Result<(), String> {
                let mut internal = serde_json::json!({"ts": ts});
                result.push(wire::write(
                    store.as_ref(),
                    &mut internal,
                    &fields,
                    sort::util::PlanMetaPath(task.base.id, step, result.len() + 1),
                )?);
                Ok(())
            };
        match next_step {
            MERGE => {
                let (groups, ids) = groups(handle, store.as_ref(), task.base.id, &[READ])?;
                let force = astersql_testkit_testfailpoint::is_active(
                    "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
                );
                let slots = task.base.runtime_slots();
                let stats = groups
                    .iter()
                    .map(|group| {
                        group
                            .files
                            .iter()
                            .map(|file| {
                                Ok(simple::MultipleFilesStat {
                                    MinKey: wire::decode(file.min.as_deref().unwrap_or(""))?,
                                    MaxKey: wire::decode(file.max.as_deref().unwrap_or(""))?,
                                    Filenames: file.filenames.clone(),
                                    MaxOverlappingNum: file.overlapping,
                                })
                            })
                            .collect::<Result<Vec<_>, String>>()
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let skip = !force
                    && stats.iter().all(|stats| {
                        simple::GetMaxOverlappingTotal(stats)
                            <= simple::GetAdjustedMergeSortOverlapThreshold(slots)
                    });
                if skip {
                    return Ok(vec![]);
                }
                for (i, group) in groups.iter().enumerate() {
                    let files = group
                        .files
                        .iter()
                        .flat_map(|file| file.filenames.iter().map(|pair| pair[0].clone()))
                        .collect::<Vec<_>>();
                    for files in sort::util::DivideMergeSortDataFiles(
                        &files,
                        node_count,
                        slots.max(1) as usize,
                    )
                    .map_err(|e| e.to_string())?
                    {
                        publish(
                            wire::ExternalFields {
                                data_files: files,
                                ele_ids: ids.get(i).copied().into_iter().collect(),
                                ..Default::default()
                            },
                            0,
                            "merge-sort",
                        )?;
                    }
                }
            }
            INGEST => {
                let (groups, ids) = groups(handle, store.as_ref(), task.base.id, &[MERGE, READ])?;
                let total = groups
                    .iter()
                    .fold(0u64, |sum, group| sum.wrapping_add(group.size));
                let live_count = if self.mock_node {
                    self.node_manager.get_nodes().len()
                } else {
                    astersql_domain_infosync::GetAllServerInfo()
                        .map_err(|e| e.to_string())?
                        .len()
                };
                if live_count == 0 {
                    return Err("global-sort planning has no live execution instances".into());
                }
                let resource =
                    storage::GetNodeResource().ok_or("DXF node resources unavailable")?;
                if resource.TotalCPU <= 0 {
                    return Err("invalid DXF total CPU".into());
                }
                let (mut region_size, mut region_keys) = if astersql_config_kerneltype::IsNextGen()
                {
                    (1 << 30, 102_400_000)
                } else {
                    (96 << 20, 960_000)
                };
                match self.domain.storage_handle().with_storage(|store| {
                    store.DDLRegionSplitConfig(&super::super::kv::Context::default())
                }) {
                    Ok(Some((size, keys))) => {
                        region_size = region_size.max(size);
                        region_keys = region_keys.max(keys);
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("fail to get region split keys and size: {error}"),
                }
                let (range_size, range_keys) = sort::split::CalRangeSize(
                    resource.TotalMem / i64::from(resource.TotalCPU),
                    region_size,
                    region_keys,
                );
                for (i, group) in groups.iter().enumerate() {
                    let mut start = wire::decode(group.start.as_deref().unwrap_or(""))?;
                    let end = wire::decode(group.end.as_deref().unwrap_or(""))?;
                    if start.is_empty() && end.is_empty() {
                        continue;
                    }
                    let ts = self
                        .domain
                        .storage_handle()
                        .with_storage(|store| {
                            store.CurrentVersion(super::super::kv::GlobalTxnScope)
                        })
                        .map_err(|e| e.to_string())?
                        .Ver;
                    if ts == 0 {
                        return Err("invalid global-sort import timestamp 0".into());
                    }
                    let mut splitter = sort::split::NewRangeSplitter(
                        &group.sort_files(),
                        store.as_ref(),
                        group.size as i64 / live_count as i64,
                        i64::MAX,
                        range_size,
                        range_keys,
                        region_size,
                        region_keys,
                    )
                    .map_err(|e| e.to_string())?;
                    loop {
                        let split = splitter.SplitOneRangesGroup().map_err(|e| e.to_string())?;
                        let final_group = split.end_key_of_group.is_empty();
                        let upper = if final_group {
                            end.clone()
                        } else {
                            split.end_key_of_group
                        };
                        if start >= upper {
                            return Err(format!(
                                "invalid global-sort range: {start:?} >= {upper:?}"
                            ));
                        }
                        let with_bounds = |interior: Vec<Vec<u8>>| {
                            std::iter::once(start.clone())
                                .chain(interior)
                                .chain(std::iter::once(upper.clone()))
                                .map(|key| wire::encode(&key))
                                .collect()
                        };
                        publish(
                            wire::ExternalFields {
                                data_files: split.data_files,
                                stat_files: split.stat_files,
                                range_job_keys: with_bounds(split.interior_range_job_keys),
                                range_split_keys: with_bounds(split.interior_region_split_keys),
                                meta_groups: vec![wire::SortedMeta {
                                    start: Some(wire::encode(&start)),
                                    end: Some(wire::encode(&upper)),
                                    size: group.size / live_count as u64,
                                    ..Default::default()
                                }],
                                ele_ids: ids
                                    .get(i)
                                    .copied()
                                    .filter(|id| *id > 0)
                                    .into_iter()
                                    .collect(),
                                ..Default::default()
                            },
                            ts,
                            "ingest",
                        )?;
                        if final_group {
                            break;
                        }
                        start = upper;
                    }
                    splitter.Close().map_err(|e| e.to_string())?;
                }
                let mut meta: TaskMeta =
                    serde_json::from_slice(&task.meta).map_err(|e| e.to_string())?;
                meta.summary = Some(TaskSummary {
                    index_kv_size: total,
                });
                task.meta = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
            }
            _ => return Err(format!("unknown cloud plan step {next_step}")),
        }
        Ok(result)
    }
}
