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

//! Test / mock PD and split clients, matching `mock_pd_client.go`.

//! MockPDClientForSplit：在进程内模拟 PD region 树与 split/scatter 算子。
//! RegionTree 维护半开区间，供 ScanRegions/GetRegion 查询。
//! 可注入错误与空结果，覆盖客户端重试与兼容性分支。
//! FakeSplitClient 记录调用以便断言批大小与键集合。
//! 对齐 Go mock，不连接真实 PD gRPC。
//! 符号索引补充 1：公开 API 的约束优先于内部实现细节。
//! 数据流补充 2：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 3：空输入、取消上下文、未知枚举值都应按 Go 方式处理。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use astersql_errors::{New, SharedError};

use crate::client::{PdBackend, PdHttpBackend, SplitClient};
use crate::region::RegionInfo;
use crate::stubs::{
    CodecPDClient, Context, GetRegionOption, GetStoreOption, Result, codec, metapb, pdhttp, pdpb,
};

/// In-memory region tree used by TestClient / MockPDClientForSplit.
#[derive(Clone, Debug, Default)]
/// `RegionTree`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `RegionTree` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct RegionTree {
    pub regions: Vec<RegionInfo>,
}

/// `RegionTree` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `RegionTree` 方法边界：非法参数应返回可分类错误而非 panic。
impl RegionTree {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `new` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn new() -> Self {
        Self::default()
    }

    /// `SetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetRegion(&mut self, info: RegionInfo) {
        if let Some(pos) = self.regions.iter().position(|r| {
            let current = r.Region.as_ref();
            let replacement = info.Region.as_ref();
            current.map(|m| m.Id) == replacement.map(|m| m.Id)
                || current.map(|m| m.StartKey.as_slice())
                    == replacement.map(|m| m.StartKey.as_slice())
        }) {
            self.regions[pos] = info;
        } else {
            self.regions.push(info);
            self.regions.sort_by(|a, b| {
                let sa = a
                    .Region
                    .as_ref()
                    .map(|r| r.StartKey.as_slice())
                    .unwrap_or(&[]);
                let sb = b
                    .Region
                    .as_ref()
                    .map(|r| r.StartKey.as_slice())
                    .unwrap_or(&[]);
                sa.cmp(sb)
            });
        }
    }

    /// `ScanRange`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRange` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ScanRange(&self, start: &[u8], end: &[u8], limit: usize) -> Vec<RegionInfo> {
        let mut out = Vec::new();
        for r in &self.regions {
            let Some(meta) = r.Region.as_ref() else {
                continue;
            };
            if !end.is_empty() && meta.StartKey.as_slice() >= end {
                continue;
            }
            if !meta.EndKey.is_empty() && meta.EndKey.as_slice() <= start {
                continue;
            }
            out.push(r.clone());
            // limit 0 means unlimited (Go PD ScanRegions).
            if limit > 0 && out.len() >= limit {
                break;
            }
        }
        out
    }
}

/// `TestClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `TestClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct TestClient {
    inner: TestClientMut,
    pub InjectErr: bool,
    pub InjectTimes: AtomicI32,
}

/// `NewTestClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewTestClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewTestClient(
    stores: HashMap<u64, metapb::Store>,
    regions: HashMap<u64, RegionInfo>,
    nextRegionID: u64,
) -> TestClient {
    TestClient {
        inner: NewTestClientMut(stores, regions, nextRegionID),
        InjectErr: false,
        InjectTimes: AtomicI32::new(0),
    }
}

/// `TestClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `TestClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl TestClient {
    /// `GetAllRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetAllRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn GetAllRegions(&self) -> HashMap<u64, RegionInfo> {
        self.inner.GetAllRegions()
    }

    /// `GetPDClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetPDClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn GetPDClient(&self) -> FakePDClient {
        let stores: Vec<_> = self
            .inner
            .inner
            .lock()
            .unwrap()
            .stores
            .values()
            .cloned()
            .collect();
        NewFakePDClient(stores, false, None)
    }
}

