// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// TiFlash 执行统计：扫描、列存扫描、等待摘要、网络流量与 RU 消耗合并。
//
// 对应 Go `tiflash_stats.go`。TiFlash 是列存分析引擎；MPP 为分布式执行；
// Region 为数据分片；TSO 等待指获取时间戳 oracle 的排队时间。

// 本文件对照 pkg/util/execdetails/tiflash_stats.go 实现 TiFlash scan、columnar
// scan、wait summary、network traffic 以及 RU consumption 的统计合并逻辑。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;
use protobuf::Message;

// TiflashStats contains tiflash execution stats.
#[derive(Clone, Default)]
/// 聚合 TiFlash 扫描、列存、等待与网络四类统计。
pub struct TiflashStats {
    pub scanContext: TiFlashScanContext,
    pub columnarScanContext: TiFlashColumnarScanContext,
    pub waitSummary: TiFlashWaitSummary,
    pub networkSummary: TiFlashNetworkTrafficSummary,
}

// TiFlashColumnarScanContext is used to express the table scan information in tiflash columnar read path.
#[derive(Clone, Default)]
/// TiFlash 列存读路径上的表扫描信息。
pub struct TiFlashColumnarScanContext {
    pub hasStats: bool,
    pub regions: u64,
    pub readTasks: u64,
    pub physicalTables: u64,
    pub columns: u64,
    pub userReadBytes: u64,
    pub mvccInputRows: u64,
    pub mvccInputBytes: u64,
    pub mvccOutputRows: u64,
    pub totalReadBlockMs: u64,
    pub totalSerializeBlockMs: u64,
    pub totalInitReaderMs: u64,
    pub totalPrefetchMs: u64,
    pub roughCheckTotalPacks: u64,
    pub roughCheckSelectedPacks: u64,
    pub roughCheckSkippedPacks: u64,
    pub roughCheckUnknownPacks: u64,
    pub remoteSegments: u64,
    pub totalSegments: u64,
    pub totalDeserializeBlockMs: u64,
}

// TiFlashScanContext is used to express the table scan information in tiflash
#[derive(Clone, Default)]
/// TiFlash 表扫描信息（行数、Region、流式耗时等）。
pub struct TiFlashScanContext {
    pub dmfileDataScannedRows: u64,
    pub dmfileDataSkippedRows: u64,
    pub dmfileMvccScannedRows: u64,
    pub dmfileMvccSkippedRows: u64,
    pub dmfileLmFilterScannedRows: u64,
    pub dmfileLmFilterSkippedRows: u64,
    pub totalDmfileRsCheckMs: u64,
    pub totalDmfileReadMs: u64,
    pub totalBuildSnapshotMs: u64,
    pub localRegions: u64,
    pub remoteRegions: u64,
    pub totalLearnerReadMs: u64,
    pub disaggReadCacheHitBytes: u64,
    pub disaggReadCacheMissBytes: u64,
    pub segments: u64,
    pub readTasks: u64,
    pub deltaRows: u64,
    pub deltaBytes: u64,
    pub mvccInputRows: u64,
    pub mvccInputBytes: u64,
    pub mvccOutputRows: u64,
    pub totalBuildBitmapMs: u64,
    pub totalBuildInputStreamMs: u64,
    pub staleReadRegions: u64,
    pub minLocalStreamMs: u64,
    pub maxLocalStreamMs: u64,
    pub minRemoteStreamMs: u64,
    pub maxRemoteStreamMs: u64,
    pub regionsOfInstance: HashMap<String, u64>,
    // vector index related
    pub vectorIdxLoadFromS3: u64,
    pub vectorIdxLoadFromDisk: u64,
    pub vectorIdxLoadFromCache: u64,
    pub vectorIdxLoadTimeMs: u64,
    pub vectorIdxSearchTimeMs: u64,
    pub vectorIdxSearchVisitedNodes: u64,
    pub vectorIdxSearchDiscardedNodes: u64,
    pub vectorIdxReadVecTimeMs: u64,
    pub vectorIdxReadOthersTimeMs: u64,
    // fts related
    pub ftsNFromInmemoryNoindex: u32,
    pub ftsNFromTinyIndex: u32,
    pub ftsNFromTinyNoindex: u32,
    pub ftsNFromDmfIndex: u32,
    pub ftsNFromDmfNoindex: u32,
    pub ftsRowsFromInmemoryNoindex: u64,
    pub ftsRowsFromTinyIndex: u64,
    pub ftsRowsFromTinyNoindex: u64,
    pub ftsRowsFromDmfIndex: u64,
    pub ftsRowsFromDmfNoindex: u64,
    pub ftsIdxLoadTotalMs: u64,
    pub ftsIdxLoadFromCache: u32,
    pub ftsIdxLoadFromColumnFile: u32,
    pub ftsIdxLoadFromStableS3: u32,
    pub ftsIdxLoadFromStableDisk: u32,
    pub ftsIdxSearchN: u32,
    pub ftsIdxSearchTotalMs: u64,
    pub ftsIdxDmSearchRows: u64,
    pub ftsIdxDmTotalReadFtsMs: u64,
    pub ftsIdxDmTotalReadOthersMs: u64,
    pub ftsIdxTinySearchRows: u64,
    pub ftsIdxTinyTotalReadFtsMs: u64,
    pub ftsIdxTinyTotalReadOthersMs: u64,
    pub ftsBruteTotalReadMs: u64,
    pub ftsBruteTotalSearchMs: u64,
    // inverted index related
    pub invertedIdxLoadFromS3: u32,
    pub invertedIdxLoadFromDisk: u32,
    pub invertedIdxLoadFromCache: u32,
    pub invertedIdxLoadTimeMs: u64,
    pub invertedIdxSearchTimeMs: u64,
    pub invertedIdxSearchSkippedPacks: u32,
    pub invertedIdxIndexedRows: u64,
    pub invertedIdxSearchSelectedRows: u64,
}

