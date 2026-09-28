// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// DistSQL KV 请求构造器与表/索引 range 编码。
//
// `RequestBuilder` 在调用 Select 前填充 `KvRequest`：请求类型、payload、key range、
// 会话变量、事务 scope（txn_scope）等。同时提供表 handle / 索引区间到 TiKV key
// 的编码辅助。上方机械翻译草稿保留完整 Go 语义对照；下方为当前可用的简化实现。

// RequestBuilder 如何填充 kv.Request，以及 table/index range 如何编码为 kv.KeyRange；
// 不会发送 KV 请求，也不会真正分配 TiKV 任务，Go 依赖均作为占位模块名保留。

/* Mechanical draft retained for migration history.
use std::collections::HashMap;

// RequestBuilder is used to build a "kv.Request".
// It is called before we issue a kv request by "Select".
// Notice a builder can only be used once unless it returns an error in test.
pub struct RequestBuilder {
    pub Request: kv::Request,
    pub is: Option<infoschema::MetaOnlyInfoSchema>,
    pub err: Option<errors::Error>,
    pub used: bool,

    // SetDAGRequest 调用时同时保存 dag，供 Build 阶段根据 executor 形状调并发。
    pub dag: Option<*mut tipb::DAGRequest>,
}

impl RequestBuilder {
    // Build builds a "kv.Request".
    // Go 版本会做一次性使用检查、补 replica scope、设置 closest-read store label 并校验 txn scope。
    pub fn Build(&mut self) -> Result<*mut kv::Request, errors::Error> {
        if self.used && intest::InTest {
            return Err(errors::Errorf("request builder is already used"));
        }
        self.used = true;
        if self.Request.ReadReplicaScope.is_empty() {
            self.Request.ReadReplicaScope = kv::GlobalReplicaScope.to_string();
        }
        if self.Request.ReplicaRead.IsClosestRead()
            && self.Request.ReadReplicaScope != kv::GlobalReplicaScope
        {
            self.Request.MatchStoreLabels = vec![metapb::StoreLabel {
                Key: placement::DCLabelKey.to_string(),
                Value: self.Request.ReadReplicaScope.clone(),
            }];
        }
        failpoint::Inject("assertRequestBuilderReplicaOption", |val| {
            let assertScope = val.as_string();
            if self.Request.ReplicaRead.IsClosestRead() && assertScope != self.Request.ReadReplicaScope {
                panic!("request builder get staleness option fail");
            }
        });
        if let Err(err) = self.verifyTxnScope() {
            self.err = Some(err);
        }
        if self.Request.KeyRanges.is_none() {
            self.Request.KeyRanges = Some(kv::NewNonPartitionedKeyRanges(Vec::new()));
        }

        if let Some(dag) = self.dag {
            let execCnt = unsafe { (*dag).Executors.len() };
            if execCnt == 1
                && self.Request.KeepOrder
                && self.Request.Concurrency == vardef::DefDistSQLScanConcurrency
            {
                // 简单 scan + keep order 时，Go 将默认并发改成 2，避免返回大量数据时协议层成为瓶颈。
                match unsafe { (*dag).Executors[0].Tp } {
                    tipb::ExecType_TypeTableScan
                    | tipb::ExecType_TypeIndexScan
                    | tipb::ExecType_TypePartitionTableScan => {
                        let oldConcurrency = self.Request.Concurrency;
                        self.Request.Concurrency = 2;
                        failpoint::Inject("testRateLimitActionMockConsumeAndAssert", |val| {
                            if val.as_bool() {
                                self.Request.Concurrency = oldConcurrency;
                            }
                        });
                    }
                    _ => {}
                }
            }
        }

        if let Some(err) = self.err.clone() {
            Err(err)
        } else {
            Ok(&mut self.Request)
        }
    }

    // SetMemTracker sets a memTracker for this request.
    pub fn SetMemTracker(&mut self, tracker: *mut memory::Tracker) -> &mut RequestBuilder {
        self.Request.MemTracker = tracker;
        self
    }

    // SetTableRanges converts table ranges to KeyRanges.
    // Go 注释说明该函数因 BR 依赖暂时保持导出。
    pub fn SetTableRanges(&mut self, tid: i64, tableRanges: Vec<*mut ranger::Range>) -> &mut RequestBuilder {
        if self.err.is_none() {
            self.Request.KeyRanges = Some(kv::NewNonPartitionedKeyRanges(TableRangesToKVRanges(tid, tableRanges)));
        }
        self
    }

    // SetIndexRanges converts index ranges to KeyRanges.
    pub fn SetIndexRanges(
        &mut self,
        dctx: *mut distsqlctx::DistSQLContext,
        tid: i64,
        idxID: i64,
        ranges: Vec<*mut ranger::Range>,
    ) -> &mut RequestBuilder {
        if self.err.is_none() {
            match IndexRangesToKVRanges(dctx, tid, idxID, ranges) {
                Ok(krs) => self.Request.KeyRanges = Some(krs),
                Err(err) => self.err = Some(err),
            }
        }
        self
    }

    // SetIndexRangesForTables converts multiple table index ranges to KeyRanges.
    pub fn SetIndexRangesForTables(
        &mut self,
        dctx: *mut distsqlctx::DistSQLContext,
        tids: Vec<i64>,
        idxID: i64,
        ranges: Vec<*mut ranger::Range>,
    ) -> &mut RequestBuilder {
        if self.err.is_none() {
            match IndexRangesToKVRangesForTables(dctx, tids, idxID, ranges) {
                Ok(krs) => self.Request.KeyRanges = Some(krs),
                Err(err) => self.err = Some(err),
            }
        }
        self
    }

    // SetHandleRanges converts table handle ranges and then forces non-partitioned KeyRanges.
    pub fn SetHandleRanges(
        &mut self,
        dctx: *mut distsqlctx::DistSQLContext,
        tid: i64,
        isCommonHandle: bool,
        ranges: Vec<*mut ranger::Range>,
    ) -> &mut RequestBuilder {
        self.SetHandleRangesForTables(dctx, vec![tid], isCommonHandle, ranges);
        if let Some(krs) = self.Request.KeyRanges.as_mut() {
            if let Err(err) = krs.SetToNonPartitioned() {
                self.err = Some(err);
            }
        }
        self
    }

    // SetHandleRangesForTables converts handle ranges for multiple tables.
    pub fn SetHandleRangesForTables(
        &mut self,
        dctx: *mut distsqlctx::DistSQLContext,
        tid: Vec<i64>,
        isCommonHandle: bool,
        ranges: Vec<*mut ranger::Range>,
    ) -> &mut RequestBuilder {
        if self.err.is_none() {
            match TableHandleRangesToKVRanges(dctx, tid, isCommonHandle, ranges) {
                Ok(krs) => self.Request.KeyRanges = Some(krs),
                Err(err) => self.err = Some(err),
            }
        }
        self
    }

    // SetTableHandles converts sorted handles to KeyRanges and row count hints.
    pub fn SetTableHandles(&mut self, tid: i64, handles: Vec<Box<dyn kv::Handle>>) -> &mut RequestBuilder {
        let (keyRanges, hints) = TableHandlesToKVRanges(tid, handles);
        self.Request.KeyRanges = Some(kv::NewNonParitionedKeyRangesWithHint(keyRanges, hints));
        self
    }

    // SetPartitionsAndHandles converts PartitionHandles to KeyRanges.
    pub fn SetPartitionsAndHandles(&mut self, handles: Vec<Box<dyn kv::Handle>>) -> &mut RequestBuilder {
        let (keyRanges, hints) = PartitionHandlesToKVRanges(handles);
        self.Request.KeyRanges = Some(kv::NewNonParitionedKeyRangesWithHint(keyRanges, hints));
        self
    }

    // SetDAGRequest sets the request type to ReqTypeDAG and marshals DAG data.
    pub fn SetDAGRequest(&mut self, dag: *mut tipb::DAGRequest) -> &mut RequestBuilder {
        if self.err.is_none() {
            self.Request.Tp = kv::ReqTypeDAG;
            self.Request.Cacheable = true;
            match unsafe { (*dag).Marshal() } {
                Ok(data) => self.Request.Data = data,
                Err(err) => self.err = Some(err),
            }
            self.dag = Some(dag);
            let execCnt = unsafe { (*dag).Executors.len() };
            if execCnt != 0 {
                if let Some(limit) = unsafe { (*dag).Executors[execCnt - 1].GetLimit() } {
                    self.Request.LimitSize = limit.GetLimit();
                }
            }

            if execCnt >= 2 {
                // scan -> small limit 时，按 partition 数或 1 缩小并发，保留 Go 的 minimalConcurrency 判断。
                let secondExec = unsafe { &(*dag).Executors[1] };
                if let Some(limit) = secondExec.GetLimit() {
                    if limit.Limit < estimatedRegionRowCount {
                        let mut minimalConcurrency = execCnt == 2;
                        if execCnt > 2 {
                            let limitParent = secondExec.GetParentIdx() as usize;
                            if limitParent > 0 && unsafe { (*dag).Executors[limitParent].IndexLookup.is_some() } {
                                minimalConcurrency = true;
                            }
                        }
                        if minimalConcurrency {
                            self.Request.Concurrency = self
                                .Request
                                .KeyRanges
                                .as_ref()
                                .map(|kr| kr.PartitionNum())
                                .unwrap_or(1);
                        }
                    }
                }
            }
        }
        self
    }

    // SetAnalyzeRequest sets request type to ReqTypeAnalyze and fills analyze data.
    pub fn SetAnalyzeRequest(&mut self, ana: *mut tipb::AnalyzeReq, isoLevel: kv::IsoLevel) -> &mut RequestBuilder {
        if self.err.is_none() {
            self.Request.Tp = kv::ReqTypeAnalyze;
            match unsafe { (*ana).Marshal() } {
                Ok(data) => self.Request.Data = data,
                Err(err) => self.err = Some(err),
            }
            self.Request.NotFillCache = true;
            self.Request.IsolationLevel = isoLevel;
            self.Request.Priority = kv::PriorityLow;
        }
        self
    }

    // SetChecksumRequest sets request type to ReqTypeChecksum and marshals checksum data.
    pub fn SetChecksumRequest(&mut self, checksum: *mut tipb::ChecksumRequest) -> &mut RequestBuilder {
        if self.err.is_none() {
            self.Request.Tp = kv::ReqTypeChecksum;
            match unsafe { (*checksum).Marshal() } {
                Ok(data) => self.Request.Data = data,
                Err(err) => self.err = Some(err),
            }
            self.Request.NotFillCache = true;
        }
        self
    }

    // SetKeyRanges sets KeyRanges for kv.Request.
    pub fn SetKeyRanges(&mut self, keyRanges: Vec<kv::KeyRange>) -> &mut RequestBuilder {
        self.Request.KeyRanges = Some(kv::NewNonPartitionedKeyRanges(keyRanges));
        self
    }

    // SetKeyRangesWithHints sets KeyRanges with row count hints.
    pub fn SetKeyRangesWithHints(&mut self, keyRanges: Vec<kv::KeyRange>, hints: Vec<i32>) -> &mut RequestBuilder {
        self.Request.KeyRanges = Some(kv::NewNonParitionedKeyRangesWithHint(keyRanges, hints));
        self
    }

    // SetWrappedKeyRanges sets already wrapped KeyRanges.
    pub fn SetWrappedKeyRanges(&mut self, keyRanges: kv::KeyRanges) -> &mut RequestBuilder {
        self.Request.KeyRanges = Some(keyRanges);
        self
    }

    // SetPartitionKeyRanges sets partitioned table KeyRanges.
    pub fn SetPartitionKeyRanges(&mut self, keyRanges: Vec<Vec<kv::KeyRange>>) -> &mut RequestBuilder {
        self.Request.KeyRanges = Some(kv::NewPartitionedKeyRanges(keyRanges));
        self
    }

    // SetStartTS sets StartTS.
    pub fn SetStartTS(&mut self, startTS: u64) -> &mut RequestBuilder {
        self.Request.StartTs = startTS;
        self
    }

    // SetDesc sets Desc.
    pub fn SetDesc(&mut self, desc: bool) -> &mut RequestBuilder {
        self.Request.Desc = desc;
        self
    }

    // SetKeepOrder sets KeepOrder.
    pub fn SetKeepOrder(&mut self, order: bool) -> &mut RequestBuilder {
        self.Request.KeepOrder = order;
        self
    }

    // SetStoreType sets StoreType.
    pub fn SetStoreType(&mut self, storeType: kv::StoreType) -> &mut RequestBuilder {
        self.Request.StoreType = storeType;
        self
    }

    // SetAllowBatchCop sets BatchCop.
    pub fn SetAllowBatchCop(&mut self, batchCop: bool) -> &mut RequestBuilder {
        self.Request.BatchCop = batchCop;
        self
    }

    // SetPartitionIDAndRanges sets PartitionIDAndRanges.
    pub fn SetPartitionIDAndRanges(&mut self, partitionIDAndRanges: Vec<kv::PartitionIDAndRanges>) -> &mut RequestBuilder {
        self.Request.PartitionIDAndRanges = partitionIDAndRanges;
        self
    }

    fn getIsolationLevel(&self) -> kv::IsoLevel {
        if self.Request.Tp == kv::ReqTypeAnalyze {
            return kv::RC;
        }
        kv::SI
    }

    fn getKVPriority(&self, dctx: *mut distsqlctx::DistSQLContext) -> i32 {
        match unsafe { (*dctx).Priority } {
            mysql::NoPriority | mysql::DelayedPriority => kv::PriorityNormal,
            mysql::LowPriority => kv::PriorityLow,
            mysql::HighPriority => kv::PriorityHigh,
            _ => kv::PriorityNormal,
        }
    }

    // SetFromSessionVars copies DistSQL/session fields into kv.Request.
    pub fn SetFromSessionVars(&mut self, dctx: *mut distsqlctx::DistSQLContext) -> &mut RequestBuilder {
        let distsqlConcurrency = unsafe { (*dctx).DistSQLConcurrency };
        if self.Request.Concurrency == 0 {
            self.Request.Concurrency = distsqlConcurrency;
        } else if self.Request.Concurrency > distsqlConcurrency {
            self.Request.Concurrency = distsqlConcurrency;
        }
        let mut replicaReadType = unsafe { (*dctx).ReplicaReadType };
        if unsafe { (*dctx).WeakConsistency } {
            self.Request.IsolationLevel = kv::RC;
        } else if unsafe { (*dctx).RCCheckTS } {
            self.Request.IsolationLevel = kv::RCCheckTS;
            replicaReadType = kv::ReplicaReadLeader;
        } else {
            self.Request.IsolationLevel = self.getIsolationLevel();
        }
        self.Request.NotFillCache = unsafe { (*dctx).NotFillCache };
        self.Request.TaskID = unsafe { (*dctx).TaskID };
        self.Request.Priority = self.getKVPriority(dctx);
        self.Request.ReplicaRead = replicaReadType;
        self.SetResourceGroupTagger(unsafe { (*dctx).ResourceGroupTagger });
        self.SetPaging(unsafe { (*dctx).EnablePaging });
        self.Request.Paging.MinPagingSize = unsafe { (*dctx).MinPagingSize as u64 };
        self.Request.Paging.MaxPagingSize = unsafe { (*dctx).MaxPagingSize as u64 };
        self.Request.Paging.PagingSizeBytes = unsafe { (*dctx).PagingSizeBytes as u64 };
        self.Request.RequestSource.RequestSourceInternal = unsafe { (*dctx).InRestrictedSQL };
        self.Request.RequestSource.RequestSourceType = unsafe { (*dctx).RequestSourceType.clone() };
        self.Request.RequestSource.ExplicitRequestSourceType = unsafe { (*dctx).ExplicitRequestSourceType.clone() };
        self.Request.StoreBatchSize = unsafe { (*dctx).StoreBatchSize };
        self.Request.ResourceGroupName = unsafe { (*dctx).ResourceGroupName.clone() };
        self.Request.StoreBusyThreshold = unsafe { (*dctx).LoadBasedReplicaReadThreshold };
        self.Request.RunawayChecker = unsafe { (*dctx).RunawayChecker.as_ref() }
            .and_then(|value| value.downcast_ref::<kv::resourcegroup::SharedRunawayChecker>())
            .cloned();
        self.Request.TiKVClientReadTimeout = unsafe { (*dctx).TiKVClientReadTimeout };
        self.Request.MaxExecutionTime = unsafe { (*dctx).MaxExecutionTime };
        self.Request.MaxKeysRead = unsafe { (*dctx).MaxKeysRead };
        self.Request.MaxKeysReadCounter = unsafe { (*dctx).MaxKeysReadCounter };
        self
    }

    // SetPaging sets Paging flag.
    pub fn SetPaging(&mut self, paging: bool) -> &mut RequestBuilder {
        self.Request.Paging.Enable = paging;
        self
    }

    // SetConcurrency sets Concurrency.
    pub fn SetConcurrency(&mut self, concurrency: i32) -> &mut RequestBuilder {
        self.Request.Concurrency = concurrency;
        self
    }

    // SetCoprRequestRateLimit sets a shared in-flight cop request limiter.
    pub fn SetCoprRequestRateLimit(&mut self, rateLimit: *mut util::RateLimit) -> &mut RequestBuilder {
        self.Request.CoprRequestRateLimit = rateLimit;
        self
    }

    // SetTiDBServerID sets TiDBServerID.
    // ServerID is a unique id of TiDB instance among the cluster.
    pub fn SetTiDBServerID(&mut self, serverID: u64) -> &mut RequestBuilder {
        self.Request.TiDBServerID = serverID;
        self
    }

    // SetFromInfoSchema sets schema metadata version from infoSchema.
    pub fn SetFromInfoSchema(&mut self, is: infoschema::MetaOnlyInfoSchema) -> &mut RequestBuilder {
        self.Request.SchemaVar = is.SchemaMetaVersion();
        self.is = Some(is);
        self
    }

    // SetResourceGroupTagger sets request resource group tagger.
    pub fn SetResourceGroupTagger(&mut self, tagger: *mut kv::ResourceGroupTagBuilder) -> &mut RequestBuilder {
        self.Request.ResourceGroupTagger = tagger;
        self
    }

    // SetResourceGroupName sets request resource group name.
    pub fn SetResourceGroupName(&mut self, name: String) -> &mut RequestBuilder {
        self.Request.ResourceGroupName = name;
        self
    }

    // SetRequestSource sets request source.
    pub fn SetRequestSource(&mut self, reqSource: util::RequestSource) -> &mut RequestBuilder {
        self.Request.RequestSource = reqSource;
        self
    }

    // SetExplicitRequestSourceType sets explicit request source type.
    pub fn SetExplicitRequestSourceType(&mut self, sourceType: String) -> &mut RequestBuilder {
        self.Request.RequestSource.ExplicitRequestSourceType = sourceType;
        self
    }

    // verifyTxnScope 校验访问的物理表是否符合 txn_scope 与 placement leader DC 规则。
    fn verifyTxnScope(&self) -> Result<(), errors::Error> {
        let txnScope = self.Request.TxnScope.clone();
        if txnScope.is_empty() || txnScope == kv::GlobalReplicaScope || self.is.is_none() {
            return Ok(());
        }
        let mut visitPhysicalTableID: HashMap<i64, ()> = HashMap::new();
        let tids = tablecodec::VerifyTableIDForRanges(self.Request.KeyRanges.clone())?;
        for tid in tids {
            visitPhysicalTableID.insert(tid, ());
        }

        for phyTableID in visitPhysicalTableID.keys() {
            let valid = VerifyTxnScope(txnScope.clone(), *phyTableID, self.is.as_ref().unwrap());
            if !valid {
                let mut tblName = String::new();
                let mut partName = String::new();
                let (tblInfo, _, partInfo) = self.is.as_ref().unwrap().FindTableInfoByPartitionID(*phyTableID);
                if tblInfo.is_some() && partInfo.is_some() {
                    tblName = tblInfo.unwrap().Name.String();
                    partName = partInfo.unwrap().Name.String();
                } else {
                    let (tblInfo, _) = self.is.as_ref().unwrap().TableInfoByID(*phyTableID);
                    tblName = tblInfo.Name.String();
                }
                if !partName.is_empty() {
                    return Err(errors::Errorf(format!(
                        "table {}'s partition {} can not be read by {} txn_scope",
                        tblName, partName, txnScope
                    )));
                }
                return Err(errors::Errorf(format!(
                    "table {} can not be read by {} txn_scope",
                    tblName, txnScope
                )));
            }
        }
        Ok(())
    }

    // SetTxnScope sets request TxnScope.
    pub fn SetTxnScope(&mut self, scope: String) -> &mut RequestBuilder {
        self.Request.TxnScope = scope;
        self
    }

    // SetReadReplicaScope sets request ReadReplicaScope.
    pub fn SetReadReplicaScope(&mut self, scope: String) -> &mut RequestBuilder {
        self.Request.ReadReplicaScope = scope;
        self
    }

    // SetIsStaleness sets request IsStaleness.
    pub fn SetIsStaleness(&mut self, is: bool) -> &mut RequestBuilder {
        self.Request.IsStaleness = is;
        self
    }

    // SetClosestReplicaReadAdjuster sets request CoprRequestAdjuster.
    pub fn SetClosestReplicaReadAdjuster(&mut self, chkFn: kv::CoprRequestAdjuster) -> &mut RequestBuilder {
        self.Request.ClosestReplicaReadAdjuster = chkFn;
        self
    }

    // SetConnIDAndConnAlias sets connection id and alias.
    pub fn SetConnIDAndConnAlias(&mut self, connID: u64, connAlias: String) -> &mut RequestBuilder {
        self.Request.ConnID = connID;
        self.Request.ConnAlias = connAlias;
        self
    }
}

// estimatedRegionRowCount 对应 Go 常量，用于小 limit 时调整并发。
pub const estimatedRegionRowCount: u64 = 100000;

// TableHandleRangesToKVRanges converts table handle ranges to KeyRanges for multiple tables.
pub fn TableHandleRangesToKVRanges(
    dctx: *mut distsqlctx::DistSQLContext,
    tid: Vec<i64>,
    isCommonHandle: bool,
    ranges: Vec<*mut ranger::Range>,
) -> Result<kv::KeyRanges, errors::Error> {
    if !isCommonHandle {
        return Ok(tablesRangesToKVRanges(tid, ranges));
    }
    CommonHandleRangesToKVRanges(dctx, tid, ranges)
}

// TableRangesToKVRanges converts table ranges to KeyRange.
pub fn TableRangesToKVRanges(tid: i64, ranges: Vec<*mut ranger::Range>) -> Vec<kv::KeyRange> {
    if ranges.is_empty() {
        return Vec::new();
    }
    tablesRangesToKVRanges(vec![tid], ranges).FirstPartitionRange()
}

// tablesRangesToKVRanges converts table ranges to partitioned KeyRanges.
pub fn tablesRangesToKVRanges(tids: Vec<i64>, ranges: Vec<*mut ranger::Range>) -> kv::KeyRanges {
    tableRangesToKVRangesWithoutSplit(tids, ranges)
}

fn tableRangesToKVRangesWithoutSplit(tids: Vec<i64>, ranges: Vec<*mut ranger::Range>) -> kv::KeyRanges {
    let mut krs: Vec<Vec<kv::KeyRange>> = tids.iter().map(|_| Vec::with_capacity(ranges.len())).collect();
    for ran in ranges {
        let (low, high) = encodeHandleKey(ran);
        for (i, tid) in tids.iter().enumerate() {
            let startKey = tablecodec::EncodeRowKey(*tid, low.clone());
            let endKey = tablecodec::EncodeRowKey(*tid, high.clone());
            krs[i].push(kv::KeyRange { StartKey: startKey, EndKey: endKey });
        }
    }
    kv::NewPartitionedKeyRanges(krs)
}

// encodeHandleKey 编码 handle range 边界，并按 LowExclude/HighExclude 调整 PrefixNext。
fn encodeHandleKey(ran: *mut ranger::Range) -> (Vec<u8>, Vec<u8>) {
    let mut low = codec::EncodeInt(Vec::new(), unsafe { (*ran).LowVal[0].GetInt64() });
    let mut high = codec::EncodeInt(Vec::new(), unsafe { (*ran).HighVal[0].GetInt64() });
    if unsafe { (*ran).LowExclude } {
        low = kv::Key(low).PrefixNext();
    }
    if unsafe { !(*ran).HighExclude } {
        high = kv::Key(high).PrefixNext();
    }
    (low, high)
}

// SplitRangesAcrossInt64Boundary 将 unsigned handle ranges 按 MaxInt64 边界拆成 signed/unsigned 两组。
pub fn SplitRangesAcrossInt64Boundary(
    ranges: Vec<*mut ranger::Range>,
    keepOrder: bool,
    desc: bool,
    isCommonHandle: bool,
) -> (Vec<*mut ranger::Range>, Vec<*mut ranger::Range>) {
    if isCommonHandle
        || ranges.is_empty()
        || unsafe { (*ranges[0]).LowVal[0].Kind() == types::KindInt64 }
    {
        return (ranges, Vec::new());
    }
    let idx = ranges
        .iter()
        .position(|ran| unsafe { (**ran).HighVal[0].GetUint64() > math::MaxInt64 as u64 })
        .unwrap_or(ranges.len());
    if idx == ranges.len() {
        return (ranges, Vec::new());
    }
    if unsafe { (*ranges[idx]).LowVal[0].GetUint64() > math::MaxInt64 as u64 } {
        let signedRanges = ranges[..idx].to_vec();
        let unsignedRanges = ranges[idx..].to_vec();
        if !keepOrder {
            let mut merged = unsignedRanges.clone();
            merged.extend(signedRanges);
            return (merged, Vec::new());
        }
        if desc {
            return (unsignedRanges, signedRanges);
        }
        return (signedRanges, unsignedRanges);
    }

    // 当前 range 跨越 int64 边界，需要拆出 <= MaxInt64 与 > MaxInt64 两段。
    let mut signedRanges = Vec::with_capacity(idx + 1);
    let mut unsignedRanges = Vec::with_capacity(ranges.len() - idx);
    signedRanges.extend_from_slice(&ranges[..idx]);
    if !(unsafe { (*ranges[idx]).LowVal[0].GetUint64() == math::MaxInt64 as u64 && (*ranges[idx]).LowExclude }) {
        signedRanges.push(Box::into_raw(Box::new(ranger::Range {
            LowVal: unsafe { (*ranges[idx]).LowVal.clone() },
            LowExclude: unsafe { (*ranges[idx]).LowExclude },
            HighVal: vec![types::NewUintDatum(math::MaxInt64 as u64)],
            Collators: unsafe { (*ranges[idx]).Collators.clone() },
            ..Default::default()
        })));
    }
    if !(unsafe { (*ranges[idx]).HighVal[0].GetUint64() == math::MaxInt64 as u64 + 1 && (*ranges[idx]).HighExclude }) {
        unsignedRanges.push(Box::into_raw(Box::new(ranger::Range {
            LowVal: vec![types::NewUintDatum(math::MaxInt64 as u64 + 1)],
            HighVal: unsafe { (*ranges[idx]).HighVal.clone() },
            HighExclude: unsafe { (*ranges[idx]).HighExclude },
            Collators: unsafe { (*ranges[idx]).Collators.clone() },
            ..Default::default()
        })));
    }
    if idx < ranges.len() {
        unsignedRanges.extend_from_slice(&ranges[idx + 1..]);
    }
    if !keepOrder {
        let mut merged = unsignedRanges.clone();
        merged.extend(signedRanges);
        return (merged, Vec::new());
    }
    if desc {
        return (unsignedRanges, signedRanges);
    }
    (signedRanges, unsignedRanges)
}

// TableHandlesToKVRanges converts sorted handles to kv ranges and merges continuous int handles.
pub fn TableHandlesToKVRanges(mut tid: i64, handles: Vec<Box<dyn kv::Handle>>) -> (Vec<kv::KeyRange>, Vec<i32>) {
    let mut krs = Vec::with_capacity(handles.len());
    let mut hints = Vec::with_capacity(handles.len());
    let mut i = 0;
    while i < handles.len() {
        let mut commonHandle = handles[i].as_common_handle();
        if let Some(partitionHandle) = handles[i].as_partition_handle() {
            tid = partitionHandle.PartitionID;
            commonHandle = partitionHandle.Handle.as_common_handle();
        }
        if let Some(common) = commonHandle {
            krs.push(kv::KeyRange {
                StartKey: tablecodec::EncodeRowKey(tid, common.Encoded()),
                EndKey: tablecodec::EncodeRowKey(tid, kv::Key(common.Encoded()).Next()),
            });
            hints.push(1);
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < handles.len() && handles[j - 1].IntValue() != math::MaxInt64 {
            if let Some(p) = handles[j].as_partition_handle() {
                if p.PartitionID != tid {
                    break;
                }
            }
            if handles[j].IntValue() != handles[j - 1].IntValue() + 1 {
                break;
            }
            j += 1;
        }
        let low = codec::EncodeInt(Vec::new(), handles[i].IntValue());
        let high = kv::Key(codec::EncodeInt(Vec::new(), handles[j - 1].IntValue())).PrefixNext();
        krs.push(kv::KeyRange {
            StartKey: tablecodec::EncodeRowKey(tid, low),
            EndKey: tablecodec::EncodeRowKey(tid, high),
        });
        hints.push((j - i) as i32);
        i = j;
    }
    (krs, hints)
}

// PartitionHandlesToKVRanges converts PartitionHandles to kv ranges.
pub fn PartitionHandlesToKVRanges(handles: Vec<Box<dyn kv::Handle>>) -> (Vec<kv::KeyRange>, Vec<i32>) {
    let mut krs = Vec::with_capacity(handles.len());
    let mut hints = Vec::with_capacity(handles.len());
    let mut i = 0;
    while i < handles.len() {
        let ph = handles[i].as_partition_handle().unwrap();
        let h = ph.Handle;
        let pid = ph.PartitionID;
        if let Some(commonHandle) = h.as_common_handle() {
            krs.push(kv::KeyRange {
                StartKey: tablecodec::EncodeRowKey(pid, commonHandle.Encoded()),
                EndKey: tablecodec::EncodeRowKey(pid, append(commonHandle.Encoded(), 0)),
            });
            hints.push(1);
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < handles.len() && handles[j - 1].IntValue() != math::MaxInt64 {
            if handles[j].IntValue() != handles[j - 1].IntValue() + 1 {
                break;
            }
            if handles[j].as_partition_handle().unwrap().PartitionID != pid {
                break;
            }
            j += 1;
        }
        let low = codec::EncodeInt(Vec::new(), handles[i].IntValue());
        let high = kv::Key(codec::EncodeInt(Vec::new(), handles[j - 1].IntValue())).PrefixNext();
        krs.push(kv::KeyRange {
            StartKey: tablecodec::EncodeRowKey(pid, low),
            EndKey: tablecodec::EncodeRowKey(pid, high),
        });
        hints.push((j - i) as i32);
        i = j;
    }
    (krs, hints)
}

// IndexRangesToKVRanges converts index ranges to KeyRanges.
pub fn IndexRangesToKVRanges(
    dctx: *mut distsqlctx::DistSQLContext,
    tid: i64,
    idxID: i64,
    ranges: Vec<*mut ranger::Range>,
) -> Result<kv::KeyRanges, errors::Error> {
    IndexRangesToKVRangesWithInterruptSignal(dctx, tid, idxID, ranges, None, None)
}

// IndexRangesToKVRangesWithInterruptSignal converts index ranges and can be interrupted by interruptSignal.
pub fn IndexRangesToKVRangesWithInterruptSignal(
    dctx: *mut distsqlctx::DistSQLContext,
    tid: i64,
    idxID: i64,
    ranges: Vec<*mut ranger::Range>,
    memTracker: Option<*mut memory::Tracker>,
    interruptSignal: Option<*mut atomic::Value>,
) -> Result<kv::KeyRanges, errors::Error> {
    let mut keyRanges = indexRangesToKVRangesForTablesWithInterruptSignal(
        dctx,
        vec![tid],
        idxID,
        ranges,
        memTracker,
        interruptSignal,
    )?;
    keyRanges.SetToNonPartitioned()?;
    Ok(keyRanges)
}

// IndexRangesToKVRangesForTables converts indexes ranges to KeyRange.
pub fn IndexRangesToKVRangesForTables(
    dctx: *mut distsqlctx::DistSQLContext,
    tids: Vec<i64>,
    idxID: i64,
    ranges: Vec<*mut ranger::Range>,
) -> Result<kv::KeyRanges, errors::Error> {
    indexRangesToKVRangesForTablesWithInterruptSignal(dctx, tids, idxID, ranges, None, None)
}

// indexRangesToKVRangesForTablesWithInterruptSignal 保留 Go 的 wrapper 层。
fn indexRangesToKVRangesForTablesWithInterruptSignal(
    dctx: *mut distsqlctx::DistSQLContext,
    tids: Vec<i64>,
    idxID: i64,
    ranges: Vec<*mut ranger::Range>,
    memTracker: Option<*mut memory::Tracker>,
    interruptSignal: Option<*mut atomic::Value>,
) -> Result<kv::KeyRanges, errors::Error> {
    indexRangesToKVWithoutSplit(dctx, tids, idxID, ranges, memTracker, interruptSignal)
}

// CommonHandleRangesToKVRanges encodes common handle ranges as row key ranges.
pub fn CommonHandleRangesToKVRanges(
    dctx: *mut distsqlctx::DistSQLContext,
    tids: Vec<i64>,
    ranges: Vec<*mut ranger::Range>,
) -> Result<kv::KeyRanges, errors::Error> {
    let mut rans = Vec::with_capacity(ranges.len());
    for ran in ranges {
        let (low, high) = EncodeIndexKey(dctx, ran)?;
        rans.push(Box::into_raw(Box::new(ranger::Range {
            LowVal: vec![types::NewBytesDatum(low)],
            HighVal: vec![types::NewBytesDatum(high)],
            LowExclude: false,
            HighExclude: true,
            Collators: collate::GetBinaryCollatorSlice(1),
            ..Default::default()
        })));
    }
    let mut krs: Vec<Vec<kv::KeyRange>> = tids.iter().map(|_| Vec::with_capacity(rans.len())).collect();
    for ran in rans {
        let mut low = unsafe { (*ran).LowVal[0].GetBytes() };
        let high = unsafe { (*ran).HighVal[0].GetBytes() };
        if unsafe { (*ran).LowExclude } {
            low = kv::Key(low).PrefixNext();
        }
        unsafe { (*ran).LowVal[0].SetBytes(low.clone()) };
        for (i, tid) in tids.iter().enumerate() {
            krs[i].push(kv::KeyRange {
                StartKey: tablecodec::EncodeRowKey(*tid, low.clone()),
                EndKey: tablecodec::EncodeRowKey(*tid, high.clone()),
            });
        }
    }
    Ok(kv::NewPartitionedKeyRanges(krs))
}

// VerifyTxnScope verifies whether txnScope and visited physical table break leader rule dcLocation.
pub fn VerifyTxnScope(txnScope: String, physicalTableID: i64, is: &infoschema::MetaOnlyInfoSchema) -> bool {
    if txnScope.is_empty() || txnScope == kv::GlobalTxnScope {
        return true;
    }
    let (bundle, ok) = is.PlacementBundleByPhysicalTableID(physicalTableID);
    if !ok {
        return true;
    }
    let (leaderDC, ok) = bundle.GetLeaderDC(placement::DCLabelKey);
    if !ok {
        return true;
    }
    leaderDC == txnScope
}

// indexRangesToKVWithoutSplit encodes every index range for every table ID, and periodically checks kill signal.
fn indexRangesToKVWithoutSplit(
    dctx: *mut distsqlctx::DistSQLContext,
    tids: Vec<i64>,
    idxID: i64,
    ranges: Vec<*mut ranger::Range>,
    memTracker: Option<*mut memory::Tracker>,
    interruptSignal: Option<*mut atomic::Value>,
) -> Result<kv::KeyRanges, errors::Error> {
    let mut krs: Vec<Vec<kv::KeyRange>> = tids.iter().map(|_| Vec::with_capacity(ranges.len())).collect();
    if let Some(tracker) = memTracker {
        unsafe { (*tracker).Consume(core::mem::size_of::<kv::KeyRange>() as i64 * ranges.len() as i64) };
    }
    const checkSignalStep: usize = 8;
    let mut estimatedMemUsage: i64 = 0;
    for (i, ran) in ranges.iter().enumerate() {
        let (low, high) = EncodeIndexKey(dctx, *ran)?;
        if i == 0 {
            estimatedMemUsage += (low.capacity() + high.capacity()) as i64;
        }
        for (j, tid) in tids.iter().enumerate() {
            let startKey = tablecodec::EncodeIndexSeekKey(*tid, idxID, low.clone());
            let endKey = tablecodec::EncodeIndexSeekKey(*tid, idxID, high.clone());
            if i == 0 {
                estimatedMemUsage += (startKey.capacity() + endKey.capacity()) as i64;
            }
            krs[j].push(kv::KeyRange { StartKey: startKey, EndKey: endKey });
        }
        if i % checkSignalStep == 0 {
            if i == 0 {
                if let Some(tracker) = memTracker {
                    estimatedMemUsage *= ranges.len() as i64;
                    unsafe { (*tracker).Consume(estimatedMemUsage) };
                }
            }
            if let Some(signal) = interruptSignal {
                if unsafe { (*signal).Load().as_bool() } {
                    return Ok(kv::NewPartitionedKeyRanges(Vec::new()));
                }
            }
            if let Some(tracker) = memTracker {
                unsafe { (*tracker).HandleKillSignal() };
            }
        }
    }
    Ok(kv::NewPartitionedKeyRanges(krs))
}

// EncodeIndexKey encodes ranger low/high datum into index seek key payload.
pub fn EncodeIndexKey(
    dctx: *mut distsqlctx::DistSQLContext,
    ran: *mut ranger::Range,
) -> Result<(Vec<u8>, Vec<u8>), errors::Error> {
    let mut tz = time::UTC;
    let mut errCtx = errctx::StrictNoWarningContext;
    if !dctx.is_null() {
        tz = unsafe { (*dctx).Location };
        errCtx = unsafe { (*dctx).ErrCtx };
    }

    let mut low = errCtx.HandleError(codec::EncodeKey(tz, Vec::new(), unsafe { (*ran).LowVal.clone() }))?;
    if unsafe { (*ran).LowExclude } {
        low = kv::Key(low).PrefixNext();
    }
    let mut high = errCtx.HandleError(codec::EncodeKey(tz, Vec::new(), unsafe { (*ran).HighVal.clone() }))?;
    if unsafe { !(*ran).HighExclude } {
        high = kv::Key(high).PrefixNext();
    }
    Ok((low, high))
}

// BuildTableRanges returns ranges encompassing the entire table and partitions if any.
pub fn BuildTableRanges(tbl: *mut model::TableInfo) -> Result<Vec<kv::KeyRange>, errors::Error> {
    let pis = unsafe { (*tbl).GetPartitionInfo() };
    if pis.is_none() {
        // 无 partition 时走短路径，只构造表自身 handle/index ranges。
        return appendRanges(tbl, unsafe { (*tbl).ID });
    }

    let pis = pis.unwrap();
    let mut ranges = Vec::with_capacity(
        pis.Definitions.len() * (unsafe { (*tbl).Indices.len() } + 1) + 1,
    );
    // Global index 使用 table ID 编码，之后每个 partition 再分别追加自己的 ranges。
    for idx in unsafe { &(*tbl).Indices } {
        if idx.State != model::StatePublic || !idx.Global {
            continue;
        }
        let idxRanges = IndexRangesToKVRanges(std::ptr::null_mut(), unsafe { (*tbl).ID }, idx.ID, ranger::FullRange())?;
        ranges = idxRanges.AppendSelfTo(ranges);
    }

    for def in pis.Definitions {
        let rgs = appendRanges(tbl, def.ID).map_err(errors::Trace)?;
        ranges.extend(rgs);
    }
    Ok(ranges)
}

// appendRanges appends full handle range and all public local index ranges for a table/partition ID.
fn appendRanges(tbl: *mut model::TableInfo, tblID: i64) -> Result<Vec<kv::KeyRange>, errors::Error> {
    let mut ranges = if unsafe { (*tbl).IsCommonHandle } {
        ranger::FullNotNullRange()
    } else {
        ranger::FullIntRange(false)
    };

    let mut retRanges = Vec::with_capacity(1 + unsafe { (*tbl).Indices.len() });
    let kvRanges = TableHandleRangesToKVRanges(std::ptr::null_mut(), vec![tblID], unsafe { (*tbl).IsCommonHandle }, ranges)
        .map_err(errors::Trace)?;
    retRanges = kvRanges.AppendSelfTo(retRanges);

    for index in unsafe { &(*tbl).Indices } {
        if index.State != model::StatePublic || index.Global {
            continue;
        }
        ranges = ranger::FullRange();
        let idxRanges = IndexRangesToKVRanges(std::ptr::null_mut(), tblID, index.ID, ranges)
            .map_err(errors::Trace)?;
        retRanges = idxRanges.AppendSelfTo(retRanges);
    }
    Ok(retRanges)
}
*/