/// `TestClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `TestClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl SplitClient for TestClient {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetStore(
        &self,
        _ctx: &Context,
        storeID: u64,
        _opts: &[GetStoreOption],
    ) -> Result<metapb::Store> {
        self.inner.GetStore(_ctx, storeID, _opts)
    }

    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegion(&self, _ctx: &Context, key: &[u8]) -> Result<RegionInfo> {
        self.inner.GetRegion(_ctx, key)
    }

    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegionByID(&self, _ctx: &Context, regionID: u64) -> Result<RegionInfo> {
        self.inner.GetRegionByID(_ctx, regionID)
    }

    /// `SplitKeysAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitKeysAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitKeysAndScatter(&self, ctx: &Context, keys: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        self.SplitWaitAndScatter(ctx, &RegionInfo::default(), keys)
    }

    /// `SplitKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitKeys` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitKeys(&self, ctx: &Context, keys: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        self.SplitWaitAndScatter(ctx, &RegionInfo::default(), keys)
    }

    /// `SplitWaitAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitWaitAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitWaitAndScatter(
        &self,
        _ctx: &Context,
        _region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        self.inner.SplitWaitAndScatter(_ctx, _region, keys)
    }

    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetOperator(&self, _ctx: &Context, _regionID: u64) -> Result<pdpb::GetOperatorResponse> {
        Ok(pdpb::GetOperatorResponse {
            Header: Some(pdpb::ResponseHeader::default()),
            ..Default::default()
        })
    }

    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScanRegions(
        &self,
        _ctx: &Context,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
        _opts: &[GetRegionOption],
    ) -> Result<Vec<RegionInfo>> {
        if self.InjectErr && self.InjectTimes.load(Ordering::SeqCst) > 0 {
            self.InjectTimes.fetch_sub(1, Ordering::SeqCst);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(New("not leader"));
        }
        if !key.is_empty() && key == endKey {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(New("key and endKey are the same"));
        }
        self.inner.ScanRegions(_ctx, key, endKey, limit, _opts)
    }

    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetPlacementRule(
        &self,
        _ctx: &Context,
        _groupID: &str,
        _ruleID: &str,
    ) -> Result<pdhttp::Rule> {
        Err(New("not implemented"))
    }

    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetPlacementRule(&self, _ctx: &Context, _rule: &pdhttp::Rule) -> Result<()> {
        Err(New("not implemented"))
    }

    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `DeletePlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn DeletePlacementRule(&self, _ctx: &Context, _groupID: &str, _ruleID: &str) -> Result<()> {
        Err(New("not implemented"))
    }

    /// `SetStoresLabel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetStoresLabel` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetStoresLabel(
        &self,
        _ctx: &Context,
        _stores: &[u64],
        _labelKey: &str,
        _labelValue: &str,
    ) -> Result<()> {
        Ok(())
    }

    /// `WaitRegionsScattered`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `WaitRegionsScattered` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn WaitRegionsScattered(
        &self,
        _ctx: &Context,
        _regionInfos: &[RegionInfo],
    ) -> Result<(i32, SharedError)> {
        Ok((0, New("")))
    }

    /// `GetCodecPDClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetCodecPDClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetCodecPDClient(&self) -> Option<CodecPDClient> {
        None
    }
}

/// Mutable test client that can perform splits (Go TestClient with mutex).
/// `TestClientMut`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `TestClientMut` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct TestClientMut {
    inner: Mutex<TestClientInner>,
}

/// `TestClientInner`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `TestClientInner` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct TestClientInner {
    stores: HashMap<u64, metapb::Store>,
    Regions: HashMap<u64, RegionInfo>,
    RegionsInfo: RegionTree,
    nextRegionID: u64,
    InjectErr: bool,
    InjectTimes: i32,
}

/// `NewTestClientMut`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewTestClientMut` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewTestClientMut(
    stores: HashMap<u64, metapb::Store>,
    regions: HashMap<u64, RegionInfo>,
    nextRegionID: u64,
) -> TestClientMut {
    let mut regionsInfo = RegionTree::new();
    for regionInfo in regions.values() {
        regionsInfo.SetRegion(regionInfo.clone());
    }
    TestClientMut {
        inner: Mutex::new(TestClientInner {
            stores,
            Regions: regions,
            RegionsInfo: regionsInfo,
            nextRegionID,
            InjectErr: false,
            InjectTimes: 0,
        }),
    }
}

/// `TestClientMut` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `TestClientMut` 方法边界：非法参数应返回可分类错误而非 panic。
impl TestClientMut {
    /// `GetAllRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetAllRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn GetAllRegions(&self) -> HashMap<u64, RegionInfo> {
        self.inner.lock().unwrap().Regions.clone()
    }

    /// `set_inject`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_inject` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_inject(&self, err: bool, times: i32) {
        let mut g = self.inner.lock().unwrap();
        g.InjectErr = err;
        g.InjectTimes = times;
    }
}

/// `TestClientMut` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `TestClientMut` 方法边界：非法参数应返回可分类错误而非 panic。
impl SplitClient for TestClientMut {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetStore(
        &self,
        _ctx: &Context,
        storeID: u64,
        _opts: &[GetStoreOption],
    ) -> Result<metapb::Store> {
        self.inner
            .lock()
            .unwrap()
            .stores
            .get(&storeID)
            .cloned()
            .ok_or_else(|| New("store not found"))
    }

    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegion(&self, _ctx: &Context, key: &[u8]) -> Result<RegionInfo> {
        let g = self.inner.lock().unwrap();
        for region in g.Regions.values() {
            let meta = region.Region.as_ref().unwrap();
            if key >= meta.StartKey.as_slice()
                && (meta.EndKey.is_empty() || key < meta.EndKey.as_slice())
            {
                return Ok(region.clone());
            }
        }
        Err(New(format!(
            "region not found: key={}",
            String::from_utf8_lossy(key)
        )))
    }

    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegionByID(&self, _ctx: &Context, regionID: u64) -> Result<RegionInfo> {
        self.inner
            .lock()
            .unwrap()
            .Regions
            .get(&regionID)
            .cloned()
            .ok_or_else(|| New(format!("region not found: id={regionID}")))
    }