impl TiFlashScanContext {
    // Clone implements the deep copy of * TiFlashshScanContext
    pub fn Clone(&self) -> TiFlashScanContext {
        // Go 用 make(map) + maps.Copy 深拷贝 regionsOfInstance；直接 clone map 保留同义。
        self.clone()
    }

    // String 对应 Go 的 TiFlash scan 详情格式化。
    pub fn String(&self) -> String {
        let mut output: Vec<String> = Vec::new();
        if self.vectorIdxLoadFromS3 + self.vectorIdxLoadFromDisk + self.vectorIdxLoadFromCache > 0 {
            let mut items = Vec::new();
            items.push(format!(
                "load:{{total:{}ms,from_s3:{},from_disk:{},from_cache:{}}}",
                self.vectorIdxLoadTimeMs, self.vectorIdxLoadFromS3, self.vectorIdxLoadFromDisk, self.vectorIdxLoadFromCache
            ));
            items.push(format!(
                "search:{{total:{}ms,visited_nodes:{},discarded_nodes:{}}}",
                self.vectorIdxSearchTimeMs, self.vectorIdxSearchVisitedNodes, self.vectorIdxSearchDiscardedNodes
            ));
            items.push(format!(
                "read:{{vec_total:{}ms,others_total:{}ms}}",
                self.vectorIdxReadVecTimeMs, self.vectorIdxReadOthersTimeMs
            ));
            output.push(format!("vector_idx:{{{}}}", items.join(",")));
        }
        if self.invertedIdxLoadFromS3 + self.invertedIdxLoadFromDisk + self.invertedIdxLoadFromCache > 0 {
            let mut items = Vec::new();
            items.push(format!(
                "load:{{total:{}ms,from_s3:{},from_disk:{},from_cache:{}}}",
                self.invertedIdxLoadTimeMs, self.invertedIdxLoadFromS3, self.invertedIdxLoadFromDisk, self.invertedIdxLoadFromCache
            ));
            items.push(format!(
                "search:{{total:{}ms,skipped_packs:{},indexed_rows:{},selected_rows:{}}}",
                self.invertedIdxSearchTimeMs, self.invertedIdxSearchSkippedPacks, self.invertedIdxIndexedRows, self.invertedIdxSearchSelectedRows
            ));
            output.push(format!("inverted_idx:{{{}}}", items.join(",")));
        }
        if self.ftsNFromInmemoryNoindex
            + self.ftsNFromTinyIndex
            + self.ftsNFromTinyNoindex
            + self.ftsNFromDmfIndex
            + self.ftsNFromDmfNoindex
            > 0
        {
            let avg = if self.ftsIdxSearchN > 0 {
                self.ftsIdxSearchTotalMs / self.ftsIdxSearchN as u64
            } else {
                0
            };
            let items = vec![
                format!("hit_rows:{{delta:{},dmf:{}}}", self.ftsRowsFromTinyIndex, self.ftsRowsFromDmfIndex),
                format!(
                    "miss_rows:{{mem:{},delta:{},dmf:{}}}",
                    self.ftsRowsFromInmemoryNoindex, self.ftsRowsFromTinyNoindex, self.ftsRowsFromDmfNoindex
                ),
                format!(
                    "idx_load:{{total:{}ms,from:{{s3:{},disk:{},cache:{}}}}}",
                    self.ftsIdxLoadTotalMs,
                    self.ftsIdxLoadFromStableS3,
                    self.ftsIdxLoadFromStableDisk + self.ftsIdxLoadFromColumnFile,
                    self.ftsIdxLoadFromCache
                ),
                format!("idx_search:{{total:{}ms,avg:{}ms}}", self.ftsIdxSearchTotalMs, avg),
                format!(
                    "idx_read:{{rows:{},fts_total:{}ms,others_total:{}ms}}",
                    self.ftsIdxDmSearchRows + self.ftsIdxTinySearchRows,
                    self.ftsIdxDmTotalReadFtsMs + self.ftsIdxTinyTotalReadFtsMs,
                    self.ftsIdxDmTotalReadOthersMs + self.ftsIdxTinyTotalReadOthersMs
                ),
                format!("miss:{{read:{}ms,search:{}ms}}", self.ftsBruteTotalReadMs, self.ftsBruteTotalSearchMs),
            ];
            output.push(format!("fts:{{{}}}", items.join(",")));
        }

        let mut regionBalanceInfo = "none".to_string();
        if !self.regionsOfInstance.is_empty() {
            let maxNum = self.regionsOfInstance.values().copied().max().unwrap_or(0);
            let minNum = self
                .regionsOfInstance
                .values()
                .copied()
                .filter(|v| *v > 0)
                .min()
                .unwrap_or(u64::MAX);
            // Go 直接用 float64(max)/float64(min)，这里保留同样的除法展示语义。
            regionBalanceInfo = format!(
                "{{instance_num: {}, max/min: {}/{}={:.6}}}",
                self.regionsOfInstance.len(),
                maxNum,
                minNum,
                maxNum as f64 / minNum as f64
            );
        }
        let dmfileDisaggInfo = if self.disaggReadCacheHitBytes != 0 || self.disaggReadCacheMissBytes != 0 {
            format!(
                ", disagg_cache_hit_bytes: {}, disagg_cache_miss_bytes: {}",
                self.disaggReadCacheHitBytes, self.disaggReadCacheMissBytes
            )
        } else {
            String::new()
        };
        let remoteStreamInfo = if self.minRemoteStreamMs != 0 || self.maxRemoteStreamMs != 0 {
            format!("min_remote_stream:{}ms, max_remote_stream:{}ms, ", self.minRemoteStreamMs, self.maxRemoteStreamMs)
        } else {
            String::new()
        };

        // note: "tot" is short for "total"
        output.push(format!(
            "tiflash_scan:{{mvcc_input_rows:{}, mvcc_input_bytes:{}, mvcc_output_rows:{}, local_regions:{}, remote_regions:{}, tot_learner_read:{}ms, region_balance:{}, delta_rows:{}, delta_bytes:{}, segments:{}, stale_read_regions:{}, tot_build_snapshot:{}ms, tot_build_bitmap:{}ms, tot_build_inputstream:{}ms, min_local_stream:{}ms, max_local_stream:{}ms, {}dtfile:{{data_scanned_rows:{}, data_skipped_rows:{}, mvcc_scanned_rows:{}, mvcc_skipped_rows:{}, lm_filter_scanned_rows:{}, lm_filter_skipped_rows:{}, tot_rs_index_check:{}ms, tot_read:{}ms{}}}}}",
            self.mvccInputRows,
            self.mvccInputBytes,
            self.mvccOutputRows,
            self.localRegions,
            self.remoteRegions,
            self.totalLearnerReadMs,
            regionBalanceInfo,
            self.deltaRows,
            self.deltaBytes,
            self.segments,
            self.staleReadRegions,
            self.totalBuildSnapshotMs,
            self.totalBuildBitmapMs,
            self.totalBuildInputStreamMs,
            self.minLocalStreamMs,
            self.maxLocalStreamMs,
            remoteStreamInfo,
            self.dmfileDataScannedRows,
            self.dmfileDataSkippedRows,
            self.dmfileMvccScannedRows,
            self.dmfileMvccSkippedRows,
            self.dmfileLmFilterScannedRows,
            self.dmfileLmFilterSkippedRows,
            self.totalDmfileRsCheckMs,
            self.totalDmfileReadMs,
            dmfileDisaggInfo,
        ));

        output.join(", ")
    }