use std::sync::Arc;
use std::time::Duration;

use crate::{DistSqlError, DistSqlResult, KeyRange, RequestType, StoreType};

/// 事务隔离级别（简化枚举，对齐常见 Snapshot / ReadCommitted）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IsolationLevel {
    /// 快照隔离（SI）：读已提交快照。
    #[default]
    Snapshot,
    /// 读已提交（RC）。
    ReadCommitted,
}
/// KV 请求优先级。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}
/// 请求载荷：DAG / Analyze / Checksum 的序列化字节。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestPayload {
    /// 尚未设置 payload；Go 的 `kv.Request` 允许构造后再补充请求类型。
    Empty,
    /// DAG（算子图）序列化数据。
    Dag(Vec<u8>),
    /// Analyze 请求序列化数据。
    Analyze(Vec<u8>),
    /// Checksum 请求序列化数据。
    Checksum(Vec<u8>),
}
/// 分区 ID 与其对应的 key ranges（分区表扫描用）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionIDAndRanges {
    /// 物理分区表 ID。
    pub partition_id: i64,
    /// 该分区下的扫描区间。
    pub ranges: Vec<KeyRange>,
}

/// 发往存储层的完整 KV 请求描述。
#[derive(Clone, Debug)]
pub struct KvRequest {
    /// 请求类型。
    pub request_type: RequestType,
    /// 序列化后的请求体。
    pub payload: RequestPayload,
    /// 非分区 key ranges。
    pub key_ranges: Vec<KeyRange>,
    /// 按分区组织的 ranges。
    pub partition_ranges: Vec<PartitionIDAndRanges>,
    /// 事务 start_ts（MVCC 可见性）。
    pub start_ts: u64,
    /// 是否降序。
    pub descending: bool,
    /// 是否保序。
    pub keep_order: bool,
    /// Coprocessor 并发。
    pub concurrency: usize,
    /// 目标存储。
    pub store_type: StoreType,
    /// 隔离级别。
    pub isolation_level: IsolationLevel,
    /// 优先级。
    pub priority: Priority,
    /// 是否禁止写入 coprocessor cache（Analyze/Checksum 默认开启）。
    pub not_fill_cache: bool,
    /// 是否流式。
    pub streaming: bool,
    /// 是否启用分页拉取。
    pub paging: bool,
    /// 是否允许 Batch Cop（批量 coprocessor）。
    pub allow_batch_cop: bool,
    /// 超时。
    pub timeout: Duration,
    /// 事务作用域（如 global 或具体 DC）。
    pub txn_scope: String,
    /// 读副本作用域。
    pub read_replica_scope: String,
    /// 是否 stale read（读历史快照）。
    pub is_staleness: bool,
    /// 资源组名称。
    pub resource_group_name: String,
    /// 请求来源标签。
    pub request_source: String,
    /// 显式请求来源类型。
    pub explicit_source_type: String,
    /// TiDB 实例 server id。
    pub server_id: u64,
    /// 连接 ID。
    pub connection_id: u64,
    /// 连接别名。
    pub connection_alias: String,
    /// 各 range 预估行数提示。
    pub key_range_hints: Vec<usize>,
}