    /// `SplitKeysAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitKeysAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitKeysAndScatter(&self, ctx: &Context, keys: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        self.SplitWaitAndScatter(ctx, &RegionInfo::default(), keys)
    }

    /// `SplitKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitKeys` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitKeys(&self, ctx: &Context, keys: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        self.SplitWaitAndScatter(ctx, &RegionInfo::default(), keys)
    }

    /// `SplitWaitAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitWaitAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitWaitAndScatter(
        &self,
        _ctx: &Context,
        _region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        let mut g = self.inner.lock().unwrap();
        let mut newRegions = Vec::new();
        for key in keys {
            let splitKey = codec::EncodeBytes(Vec::new(), key);
            let mut target_id = None;
            for (id, region) in g.Regions.iter() {
                if region.ContainsInterior(&splitKey) {
                    target_id = Some(*id);
                    break;
                }
            }
            let Some(tid) = target_id else {
                continue;
            };
            let target = g.Regions.get_mut(&tid).unwrap();
            let peers = target
                .Region
                .as_ref()
                .map(|r| r.Peers.clone())
                .unwrap_or_default();
            let start = target
                .Region
                .as_ref()
                .map(|r| r.StartKey.clone())
                .unwrap_or_default();
            let new_id = g.nextRegionID;
            g.nextRegionID += 1;
            let newRegion = RegionInfo {
                Region: Some(metapb::Region {
                    Peers: peers,
                    Id: new_id,
                    StartKey: start,
                    EndKey: splitKey.clone(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Some(t) = g.Regions.get_mut(&tid) {
                if let Some(meta) = t.Region.as_mut() {
                    meta.StartKey = splitKey;
                }
            }
            g.Regions.insert(new_id, newRegion.clone());
            g.RegionsInfo.SetRegion(newRegion.clone());
            let target_snapshot = g.Regions.get(&tid).cloned();
            if let Some(t) = target_snapshot {
                g.RegionsInfo.SetRegion(t);
            }
            newRegions.push(newRegion);
        }
        Ok(newRegions)
    }

    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetOperator(&self, _ctx: &Context, _regionID: u64) -> Result<pdpb::GetOperatorResponse> {
        Ok(pdpb::GetOperatorResponse {
            Header: Some(pdpb::ResponseHeader::default()),
            ..Default::default()
        })
    }

    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScanRegions(
        &self,
        _ctx: &Context,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
        _opts: &[GetRegionOption],
    ) -> Result<Vec<RegionInfo>> {
        let mut g = self.inner.lock().unwrap();
        if g.InjectErr && g.InjectTimes > 0 {
            g.InjectTimes -= 1;
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(New("not leader"));
        }
        if !key.is_empty() && key == endKey {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(New("key and endKey are the same"));
        }
        Ok(g.RegionsInfo.ScanRange(key, endKey, limit.max(0) as usize))
    }

    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetPlacementRule(
        &self,
        _ctx: &Context,
        _groupID: &str,
        _ruleID: &str,
    ) -> Result<pdhttp::Rule> {
        Err(New("not implemented"))
    }
    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetPlacementRule(&self, _ctx: &Context, _rule: &pdhttp::Rule) -> Result<()> {
        Err(New("not implemented"))
    }
    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `DeletePlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn DeletePlacementRule(&self, _ctx: &Context, _groupID: &str, _ruleID: &str) -> Result<()> {
        Err(New("not implemented"))
    }
    /// `SetStoresLabel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetStoresLabel` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetStoresLabel(
        &self,
        _ctx: &Context,
        _stores: &[u64],
        _labelKey: &str,
        _labelValue: &str,
    ) -> Result<()> {
        Ok(())
    }
    /// `WaitRegionsScattered`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `WaitRegionsScattered` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn WaitRegionsScattered(
        &self,
        _ctx: &Context,
        _regionInfos: &[RegionInfo],
    ) -> Result<(i32, SharedError)> {
        Ok((0, New("")))
    }
    /// `GetCodecPDClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetCodecPDClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetCodecPDClient(&self) -> Option<CodecPDClient> {
        None
    }
}

/// `SplitHijack`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
/// 实现方需保持与 Go 接口相同的错误可重试语义。
/// `SplitHijack` 契约：返回值与副作用应可被上层测试稳定观察。
pub type SplitHijack = Box<dyn FnMut() -> Result<(RegionInfo, Vec<RegionInfo>)> + Send>;

#[derive(Clone)]
/// `MockPDClientForSplit`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `MockPDClientForSplit` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct MockPDClientForSplit {
    mu: Arc<Mutex<MockPDInner>>,
}

/// `MockPDInner`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `MockPDInner` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct MockPDInner {
    stores: HashMap<u64, metapb::Store>,
    pub Regions: RegionTree,
    lastRegionID: u64,
    /// `None` => empty scan result without error (Go `errors: []error{nil,...}`).
    scan_errors: Vec<Option<SharedError>>,
    scan_before_hook: Option<Box<dyn FnMut() + Send>>,
    split_count: i32,
    split_hijack: Option<SplitHijack>,
    split_hijack_persistent: bool,
    scatter_each_fail_before: i32,
    scatter_count: HashMap<u64, i32>,
    scatter_regions_not_implemented: bool,
    scatter_regions_failed_count: i32,
    scatter_regions_region_count: i32,
    scatter_finished_percentage: i32,
    get_operator: HashMap<u64, Vec<pdpb::GetOperatorResponse>>,
}