    // Merge make sum to merge the information in TiFlashScanContext
    pub fn Merge(&mut self, other: TiFlashScanContext) {
        self.dmfileDataScannedRows += other.dmfileDataScannedRows;
        self.dmfileDataSkippedRows += other.dmfileDataSkippedRows;
        self.dmfileMvccScannedRows += other.dmfileMvccScannedRows;
        self.dmfileMvccSkippedRows += other.dmfileMvccSkippedRows;
        self.dmfileLmFilterScannedRows += other.dmfileLmFilterScannedRows;
        self.dmfileLmFilterSkippedRows += other.dmfileLmFilterSkippedRows;
        self.totalDmfileRsCheckMs += other.totalDmfileRsCheckMs;
        self.totalDmfileReadMs += other.totalDmfileReadMs;
        self.totalBuildSnapshotMs += other.totalBuildSnapshotMs;
        self.localRegions += other.localRegions;
        self.remoteRegions += other.remoteRegions;
        self.totalLearnerReadMs += other.totalLearnerReadMs;
        self.disaggReadCacheHitBytes += other.disaggReadCacheHitBytes;
        self.disaggReadCacheMissBytes += other.disaggReadCacheMissBytes;
        self.segments += other.segments;
        self.readTasks += other.readTasks;
        self.deltaRows += other.deltaRows;
        self.deltaBytes += other.deltaBytes;
        self.mvccInputRows += other.mvccInputRows;
        self.mvccInputBytes += other.mvccInputBytes;
        self.mvccOutputRows += other.mvccOutputRows;
        self.totalBuildBitmapMs += other.totalBuildBitmapMs;
        self.totalBuildInputStreamMs += other.totalBuildInputStreamMs;
        self.staleReadRegions += other.staleReadRegions;

        self.vectorIdxLoadFromS3 += other.vectorIdxLoadFromS3;
        self.vectorIdxLoadFromDisk += other.vectorIdxLoadFromDisk;
        self.vectorIdxLoadFromCache += other.vectorIdxLoadFromCache;
        self.vectorIdxLoadTimeMs += other.vectorIdxLoadTimeMs;
        self.vectorIdxSearchTimeMs += other.vectorIdxSearchTimeMs;
        self.vectorIdxSearchVisitedNodes += other.vectorIdxSearchVisitedNodes;
        self.vectorIdxSearchDiscardedNodes += other.vectorIdxSearchDiscardedNodes;
        self.vectorIdxReadVecTimeMs += other.vectorIdxReadVecTimeMs;
        self.vectorIdxReadOthersTimeMs += other.vectorIdxReadOthersTimeMs;

        self.ftsNFromInmemoryNoindex += other.ftsNFromInmemoryNoindex;
        self.ftsNFromTinyIndex += other.ftsNFromTinyIndex;
        self.ftsNFromTinyNoindex += other.ftsNFromTinyNoindex;
        self.ftsNFromDmfIndex += other.ftsNFromDmfIndex;
        self.ftsNFromDmfNoindex += other.ftsNFromDmfNoindex;
        self.ftsRowsFromInmemoryNoindex += other.ftsRowsFromInmemoryNoindex;
        self.ftsRowsFromTinyIndex += other.ftsRowsFromTinyIndex;
        self.ftsRowsFromTinyNoindex += other.ftsRowsFromTinyNoindex;
        self.ftsRowsFromDmfIndex += other.ftsRowsFromDmfIndex;
        self.ftsRowsFromDmfNoindex += other.ftsRowsFromDmfNoindex;
        self.ftsIdxLoadTotalMs += other.ftsIdxLoadTotalMs;
        self.ftsIdxLoadFromCache += other.ftsIdxLoadFromCache;
        self.ftsIdxLoadFromColumnFile += other.ftsIdxLoadFromColumnFile;
        self.ftsIdxLoadFromStableS3 += other.ftsIdxLoadFromStableS3;
        self.ftsIdxLoadFromStableDisk += other.ftsIdxLoadFromStableDisk;
        self.ftsIdxSearchN += other.ftsIdxSearchN;
        self.ftsIdxSearchTotalMs += other.ftsIdxSearchTotalMs;
        self.ftsIdxDmSearchRows += other.ftsIdxDmSearchRows;
        self.ftsIdxDmTotalReadFtsMs += other.ftsIdxDmTotalReadFtsMs;
        self.ftsIdxDmTotalReadOthersMs += other.ftsIdxDmTotalReadOthersMs;
        self.ftsIdxTinySearchRows += other.ftsIdxTinySearchRows;
        self.ftsIdxTinyTotalReadFtsMs += other.ftsIdxTinyTotalReadFtsMs;
        self.ftsIdxTinyTotalReadOthersMs += other.ftsIdxTinyTotalReadOthersMs;
        self.ftsBruteTotalReadMs += other.ftsBruteTotalReadMs;
        self.ftsBruteTotalSearchMs += other.ftsBruteTotalSearchMs;

        self.invertedIdxLoadFromS3 += other.invertedIdxLoadFromS3;
        self.invertedIdxLoadFromDisk += other.invertedIdxLoadFromDisk;
        self.invertedIdxLoadFromCache += other.invertedIdxLoadFromCache;
        self.invertedIdxLoadTimeMs += other.invertedIdxLoadTimeMs;
        self.invertedIdxSearchTimeMs += other.invertedIdxSearchTimeMs;
        self.invertedIdxSearchSkippedPacks += other.invertedIdxSearchSkippedPacks;
        self.invertedIdxIndexedRows += other.invertedIdxIndexedRows;
        self.invertedIdxSearchSelectedRows += other.invertedIdxSearchSelectedRows;

        // Go 的 min/max stream 字段在 0 值时把另一侧作为初始值。
        // 合并时取更小的本地流式耗时（0 表示尚未设置）。
        if self.minLocalStreamMs == 0 || other.minLocalStreamMs < self.minLocalStreamMs {
            self.minLocalStreamMs = other.minLocalStreamMs;
        }
        if other.maxLocalStreamMs > self.maxLocalStreamMs {
            self.maxLocalStreamMs = other.maxLocalStreamMs;
        }
        if self.minRemoteStreamMs == 0 || other.minRemoteStreamMs < self.minRemoteStreamMs {
            self.minRemoteStreamMs = other.minRemoteStreamMs;
        }
        if other.maxRemoteStreamMs > self.maxRemoteStreamMs {
            self.maxRemoteStreamMs = other.maxRemoteStreamMs;
        }

        for (k, v) in other.regionsOfInstance {
            *self.regionsOfInstance.entry(k).or_insert(0) += v;
        }
    }