/// 从会话变量拷贝到请求的 DistSQL 相关字段子集。
#[derive(Clone, Debug)]
pub struct SessionVars {
    pub isolation_level: IsolationLevel,
    pub priority: Priority,
    pub concurrency: usize,
    pub streaming: bool,
    pub paging: bool,
    pub allow_batch_cop: bool,
    pub txn_scope: String,
    pub read_replica_scope: String,
    pub is_staleness: bool,
    pub resource_group_name: String,
    pub request_source: String,
}
impl Default for SessionVars {
    fn default() -> Self {
        Self {
            isolation_level: IsolationLevel::Snapshot,
            priority: Priority::Normal,
            concurrency: 15,
            streaming: false,
            paging: false,
            allow_batch_cop: false,
            txn_scope: "global".into(),
            read_replica_scope: "global".into(),
            is_staleness: false,
            resource_group_name: String::new(),
            request_source: String::new(),
        }
    }
}

/// 完整版请求构造器：一次性使用，可携带延迟错误与 txn_scope 校验器。
#[derive(Default)]
pub struct RequestBuilder {
    request_type: Option<RequestType>,
    payload: Option<RequestPayload>,
    key_ranges: Vec<KeyRange>,
    partition_ranges: Vec<PartitionIDAndRanges>,
    start_ts: u64,
    descending: bool,
    keep_order: bool,
    concurrency: usize,
    store_type: Option<StoreType>,
    isolation_level: IsolationLevel,
    priority: Priority,
    not_fill_cache: bool,
    streaming: bool,
    paging: bool,
    allow_batch_cop: bool,
    timeout: Option<Duration>,
    txn_scope: String,
    read_replica_scope: String,
    is_staleness: bool,
    resource_group_name: String,
    request_source: String,
    explicit_source_type: String,
    server_id: u64,
    connection_id: u64,
    connection_alias: String,
    hints: Vec<usize>,
    used: bool,
    error: Option<DistSqlError>,
    scope_checker: Option<Arc<dyn TxnScopeChecker>>,
}