/// `NewMockPDClientForSplit`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewMockPDClientForSplit` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewMockPDClientForSplit() -> MockPDClientForSplit {
    MockPDClientForSplit {
        mu: Arc::new(Mutex::new(MockPDInner {
            stores: HashMap::new(),
            Regions: RegionTree::new(),
            lastRegionID: 0,
            scan_errors: Vec::new(),
            scan_before_hook: None,
            split_count: 0,
            split_hijack: None,
            split_hijack_persistent: false,
            scatter_each_fail_before: 0,
            scatter_count: HashMap::new(),
            scatter_regions_not_implemented: false,
            scatter_regions_failed_count: 0,
            scatter_regions_region_count: 0,
            scatter_finished_percentage: 100,
            get_operator: HashMap::new(),
        })),
    }
}

/// `MockPDClientForSplit` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `MockPDClientForSplit` 方法边界：非法参数应返回可分类错误而非 panic。
impl MockPDClientForSplit {
    /// `SetRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetRegions(&self, boundaries: &[Vec<u8>]) -> Vec<metapb::Region> {
        let mut g = self.mu.lock().unwrap();
        g.set_regions(boundaries)
    }

    /// `SetStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetStores(&self, stores: HashMap<u64, metapb::Store>) {
        self.mu.lock().unwrap().stores = stores;
    }

    /// `push_scan_error`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `push_scan_error` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn push_scan_error(&self, err: Option<SharedError>) {
        self.mu.lock().unwrap().scan_errors.push(err);
    }

    /// `set_scan_before_hook`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub fn set_scan_before_hook(&self, hook: impl FnMut() + Send + 'static) {
        self.mu.lock().unwrap().scan_before_hook = Some(Box::new(hook));
    }

    /// `set_split_hijack`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_split_hijack` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_split_hijack(&self, hijack: Option<SplitHijack>) {
        let mut g = self.mu.lock().unwrap();
        g.split_hijack = hijack;
        g.split_hijack_persistent = false;
    }

    /// Install a hijack that remains until cleared (Go persistent `hijacked` func).
    /// `set_split_hijack_persistent`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_split_hijack_persistent` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_split_hijack_persistent(&self, hijack: SplitHijack) {
        let mut g = self.mu.lock().unwrap();
        g.split_hijack = Some(hijack);
        g.split_hijack_persistent = true;
    }

    /// `split_count`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `split_count` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn split_count(&self) -> i32 {
        self.mu.lock().unwrap().split_count
    }

    /// `reset_split_count`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `reset_split_count` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn reset_split_count(&self) {
        self.mu.lock().unwrap().split_count = 0;
    }

    /// `scatter_regions_region_count`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `scatter_regions_region_count` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn scatter_regions_region_count(&self) -> i32 {
        self.mu.lock().unwrap().scatter_regions_region_count
    }

    /// `set_scatter_regions_not_implemented`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_scatter_regions_not_implemented` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_scatter_regions_not_implemented(&self, v: bool) {
        self.mu.lock().unwrap().scatter_regions_not_implemented = v;
    }

    /// `set_scatter_regions_failed_count`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_scatter_regions_failed_count` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_scatter_regions_failed_count(&self, v: i32) {
        self.mu.lock().unwrap().scatter_regions_failed_count = v;
    }

    /// `set_scatter_finished_percentage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_scatter_finished_percentage` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_scatter_finished_percentage(&self, v: i32) {
        self.mu.lock().unwrap().scatter_finished_percentage = v;
    }

    /// `set_scatter_each_fail_before`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_scatter_each_fail_before` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_scatter_each_fail_before(&self, v: i32) {
        self.mu.lock().unwrap().scatter_each_fail_before = v;
    }

    /// `scatter_region_count`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `scatter_region_count` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn scatter_region_count(&self) -> HashMap<u64, i32> {
        self.mu.lock().unwrap().scatter_count.clone()
    }

    /// `reset_scatter_region_count`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `reset_scatter_region_count` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn reset_scatter_region_count(&self) {
        self.mu.lock().unwrap().scatter_count.clear();
    }

    /// `set_get_operator_responses`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_get_operator_responses` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_get_operator_responses(
        &self,
        responses: HashMap<u64, Vec<pdpb::GetOperatorResponse>>,
    ) {
        self.mu.lock().unwrap().get_operator = responses;
    }

    /// `get_operator_len`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `get_operator_len` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn get_operator_len(&self, region_id: u64) -> usize {
        self.mu
            .lock()
            .unwrap()
            .get_operator
            .get(&region_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// `scan_regions_tree`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `scan_regions_tree` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn scan_regions_tree(&self) -> RegionTree {
        self.mu.lock().unwrap().Regions.clone()
    }

    /// `replace_regions_tree`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `replace_regions_tree` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn replace_regions_tree(&self, tree: RegionTree) {
        self.mu.lock().unwrap().Regions = tree;
    }

    /// `set_region_info`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_region_info` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn set_region_info(&self, info: RegionInfo) {
        self.mu.lock().unwrap().Regions.SetRegion(info);
    }
}