    // mergeExecSummary 对应 Go 从 protobuf TiFlashScanContext 累加字段。
    pub fn mergeExecSummary(&mut self, summary: Option<&tipb::TiFlashScanContext>) {
        let Some(summary) = summary else {
            return;
        };
        self.dmfileDataScannedRows += summary.get_dmfile_data_scanned_rows();
        self.dmfileDataSkippedRows += summary.get_dmfile_data_skipped_rows();
        self.dmfileMvccScannedRows += summary.get_dmfile_mvcc_scanned_rows();
        self.dmfileMvccSkippedRows += summary.get_dmfile_mvcc_skipped_rows();
        self.dmfileLmFilterScannedRows += summary.get_dmfile_lm_filter_scanned_rows();
        self.dmfileLmFilterSkippedRows += summary.get_dmfile_lm_filter_skipped_rows();
        self.totalDmfileRsCheckMs += summary.get_total_dmfile_rs_check_ms();
        self.totalDmfileReadMs += summary.get_total_dmfile_read_ms();
        self.totalBuildSnapshotMs += summary.get_total_build_snapshot_ms();
        self.localRegions += summary.get_local_regions();
        self.remoteRegions += summary.get_remote_regions();
        self.totalLearnerReadMs += summary.get_total_learner_read_ms();
        self.disaggReadCacheHitBytes += summary.get_disagg_read_cache_hit_bytes();
        self.disaggReadCacheMissBytes += summary.get_disagg_read_cache_miss_bytes();
        self.segments += summary.get_segments();
        self.readTasks += summary.get_read_tasks();
        self.deltaRows += summary.get_delta_rows();
        self.deltaBytes += summary.get_delta_bytes();
        self.mvccInputRows += summary.get_mvcc_input_rows();
        self.mvccInputBytes += summary.get_mvcc_input_bytes();
        self.mvccOutputRows += summary.get_mvcc_output_rows();
        self.totalBuildBitmapMs += summary.get_total_build_bitmap_ms();
        self.totalBuildInputStreamMs += summary.get_total_build_inputstream_ms();
        self.staleReadRegions += summary.get_stale_read_regions();

        self.vectorIdxLoadFromS3 += summary.get_vector_idx_load_from_s3();
        self.vectorIdxLoadFromDisk += summary.get_vector_idx_load_from_disk();
        self.vectorIdxLoadFromCache += summary.get_vector_idx_load_from_cache();
        self.vectorIdxLoadTimeMs += summary.get_vector_idx_load_time_ms();
        self.vectorIdxSearchTimeMs += summary.get_vector_idx_search_time_ms();
        self.vectorIdxSearchVisitedNodes += summary.get_vector_idx_search_visited_nodes();
        self.vectorIdxSearchDiscardedNodes += summary.get_vector_idx_search_discarded_nodes();
        self.vectorIdxReadVecTimeMs += summary.get_vector_idx_read_vec_time_ms();
        self.vectorIdxReadOthersTimeMs += summary.get_vector_idx_read_others_time_ms();

        self.ftsNFromInmemoryNoindex += summary.get_fts_n_from_inmemory_noindex();
        self.ftsNFromTinyIndex += summary.get_fts_n_from_tiny_index();
        self.ftsNFromTinyNoindex += summary.get_fts_n_from_tiny_noindex();
        self.ftsNFromDmfIndex += summary.get_fts_n_from_dmf_index();
        self.ftsNFromDmfNoindex += summary.get_fts_n_from_dmf_noindex();
        self.ftsRowsFromInmemoryNoindex += summary.get_fts_rows_from_inmemory_noindex();
        self.ftsRowsFromTinyIndex += summary.get_fts_rows_from_tiny_index();
        self.ftsRowsFromTinyNoindex += summary.get_fts_rows_from_tiny_noindex();
        self.ftsRowsFromDmfIndex += summary.get_fts_rows_from_dmf_index();
        self.ftsRowsFromDmfNoindex += summary.get_fts_rows_from_dmf_noindex();
        self.ftsIdxLoadTotalMs += summary.get_fts_idx_load_total_ms();
        self.ftsIdxLoadFromCache += summary.get_fts_idx_load_from_cache();
        self.ftsIdxLoadFromColumnFile += summary.get_fts_idx_load_from_column_file();
        self.ftsIdxLoadFromStableS3 += summary.get_fts_idx_load_from_stable_s3();
        self.ftsIdxLoadFromStableDisk += summary.get_fts_idx_load_from_stable_disk();
        self.ftsIdxSearchN += summary.get_fts_idx_search_n();
        self.ftsIdxSearchTotalMs += summary.get_fts_idx_search_total_ms();
        self.ftsIdxDmSearchRows += summary.get_fts_idx_dm_search_rows();
        self.ftsIdxDmTotalReadFtsMs += summary.get_fts_idx_dm_total_read_fts_ms();
        self.ftsIdxDmTotalReadOthersMs += summary.get_fts_idx_dm_total_read_others_ms();
        self.ftsIdxTinySearchRows += summary.get_fts_idx_tiny_search_rows();
        self.ftsIdxTinyTotalReadFtsMs += summary.get_fts_idx_tiny_total_read_fts_ms();
        self.ftsIdxTinyTotalReadOthersMs += summary.get_fts_idx_tiny_total_read_others_ms();
        self.ftsBruteTotalReadMs += summary.get_fts_brute_total_read_ms();
        self.ftsBruteTotalSearchMs += summary.get_fts_brute_total_search_ms();

        self.invertedIdxLoadFromS3 += summary.get_inverted_idx_load_from_s3();
        self.invertedIdxLoadFromDisk += summary.get_inverted_idx_load_from_disk();
        self.invertedIdxLoadFromCache += summary.get_inverted_idx_load_from_cache();
        self.invertedIdxLoadTimeMs += summary.get_inverted_idx_load_time_ms();
        self.invertedIdxSearchTimeMs += summary.get_inverted_idx_search_time_ms();
        self.invertedIdxSearchSkippedPacks += summary.get_inverted_idx_search_skipped_packs();
        self.invertedIdxIndexedRows += summary.get_inverted_idx_indexed_rows();
        self.invertedIdxSearchSelectedRows += summary.get_inverted_idx_search_selected_rows();

        // 从 tipb 摘要合并时同样保留最小 local stream。
        if self.minLocalStreamMs == 0 || summary.get_min_local_stream_ms() < self.minLocalStreamMs {
            self.minLocalStreamMs = summary.get_min_local_stream_ms();
        }
        if summary.get_max_local_stream_ms() > self.maxLocalStreamMs {
            self.maxLocalStreamMs = summary.get_max_local_stream_ms();
        }
        if self.minRemoteStreamMs == 0 || summary.get_min_remote_stream_ms() < self.minRemoteStreamMs {
            self.minRemoteStreamMs = summary.get_min_remote_stream_ms();
        }
        if summary.get_max_remote_stream_ms() > self.maxRemoteStreamMs {
            self.maxRemoteStreamMs = summary.get_max_remote_stream_ms();
        }

        // Go 遍历 summary.get_regions_of_instance()，按 instance_id 聚合 region_num。
        for instance in summary.get_regions_of_instance() {
            *self.regionsOfInstance.entry(instance.get_instance_id().to_string()).or_insert(0) += instance.get_region_num();
        }
    }