impl RequestBuilder {
    /// 创建构造器并设置 Go `kv.Request` 的零值语义。
    pub fn new() -> Self {
        Self {
            concurrency: 0,
            isolation_level: IsolationLevel::Snapshot,
            priority: Priority::Normal,
            txn_scope: "global".into(),
            read_replica_scope: "global".into(),
            ..Self::default()
        }
    }
    /// 构建最终 KvRequest；同一 builder 成功后不可复用。
    pub fn Build(&mut self) -> DistSqlResult<KvRequest> {
        if self.used {
            return Err(DistSqlError("RequestBuilder cannot be reused".into()));
        }
        self.used = true;
        // 延迟错误优先返回，并允许调用方重试（重置 used）。
        if let Some(error) = self.error.take() {
            self.used = false;
            return Err(error);
        }
        self.verifyTxnScope()?;
        let request_type = self.request_type.unwrap_or(RequestType::Dag);
        let payload = self.payload.clone().unwrap_or(RequestPayload::Empty);
        Ok(KvRequest {
            request_type,
            payload,
            key_ranges: self.key_ranges.clone(),
            partition_ranges: self.partition_ranges.clone(),
            start_ts: self.start_ts,
            descending: self.descending,
            keep_order: self.keep_order,
            concurrency: self.concurrency,
            store_type: self.store_type.unwrap_or(StoreType::TiKv),
            isolation_level: self.isolation_level,
            priority: self.priority,
            not_fill_cache: self.not_fill_cache,
            streaming: self.streaming,
            paging: self.paging,
            allow_batch_cop: self.allow_batch_cop,
            timeout: self.timeout.unwrap_or(Duration::from_secs(60)),
            txn_scope: self.txn_scope.clone(),
            read_replica_scope: self.read_replica_scope.clone(),
            is_staleness: self.is_staleness,
            resource_group_name: self.resource_group_name.clone(),
            request_source: self.request_source.clone(),
            explicit_source_type: self.explicit_source_type.clone(),
            server_id: self.server_id,
            connection_id: self.connection_id,
            connection_alias: self.connection_alias.clone(),
            key_range_hints: self.hints.clone(),
        })
    }
    /// 设置为 DAG 请求并保存 payload。
    pub fn SetDAGRequest(&mut self, payload: Vec<u8>) -> &mut Self {
        self.request_type = Some(RequestType::Dag);
        self.payload = Some(RequestPayload::Dag(payload));
        self
    }
    /// 设置为 Analyze 请求，并指定隔离级别。
    pub fn SetAnalyzeRequest(&mut self, payload: Vec<u8>, isolation: IsolationLevel) -> &mut Self {
        self.request_type = Some(RequestType::Analyze);
        self.payload = Some(RequestPayload::Analyze(payload));
        self.isolation_level = isolation;
        self.priority = Priority::Low;
        self.not_fill_cache = true;
        self
    }
    /// 设置为 Checksum 请求。
    pub fn SetChecksumRequest(&mut self, payload: Vec<u8>) -> &mut Self {
        self.request_type = Some(RequestType::Checksum);
        self.payload = Some(RequestPayload::Checksum(payload));
        self.not_fill_cache = true;
        self
    }
    /// 设置非分区 key ranges。
    pub fn SetKeyRanges(&mut self, ranges: Vec<KeyRange>) -> &mut Self {
        self.key_ranges = ranges;
        self
    }
    /// 设置 key ranges 及行数提示。
    pub fn SetKeyRangesWithHints(&mut self, ranges: Vec<KeyRange>, hints: Vec<usize>) -> &mut Self {
        self.key_ranges = ranges;
        self.hints = hints;
        self
    }
    /// 设置分区 ID 与 ranges。
    pub fn SetPartitionIDAndRanges(&mut self, ranges: Vec<PartitionIDAndRanges>) -> &mut Self {
        self.partition_ranges = ranges;
        self
    }
    /// 设置 start_ts。
    pub fn SetStartTS(&mut self, value: u64) -> &mut Self {
        self.start_ts = value;
        self
    }
    /// 设置降序扫描。
    pub fn SetDesc(&mut self, value: bool) -> &mut Self {
        self.descending = value;
        self
    }
    /// 设置保序。
    pub fn SetKeepOrder(&mut self, value: bool) -> &mut Self {
        self.keep_order = value;
        self
    }
    /// 设置存储类型。
    pub fn SetStoreType(&mut self, value: StoreType) -> &mut Self {
        self.store_type = Some(value);
        self
    }
    /// 设置是否允许 Batch Cop。
    pub fn SetAllowBatchCop(&mut self, value: bool) -> &mut Self {
        self.allow_batch_cop = value;
        self
    }
    /// 设置分页开关。
    pub fn SetPaging(&mut self, value: bool) -> &mut Self {
        self.paging = value;
        self
    }
    /// 设置并发度。
    pub fn SetConcurrency(&mut self, value: usize) -> &mut Self {
        self.concurrency = value;
        self
    }
    /// 设置 TiDB server id。
    pub fn SetTiDBServerID(&mut self, value: u64) -> &mut Self {
        self.server_id = value;
        self
    }
    /// 设置资源组名。
    pub fn SetResourceGroupName(&mut self, value: impl Into<String>) -> &mut Self {
        self.resource_group_name = value.into();
        self
    }
    /// 设置请求来源。
    pub fn SetRequestSource(&mut self, value: impl Into<String>) -> &mut Self {
        self.request_source = value.into();
        self
    }
    /// 设置显式请求来源类型。
    pub fn SetExplicitRequestSourceType(&mut self, value: impl Into<String>) -> &mut Self {
        self.explicit_source_type = value.into();
        self
    }
    /// 设置事务作用域。
    pub fn SetTxnScope(&mut self, value: impl Into<String>) -> &mut Self {
        self.txn_scope = value.into();
        self
    }
    /// 设置读副本作用域。
    pub fn SetReadReplicaScope(&mut self, value: impl Into<String>) -> &mut Self {
        self.read_replica_scope = value.into();
        self
    }
    /// 设置是否 stale read。
    pub fn SetIsStaleness(&mut self, value: bool) -> &mut Self {
        self.is_staleness = value;
        self
    }
    /// 设置连接 ID 与别名。
    pub fn SetConnIDAndConnAlias(&mut self, id: u64, alias: impl Into<String>) -> &mut Self {
        self.connection_id = id;
        self.connection_alias = alias.into();
        self
    }
    /// 从会话变量批量拷贝 DistSQL 相关字段。
    pub fn SetFromSessionVars(&mut self, vars: &SessionVars) -> &mut Self {
        self.isolation_level = vars.isolation_level;
        self.priority = vars.priority;
        if self.concurrency == 0 {
            self.concurrency = vars.concurrency;
        } else {
            self.concurrency = self.concurrency.min(vars.concurrency);
        }
        self.streaming = vars.streaming;
        self.paging = vars.paging;
        self.allow_batch_cop = vars.allow_batch_cop;
        self.txn_scope = vars.txn_scope.clone();
        self.read_replica_scope = vars.read_replica_scope.clone();
        self.is_staleness = vars.is_staleness;
        self.resource_group_name = vars.resource_group_name.clone();
        self.request_source = vars.request_source.clone();
        self
    }
    /// 绑定 schema 侧的 txn_scope 校验器（对齐 Go SetFromInfoSchema）。
    pub fn SetFromInfoSchema(&mut self, checker: Arc<dyn TxnScopeChecker>) -> &mut Self {
        self.scope_checker = Some(checker);
        self
    }
    /// 非 global 的 txn_scope 时，校验各分区物理表是否允许被该 scope 读取。
    fn verifyTxnScope(&self) -> DistSqlResult<()> {
        if self.txn_scope.is_empty() || self.txn_scope == "global" {
            return Ok(());
        }
        if let Some(checker) = &self.scope_checker {
            for range in &self.partition_ranges {
                if !checker.verify_txn_scope(&self.txn_scope, range.partition_id) {
                    return Err(DistSqlError(format!(
                        "table {} is outside txn scope {}",
                        range.partition_id, self.txn_scope
                    )));
                }
            }
        }
        Ok(())
    }
}