/// `MockPDInner` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `MockPDInner` 方法边界：非法参数应返回可分类错误而非 panic。
impl MockPDInner {
    /// `set_regions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `set_regions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn set_regions(&mut self, boundaries: &[Vec<u8>]) -> Vec<metapb::Region> {
        let mut out = Vec::new();
        for i in 0..boundaries.len().saturating_sub(1) {
            self.lastRegionID += 1;
            let region = metapb::Region {
                Id: self.lastRegionID,
                StartKey: boundaries[i].clone(),
                EndKey: boundaries[i + 1].clone(),
                Peers: vec![metapb::Peer {
                    Id: self.lastRegionID,
                    StoreId: 1,
                }],
                RegionEpoch: Some(crate::stubs::RegionEpoch {
                    ConfVer: 1,
                    Version: 1,
                }),
            };
            let info = RegionInfo {
                Region: Some(region.clone()),
                Leader: Some(metapb::Peer {
                    Id: self.lastRegionID,
                    StoreId: 1,
                }),
                ..Default::default()
            };
            self.Regions.SetRegion(info);
            out.push(region);
        }
        out
    }

    /// `split_region_inner`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `split_region_inner` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn split_region_inner(
        &mut self,
        region: &RegionInfo,
        keys: &[Vec<u8>],
        is_raw_kv: bool,
    ) -> Result<(RegionInfo, Vec<RegionInfo>)> {
        let mut encoded_keys: Vec<Vec<u8>> = Vec::with_capacity(keys.len());
        for key in keys {
            if is_raw_kv {
                encoded_keys.push(key.clone());
            } else {
                encoded_keys.push(codec::EncodeBytes(Vec::new(), key));
            }
        }
        let start = region
            .Region
            .as_ref()
            .map(|r| r.StartKey.clone())
            .unwrap_or_default();
        let end = region
            .Region
            .as_ref()
            .map(|r| r.EndKey.clone())
            .unwrap_or_default();
        let origin_id = region.Region.as_ref().map(|r| r.Id).unwrap_or(0);

        // Drop the region being split (and any with same key span start).
        self.Regions.regions.retain(|r| {
            let Some(meta) = r.Region.as_ref() else {
                return false;
            };
            meta.Id != origin_id && meta.StartKey != start
        });

        let mut boundaries = Vec::with_capacity(encoded_keys.len() + 2);
        boundaries.push(start);
        boundaries.extend(encoded_keys);
        boundaries.push(end);

        let mut infos = Vec::new();
        for i in 0..boundaries.len().saturating_sub(1) {
            self.lastRegionID += 1;
            let id = if i == 0 && origin_id != 0 {
                origin_id
            } else {
                self.lastRegionID
            };
            let region_meta = metapb::Region {
                Id: id,
                StartKey: boundaries[i].clone(),
                EndKey: boundaries[i + 1].clone(),
                Peers: vec![metapb::Peer { Id: id, StoreId: 1 }],
                RegionEpoch: Some(crate::stubs::RegionEpoch {
                    ConfVer: 1,
                    Version: 1,
                }),
            };
            let info = RegionInfo {
                Region: Some(region_meta),
                Leader: Some(metapb::Peer { Id: id, StoreId: 1 }),
                ..Default::default()
            };
            self.Regions.SetRegion(info.clone());
            infos.push(info);
        }
        if infos.is_empty() {
            return Ok((region.clone(), Vec::new()));
        }
        // Match Go batchSplitRegionsWithOrigin: origin (same id) is not scattered.
        let origin_id = region.Region.as_ref().map(|r| r.Id).unwrap_or(0);
        let mut origin = infos[0].clone();
        let mut others = Vec::new();
        for info in infos {
            if info.Region.as_ref().map(|r| r.Id) == Some(origin_id) {
                origin = info;
            } else {
                others.push(info);
            }
        }
        Ok((origin, others))
    }
}