    // Empty check whether TiFlashScanContext is Empty, if scan no pack and skip no pack, we regard it as empty
    pub fn Empty(&self) -> bool {
        self.dmfileDataScannedRows == 0
            && self.dmfileDataSkippedRows == 0
            && self.dmfileMvccScannedRows == 0
            && self.dmfileMvccSkippedRows == 0
            && self.dmfileLmFilterScannedRows == 0
            && self.dmfileLmFilterSkippedRows == 0
            && self.localRegions == 0
            && self.remoteRegions == 0
            && self.vectorIdxLoadFromDisk == 0
            && self.vectorIdxLoadFromCache == 0
            && self.vectorIdxLoadFromS3 == 0
            && self.invertedIdxLoadFromDisk == 0
            && self.invertedIdxLoadFromCache == 0
            && self.invertedIdxLoadFromS3 == 0
            && self.ftsNFromInmemoryNoindex == 0
            && self.ftsNFromTinyIndex == 0
            && self.ftsNFromTinyNoindex == 0
            && self.ftsNFromDmfIndex == 0
            && self.ftsNFromDmfNoindex == 0
    }
}

impl TiFlashColumnarScanContext {
    // Clone implements the deep copy of * TiFlashColumnarScanContext
    pub fn Clone(&self) -> TiFlashColumnarScanContext {
        self.clone()
    }

