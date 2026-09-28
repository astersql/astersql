// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// This file is copied from pingcap/tidb/store/tikv/pd_codec.go https://git.io/Je1Ww

//! PD client wrapper that encodes keys before requests and decodes region meta keys
//! after responses — port of `br/tests/br_key_locked/codec.go`.
//! key-locked 集成测试用的编解码 PD 客户端包装器。
//! 出站 key 做 EncodeBytes，入站 Region.Meta 的 Start/EndKey 再 Decode，对齐 TiDB 内存键空间。
//! 复制自 tikv pd_codec，便于在 BR 测试中隔离 PD 边界而不依赖完整 store 栈。
//! EncodeBytes 使用空缓冲前缀，输出为 TiDB 编码键。
//! DecodeBytes 还原 Meta 边界，供上层以原始键比较/断言。
//! 空 EndKey 表示正无穷，扫描时不得编码，以免改变区间语义。
//! GetRegionByID 无出站 key，但仍须解码响应 Meta。
//! GetTS 与键空间无关，完全透传内层客户端。
//! PdClient trait 实现便于在测试中替换真实 PD。
//! processRegionResult 统一错误 Trace 与 None 短路。
//! decodeRegionMetaKey 对空 Start/End 跳过，避免误解码。
//! 本包装器只服务 br_key_locked 场景，不是通用生产 PD 客户端。
//! 与 Go codec.go 方法集合保持一一对应，便于对照移植。
//! ScanRegions 保留 Go 调用形态注释，抑制静态检查噪音。

use crate::stubs::{Context, Error, PdClient, Result, codec, metapb, opt, router};

/// codecPDClient wraps a PD client and applies TiDB key encoding at the boundary.
/// 在 PD 边界统一编解码，避免测试直接操作 encoded key。
pub struct CodecPDClient<C: PdClient> {
    pub inner: C,
}

impl<C: PdClient> CodecPDClient<C> {
    /// 包装内层 PD 客户端。
    pub fn new(inner: C) -> Self {
        Self { inner }
    }

    /// GetRegion encodes the key before send requests to pd-server and decodes the
    /// returned StartKey && EndKey from pd-server.
    /// 查询前编码 key，返回后解码 Meta，保证调用方始终见原始键。
    pub fn GetRegion(
        &self,
        ctx: &Context,
        key: &[u8],
        _opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>> {
        // 出站编码，入站由 processRegionResult 解码。
        let encoded_key = codec::EncodeBytes(Vec::new(), key);
        let region = self.inner.GetRegion(ctx, &encoded_key, &[]);
        processRegionResult(region)
    }

    /// GetPrevRegion 同样先编码再查询，语义对齐 Go 包装器。
    pub fn GetPrevRegion(
        &self,
        ctx: &Context,
        key: &[u8],
        _opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>> {
        // Prev 查询同样需要编码边界键。
        let encoded_key = codec::EncodeBytes(Vec::new(), key);
        let region = self.inner.GetPrevRegion(ctx, &encoded_key, &[]);
        processRegionResult(region)
    }

    /// GetRegionByID encodes the key before send requests to pd-server and decodes the
    /// returned StartKey && EndKey from pd-server.
    /// 按 ID 查询无需编码入参 key，但仍须解码返回的 Meta 边界。
    pub fn GetRegionByID(
        &self,
        ctx: &Context,
        region_id: u64,
        _opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>> {
        let region = self.inner.GetRegionByID(ctx, region_id, &[]);
        processRegionResult(region)
    }

    /// 扫描区间：start 必编码；end 为空则保持空（表示正无穷），非空才编码。
    pub fn ScanRegions(
        &self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
        limit: i32,
        opts: &[opt::GetRegionOption],
    ) -> Result<Vec<Option<router::Region>>> {
        let start_key = codec::EncodeBytes(Vec::new(), start_key);
        let end_key = if !end_key.is_empty() {
            codec::EncodeBytes(Vec::new(), end_key)
        } else {
            end_key.to_vec()
        };

        //nolint:staticcheck — keep Go ScanRegions call shape.
        // 保留 Go ScanRegions 调用形态；逐条解码 Meta，跳过 None 槽位。
        let mut regions = self
            .inner
            .ScanRegions(ctx, &start_key, &end_key, limit, opts)
            .map_err(Error::Trace)?;
        for region in &mut regions {
            if let Some(r) = region.as_mut() {
                decodeRegionMetaKey(&mut r.Meta).map_err(Error::Trace)?;
            }
        }
        Ok(regions)
    }

    /// GetTS 不涉及 key 编解码，直接透传内层 PD。
    pub fn GetTS(&self, ctx: &Context) -> Result<(i64, i64)> {
        self.inner.GetTS(ctx)
    }
}

/// 实现 PdClient trait，委托到同名方法，便于泛型替换。
impl<C: PdClient> PdClient for CodecPDClient<C> {
    fn GetRegion(
        &self,
        ctx: &Context,
        key: &[u8],
        opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>> {
        CodecPDClient::GetRegion(self, ctx, key, opts)
    }

    fn GetPrevRegion(
        &self,
        ctx: &Context,
        key: &[u8],
        opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>> {
        CodecPDClient::GetPrevRegion(self, ctx, key, opts)
    }

    fn GetRegionByID(
        &self,
        ctx: &Context,
        region_id: u64,
        opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>> {
        CodecPDClient::GetRegionByID(self, ctx, region_id, opts)
    }

    fn ScanRegions(
        &self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
        limit: i32,
        opts: &[opt::GetRegionOption],
    ) -> Result<Vec<Option<router::Region>>> {
        CodecPDClient::ScanRegions(self, ctx, start_key, end_key, limit, opts)
    }

    fn GetTS(&self, ctx: &Context) -> Result<(i64, i64)> {
        CodecPDClient::GetTS(self, ctx)
    }
}

/// 统一处理单 Region 查询结果：错误包装 Trace，None 透传，Some 则解码 Meta。
fn processRegionResult(region: Result<Option<router::Region>>) -> Result<Option<router::Region>> {
    let mut region = match region {
        Err(err) => return Err(Error::Trace(err)),
        Ok(None) => return Ok(None),
        Ok(Some(r)) => r,
    };
    decodeRegionMetaKey(&mut region.Meta).map_err(Error::Trace)?;
    Ok(Some(region))
}

/// 解码 Region Meta 的 StartKey/EndKey；空键跳过（空表示边界无穷）。
fn decodeRegionMetaKey(r: &mut metapb::Region) -> Result<()> {
    // 非空 StartKey 解码回原始键。
    if !r.StartKey.is_empty() {
        let (_, decoded) = codec::DecodeBytes(&r.StartKey, None).map_err(Error::Trace)?;
        r.StartKey = decoded;
    }
    // 非空 EndKey 同理；空键表示无穷界，保持不动。
    if !r.EndKey.is_empty() {
        let (_, decoded) = codec::DecodeBytes(&r.EndKey, None).map_err(Error::Trace)?;
        r.EndKey = decoded;
    }
    Ok(())
}