/// `MockPDClientForSplit` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `MockPDClientForSplit` 方法边界：非法参数应返回可分类错误而非 panic。
impl PdBackend for MockPDClientForSplit {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetStore(&self, storeID: u64) -> Result<metapb::Store> {
        self.mu
            .lock()
            .unwrap()
            .stores
            .get(&storeID)
            .cloned()
            .ok_or_else(|| New("store not found"))
    }

    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegion(&self, key: &[u8]) -> Result<RegionInfo> {
        let g = self.mu.lock().unwrap();
        g.Regions
            .ScanRange(key, &[], 1)
            .into_iter()
            .next()
            .ok_or_else(|| New("region not found"))
    }

    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegionByID(&self, regionID: u64) -> Result<RegionInfo> {
        let g = self.mu.lock().unwrap();
        g.Regions
            .regions
            .iter()
            .find(|r| r.Region.as_ref().map(|m| m.Id) == Some(regionID))
            .cloned()
            .ok_or_else(|| New("region not found"))
    }

    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScanRegions(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
        _allow_follower: bool,
    ) -> Result<Vec<RegionInfo>> {
        let mut hook = {
            let mut g = self.mu.lock().unwrap();
            if !g.scan_errors.is_empty() {
                match g.scan_errors.remove(0) {
                    Some(err) => return Err(err),
                    None => return Ok(Vec::new()),
                }
            }
            g.scan_before_hook.take()
        };
        if let Some(h) = hook.as_mut() {
            h();
        }
        let mut g = self.mu.lock().unwrap();
        if let Some(h) = hook {
            g.scan_before_hook = Some(h);
        }
        let lim = if limit <= 0 { 0 } else { limit as usize };
        Ok(g.Regions.ScanRange(key, endKey, lim))
    }

    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetOperator(&self, regionID: u64) -> Result<pdpb::GetOperatorResponse> {
        let mut g = self.mu.lock().unwrap();
        if let Some(list) = g.get_operator.get_mut(&regionID) {
            if !list.is_empty() {
                return Ok(list.remove(0));
            }
        }
        Ok(pdpb::GetOperatorResponse {
            Status: pdpb::OperatorStatus::SUCCESS,
            Desc: b"scatter-region".to_vec(),
            Header: Some(pdpb::ResponseHeader::default()),
        })
    }

    /// `ScatterRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScatterRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScatterRegions(&self, regionIDs: &[u64]) -> Result<pdpb::ScatterRegionResponse> {
        let mut g = self.mu.lock().unwrap();
        if g.scatter_regions_not_implemented {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(New("unimplemented"));
        }
        if g.scatter_regions_failed_count > 0 {
            g.scatter_regions_failed_count -= 1;
            return Ok(pdpb::ScatterRegionResponse {
                FinishedPercentage: 0,
                FailedRegionsId: regionIDs.to_vec(),
                Header: Some(pdpb::ResponseHeader::default()),
            });
        }
        g.scatter_regions_region_count +=
            (regionIDs.len() as i32) * g.scatter_finished_percentage / 100;
        Ok(pdpb::ScatterRegionResponse {
            FinishedPercentage: g.scatter_finished_percentage as u64,
            Header: Some(pdpb::ResponseHeader::default()),
            ..Default::default()
        })
    }

    /// `ScatterRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScatterRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScatterRegion(&self, regionID: u64) -> Result<()> {
        let mut g = self.mu.lock().unwrap();
        let count = g.scatter_count.entry(regionID).or_insert(0);
        *count += 1;
        if *count <= g.scatter_each_fail_before {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(New(format!("region {regionID} is not fully replicated")));
        }
        Ok(())
    }

    /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetAllStores(&self) -> Result<Vec<metapb::Store>> {
        Ok(self.mu.lock().unwrap().stores.values().cloned().collect())
    }

    /// `SplitRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitRegion(
        &self,
        region: &RegionInfo,
        keys: &[Vec<u8>],
        is_raw_kv: bool,
    ) -> Result<(RegionInfo, Vec<RegionInfo>)> {
        // Run hijack outside the lock so clear/reinstall cannot deadlock.
        let (hijack, persistent) = {
            let mut g = self.mu.lock().unwrap();
            g.split_count += 1;
            (g.split_hijack.take(), g.split_hijack_persistent)
        };
        if let Some(mut hijack) = hijack {
            let result = hijack();
            if persistent {
                let mut g = self.mu.lock().unwrap();
                // Keep persistent until an explicit set_split_hijack(None).
                if g.split_hijack.is_none() && g.split_hijack_persistent {
                    g.split_hijack = Some(hijack);
                }
            }
            return result;
        }
        self.mu
            .lock()
            .unwrap()
            .split_region_inner(region, keys, is_raw_kv)
    }
}