    // String 对应 Go 的 columnar_scan 输出。
    pub fn String(&self) -> String {
        format!(
            "columnar_scan:{{mvcc_input_rows:{}, mvcc_input_bytes:{}, mvcc_output_rows:{}, regions:{}, read_tasks:{}, physical_tables:{}, columns:{}, user_read_bytes:{}, read_block:{}ms, serialize_block:{}ms, init_reader:{}ms, prefetch:{}ms, deserialize_block:{}ms, rough_check:{{total:{}, selected:{}, skipped:{}, unknown:{}}}, remote_segments:{}, total_segments:{}}}",
            self.mvccInputRows,
            self.mvccInputBytes,
            self.mvccOutputRows,
            self.regions,
            self.readTasks,
            self.physicalTables,
            self.columns,
            self.userReadBytes,
            self.totalReadBlockMs,
            self.totalSerializeBlockMs,
            self.totalInitReaderMs,
            self.totalPrefetchMs,
            self.totalDeserializeBlockMs,
            self.roughCheckTotalPacks,
            self.roughCheckSelectedPacks,
            self.roughCheckSkippedPacks,
            self.roughCheckUnknownPacks,
            self.remoteSegments,
            self.totalSegments
        )
    }

    // Merge make sum to merge the information in TiFlashColumnarScanContext
    pub fn Merge(&mut self, other: TiFlashColumnarScanContext) {
        self.hasStats = self.hasStats || other.hasStats;
        self.regions += other.regions;
        self.readTasks += other.readTasks;
        if other.physicalTables > self.physicalTables {
            self.physicalTables = other.physicalTables;
        }
        if other.columns > self.columns {
            self.columns = other.columns;
        }
        self.userReadBytes += other.userReadBytes;
        self.mvccInputRows += other.mvccInputRows;
        self.mvccInputBytes += other.mvccInputBytes;
        self.mvccOutputRows += other.mvccOutputRows;
        self.totalReadBlockMs += other.totalReadBlockMs;
        self.totalSerializeBlockMs += other.totalSerializeBlockMs;
        self.totalInitReaderMs += other.totalInitReaderMs;
        self.totalPrefetchMs += other.totalPrefetchMs;
        self.roughCheckTotalPacks += other.roughCheckTotalPacks;
        self.roughCheckSelectedPacks += other.roughCheckSelectedPacks;
        self.roughCheckSkippedPacks += other.roughCheckSkippedPacks;
        self.roughCheckUnknownPacks += other.roughCheckUnknownPacks;
        self.remoteSegments += other.remoteSegments;
        self.totalSegments += other.totalSegments;
        self.totalDeserializeBlockMs += other.totalDeserializeBlockMs;
    }

    // mergeExecSummary 从 tipb.ColumnarScanContext 按字段累加。
    pub fn mergeExecSummary(&mut self, summary: Option<&tipb::ColumnarScanContext>) {
        let Some(summary) = summary else {
            return;
        };
        self.hasStats = true;
        self.regions += summary.get_regions();
        self.readTasks += summary.get_read_tasks();
        if summary.get_physical_tables() > self.physicalTables {
            self.physicalTables = summary.get_physical_tables();
        }
        if summary.get_columns() > self.columns {
            self.columns = summary.get_columns();
        }
        self.userReadBytes += summary.get_user_read_bytes();
        self.mvccInputRows += summary.get_mvcc_input_rows();
        self.mvccInputBytes += summary.get_mvcc_input_bytes();
        self.mvccOutputRows += summary.get_mvcc_output_rows();
        self.totalReadBlockMs += summary.get_total_read_block_ms();
        self.totalSerializeBlockMs += summary.get_total_serialize_block_ms();
        self.totalInitReaderMs += summary.get_total_init_reader_ms();
        self.totalPrefetchMs += summary.get_total_prefetch_ms();
        self.roughCheckTotalPacks += summary.get_rough_check_total_packs();
        self.roughCheckSelectedPacks += summary.get_rough_check_selected_packs();
        self.roughCheckSkippedPacks += summary.get_rough_check_skipped_packs();
        self.roughCheckUnknownPacks += summary.get_rough_check_unknown_packs();
        self.remoteSegments += summary.get_remote_segments();
        self.totalSegments += summary.get_total_segments();
        self.totalDeserializeBlockMs += summary.get_total_deserialize_block_ms();
    }