/// 校验物理表是否符合给定 txn_scope（通常对照 placement leader DC）。
pub trait TxnScopeChecker: Send + Sync {
    /// 返回该物理表 ID 是否可被 `scope` 读取。
    fn verify_txn_scope(&self, scope: &str, physical_table_id: i64) -> bool;
}
/// global scope 恒为 true；否则委托 `TxnScopeChecker`。
pub fn VerifyTxnScope(scope: &str, physical_table_id: i64, checker: &dyn TxnScopeChecker) -> bool {
    scope.is_empty() || scope == "global" || checker.verify_txn_scope(scope, physical_table_id)
}

/// 将 i64 编码为 TiDB 表键用的大端有符号变换字节。
fn encode_i64(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1u64 << 63)).to_be_bytes()
}
/// 表前缀：`t` + encoded table_id。
fn table_prefix(table_id: i64) -> Vec<u8> {
    let mut key = vec![b't'];
    key.extend_from_slice(&encode_i64(table_id));
    key
}
/// 行记录前缀：`t{tid}_r`。
fn record_prefix(table_id: i64) -> Vec<u8> {
    let mut key = table_prefix(table_id);
    key.extend_from_slice(b"_r");
    key
}
/// 索引前缀：`t{tid}_i{index_id}`。
fn index_prefix(table_id: i64, index_id: i64) -> Vec<u8> {
    let mut key = table_prefix(table_id);
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&encode_i64(index_id));
    key
}
/// 计算字典序上的下一个前缀（PrefixNext），用于半开区间上界。
fn prefix_next(mut key: Vec<u8>) -> Vec<u8> {
    for index in (0..key.len()).rev() {
        if key[index] != 0xff {
            key[index] += 1;
            key.truncate(index + 1);
            return key;
        }
    }
    key.push(0);
    key
}