/// `FakePDHTTPClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `FakePDHTTPClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct FakePDHTTPClient {
    rules: Mutex<HashMap<String, pdhttp::Rule>>,
    schedule: Mutex<HashMap<String, serde_json_stub::Value>>,
}

/// `serde_json_stub`：子模块，聚合相关桩类型与辅助函数。
mod serde_json_stub {
    /// `Value`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `Value` 契约：返回值与副作用应可被上层测试稳定观察。
    pub type Value = f64;
}

/// `NewFakePDHTTPClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewFakePDHTTPClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewFakePDHTTPClient() -> FakePDHTTPClient {
    FakePDHTTPClient {
        rules: Mutex::new(HashMap::new()),
        schedule: Mutex::new(HashMap::new()),
    }
}

/// `FakePDHTTPClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `FakePDHTTPClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl PdHttpBackend for FakePDHTTPClient {
    /// `GetReplicateConfig`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetReplicateConfig` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetReplicateConfig(&self) -> Result<HashMap<String, f64>> {
        Ok(HashMap::from([("max-replicas".into(), 3.0)]))
    }
    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetPlacementRule(&self, groupID: &str, ruleID: &str) -> Result<pdhttp::Rule> {
        let mut rules = self.rules.lock().unwrap();
        Ok(rules
            .entry(ruleID.to_string())
            .or_insert_with(|| pdhttp::Rule {
                GroupID: groupID.to_string(),
                ID: ruleID.to_string(),
                ..Default::default()
            })
            .clone())
    }
    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetPlacementRule(&self, rule: &pdhttp::Rule) -> Result<()> {
        self.rules
            .lock()
            .unwrap()
            .insert(rule.ID.clone(), rule.clone());
        Ok(())
    }
    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `DeletePlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn DeletePlacementRule(&self, groupID: &str, ruleID: &str) -> Result<()> {
        self.rules.lock().unwrap().remove(ruleID);
        Ok(())
    }
}

/// `FakePDClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `FakePDClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct FakePDClient {
    stores: Vec<metapb::Store>,
    regions: Mutex<Vec<RegionInfo>>,
    notLeader: bool,
    retryTime: Option<AtomicI32>,
}

/// `NewFakePDClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewFakePDClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewFakePDClient(
    stores: Vec<metapb::Store>,
    notLeader: bool,
    retryTime: Option<i32>,
) -> FakePDClient {
    FakePDClient {
        stores,
        regions: Mutex::new(Vec::new()),
        notLeader,
        retryTime: retryTime.map(AtomicI32::new),
    }
}

/// `FakePDClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `FakePDClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl FakePDClient {
    /// `SetRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetRegions(&self, regions: Vec<RegionInfo>) {
        *self.regions.lock().unwrap() = regions;
    }
}

/// `FakePDClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `FakePDClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl PdBackend for FakePDClient {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetStore(&self, storeID: u64) -> Result<metapb::Store> {
        self.stores
            .iter()
            .find(|s| s.Id == storeID)
            .cloned()
            .ok_or_else(|| New("store not found"))
    }
    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegion(&self, key: &[u8]) -> Result<RegionInfo> {
        self.ScanRegions(key, &[], 1, false)?
            .into_iter()
            .next()
            .ok_or_else(|| New("region not found"))
    }
    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegionByID(&self, regionID: u64) -> Result<RegionInfo> {
        self.regions
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.Region.as_ref().map(|m| m.Id) == Some(regionID))
            .cloned()
            .ok_or_else(|| New("region not found"))
    }
    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScanRegions(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
        _allow_follower: bool,
    ) -> Result<Vec<RegionInfo>> {
        if self.notLeader {
            if let Some(rt) = &self.retryTime {
                if rt.load(Ordering::SeqCst) > 0 {
                    rt.fetch_sub(1, Ordering::SeqCst);
                    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                    return Err(New("not leader"));
                }
            } else {
                // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                return Err(New("not leader"));
            }
        }
        let g = self.regions.lock().unwrap();
        let mut out = Vec::new();
        for r in g.iter() {
            let meta = r.Region.as_ref().unwrap();
            if !endKey.is_empty() && meta.StartKey.as_slice() >= endKey {
                continue;
            }
            if !meta.EndKey.is_empty() && meta.EndKey.as_slice() <= key {
                continue;
            }
            out.push(r.clone());
            if out.len() >= limit as usize {
                break;
            }
        }
        Ok(out)
    }
    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetOperator(&self, _regionID: u64) -> Result<pdpb::GetOperatorResponse> {
        Ok(pdpb::GetOperatorResponse {
            Status: pdpb::OperatorStatus::SUCCESS,
            ..Default::default()
        })
    }
    /// `ScatterRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScatterRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScatterRegions(&self, _regionIDs: &[u64]) -> Result<pdpb::ScatterRegionResponse> {
        Ok(pdpb::ScatterRegionResponse {
            FinishedPercentage: 100,
            ..Default::default()
        })
    }
    /// `ScatterRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScatterRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScatterRegion(&self, _regionID: u64) -> Result<()> {
        Ok(())
    }
    /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetAllStores(&self) -> Result<Vec<metapb::Store>> {
        Ok(self.stores.clone())
    }
    /// `SplitRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitRegion(
        &self,
        region: &RegionInfo,
        _keys: &[Vec<u8>],
        _is_raw_kv: bool,
    ) -> Result<(RegionInfo, Vec<RegionInfo>)> {
        Ok((region.clone(), vec![region.clone()]))
    }
}