    // Empty check whether TiFlashColumnarScanContext is empty.
    pub fn Empty(&self) -> bool {
        !self.hasStats
            && self.regions == 0
            && self.readTasks == 0
            && self.physicalTables == 0
            && self.columns == 0
            && self.userReadBytes == 0
            && self.mvccInputRows == 0
            && self.mvccInputBytes == 0
            && self.mvccOutputRows == 0
            && self.totalReadBlockMs == 0
            && self.totalSerializeBlockMs == 0
            && self.totalInitReaderMs == 0
            && self.totalPrefetchMs == 0
            && self.roughCheckTotalPacks == 0
            && self.roughCheckSelectedPacks == 0
            && self.roughCheckSkippedPacks == 0
            && self.roughCheckUnknownPacks == 0
            && self.remoteSegments == 0
            && self.totalSegments == 0
            && self.totalDeserializeBlockMs == 0
    }
}

// TiFlashWaitSummary is used to express all kinds of wait information in tiflash
#[derive(Clone, Default)]
/// TiFlash 各类等待（minTSO、pipeline breaker/queue）。
pub struct TiFlashWaitSummary {
    // keep execution time to do merge work, always record the wait time with largest execution time
    pub executionTime: u64,
    pub minTSOWaitTime: u64,
    pub pipelineBreakerWaitTime: u64,
    pub pipelineQueueWaitTime: u64,
}

impl TiFlashWaitSummary {
    // Clone implements the deep copy of * TiFlashWaitSummary
    pub fn Clone(&self) -> TiFlashWaitSummary {
        self.clone()
    }

    // String dumps TiFlashWaitSummary info as string
    pub fn String(&self) -> String {
        if self.CanBeIgnored() {
            return String::new();
        }
        let mut parts = Vec::new();
        if self.minTSOWaitTime >= Duration::from_millis(1).as_nanos() as u64 {
            parts.push(format!("minTSO_wait: {}ms", Duration::from_nanos(self.minTSOWaitTime).as_millis()));
        }
        if self.pipelineBreakerWaitTime >= Duration::from_millis(1).as_nanos() as u64 {
            parts.push(format!(
                "pipeline_breaker_wait: {}ms",
                Duration::from_nanos(self.pipelineBreakerWaitTime).as_millis()
            ));
        }
        if self.pipelineQueueWaitTime >= Duration::from_millis(1).as_nanos() as u64 {
            parts.push(format!(
                "pipeline_queue_wait: {}ms",
                Duration::from_nanos(self.pipelineQueueWaitTime).as_millis()
            ));
        }
        format!("tiflash_wait: {{{}}}", parts.join(", "))
    }

    // Merge make sum to merge the information in TiFlashWaitSummary
    pub fn Merge(&mut self, other: TiFlashWaitSummary) {
        // Go 只保留 executionTime 最大那次的 wait summary，而不是简单累加。
        if self.executionTime < other.executionTime {
            self.executionTime = other.executionTime;
            self.minTSOWaitTime = other.minTSOWaitTime;
            self.pipelineBreakerWaitTime = other.pipelineBreakerWaitTime;
            self.pipelineQueueWaitTime = other.pipelineQueueWaitTime;
        }
    }

    // mergeExecSummary 从 protobuf wait summary 读取 ns 级等待时间。
    pub fn mergeExecSummary(&mut self, summary: Option<&tipb::TiFlashWaitSummary>, executionTime: u64) {
        let Some(summary) = summary else {
            return;
        };
        if self.executionTime < executionTime {
            self.executionTime = executionTime;
            self.minTSOWaitTime = summary.get_min_tso_wait_ns();
            self.pipelineBreakerWaitTime = summary.get_pipeline_breaker_wait_ns();
            self.pipelineQueueWaitTime = summary.get_pipeline_queue_wait_ns();
        }
    }

    // CanBeIgnored check whether TiFlashWaitSummary can be ignored, not all tidb executors have significant tiflash wait summary
    pub fn CanBeIgnored(&self) -> bool {
        let ms = Duration::from_millis(1).as_nanos() as u64;
        self.minTSOWaitTime < ms && self.pipelineBreakerWaitTime < ms && self.pipelineQueueWaitTime < ms
    }
}

// TiFlashNetworkTrafficSummary is used to express network traffic in tiflash
#[derive(Clone, Default)]
/// TiFlash 区内/跨区网络收发字节。
pub struct TiFlashNetworkTrafficSummary {
    pub innerZoneSendBytes: u64,
    pub interZoneSendBytes: u64,
    pub innerZoneReceiveBytes: u64,
    pub interZoneReceiveBytes: u64,
}