/// 整型 handle（行主键）区间，含开闭标记。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandleRange {
    pub low: i64,
    pub high: i64,
    pub low_exclusive: bool,
    pub high_inclusive: bool,
}
/// 将表 handle 区间编码为行 key ranges。
pub fn TableRangesToKVRanges(table_id: i64, ranges: &[HandleRange]) -> Vec<KeyRange> {
    let prefix = record_prefix(table_id);
    ranges
        .iter()
        .map(|range| {
            // 按开闭区间调整端点后再编码为 [start, end)。
            let low = encode_i64(range.low);
            let high = encode_i64(range.high);
            let mut start = prefix.clone();
            start.extend_from_slice(&low);
            if range.low_exclusive {
                start = prefix_next(start);
            }
            let mut end = prefix.clone();
            end.extend_from_slice(&high);
            if range.high_inclusive {
                end = prefix_next(end);
            }
            // Go 的 kv.KeyRange 允许空区间（例如被排除的单点），这里不能
            // 通过 KeyRange::new 丢弃该范围。
            KeyRange { start, end }
        })
        .collect()
}

fn handle_range(table_id: i64, first: i64, last: i64) -> KeyRange {
    let prefix = record_prefix(table_id);
    let mut start = prefix.clone();
    start.extend_from_slice(&encode_i64(first));
    let mut end = prefix;
    end.extend_from_slice(&encode_i64(last));
    // PrefixNext 在编码后的 key 上计算，因而也正确覆盖 i64::MAX。
    end = prefix_next(end);
    KeyRange { start, end }
}
/// 将离散 handle 列表转为逐点 key range，并返回提示下标列表。
pub fn TableHandlesToKVRanges(table_id: i64, handles: &[i64]) -> (Vec<KeyRange>, Vec<usize>) {
    if handles.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut ranges = Vec::new();
    let mut hints = Vec::new();
    let mut first = handles[0];
    let mut last = first;
    let mut count = 1;
    for &handle in &handles[1..] {
        if last != i64::MAX && handle == last + 1 {
            last = handle;
            count += 1;
        } else {
            ranges.push(handle_range(table_id, first, last));
            hints.push(count);
            first = handle;
            last = handle;
            count = 1;
        }
    }
    ranges.push(handle_range(table_id, first, last));
    hints.push(count);
    (ranges, hints)
}
/// 将 (partition_id, handle) 列表按分区分组后编码为 key ranges。
pub fn PartitionHandlesToKVRanges(handles: &[(i64, i64)]) -> (Vec<KeyRange>, Vec<usize>) {
    let mut ranges = Vec::new();
    let mut hints = Vec::new();
    if handles.is_empty() {
        return (ranges, hints);
    }
    let mut start = handles[0].1;
    let mut last = start;
    let mut partition = handles[0].0;
    let mut count = 1;
    for &(next_partition, handle) in &handles[1..] {
        if next_partition == partition && last != i64::MAX && handle == last + 1 {
            last = handle;
            count += 1;
        } else {
            ranges.push(handle_range(partition, start, last));
            hints.push(count);
            partition = next_partition;
            start = handle;
            last = handle;
            count = 1;
        }
    }
    ranges.push(handle_range(partition, start, last));
    hints.push(count);
    (ranges, hints)
}
/// 将索引列区间编码为索引 seek key ranges（多表 ID 笛卡尔展开）。
pub fn IndexRangesToKVRanges(
    table_ids: &[i64],
    index_id: i64,
    ranges: &[(Vec<u8>, Vec<u8>)],
) -> DistSqlResult<Vec<KeyRange>> {
    let mut result = Vec::new();
    for table_id in table_ids {
        let prefix = index_prefix(*table_id, index_id);
        for (low, high) in ranges {
            let mut start = prefix.clone();
            start.extend_from_slice(low);
            let mut end = prefix.clone();
            end.extend_from_slice(high);
            result.push(KeyRange { start, end });
        }
    }
    Ok(result)
}
/// 将 common handle（聚簇索引主键）区间编码为行 key ranges。
pub fn CommonHandleRangesToKVRanges(
    table_ids: &[i64],
    ranges: &[(Vec<u8>, Vec<u8>)],
) -> DistSqlResult<Vec<KeyRange>> {
    let mut result = Vec::new();
    for table_id in table_ids {
        let prefix = record_prefix(*table_id);
        for (low, high) in ranges {
            let mut start = prefix.clone();
            start.extend_from_slice(low);
            let mut end = prefix.clone();
            end.extend_from_slice(high);
            result.push(KeyRange { start, end });
        }
    }
    Ok(result)
}
/// 按 int64 边界拆分 handle ranges（有符号/无符号）；保序降序时交换两组返回顺序。
pub fn SplitRangesAcrossInt64Boundary(
    ranges: &[HandleRange],
    keep_order: bool,
    descending: bool,
    common_handle: bool,
) -> (Vec<HandleRange>, Vec<HandleRange>) {
    if common_handle {
        return (ranges.to_vec(), Vec::new());
    }
    let mut signed = Vec::new();
    let mut unsigned = Vec::new();
    for range in ranges {
        if range.low < 0 {
            signed.push(range.clone());
        } else {
            unsigned.push(range.clone());
        }
    }
    if keep_order && descending {
        (unsigned, signed)
    } else {
        (signed, unsigned)
    }
}
/// 构造覆盖整表记录前缀及指定索引前缀的全表扫描 ranges。
pub fn BuildTableRanges(table_id: i64, index_ids: &[i64]) -> Vec<KeyRange> {
    let mut result = Vec::new();
    let record = record_prefix(table_id);
    if let Ok(range) = KeyRange::new(record.clone(), prefix_next(record)) {
        result.push(range);
    }
    for index_id in index_ids {
        let prefix = index_prefix(table_id, *index_id);
        if let Ok(range) = KeyRange::new(prefix.clone(), prefix_next(prefix)) {
            result.push(range);
        }
    }
    result
}
/// 估算单 Region 行数常量，用于小 limit 时调整并发（对齐 Go）。
pub const estimatedRegionRowCount: usize = 100_000;