/// `FakeSplitClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `FakeSplitClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct FakeSplitClient {
    regions: Mutex<Vec<RegionInfo>>,
}

/// `NewFakeSplitClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewFakeSplitClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewFakeSplitClient() -> FakeSplitClient {
    FakeSplitClient {
        regions: Mutex::new(Vec::new()),
    }
}

/// `FakeSplitClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `FakeSplitClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl FakeSplitClient {
    /// `AppendRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AppendRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AppendRegion(&self, startKey: Vec<u8>, endKey: Vec<u8>) {
        self.regions.lock().unwrap().push(RegionInfo {
            Region: Some(metapb::Region {
                StartKey: startKey,
                EndKey: endKey,
                ..Default::default()
            }),
            ..Default::default()
        });
    }
}

/// `FakeSplitClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `FakeSplitClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl SplitClient for FakeSplitClient {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetStore(
        &self,
        _ctx: &Context,
        _storeID: u64,
        _opts: &[GetStoreOption],
    ) -> Result<metapb::Store> {
        Err(New("not implemented"))
    }
    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegion(&self, _ctx: &Context, _key: &[u8]) -> Result<RegionInfo> {
        Err(New("not implemented"))
    }
    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegionByID(&self, _ctx: &Context, _regionID: u64) -> Result<RegionInfo> {
        Err(New("not implemented"))
    }
    /// `SplitKeysAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitKeysAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitKeysAndScatter(&self, _ctx: &Context, _keys: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        Ok(Vec::new())
    }
    /// `SplitKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitKeys` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitKeys(&self, _ctx: &Context, _keys: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        Ok(Vec::new())
    }
    /// `SplitWaitAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitWaitAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitWaitAndScatter(
        &self,
        _ctx: &Context,
        _region: &RegionInfo,
        _keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        Ok(Vec::new())
    }
    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetOperator(&self, _ctx: &Context, _regionID: u64) -> Result<pdpb::GetOperatorResponse> {
        Err(New("not implemented"))
    }
    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScanRegions(
        &self,
        _ctx: &Context,
        startKey: &[u8],
        endKey: &[u8],
        limit: i32,
        _opts: &[GetRegionOption],
    ) -> Result<Vec<RegionInfo>> {
        let g = self.regions.lock().unwrap();
        let mut result = Vec::new();
        let mut count = 0;
        for rng in g.iter() {
            let meta = rng.Region.as_ref().unwrap();
            let end_ok = meta.StartKey.as_slice() <= endKey;
            if end_ok && meta.EndKey.as_slice() > startKey {
                result.push(rng.clone());
                count += 1;
            }
            if count >= limit {
                break;
            }
        }
        Ok(result)
    }
    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetPlacementRule(
        &self,
        _ctx: &Context,
        _groupID: &str,
        _ruleID: &str,
    ) -> Result<pdhttp::Rule> {
        Err(New("not implemented"))
    }
    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetPlacementRule(&self, _ctx: &Context, _rule: &pdhttp::Rule) -> Result<()> {
        Err(New("not implemented"))
    }
    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `DeletePlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn DeletePlacementRule(&self, _ctx: &Context, _groupID: &str, _ruleID: &str) -> Result<()> {
        Err(New("not implemented"))
    }
    /// `SetStoresLabel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetStoresLabel` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetStoresLabel(
        &self,
        _ctx: &Context,
        _stores: &[u64],
        _labelKey: &str,
        _labelValue: &str,
    ) -> Result<()> {
        Ok(())
    }
    /// `WaitRegionsScattered`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `WaitRegionsScattered` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn WaitRegionsScattered(
        &self,
        _ctx: &Context,
        _regionInfos: &[RegionInfo],
    ) -> Result<(i32, SharedError)> {
        Ok((0, New("")))
    }
    /// `GetCodecPDClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetCodecPDClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetCodecPDClient(&self) -> Option<CodecPDClient> {
        None
    }
}