impl TiFlashNetworkTrafficSummary {
    // UpdateTiKVExecDetails update tikvDetails with TiFlashNetworkTrafficSummary's values
    pub fn UpdateTiKVExecDetails(&self, tikvDetails: Option<&tikvutil::ExecDetails>) {
        let Some(tikvDetails) = tikvDetails else {
            return;
        };
        // Go 对 ExecDetails 中 MPP traffic 字段做 atomic.AddInt64；Rust 保留同样的并发累加语义。
        tikvDetails.UnpackedBytesSentMPPCrossZone.fetch_add(self.interZoneSendBytes as i64, Ordering::SeqCst);
        tikvDetails.UnpackedBytesSentMPPTotal.fetch_add(self.interZoneSendBytes as i64, Ordering::SeqCst);
        tikvDetails.UnpackedBytesSentMPPTotal.fetch_add(self.innerZoneSendBytes as i64, Ordering::SeqCst);
        tikvDetails.UnpackedBytesReceivedMPPCrossZone.fetch_add(self.interZoneReceiveBytes as i64, Ordering::SeqCst);
        tikvDetails.UnpackedBytesReceivedMPPTotal.fetch_add(self.interZoneReceiveBytes as i64, Ordering::SeqCst);
        tikvDetails.UnpackedBytesReceivedMPPTotal.fetch_add(self.innerZoneReceiveBytes as i64, Ordering::SeqCst);
    }

    // Clone implements the deep copy of * TiFlashNetworkTrafficSummary
    pub fn Clone(&self) -> TiFlashNetworkTrafficSummary {
        self.clone()
    }

    // Empty check whether TiFlashNetworkTrafficSummary is Empty, if no any network traffic, we regard it as empty
    pub fn Empty(&self) -> bool {
        self.innerZoneSendBytes == 0
            && self.interZoneSendBytes == 0
            && self.innerZoneReceiveBytes == 0
            && self.interZoneReceiveBytes == 0
    }

    // String dumps TiFlashNetworkTrafficSummary info as string
    pub fn String(&self) -> String {
        let mut parts = Vec::new();
        if self.innerZoneSendBytes != 0 {
            parts.push(format!("inner_zone_send_bytes: {}", self.innerZoneSendBytes as i64));
        }
        if self.interZoneSendBytes != 0 {
            parts.push(format!("inter_zone_send_bytes: {}", self.interZoneSendBytes as i64));
        }
        if self.innerZoneReceiveBytes != 0 {
            parts.push(format!("inner_zone_receive_bytes: {}", self.innerZoneReceiveBytes as i64));
        }
        if self.interZoneReceiveBytes != 0 {
            parts.push(format!("inter_zone_receive_bytes: {}", self.interZoneReceiveBytes as i64));
        }
        format!("tiflash_network: {{{}}}", parts.join(", "))
    }

    // Merge make sum to merge the information in TiFlashNetworkTrafficSummary
    pub fn Merge(&mut self, other: TiFlashNetworkTrafficSummary) {
        self.innerZoneSendBytes += other.innerZoneSendBytes;
        self.interZoneSendBytes += other.interZoneSendBytes;
        self.innerZoneReceiveBytes += other.innerZoneReceiveBytes;
        self.interZoneReceiveBytes += other.interZoneReceiveBytes;
    }

    // mergeExecSummary 从 tipb.TiFlashNetWorkSummary 中累加流量。
    pub fn mergeExecSummary(&mut self, summary: Option<&tipb::TiFlashNetWorkSummary>) {
        let Some(summary) = summary else {
            return;
        };
        self.innerZoneSendBytes += summary.get_inner_zone_send_bytes();
        self.interZoneSendBytes += summary.get_inter_zone_send_bytes();
        self.innerZoneReceiveBytes += summary.get_inner_zone_receive_bytes();
        self.interZoneReceiveBytes += summary.get_inter_zone_receive_bytes();
    }

    // GetInterZoneTrafficBytes returns the inter zone network traffic bytes involved
    // between tiflash instances.
    pub fn GetInterZoneTrafficBytes(&self) -> u64 {
        // NOTE: we only count the inter zone sent bytes here because tiflash count the traffic bytes
        // of all sub request. For each sub request, both side with count the send and recv traffic.
        // So here, we only use the send bytes as the overall traffic to avoid count the traffic twice.
        // While this statistics logic seems a bit weird to me, but this is the tiflash side desicion.
        self.interZoneSendBytes
    }
}

// MergeTiFlashRUConsumption merge execution summaries from selectResponse into ruDetails.
/// 从执行摘要解码 Consumption 并累加到 RUDetails。
pub fn MergeTiFlashRUConsumption(executionSummaries: &[Option<tipb::ExecutorExecutionSummary>], ruDetails: &mut tikvutil::RUDetails) -> Result<(), error::Error> {
    let mut newRUDetails = tikvutil::NewRUDetails();
    for summary in executionSummaries {
        if let Some(summary) = summary {
            if !summary.get_ru_consumption().is_empty() {
                // Go 为每条 summary 新建 resource_manager.Consumption 并 Unmarshal 二进制 RU 消费。
                let mut tiflashRU = resource_manager::Consumption::default();
                if let Err(err) = tiflashRU.merge_from_bytes(summary.get_ru_consumption()) {
                    return Err(err);
                }
                newRUDetails.UpdateTiFlash(&tiflashRU);
            }
        }
    }
    ruDetails.Merge(&newRUDetails);
    Ok(())
}
