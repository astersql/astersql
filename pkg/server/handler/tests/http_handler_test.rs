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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use astersql_executor_mppcoordmanager::InstanceMPPCoordinatorManager;
use astersql_server::server::{
    Domain, Server, ServerConfig, ServerDriver, StatusConfig, TlsConfig,
};
use astersql_server_internal_testserverclient::TestServerClient;
use astersql_sessionctx_stmtctx::NewStmtCtx;
use astersql_types::datum::{NewBytesDatum, NewFloat64Datum, NewIntDatum};
use astersql_util_codec::NewEncoder;

// HTTP status handler 主测试集。
//
// 覆盖 Region 范围、MVCC、schema、DDL、标签与升级等接口；
// Region 为 TiKV 数据分片，MVCC 为多版本并发控制。

/// Go `basicHTTPHandlerTestSuite` 的可运行 Rust 测试夹具。
pub struct BasicHttpHandlerTestSuite {
    /// 限制同时存活的真实 TCP suite，避免默认测试并行度压垮 listener。
    _suite_permit: HandlerSuitePermit,
    /// 真实 TCP status 请求客户端。
    pub client: TestServerClient,
    /// 运行中的 HTTP status server。
    pub server: Arc<Server>,
}

fn labels_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct HandlerSuitePermit;

fn handler_suite_limit() -> &'static (Mutex<usize>, Condvar) {
    static LIMIT: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    LIMIT.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

impl HandlerSuitePermit {
    fn acquire() -> Self {
        const MAX_CONCURRENT_SUITES: usize = 4;
        let (active, changed) = handler_suite_limit();
        let mut active = active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *active >= MAX_CONCURRENT_SUITES {
            active = changed
                .wait(active)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *active += 1;
        Self
    }
}

impl Drop for HandlerSuitePermit {
    fn drop(&mut self) {
        let (active, changed) = handler_suite_limit();
        let mut active = active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active -= 1;
        changed.notify_one();
    }
}

impl Drop for BasicHttpHandlerTestSuite {
    fn drop(&mut self) {
        self.server.close();
    }
}

struct HandlerTestDriver;

impl ServerDriver for HandlerTestDriver {
    fn name(&self) -> &str {
        "handler-test"
    }
}

struct HandlerTestDomain {
    dxf_runtime: Arc<super::dxf_test::ScheduleStatusRuntime>,
}

impl Domain for HandlerTestDomain {
    fn server_id(&self) -> u64 {
        1
    }

    fn start_timestamp(&self) -> i64 {
        0
    }

    fn tikv_runtime(&self) -> Option<Arc<dyn astersql_server_handler_tikvhandler::TikvRuntime>> {
        Some(Arc::new(
            super::http_handler_serial_test::SerialHandlerRuntime,
        ))
    }

    fn dxf_runtime(&self) -> Option<Arc<dyn astersql_server_handler_tikvhandler::DxfRuntime>> {
        Some(self.dxf_runtime.clone())
    }
}

/// 创建绑定临时端口的 status server，并返回使用真实 TCP 的测试客户端。
pub fn create_basic_http_handler_test_suite() -> BasicHttpHandlerTestSuite {
    let suite_permit = HandlerSuitePermit::acquire();
    super::main_test::initialize_handler_test_environment()
        .expect("Go TestMain-equivalent setup must complete before starting a handler server");
    let server = Server::new(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(HandlerTestDriver),
    )
    .expect("handler test server configuration must be valid");
    server
        .run(Arc::new(HandlerTestDomain {
            dxf_runtime: Arc::new(super::dxf_test::ScheduleStatusRuntime::default()),
        }))
        .expect("handler test status server must start");

    let status_address = server
        .status_listener_addr()
        .expect("running server must expose a status listener");
    let sql_address = server
        .listener_addr()
        .expect("running server must expose a SQL listener");
    let mut client = TestServerClient::new();
    client.host = status_address.ip().to_string();
    client.port = sql_address.port();
    client.status_port = status_address.port();
    client
        .wait_until_server_online(Duration::from_secs(2))
        .expect("handler test status server must become online");

    BasicHttpHandlerTestSuite {
        _suite_permit: suite_permit,
        client,
        server,
    }
}

#[test]
fn basic_http_handler_suite_serves_status_over_real_tcp() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/status")
        .expect("the fixture must expose a real status listener");

    assert_eq!(response.status, 200);
    assert!(
        response
            .text()
            .expect("status body is UTF-8")
            .contains("connections")
    );
}

#[test]
// TestRegionIndexRange 对应 Go 函数 `func TestRegionIndexRange(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestRegionIndexRange 对应 Go 函数 `func TestRegionIndexRange(t *testing.T) {`。
pub fn test_region_index_range() {
    let index_values = vec![
        NewIntDatum(100),
        NewBytesDatum(b"foobar".to_vec()),
        NewFloat64Datum(-100.25),
    ];
    let encoded_value = NewEncoder(false)
        .EncodeKey(NewStmtCtx().TimeZone(), Vec::new(), index_values)
        .expect("Go's comparable index datum encoding must succeed");
    let region = astersql_store_helper::KeyLocation {
        Region: astersql_store_helper::RegionVerID::default(),
        StartKey: astersql_tablecodec::EncodeIndexSeekKey(3, 11, Some(encoded_value)).0,
        EndKey: astersql_tablecodec::EncodeRecordKey(
            astersql_tablecodec::GenTableRecordPrefix(9),
            Box::new(astersql_kv::IntHandle(133)),
        )
        .0,
    };
    let mut frames = astersql_store_helper::NewRegionFrameRange(region)
        .expect("index and record keys must form a region frame range");
    assert_eq!(frames.First.IndexID, 11);
    assert!(!frames.First.IsRecord);
    assert_eq!(frames.First.RecordID, 0);
    assert_eq!(frames.First.IndexValues, ["100", "foobar", "-100.25"]);
    assert_eq!(frames.Last.RecordID, 133);
    assert!(frames.Last.IndexValues.is_empty());
    for (table_id, index_id, covered) in [
        (2, 0, false),
        (3, 0, true),
        (9, 0, true),
        (10, 0, false),
        (2, 10, false),
        (3, 10, false),
        (3, 11, true),
        (3, 20, true),
        (9, 10, true),
        (10, 1, false),
    ] {
        let frame = if index_id == 0 {
            frames.GetRecordFrame(table_id, "", "", false)
        } else {
            frames.GetIndexFrame(table_id, index_id, "", "", "")
        };
        assert_eq!(frame.is_some(), covered, "{table_id}/{index_id}");
    }

    // Go 原始签名: func TestRegionIndexRange(t *testing.T) {
    // 状态准备: sTableID := int64(3)
    // 状态准备: sIndex := int64(11)
    // 状态准备: eTableID := int64(9)
    // 状态准备: recordID := int64(133)
    // 状态准备: indexValues := []types.Datum{
    // 迁移语句: types.NewIntDatum(100),
    // 迁移语句: types.NewBytesDatum([]byte("foobar")),
    // 迁移语句: types.NewFloat64Datum(-100.25),
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: expectIndexValues := make([]string, 0, len(indexValues))
    // 循环遍历: for _, v := range indexValues {
    // 状态准备: str, err := v.ToString()
    // 关键分支: if err != nil {
    // 格式化参数: str = fmt.Sprintf("%d-%v", v.Kind(), v.GetValue())
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: expectIndexValues = append(expectIndexValues, str)
    // 迁移语句: 结束上一层 Go 代码块。
    // 时间相关: encodedValue, err := codec.EncodeKey(stmtctx.NewStmtCtxWithTimeZone(time.Local).TimeZone(), nil, indexValues...)
    // 错误处理: require.NoError(t, err)

    // 状态准备: startKey := tablecodec.EncodeIndexSeekKey(sTableID, sIndex, encodedValue)
    // 状态准备: recordPrefix := tablecodec.GenTableRecordPrefix(eTableID)
    // 状态准备: endKey := tablecodec.EncodeRecordKey(recordPrefix, kv.IntHandle(recordID))

    // 状态准备: region := &tikv.KeyLocation{
    // 迁移语句: Region: tikv.RegionVerID{},
    // 迁移语句: StartKey: startKey,
    // 迁移语句: EndKey: endKey,
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: r, err := helper.NewRegionFrameRange(region)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, sIndex, r.First.IndexID)
    // 断言: require.False(t, r.First.IsRecord)
    // 断言: require.Equal(t, int64(0), r.First.RecordID)
    // 断言: require.Equal(t, expectIndexValues, r.First.IndexValues)
    // 断言: require.Equal(t, recordID, r.Last.RecordID)
    // 迁移语句: require.Nil(t, r.Last.IndexValues)

    // 状态准备: testCases := []struct {
    // 迁移语句: tableID int64
    // 迁移语句: indexID int64
    // 迁移语句: isCover bool
    // 迁移语句: }{
    // 迁移语句: {2, 0, false},
    // 迁移语句: {3, 0, true},
    // 迁移语句: {9, 0, true},
    // 迁移语句: {10, 0, false},
    // 迁移语句: {2, 10, false},
    // 迁移语句: {3, 10, false},
    // 迁移语句: {3, 11, true},
    // 迁移语句: {3, 20, true},
    // 迁移语句: {9, 10, true},
    // 迁移语句: {10, 1, false},
    // 迁移语句: 结束上一层 Go 代码块。
    // 循环遍历: for _, c := range testCases {
    // 迁移语句: var f *helper.FrameItem
    // 关键分支: if c.indexID == 0 {
    // 状态准备: f = r.GetRecordFrame(c.tableID, "", "", false)
    // 迁移语句: } else {
    // 状态准备: f = r.GetIndexFrame(c.tableID, c.indexID, "", "", "")
    // 迁移语句: 结束上一层 Go 代码块。
    // 关键分支: if c.isCover {
    // 断言: require.NotNil(t, f)
    // 迁移语句: } else {
    // 迁移语句: require.Nil(t, f)
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
/// auto id owner handler returns http 500 when unhealthy。
fn auto_id_owner_handler_returns_http_500_when_unhealthy() {
    use astersql_server_handler::auto_id_owner_handler::{
        AutoIDOwnerChecker, AutoIDOwnerResponse, NewAutoIDOwnerHandler, autoIDOwnerStatus,
    };

    /// Checker。
    struct Checker;
    impl AutoIDOwnerChecker for Checker {
        /// Health。
        fn Health(&self) -> bool {
            false
        }
        /// IsAutoIDOwner。
        fn IsAutoIDOwner(&self) -> bool {
            panic!("owner state must not be queried when health check fails")
        }
    }

    #[derive(Default)]
    /// Response。
    struct Response {
        status: Option<u16>,
        owner_written: bool,
    }
    impl AutoIDOwnerResponse for Response {
        /// Error。
        type Error = ();
        /// write status。
        fn write_status(&mut self, status: u16) -> Result<(), Self::Error> {
            self.status = Some(status);
            Ok(())
        }
        /// write owner status。
        fn write_owner_status(&mut self, _status: autoIDOwnerStatus) -> Result<(), Self::Error> {
            self.owner_written = true;
            Ok(())
        }
    }

    let mut response = Response::default();
    NewAutoIDOwnerHandler(Checker)
        .ServeHTTP(&mut response)
        .expect("health failure is rendered as an HTTP status");
    assert_eq!(response.status, Some(500));
    assert!(!response.owner_written);
}

#[test]
// TestRegionCommonHandleRange 对应 Go 函数 `func TestRegionCommonHandleRange(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestRegionCommonHandleRange 对应 Go 函数 `func TestRegionCommonHandleRange(t *testing.T) {`。
pub fn test_region_common_handle_range() {
    let index_values = vec![
        NewIntDatum(100),
        NewBytesDatum(b"foobar".to_vec()),
        NewFloat64Datum(-100.25),
    ];
    let encoded_value = NewEncoder(false)
        .EncodeKey(NewStmtCtx().TimeZone(), Vec::new(), index_values)
        .expect("Go's common-handle datum encoding must succeed");
    let region = astersql_store_helper::KeyLocation {
        Region: astersql_store_helper::RegionVerID::default(),
        StartKey: astersql_tablecodec::EncodeRowKey(3, &encoded_value).0,
        EndKey: Vec::new(),
    };
    let frames = astersql_store_helper::NewRegionFrameRange(region)
        .expect("a common-handle record key with an open end must decode");
    assert!(frames.First.IsRecord);
    assert_eq!(frames.First.RecordID, 0);
    assert_eq!(frames.First.IndexValues, ["100", "foobar", "-100.25"]);
    assert_eq!(frames.First.IndexName, "PRIMARY");
    assert_eq!(frames.Last.RecordID, 0);
    assert!(frames.Last.IndexValues.is_empty());

    // Go 原始签名: func TestRegionCommonHandleRange(t *testing.T) {
    // 状态准备: sTableID := int64(3)
    // 状态准备: indexValues := []types.Datum{
    // 迁移语句: types.NewIntDatum(100),
    // 迁移语句: types.NewBytesDatum([]byte("foobar")),
    // 迁移语句: types.NewFloat64Datum(-100.25),
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: expectIndexValues := make([]string, 0, len(indexValues))
    // 循环遍历: for _, v := range indexValues {
    // 状态准备: str, err := v.ToString()
    // 关键分支: if err != nil {
    // 格式化参数: str = fmt.Sprintf("%d-%v", v.Kind(), v.GetValue())
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: expectIndexValues = append(expectIndexValues, str)
    // 迁移语句: 结束上一层 Go 代码块。
    // 时间相关: encodedValue, err := codec.EncodeKey(stmtctx.NewStmtCtxWithTimeZone(time.Local).TimeZone(), nil, indexValues...)
    // 错误处理: require.NoError(t, err)

    // 状态准备: startKey := tablecodec.EncodeRowKey(sTableID, encodedValue)

    // 状态准备: region := &tikv.KeyLocation{
    // 迁移语句: Region: tikv.RegionVerID{},
    // 迁移语句: StartKey: startKey,
    // 迁移语句: EndKey: nil,
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: r, err := helper.NewRegionFrameRange(region)
    // 错误处理: require.NoError(t, err)
    // 断言: require.True(t, r.First.IsRecord)
    // 断言: require.Equal(t, int64(0), r.First.RecordID)
    // 断言: require.Equal(t, expectIndexValues, r.First.IndexValues)
    // 断言: require.Equal(t, "PRIMARY", r.First.IndexName)
    // 断言: require.Equal(t, int64(0), r.Last.RecordID)
    // 迁移语句: require.Nil(t, r.Last.IndexValues)
}

#[test]
// TestRegionIndexRangeWithEndNoLimit 对应 Go 函数 `func TestRegionIndexRangeWithEndNoLimit(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestRegionIndexRangeWithEndNoLimit 对应 Go 函数 `func TestRegionIndexRangeWithEndNoLimit(t *testing.T) {`。
pub fn test_region_index_range_with_end_no_limit() {
    let region = astersql_store_helper::KeyLocation {
        Region: astersql_store_helper::RegionVerID::default(),
        StartKey: astersql_tablecodec::GenTableRecordPrefix(15).0,
        EndKey: b"z_aaaaafdfd".to_vec(),
    };
    let mut frames = astersql_store_helper::NewRegionFrameRange(region)
        .expect("table record prefix and open upper range must decode");
    assert!(frames.First.IsRecord);
    assert!(frames.Last.IsRecord);
    assert!(frames.GetRecordFrame(300, "", "", false).is_some());
    assert!(frames.GetIndexFrame(200, 100, "", "", "").is_some());

    // Go 原始签名: func TestRegionIndexRangeWithEndNoLimit(t *testing.T) {
    // 状态准备: sTableID := int64(15)
    // 状态准备: startKey := tablecodec.GenTableRecordPrefix(sTableID)
    // 状态准备: endKey := []byte("z_aaaaafdfd")
    // 状态准备: region := &tikv.KeyLocation{
    // 迁移语句: Region: tikv.RegionVerID{},
    // 迁移语句: StartKey: startKey,
    // 迁移语句: EndKey: endKey,
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: r, err := helper.NewRegionFrameRange(region)
    // 错误处理: require.NoError(t, err)
    // 断言: require.True(t, r.First.IsRecord)
    // 断言: require.True(t, r.Last.IsRecord)
    // 断言: require.NotNil(t, r.GetRecordFrame(300, "", "", false))
    // 断言: require.NotNil(t, r.GetIndexFrame(200, 100, "", "", ""))
}

#[test]
// TestRegionIndexRangeWithStartNoLimit 对应 Go 函数 `func TestRegionIndexRangeWithStartNoLimit(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestRegionIndexRangeWithStartNoLimit 对应 Go 函数 `func TestRegionIndexRangeWithStartNoLimit(t *testing.T) {`。
pub fn test_region_index_range_with_start_no_limit() {
    let region = astersql_store_helper::KeyLocation {
        Region: astersql_store_helper::RegionVerID::default(),
        StartKey: b"m_aaaaafdfd".to_vec(),
        EndKey: astersql_tablecodec::GenTableRecordPrefix(9).0,
    };
    let mut frames = astersql_store_helper::NewRegionFrameRange(region)
        .expect("open lower range and table record prefix must decode");
    assert!(!frames.First.IsRecord);
    assert!(frames.Last.IsRecord);
    assert!(frames.GetRecordFrame(3, "", "", false).is_some());
    assert!(frames.GetIndexFrame(8, 1, "", "", "").is_some());

    // Go 原始签名: func TestRegionIndexRangeWithStartNoLimit(t *testing.T) {
    // 状态准备: eTableID := int64(9)
    // 状态准备: startKey := []byte("m_aaaaafdfd")
    // 状态准备: endKey := tablecodec.GenTableRecordPrefix(eTableID)
    // 状态准备: region := &tikv.KeyLocation{
    // 迁移语句: Region: tikv.RegionVerID{},
    // 迁移语句: StartKey: startKey,
    // 迁移语句: EndKey: endKey,
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: r, err := helper.NewRegionFrameRange(region)
    // 错误处理: require.NoError(t, err)
    // 断言: require.False(t, r.First.IsRecord)
    // 断言: require.True(t, r.Last.IsRecord)
    // 断言: require.NotNil(t, r.GetRecordFrame(3, "", "", false))
    // 断言: require.NotNil(t, r.GetIndexFrame(8, 1, "", "", ""))
}

#[test]
// TestRegionsAPI 对应 Go 函数 `func TestRegionsAPI(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestRegionsAPI 对应 Go 函数 `func TestRegionsAPI(t *testing.T) {`。
pub fn test_regions_api() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/tables/tidb/t/regions")
        .expect("table regions route must reach TableHandler");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"name\":\"t\""));
    assert!(body.contains("\"id\":42"));
    assert!(body.contains("\"id\":11"));
    assert!(body.contains("\"indices\":["));
    assert!(body.contains("\"name\":\"PRIMARY\""));
    assert!(body.contains("\"name\":\"idx\""));

    // Go 原始签名: func TestRegionsAPI(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // 迁移语句: ts.prepareData(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/tables/tidb/t/regions")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)

    // 迁移语句: var data tikvhandler.TableRegions
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 断言: require.True(t, len(data.RecordRegions) > 0)

    // 保留 Go 注释: // list region
    // 循环遍历: for _, region := range data.RecordRegions {
    // 断言: require.True(t, ts.regionContainsTable(t, region.ID, data.TableID))
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestRegionsAPIForClusterIndex 对应 Go 函数 `func TestRegionsAPIForClusterIndex(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestRegionsAPIForClusterIndex 对应 Go 函数 `func TestRegionsAPIForClusterIndex(t *testing.T) {`。
pub fn test_regions_api_for_cluster_index() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/tables/tidb/t/regions")
        .expect("region list must expose a region id for detail lookup");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"id\":11"));
    let response = suite
        .client
        .fetch_status("/regions/11")
        .expect("listed region id must be queryable through RegionHandler");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"table_id\":42"));
    assert_eq!(body.matches("\"table_name\":\"t\"").count(), 2);
    assert!(body.contains("\"start_key\":\"YQ==\""));
    assert!(body.contains("\"end_key\":\"eg==\""));
    assert!(body.contains("\"start_key_hex\":\"61\""));
    assert!(body.contains("\"end_key_hex\":\"7a\""));
    assert!(body.contains("\"index_name\":\"PRIMARY\""));
    assert!(body.contains("\"index_name\":\"idx\""));

    // Go 原始签名: func TestRegionsAPIForClusterIndex(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // 迁移语句: ts.prepareData(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/tables/tidb/t/regions")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var data tikvhandler.TableRegions
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 断言: require.True(t, len(data.RecordRegions) > 0)
    // 保留 Go 注释: // list region
    // 循环遍历: for _, region := range data.RecordRegions {
    // HTTP 请求: resp, err := ts.FetchStatus(fmt.Sprintf("/regions/%d", region.ID))
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var data tikvhandler.RegionDetail
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 状态准备: frameCnt := 0
    // 循环遍历: for _, f := range data.Frames {
    // 关键分支: if f.DBName == "tidb" && f.TableName == "t" {
    // 迁移语句: frameCnt++
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 保留 Go 注释: // frameCnt = clustered primary key + secondary index(idx) = 2.
    // 断言: require.Equal(t, 2, frameCnt)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestRangesAPI 对应 Go 函数 `func TestRangesAPI(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestRangesAPI 对应 Go 函数 `func TestRangesAPI(t *testing.T) {`。
pub fn test_ranges_api() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/tables/tidb/t/ranges")
        .expect("table ranges route must reach TableHandler");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"name\":\"t\""));
    assert!(body.contains("\"id\":42"));
    assert!(body.contains("\"table\":{"));
    assert!(body.contains("\"record\":{"));
    assert!(body.contains("\"index\":{"));
    assert!(body.contains("\"indices\":{\"PRIMARY\":"));
    assert!(body.contains("\"idx\":{"));

    // Go 原始签名: func TestRangesAPI(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // 迁移语句: ts.prepareData(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/tables/tidb/t/ranges")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)

    // 迁移语句: var data tikvhandler.TableRanges
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, "t", data.TableName)
    // 断言: require.Equal(t, 2, len(data.Indices))
    // 状态准备: _, ok := data.Indices["PRIMARY"]
    // 断言: require.True(t, ok)
    // 状态准备: _, ok = data.Indices["idx"]
    // 断言: require.True(t, ok)
}

// regionContainsTable 对应 Go 函数 `func (ts *basicHTTPHandlerTestSuite) regionContainsTable(t *testing.T, regionID uint64, tableID int64) bool {`。
// 这是 Go 方法：接收者只体现在函数名中，真实 impl 接线留给后续任务。
/// regionContainsTable 对应 Go 函数 `func (ts *basicHTTPHandlerTestSuite) regionContainsTable(t *testing.T, regionID uint64,...
pub fn ts_basic_http_handler_test_suite_region_contains_table() {
    // Go 原始签名: func (ts *basicHTTPHandlerTestSuite) regionContainsTable(t *testing.T, regionID uint64, tableID int64) bool {
    // HTTP 请求: resp, err := ts.FetchStatus(fmt.Sprintf("/regions/%d", regionID))
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var data tikvhandler.RegionDetail
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 循环遍历: for _, index := range data.Frames {
    // 关键分支: if index.TableID == tableID {
    // 返回值: return true
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 返回值: return false
}

#[test]
// TestListTableRegions 对应 Go 函数 `func TestListTableRegions(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestListTableRegions 对应 Go 函数 `func TestListTableRegions(t *testing.T) {`。
pub fn test_list_table_regions() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/tables/fdsfds/aaa/regions")
        .expect("unknown table regions request must receive an error response");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("table not exists"));

    let response = suite
        .client
        .fetch_status("/tables/tidb/pt/regions")
        .expect("partition table regions route must respond");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.starts_with('['));
    assert!(body.ends_with(']'));
    for (index, table_id) in [100_i64, 101, 102].into_iter().enumerate() {
        assert!(body.contains(&format!(r#""name":"p{index}""#)));
        assert!(body.contains(&format!(r#""id":{table_id}"#)));
        assert!(body.contains(&format!(r#""record_regions":[{{"id":{table_id}}}]"#)));
    }

    let response = suite
        .client
        .fetch_status("/tables/tidb/pt(p1)/regions")
        .expect("partition-qualified table regions route must respond");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains(r#""name":"p0""#));
    assert!(body.contains(r#""name":"p1""#));
    assert!(body.contains(r#""name":"p2""#));

    let response = suite
        .client
        .fetch_status("/regions/101")
        .expect("partition physical table ID must resolve as a region detail route");
    assert_eq!(response.status, 200);
    assert!(response.text().unwrap().contains("pt(p1)"));

    // Go 原始签名: func TestListTableRegions(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // 迁移语句: ts.prepareData(t)
    // 保留 Go 注释: // Test list table regions with error
    // HTTP 请求: resp, err := ts.FetchStatus("/tables/fdsfds/aaa/regions")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/tables/tidb/pt/regions")
    // 错误处理: require.NoError(t, err)

    // 迁移语句: var data []*tikvhandler.TableRegions
    // JSON 编解码: dec := json.NewDecoder(resp.Body)
    // 状态准备: err = dec.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 状态准备: region := data[1]
    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/regions/%d", region.TableID))
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
}

#[test]
// TestListTableRanges 对应 Go 函数 `func TestListTableRanges(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestListTableRanges 对应 Go 函数 `func TestListTableRanges(t *testing.T) {`。
pub fn test_list_table_ranges() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/tables/fdsfds/aaa/ranges")
        .expect("unknown table ranges request must receive an error response");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("table not exists"));

    let response = suite
        .client
        .fetch_status("/tables/tidb/pt/ranges")
        .expect("partition table ranges route must respond");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.starts_with('['));
    assert!(body.ends_with(']'));
    for (index, table_id) in [100_i64, 101, 102].into_iter().enumerate() {
        assert!(body.contains(&format!(r#""name":"p{index}""#)));
        assert!(body.contains(&format!(r#""id":{table_id}"#)));
    }
    assert_eq!(body.matches(r#""table":"#).count(), 3);
    assert_eq!(body.matches(r#""record":"#).count(), 3);
    assert_eq!(body.matches(r#""PRIMARY":"#).count(), 3);
    assert_eq!(body.matches(r#""idx":"#).count(), 3);

    // Go 原始签名: func TestListTableRanges(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // 迁移语句: ts.prepareData(t)
    // 保留 Go 注释: // Test list table regions with error
    // HTTP 请求: resp, err := ts.FetchStatus("/tables/fdsfds/aaa/ranges")
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)

    // HTTP 请求: resp, err = ts.FetchStatus("/tables/tidb/pt/ranges")
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()

    // 迁移语句: var data []*tikvhandler.TableRanges
    // JSON 编解码: dec := json.NewDecoder(resp.Body)
    // 状态准备: err = dec.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 3, len(data))
    // 循环遍历: for i, partition := range data {
    // 断言: require.Equal(t, fmt.Sprintf("p%d", i), partition.TableName)
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestGetRegionByIDWithError 对应 Go 函数 `func TestGetRegionByIDWithError(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestGetRegionByIDWithError 对应 Go 函数 `func TestGetRegionByIDWithError(t *testing.T) {`。
pub fn test_get_region_by_id_with_error() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/regions/11")
        .expect("region detail route must reach RegionHandler");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"region_id\":11"));
    assert!(body.contains("\"table_name\":\"t\""));

    let response = suite
        .client
        .fetch_status("/regions/xxx")
        .expect("invalid region id must receive a response");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("invalid region id"));

    // Go 原始签名: func TestGetRegionByIDWithError(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/regions/xxx")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
}

// startServer 对应 Go 函数 `func (ts *basicHTTPHandlerTestSuite) startServer(t *testing.T, storeOpts ...mockstore.MockTiKVStoreOption) {`。
// 这是 Go 方法：接收者只体现在函数名中，真实 impl 接线留给后续任务。
/// startServer 对应 Go 函数 `func (ts *basicHTTPHandlerTestSuite) startServer(t *testing.T, storeOpts ...mockstore.MockTiKVS...
pub fn ts_basic_http_handler_test_suite_start_server() {
    // Go 原始签名: func (ts *basicHTTPHandlerTestSuite) startServer(t *testing.T, storeOpts ...mockstore.MockTiKVStoreOption) {
    // 迁移语句: var err error
    // 状态准备: ts.store, err = teststore.NewMockStoreWithoutBootstrap(storeOpts...)
    // 错误处理: require.NoError(t, err)
    // 状态准备: ts.domain, err = session.BootstrapSession(ts.store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: ts.tidbdrv = server2.NewTiDBDriver(ts.store)

    // 状态准备: cfg := util.NewTestConfig()
    // 状态准备: cfg.Store = config.StoreTypeTiKV
    // 状态准备: cfg.Port = 0
    // 状态准备: cfg.Status.StatusPort = 0
    // 状态准备: cfg.Status.ReportStatus = true
    // 并发通道: server2.RunInGoTestChan = make(chan struct{})
    // 状态准备: server, err := server2.NewServer(cfg, ts.tidbdrv)
    // 错误处理: require.NoError(t, err)
    // 状态准备: ts.server = server
    // 迁移语句: ts.server.SetDomain(ts.domain)
    // 并发/异步: go func() {
    // 状态准备: err := server.Run(ts.domain)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 并发同步: <-server2.RunInGoTestChan
    // 状态准备: ts.Port = testutil.GetPortFromTCPAddr(server.ListenAddr())
    // 状态准备: ts.StatusPort = testutil.GetPortFromTCPAddr(server.StatusListenerAddr())
    // 迁移语句: ts.WaitUntilServerOnline()

    // 状态准备: do, err := session.GetDomain(ts.store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: ts.sh = optimizor.NewStatsHandler(do)
}

// stopServer 对应 Go 函数 `func (ts *basicHTTPHandlerTestSuite) stopServer(t *testing.T) {`。
// 这是 Go 方法：接收者只体现在函数名中，真实 impl 接线留给后续任务。
/// stopServer 对应 Go 函数 `func (ts *basicHTTPHandlerTestSuite) stopServer(t *testing.T) {`。
pub fn ts_basic_http_handler_test_suite_stop_server() {
    // Go 原始签名: func (ts *basicHTTPHandlerTestSuite) stopServer(t *testing.T) {
    // 关键分支: if ts.server != nil {
    // 迁移语句: ts.server.Close()
    // 迁移语句: 结束上一层 Go 代码块。
    // 关键分支: if ts.domain != nil {
    // 迁移语句: ts.domain.Close()
    // 迁移语句: 结束上一层 Go 代码块。
    // 关键分支: if ts.store != nil {
    // 错误处理: require.NoError(t, ts.store.Close())
    // 迁移语句: 结束上一层 Go 代码块。
}

// prepareData 对应 Go 函数 `func (ts *basicHTTPHandlerTestSuite) prepareData(t *testing.T) {`。
// 这是 Go 方法：接收者只体现在函数名中，真实 impl 接线留给后续任务。
/// prepareData 对应 Go 函数 `func (ts *basicHTTPHandlerTestSuite) prepareData(t *testing.T) {`。
pub fn ts_basic_http_handler_test_suite_prepare_data() {
    // Go 原始签名: func (ts *basicHTTPHandlerTestSuite) prepareData(t *testing.T) {
    // 外部依赖/数据库: db, err := sql.Open("mysql", ts.GetDSN())
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: dbt := testkit.NewDBTestKit(t, db)

    // 迁移语句: dbt.MustExec("create database tidb;")
    // 迁移语句: dbt.MustExec("use tidb;")
    // 迁移语句: dbt.MustExec("create table tidb.test (a int auto_increment primary key, b varchar(20));")
    // 迁移语句: dbt.MustExec("insert tidb.test values (1, 1);")
    // 状态准备: txn1, err := dbt.GetDB().Begin()
    // 错误处理: require.NoError(t, err)
    // 状态准备: _, err = txn1.Exec("update tidb.test set b = b + 1 where a = 1;")
    // 错误处理: require.NoError(t, err)
    // 状态准备: _, err = txn1.Exec("insert tidb.test values (2, 2);")
    // 错误处理: require.NoError(t, err)
    // 状态准备: _, err = txn1.Exec("insert tidb.test (a) values (3);")
    // 错误处理: require.NoError(t, err)
    // 状态准备: _, err = txn1.Exec("insert tidb.test values (4, '');")
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = txn1.Commit()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: dbt.MustExec("alter table tidb.test add index idx1 (a, b);")
    // 迁移语句: dbt.MustExec("alter table tidb.test drop index idx1;")
    // 迁移语句: dbt.MustExec("alter table tidb.test add index idx1 (a, b);")
    // 迁移语句: dbt.MustExec("alter table tidb.test add unique index idx2 (a, b);")

    // 迁移语句: dbt.MustExec(`create table tidb.pt (a int primary key, b varchar(20), key idx(a, b))
    // 迁移语句: partition by range (a)
    // 迁移语句: (partition p0 values less than (256),
    // 迁移语句: partition p1 values less than (512),
    // 迁移语句: partition p2 values less than (1024))`)

    // 状态准备: txn2, err := dbt.GetDB().Begin()
    // 错误处理: require.NoError(t, err)
    // 状态准备: _, err = txn2.Exec("insert into tidb.pt values (42, '123')")
    // 错误处理: require.NoError(t, err)
    // 状态准备: _, err = txn2.Exec("insert into tidb.pt values (256, 'b')")
    // 错误处理: require.NoError(t, err)
    // 状态准备: _, err = txn2.Exec("insert into tidb.pt values (666, 'def')")
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = txn2.Commit()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: dbt.MustExec("drop table if exists t")
    // 迁移语句: dbt.MustExec("create table t (a double, b varchar(20), c int, primary key(a,b) clustered, key idx(c))")
    // 迁移语句: dbt.MustExec("insert into t values(1.1,'111',1),(2.2,'222',2)")
}

// decodeKeyMvcc 对应 Go 函数 `func decodeKeyMvcc(closer io.ReadCloser, t *testing.T, valid bool) {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// decodeKeyMvcc 对应 Go 函数 `func decodeKeyMvcc(closer io.ReadCloser, t *testing.T, valid bool) {`。
pub fn decode_key_mvcc() {
    // Go 原始签名: func decodeKeyMvcc(closer io.ReadCloser, t *testing.T, valid bool) {
    // JSON 编解码: decoder := json.NewDecoder(closer)
    // 迁移语句: var data []helper.MvccKV
    // 状态准备: err := decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 关键分支: if valid {
    // 断言: require.NotNil(t, data[0].Value.Info)
    // 迁移语句: require.Greater(t, len(data[0].Value.Info.Writes), 0)
    // 迁移语句: } else {
    // 迁移语句: require.Nil(t, data[0].Value.Info.Lock)
    // 迁移语句: require.Nil(t, data[0].Value.Info.Writes)
    // 迁移语句: require.Nil(t, data[0].Value.Info.Values)
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestGetTableMVCC 对应 Go 函数 `func TestGetTableMVCC(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestGetTableMVCC 对应 Go 函数 `func TestGetTableMVCC(t *testing.T) {`。
pub fn test_get_table_mvcc() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/mvcc/key/tidb/test/1?decode=true")
        .expect("MVCC key route must dispatch through MvccTxnHandler");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.text().unwrap(),
        r#"{"data":"mvcc-record:handle:1:decode=true"}"#
    );

    let response = suite
        .client
        .fetch_status("/mvcc/txn/0x2a/tidb/test")
        .expect("MVCC transaction route must accept Go base-0 timestamps");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#"{"data":"mvcc-txn:42"}"#);

    let response = suite
        .client
        .fetch_status("/mvcc/key/tidb/pt(p0)/42?decode=true")
        .expect("partition-qualified MVCC record route must resolve the logical table");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.text().unwrap(),
        r#"{"data":"mvcc-record:handle:42:decode=true"}"#
    );

    // Go 原始签名: func TestGetTableMVCC(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)

    // HTTP 请求: resp, err := ts.FetchStatus("/mvcc/key/tidb/test/1")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var data helper.MvccKV
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.NotNil(t, data.Value)
    // 状态准备: info := data.Value.Info
    // 断言: require.NotNil(t, info)
    // 迁移语句: require.Greater(t, len(info.Writes), 0)

    // 保留 Go 注释: // TODO: Unistore will not return Op_Lock.
    // 保留 Go 注释: // Use this workaround to support two backend, we can remove this hack after deprecated mocktikv.
    // 迁移语句: var startTs uint64
    // 循环遍历: for _, w := range info.Writes {
    // 关键分支: if w.Type == kvrpcpb.Op_Lock {
    // 迁移语句: continue
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: startTs = w.StartTs
    // 迁移语句: break
    // 迁移语句: 结束上一层 Go 代码块。

    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/mvcc/txn/%d/tidb/test", startTs))
    // 错误处理: require.NoError(t, err)
    // 迁移语句: var p2 helper.MvccKV
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&p2)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 循环遍历: for i, expect := range info.Values {
    // 状态准备: v2 := p2.Value.Info.Values[i].Value
    // 断言: require.Equal(t, expect.Value, v2)
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: hexKey := p2.Key
    // 关键分支: if kerneltype.IsNextGen() {
    // 保留 Go 注释: // EncodeKey(nil) returns the codec's key prefix. We use this to strip the prefix from the hex key.
    // 状态准备: keyPrefix := strings.ToUpper(hex.EncodeToString(ts.store.GetCodec().EncodeKey(nil)))
    // 状态准备: hexKey = strings.TrimPrefix(hexKey, keyPrefix)
    // 迁移语句: 结束上一层 Go 代码块。
    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/hex/" + hexKey)
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 迁移语句: var data2 helper.MvccKV
    // 状态准备: err = decoder.Decode(&data2)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, data, data2)

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/key/tidb/test/1?decode=true")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 迁移语句: var data3 map[string]any
    // 状态准备: err = decoder.Decode(&data3)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.NotNil(t, data3["key"])
    // 断言: require.NotNil(t, data3["info"])
    // 断言: require.NotNil(t, data3["data"])
    // 迁移语句: require.Nil(t, data3["decode_error"])

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/key/tidb/pt(p0)/42?decode=true")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 迁移语句: var data4 map[string]any
    // 状态准备: err = decoder.Decode(&data4)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.NotNil(t, data4["key"])
    // 断言: require.NotNil(t, data4["info"])
    // 断言: require.NotNil(t, data4["data"])
    // 迁移语句: require.Nil(t, data4["decode_error"])
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/key/tidb/t/42")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/key/tidb/t?a=1.1")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/key/tidb/t?a=1.1&b=111&decode=1")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 迁移语句: var data5 map[string]any
    // 状态准备: err = decoder.Decode(&data5)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, data4["key"])
    // 断言: require.NotNil(t, data4["info"])
    // 断言: require.NotNil(t, data4["data"])
    // 迁移语句: require.Nil(t, data4["decode_error"])
    // 错误处理: require.NoError(t, resp.Body.Close())
}

#[test]
// TestGetMVCCNotFound 对应 Go 函数 `func TestGetMVCCNotFound(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestGetMVCCNotFound 对应 Go 函数 `func TestGetMVCCNotFound(t *testing.T) {`。
pub fn test_get_mvcc_not_found() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/mvcc/key/tidb/test/1234")
        .expect("missing MVCC key must produce a valid handler payload");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#"{"data":"mvcc-not-found"}"#);

    drop(suite);
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/mvcc/hex/6D766363")
        .expect("MVCC hex route must dispatch through MvccTxnHandler");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#"{"data":"mvcc-hex"}"#);

    // Go 原始签名: func TestGetMVCCNotFound(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/mvcc/key/tidb/test/1234")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var data helper.MvccKV
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: require.Nil(t, data.Value.Info.Lock)
    // 迁移语句: require.Nil(t, data.Value.Info.Writes)
    // 迁移语句: require.Nil(t, data.Value.Info.Values)
}

#[test]
// TestDecodeColumnValue 对应 Go 函数 `func TestDecodeColumnValue(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestDecodeColumnValue 对应 Go 函数 `func TestDecodeColumnValue(t *testing.T) {`。
pub fn test_decode_column_value() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/tables/2/15/0/255?rowBin=YWJj")
        .expect("column value route must decode rowBin through ValueHandler");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#""decoded-column-2:abc""#);
    let invalid = suite
        .client
        .fetch_status("/tables/2/15/0/255")
        .expect("missing rowBin must be reported by ValueHandler");
    assert_eq!(invalid.status, 400);
    assert!(invalid.text().unwrap().contains("Invalid Query"));

    // Go 原始签名: func TestDecodeColumnValue(t *testing.T) {
    // 状态准备: router := mux.NewRouter()
    // 迁移语句: router.Handle("/tables/{colID}/{colTp}/{colFlag}/{colLen}", tikvhandler.ValueHandler{})

    // 保留 Go 注释: // column is a structure used for test
    // 迁移语句: type column struct {
    // 迁移语句: id int64
    // 迁移语句: tp *types.FieldType
    // 迁移语句: 结束上一层 Go 代码块。
    // 保留 Go 注释: // Backfill columns.
    // 状态准备: c1 := &column{id: 1, tp: types.NewFieldType(mysql.TypeLonglong)}
    // 状态准备: c2 := &column{id: 2, tp: types.NewFieldType(mysql.TypeVarchar)}
    // 状态准备: c3 := &column{id: 3, tp: types.NewFieldType(mysql.TypeNewDecimal)}
    // 状态准备: c4 := &column{id: 4, tp: types.NewFieldType(mysql.TypeTimestamp)}
    // 状态准备: cols := []*column{c1, c2, c3, c4}
    // 状态准备: row := make([]types.Datum, len(cols))
    // 状态准备: row[0] = types.NewIntDatum(100)
    // 状态准备: row[1] = types.NewBytesDatum([]byte("abc"))
    // 状态准备: row[2] = types.NewDecimalDatum(types.NewDecFromInt(1))
    // 时间相关: row[3] = types.NewTimeDatum(types.NewTime(types.FromGoTime(time.Now()), mysql.TypeTimestamp, 6))

    // 保留 Go 注释: // Encode the row.
    // 状态准备: colIDs := make([]int64, 0, 3)
    // 循环遍历: for _, col := range cols {
    // 状态准备: colIDs = append(colIDs, col.id)
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: rd := rowcodec.Encoder{Enable: true}
    // 时间相关: sc := stmtctx.NewStmtCtxWithTimeZone(time.UTC)
    // 状态准备: bs, err := tablecodec.EncodeRow(codec.NewEncoder(collate.NewCollationEnabled()), sc.TimeZone(), row, colIDs, nil, nil, nil, &rd)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, bs)
    // 状态准备: bin := base64.StdEncoding.EncodeToString(bs)

    // 状态准备: unitTest := func(col *column) {
    // 格式化参数: path := fmt.Sprintf("/tables/%d/%v/%d/%d?rowBin=%s", col.id, col.tp.GetType(), col.tp.GetFlag(), col.tp.GetFlen(), bin)
    // 状态准备: req := httptest.NewRequest(http.MethodGet, path, nil)
    // 状态准备: resp := httptest.NewRecorder()
    // 迁移语句: router.ServeHTTP(resp, req)
    // 断言: require.Equalf(t, http.StatusOK, resp.Code, "url: %v", path)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var data any
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoErrorf(t, err, "url: %v\ndata: %v", path, data)
    // 状态准备: colVal, err := types.DatumsToString([]types.Datum{row[col.id-1]}, false)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equalf(t, colVal, data, "url: %v", path)
    // 迁移语句: 结束上一层 Go 代码块。

    // 循环遍历: for _, col := range cols {
    // 迁移语句: unitTest(col)
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // Test bin has `+`.
    // 保留 Go 注释: // 2018-03-08 16:01:00.315313
    // 状态准备: bin = "CAIIyAEIBAIGYWJjCAYGAQCBCAgJsZ+TgISg1M8Z"
    // 时间相关: row[3] = types.NewTimeDatum(types.NewTime(types.FromGoTime(time.Date(2018, 3, 8, 16, 1, 0, 315313000, time.UTC)), mysql.TypeTimestamp, 6))
    // 迁移语句: unitTest(cols[3])

    // 保留 Go 注释: // Test bin has `/`.
    // 保留 Go 注释: // 2018-03-08 02:44:46.409199
    // 状态准备: bin = "CAIIyAEIBAIGYWJjCAYGAQCBCAgJ7/yY8LKF1M8Z"
    // 时间相关: row[3] = types.NewTimeDatum(types.NewTime(types.FromGoTime(time.Date(2018, 3, 8, 2, 44, 46, 409199000, time.UTC)), mysql.TypeTimestamp, 6))
    // 迁移语句: unitTest(cols[3])
}

#[test]
// TestGetIndexMVCC 对应 Go 函数 `func TestGetIndexMVCC(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestGetIndexMVCC 对应 Go 函数 `func TestGetIndexMVCC(t *testing.T) {`。
pub fn test_get_index_mvcc() {
    let suite = create_basic_http_handler_test_suite();
    for (path, expected_b_count) in [
        ("/mvcc/index/tidb/test/idx1/1?a=1&b=2", 1),
        ("/mvcc/index/tidb/test/idx2/3?a=3&b", 0),
        ("/mvcc/index/tidb/test/idx1/4?a=4&b=", 1),
    ] {
        let response = suite
            .client
            .fetch_status(path)
            .expect("index MVCC request must reach the handler");
        assert_eq!(response.status, 200, "{path}");
        let body = response.text().unwrap();
        assert!(body.contains("mvcc-index"), "{path}");
        assert!(
            body.contains(&format!("b_count={expected_b_count}")),
            "{path}"
        );
    }
    let response = suite
        .client
        .fetch_status("/mvcc/index/tidb/test/idx1/1?a=1")
        .expect("missing index column value must reach the handler");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("missing index column value")
    );
    let response = suite
        .client
        .fetch_status("/mvcc/index/tidb/test/idx1/5?a=5&b=1")
        .expect("missing MVCC entry must reach the handler");
    assert_eq!(response.status, 200);
    assert!(response.text().unwrap().contains("mvcc-index-not-found"));

    let response = suite
        .client
        .fetch_status("/mvcc/index/tidb/pt(p2)/idx/666?a=666&b=def")
        .expect("partition-qualified index MVCC must resolve the logical table");
    assert_eq!(response.status, 200);
    assert!(response.text().unwrap().contains("mvcc-index:idx"));

    // Go 原始签名: func TestGetIndexMVCC(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)

    // 保留 Go 注释: // tests for normal index key
    // HTTP 请求: resp, err := ts.FetchStatus("/mvcc/index/tidb/test/idx1/1?a=1&b=2")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, true)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx2/1?a=1&b=2")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, true)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 保留 Go 注释: // tests for index key which includes null
    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx1/3?a=3&b")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, true)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx2/3?a=3&b")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, true)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 保留 Go 注释: // tests for index key which includes empty string
    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx1/4?a=4&b=")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, true)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx2/3?a=4&b=")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, true)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/t/idx?a=1.1&b=111&c=1")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, true)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 保留 Go 注释: // tests for wrong key
    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx1/5?a=5&b=1")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, false)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx2/5?a=5&b=1")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, false)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 保留 Go 注释: // tests for missing column value
    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx1/1?a=1")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var data1 helper.MvccKV
    // 状态准备: err = decoder.Decode(&data1)
    // 错误断言: require.Error(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/test/idx2/1?a=1")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 迁移语句: var data2 helper.MvccKV
    // 状态准备: err = decoder.Decode(&data2)
    // 错误断言: require.Error(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/mvcc/index/tidb/pt(p2)/idx/666?a=666&b=def")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: decodeKeyMvcc(resp.Body, t, true)
    // 错误处理: require.NoError(t, resp.Body.Close())
}

#[test]
// TestDeleteKeyHandler 对应 Go 函数 `func TestDeleteKeyHandler(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestDeleteKeyHandler 对应 Go 函数 `func TestDeleteKeyHandler(t *testing.T) {`。
pub fn test_delete_key_handler() {
    let suite = create_basic_http_handler_test_suite();
    let get = suite
        .client
        .fetch_status("/test/delete/rowkey/tidb/delete_row?handle=1")
        .expect("GET delete-key request must reach the handler");
    assert_eq!(get.status, 400);
    assert!(get.text().unwrap().contains("only support POST"));
    let post = suite
        .client
        .post_status(
            "/test/delete/rowkey/tidb/delete_row?handle=1",
            "application/x-www-form-urlencoded",
            &[],
        )
        .expect("POST delete-key request must reach the runtime");
    assert_eq!(post.status, 200);
    assert_eq!(post.text().unwrap(), r#"{"key":"DEAD"}"#);

    // Go 原始签名: func TestDeleteKeyHandler(t *testing.T) {
    // 保留 Go 注释: // on CI env, the store_cache might mark the uni-store as unreachable, and
    // 保留 Go 注释: // cause the test to fail, so we enable the failpoint to make it always reachable.
    // failpoint 注入: testfailpoint.Enable(t, "tikvclient/injectLiveness", `return("reachable")`)
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // failpoint 注入: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/server/enableTestAPI", "return")
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)

    // 上下文: ctx := context.Background()
    // 状态准备: store := ts.store

    // 子测试: t.Run("index", func(t *testing.T) {
    // 外部依赖/testkit: tk := testkit.NewTestKit(t, ts.store)
    // 迁移语句: tk.MustExec("use tidb")
    // 迁移语句: tk.MustExec("drop table if exists delete_idx")
    // 迁移语句: tk.MustExec("create table delete_idx (a int primary key, b int, key idx_ab(a, b))")
    // 迁移语句: tk.MustExec("insert into delete_idx values (1, 2)")

    // 状态准备: tbl, err := ts.domain.InfoSchema().TableByName(ctx, ast.NewCIStr("tidb"), ast.NewCIStr("delete_idx"))
    // 错误处理: require.NoError(t, err)

    // 迁移语句: var idx table.Index
    // 循环遍历: for _, v := range tbl.Indices() {
    // 关键分支: if strings.EqualFold(v.Meta().Name.String(), "idx_ab") {
    // 状态准备: idx = v
    // 迁移语句: break
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 断言: require.NotNil(t, idx)

    // 时间相关: sc := stmtctx.NewStmtCtxWithTimeZone(time.UTC)
    // 状态准备: idxRow := []types.Datum{
    // 迁移语句: types.NewIntDatum(1),
    // 迁移语句: types.NewIntDatum(2),
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: handle := kv.IntHandle(1)
    // 状态准备: encodedKey, _, err := idx.GenIndexKey(sc.ErrCtx(), sc.TimeZone(), idxRow, handle, nil)
    // 错误处理: require.NoError(t, err)

    // 上下文: err = kv.RunInNewTxn(ctx, store, true, func(_ context.Context, txn kv.Transaction) error {
    // 迁移语句: txn.SetOption(kv.ResourceGroupTagger, ddlutil.GetInternalResourceGroupTaggerForTopSQL())
    // 状态准备: _, err := txn.Get(ctx, encodedKey)
    // 返回值: return err
    // 迁移语句: 结束上一层 Go 代码块。
    // 错误处理: require.NoError(t, err)

    // HTTP 请求: resp, err := ts.PostStatus("/test/delete/indexkey/tidb/delete_idx/idx_ab?handle=1&a=1&b=2", "application/x-www-form-urlencoded", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 上下文: err = kv.RunInNewTxn(ctx, store, true, func(_ context.Context, txn kv.Transaction) error {
    // 迁移语句: txn.SetOption(kv.ResourceGroupTagger, ddlutil.GetInternalResourceGroupTaggerForTopSQL())
    // 状态准备: _, err := txn.Get(ctx, encodedKey)
    // 返回值: return err
    // 迁移语句: 结束上一层 Go 代码块。
    // 断言: require.True(t, kv.ErrNotExist.Equal(err))

    // 状态准备: err = tk.ExecToErr("admin check index tidb.delete_idx idx_ab")
    // 错误断言: require.Error(t, err)
    // 错误断言: require.ErrorContains(t, err, "data inconsistency")
    // 迁移语句: 结束上一层 Go 代码块。

    // 子测试: t.Run("row", func(t *testing.T) {
    // 外部依赖/testkit: tk := testkit.NewTestKit(t, ts.store)
    // 迁移语句: tk.MustExec("use tidb")
    // 迁移语句: tk.MustExec("drop table if exists delete_row")
    // 迁移语句: tk.MustExec("create table delete_row (a int primary key, b int, key idx_b(b))")
    // 迁移语句: tk.MustExec("insert into delete_row values (1, 2)")

    // 状态准备: tbl, err := ts.domain.InfoSchema().TableByName(ctx, ast.NewCIStr("tidb"), ast.NewCIStr("delete_row"))
    // 错误处理: require.NoError(t, err)

    // 状态准备: handle := kv.IntHandle(1)
    // 状态准备: encodedKey := tablecodec.EncodeRecordKey(tbl.RecordPrefix(), handle)
    // 上下文: err = kv.RunInNewTxn(ctx, store, true, func(_ context.Context, txn kv.Transaction) error {
    // 迁移语句: txn.SetOption(kv.ResourceGroupTagger, ddlutil.GetInternalResourceGroupTaggerForTopSQL())
    // 状态准备: _, err := txn.Get(ctx, encodedKey)
    // 返回值: return err
    // 迁移语句: 结束上一层 Go 代码块。
    // 错误处理: require.NoError(t, err)

    // HTTP 请求: resp, err := ts.PostStatus("/test/delete/rowkey/tidb/delete_row?handle=1", "application/x-www-form-urlencoded", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 上下文: err = kv.RunInNewTxn(ctx, store, true, func(_ context.Context, txn kv.Transaction) error {
    // 迁移语句: txn.SetOption(kv.ResourceGroupTagger, ddlutil.GetInternalResourceGroupTaggerForTopSQL())
    // 状态准备: _, err := txn.Get(ctx, encodedKey)
    // 返回值: return err
    // 迁移语句: 结束上一层 Go 代码块。
    // 断言: require.True(t, kv.ErrNotExist.Equal(err))

    // 状态准备: err = tk.ExecToErr("admin check table tidb.delete_row")
    // 错误断言: require.Error(t, err)
    // 错误断言: require.ErrorContains(t, err, "data inconsistency")
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestGetSettings 对应 Go 函数 `func TestGetSettings(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestGetSettings 对应 Go 函数 `func TestGetSettings(t *testing.T) {`。
pub fn test_get_settings() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/settings")
        .expect("GET /settings must reach the status server");
    assert_eq!(response.status, 200);
    let body = response.text().expect("settings must be UTF-8 JSON");
    let config = astersql_config::get_global_config();
    assert!(body.starts_with('{') && body.ends_with('}'));
    assert!(body.contains(&format!("\"host\":\"{}\"", config.host)));
    assert!(body.contains(&format!("\"port\":{}", config.port)));

    // Go 原始签名: func TestGetSettings(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/settings")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var settings *config.Config
    // 状态准备: err = decoder.Decode(&settings)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: var configBytes []byte
    // JSON 编解码: configBytes, err = json.Marshal(config.GetGlobalConfig())
    // 错误处理: require.NoError(t, err)
    // 迁移语句: var settingBytes []byte
    // JSON 编解码: settingBytes, err = json.Marshal(settings)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, configBytes, settingBytes)
}

#[test]
// TestGetSchema 对应 Go 函数 `func TestGetSchema(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestGetSchema 对应 Go 函数 `func TestGetSchema(t *testing.T) {`。
pub fn test_get_schema() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/db-table/42")
        .expect("db-table route must dispatch through DBTableHandler");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"name\":\"test\""));
    assert!(body.contains("\"name\":\"handler_table\""));
    assert!(body.contains("\"schema_version\":7"));

    let response = suite
        .client
        .fetch_status("/db-table/not-a-table")
        .expect("unknown db-table id must receive an error response");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("table id not exists"));

    let suite = create_basic_http_handler_test_suite();
    let catalog = suite
        .client
        .fetch_status("/schema")
        .expect("schema catalog must reach SchemaHandler");
    assert_eq!(catalog.status, 200);
    assert_eq!(catalog.text().unwrap(), r#"{"data":"schema-catalog"}"#);
    let database = suite
        .client
        .fetch_status("/schema/tidb")
        .expect("schema database route must reach SchemaHandler");
    assert_eq!(database.status, 200);
    assert_eq!(database.text().unwrap(), r#"{"data":"schema-db:tidb"}"#);
    let named_table = suite
        .client
        .fetch_status("/schema/tidb/t")
        .expect("schema named-table route must reach SchemaHandler");
    assert_eq!(named_table.status, 200);
    assert_eq!(
        named_table.text().unwrap(),
        r#"{"data":"schema-table-name:tidb/t"}"#
    );
    let missing_database = suite
        .client
        .fetch_status("/schema/abc")
        .expect("unknown schema database must produce an HTTP error");
    assert_eq!(missing_database.status, 400);
    assert!(
        missing_database
            .text()
            .unwrap()
            .contains("database not exists")
    );
    let missing_table = suite
        .client
        .fetch_status("/schema/tidb/abc")
        .expect("unknown schema table must produce an HTTP error");
    assert_eq!(missing_table.status, 400);
    assert!(missing_table.text().unwrap().contains("table not exists"));
    let table = suite
        .client
        .fetch_status("/schema?table_id=42")
        .expect("schema table lookup must preserve its query parameter");
    assert_eq!(table.status, 200);
    assert_eq!(table.text().unwrap(), r#"{"data":"schema-table:42"}"#);

    // Go 原始签名: func TestGetSchema(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/schema")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var dbs []*model.DBInfo
    // 状态准备: err = decoder.Decode(&dbs)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 状态准备: expects := []string{"information_schema", "metrics_schema", "mysql", "performance_schema", "sys", "test", "tidb"}
    // 状态准备: names := make([]string, len(dbs))
    // 循环遍历: for i, v := range dbs {
    // 状态准备: names[i] = v.Name.L
    // 迁移语句: 结束上一层 Go 代码块。
    // 排序/结果归一: sort.Strings(names)
    // 断言: require.Equal(t, expects, names)
    // 外部依赖/testkit: store := testkit.CreateMockStore(t)

    // 外部依赖/testkit: tk := testkit.NewTestKit(t, store)
    // 状态准备: userTbl := external.GetTableByName(t, tk, "mysql", "user")
    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/schema?table_id=%d", userTbl.Meta().ID))
    // 错误处理: require.NoError(t, err)
    // 迁移语句: var ti *model.TableInfo
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&ti)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "user", ti.Name.L)

    // HTTP 请求: resp, err = ts.FetchStatus("/schema?table_id=a")
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/schema?table_id=1")
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/schema?table_id=-1")
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/schema/tidb")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: var lt []*model.TableInfo
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&lt)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: require.Greater(t, len(lt), 2)

    // HTTP 请求: resp, err = ts.FetchStatus("/schema/tidb?id_name_only=true")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: var lti []*model.TableNameInfo
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&lti)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: require.Greater(t, len(lti), 2)

    // HTTP 请求: resp, err = ts.FetchStatus("/schema/abc")
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/schema/tidb/test")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&ti)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "test", ti.Name.L)

    // HTTP 请求: resp, err = ts.FetchStatus("/schema/tidb/abc")
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/db-table/%d", userTbl.Meta().ID))
    // 错误处理: require.NoError(t, err)
    // 迁移语句: var dbtbl *tikvhandler.DBTableInfo
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&dbtbl)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "user", dbtbl.TableInfo.Name.L)
    // 断言: require.Equal(t, "mysql", dbtbl.DBInfo.Name.L)
    // 状态准备: se, err := session.CreateSession(ts.store)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, domain.GetDomain(se.(sessionctx.Context)).InfoSchema().SchemaMetaVersion(), dbtbl.SchemaVersion)

    // 外部依赖/数据库: db, err := sql.Open("mysql", ts.GetDSN())
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: dbt := testkit.NewDBTestKit(t, db)

    // 迁移语句: dbt.MustExec("create database if not exists test;")
    // 迁移语句: dbt.MustExec("use test;")
    // 迁移语句: dbt.MustExec(` create table t1 (id int KEY)
    // 迁移语句: partition by range (id) (
    // 迁移语句: PARTITION p0 VALUES LESS THAN (3),
    // 迁移语句: PARTITION p1 VALUES LESS THAN (5),
    // 迁移语句: PARTITION p2 VALUES LESS THAN (7),
    // 迁移语句: PARTITION p3 VALUES LESS THAN (9))`)
    // 迁移语句: dbt.MustExec(`CREATE TABLE t2 (c INT)`)

    // 迁移语句: var simpleTableInfos []*model.TableNameInfo
    // HTTP 请求: resp, err = ts.FetchStatus("/schema/test?id_name_only=true")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&simpleTableInfos)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 排序/结果归一: slices.SortFunc(simpleTableInfos, func(i, j *model.TableNameInfo) int {
    // 返回值: return strings.Compare(i.Name.L, j.Name.L)
    // 迁移语句: 结束上一层 Go 代码块。
    // 断言: require.Len(t, simpleTableInfos, 2)
    // 断言: require.Equal(t, "t1", simpleTableInfos[0].Name.L)
    // 断言: require.Equal(t, "t2", simpleTableInfos[1].Name.L)
    // 状态准备: id1 := simpleTableInfos[0].ID
    // 状态准备: id2 := simpleTableInfos[1].ID
    // 迁移语句: require.NotZero(t, id1)
    // 迁移语句: require.NotZero(t, id2)

    // 保留 Go 注释: // check table_ids=... happy path
    // 参数解析: ids := strings.Join([]string{strconv.FormatInt(id1, 10), strconv.FormatInt(id2, 10)}, ",")
    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/schema?table_ids=%s", ids))
    // 错误处理: require.NoError(t, err)
    // 迁移语句: var tis map[int]*model.TableInfo
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&tis)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 2, len(tis))
    // 断言: require.Equal(t, "t1", tis[int(id1)].Name.L)
    // 断言: require.Equal(t, "t2", tis[int(id2)].Name.L)

    // 保留 Go 注释: // check table_ids=... partial missing
    // 状态准备: ids = ids + ",99999"
    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/schema?table_ids=%s", ids))
    // 错误处理: require.NoError(t, err)
    // 迁移语句: clear(tis)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&tis)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 2, len(tis))
    // 断言: require.Equal(t, "t1", tis[int(id1)].Name.L)
    // 断言: require.Equal(t, "t2", tis[int(id2)].Name.L)

    // 保留 Go 注释: // check wrong format in table_ids
    // 状态准备: ids = ids + ",abc"
    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/schema?table_ids=%s", ids))
    // 错误处理: require.NoError(t, err)
    // 迁移语句: clear(tis)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&tis)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 2, len(tis))
    // 断言: require.Equal(t, "t1", tis[int(id1)].Name.L)
    // 断言: require.Equal(t, "t2", tis[int(id2)].Name.L)

    // HTTP 请求: resp, err = ts.FetchStatus("/schema/test/t1")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&ti)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "t1", ti.Name.L)

    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/db-table/%v", ti.GetPartitionInfo().Definitions[0].ID))
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&dbtbl)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "t1", dbtbl.TableInfo.Name.L)
    // 断言: require.Equal(t, "test", dbtbl.DBInfo.Name.L)
    // 断言: require.Equal(t, ti, dbtbl.TableInfo)

    // HTTP 请求: resp, err = ts.FetchStatus(fmt.Sprintf("/schema?table_id=%v", ti.GetPartitionInfo().Definitions[0].ID))
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&ti)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "t1", ti.Name.L)
    // 断言: require.Equal(t, ti, ti)
}

#[test]
// TestAllHistory 对应 Go 函数 `func TestAllHistory(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestAllHistory 对应 Go 函数 `func TestAllHistory(t *testing.T) {`。
pub fn test_all_history() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/ddl/history")
        .expect("DDL history must be served through the status listener");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#"[{"id":3},{"id":2}]"#);

    let response = suite
        .client
        .fetch_status("/ddl/history?start_job_id=41&limit=3")
        .expect("DDL history pagination must reach the runtime");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#"[{"id":41}]"#);

    let response = suite
        .client
        .fetch_status("/ddl/history?limit=-1")
        .expect("invalid DDL history parameters must produce an HTTP response");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("ddl history limit must be greater than 0")
    );

    // Go 原始签名: func TestAllHistory(t *testing.T) {
    // 保留 Go 注释: // TestGetSchema will set schema lease to -1, while this test needs a valid
    // 保留 Go 注释: // schema lease.
    // 时间相关: vardef.SetStatsLease(time.Second)
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/ddl/history/?limit=3")
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // HTTP 请求: resp, err = ts.FetchStatus("/ddl/history/?limit=-1")
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/ddl/history")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)

    // 迁移语句: var jobs []*model.Job
    // 状态准备: s, _ := session.CreateSession(ts.server.NewTikvHandlerTool().Store.(kv.Storage))
    // 资源收尾: defer s.Close()
    // 状态准备: store := domain.GetDomain(s.(sessionctx.Context)).Store()
    // 状态准备: txn, _ := store.Begin()
    // 状态准备: txnMeta := meta.NewMutator(txn)
    // 状态准备: data, err := ddl.GetAllHistoryDDLJobs(txnMeta)
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = decoder.Decode(&jobs)
    // 断言: require.True(t, len(jobs) < ddl.DefNumGetDDLHistoryJobs)
    // 保留 Go 注释: // sort job.
    // 排序/结果归一: slices.SortFunc(jobs, func(i, j *model.Job) int {
    // 返回值: return cmp.Compare(i.ID, j.ID)
    // 迁移语句: 结束上一层 Go 代码块。

    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, len(data), len(jobs))
    // 循环遍历: for i := range data {
    // 保留 Go 注释: // For the jobs that have arguments(job.Args) for GC delete range,
    // 保留 Go 注释: // the RawArgs should be the same after filtering the spaces.
    // 状态准备: data[i].RawArgs = filterSpaces(data[i].RawArgs)
    // 状态准备: jobs[i].RawArgs = filterSpaces(jobs[i].RawArgs)
    // 断言: require.Equal(t, data[i], jobs[i], i)
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // Cover the start_job_id parameter.
    // HTTP 请求: resp, err = ts.FetchStatus("/ddl/history?start_job_id=41")
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.FetchStatus("/ddl/history?start_job_id=41&limit=3")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&jobs)
    // 错误处理: require.NoError(t, err)

    // 保留 Go 注释: // The result is in descending order
    // 状态准备: lastID := int64(42)
    // 循环遍历: for _, job := range jobs {
    // 迁移语句: require.Less(t, job.ID, lastID)
    // 状态准备: lastID = job.ID
    // 迁移语句: 结束上一层 Go 代码块。
    // 错误处理: require.NoError(t, resp.Body.Close())
}

#[test]
// TestDDLCheckHandler 对应 Go 函数 `func TestDDLCheckHandler(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestDDLCheckHandler 对应 Go 函数 `func TestDDLCheckHandler(t *testing.T) {`。
pub fn test_ddl_check_handler() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/ddl/check/tidb/test/idx1")
        .expect("DDL check GET must reach the handler");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("only support POST"));

    let response = suite
        .client
        .post_status(
            "/ddl/check/tidb/test/idx_not_exist",
            "application/json",
            &[],
        )
        .expect("DDL check failure must be encoded as a result payload");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"result\":\"failed\""));
    assert!(body.contains("\"error\":\"index not found\""));

    let response = suite
        .client
        .post_status("/ddl/check/tidb/test/idx1", "application/json", &[])
        .expect("DDL check success must reach the runtime");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"db\":\"tidb\""));
    assert!(body.contains("\"table\":\"test\""));
    assert!(body.contains("\"index\":\"idx1\""));
    assert!(body.contains("\"check_sql\":\"admin check index `tidb`.`test` `idx1`\""));
    assert!(body.contains("\"result\":\"success\""));

    // Go 原始签名: func TestDDLCheckHandler(t *testing.T) {
    // 关键分支: if !kerneltype.IsNextGen() {
    // 迁移语句: t.Skip("DDL check handler is only available for next-gen kernel")
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)

    // HTTP 请求: resp, err := ts.FetchStatus("/ddl/check/tidb/test/idx1")
    // 错误处理: require.NoError(t, err)
    // IO 读取写入: body, err := io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)
    // 断言: require.Contains(t, string(body), "only support POST")

    // HTTP 请求: resp, err = ts.PostStatus("/ddl/check/tidb/test/idx_not_exist", "application/json", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var result map[string]any
    // 状态准备: err = decoder.Decode(&result)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "failed", result["result"])
    // 断言: require.NotEmpty(t, result["error"])

    // HTTP 请求: resp, err = ts.PostStatus("/ddl/check/tidb/test/idx1", "application/json", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&result)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "tidb", result["db"])
    // 断言: require.Equal(t, "test", result["table"])
    // 断言: require.Equal(t, "idx1", result["index"])
    // 断言: require.Equal(t, "admin check index `tidb`.`test` `idx1`", result["check_sql"])
    // 断言: require.Equal(t, "success", result["result"])
}

// filterSpaces 对应 Go 函数 `func filterSpaces(bs []byte) []byte {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// filterSpaces 对应 Go 函数 `func filterSpaces(bs []byte) []byte {`。
pub fn filter_spaces(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .copied()
        .filter(|byte| !matches!(byte, b'\n' | b'\r' | b' '))
        .collect()
}

#[test]
fn filter_spaces_preserves_go_whitespace_and_empty_contract() {
    assert_eq!(filter_spaces(b""), Vec::<u8>::new());
    assert_eq!(filter_spaces(b" {\r\n \"a\": 1 } "), br#"{"a":1}"#);
}

#[test]
// TestPprof 对应 Go 函数 `func TestPprof(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestPprof 对应 Go 函数 `func TestPprof(t *testing.T) {`。
pub fn test_pprof() {
    let suite = create_basic_http_handler_test_suite();
    let mut last_error = None;
    for _ in 0..100 {
        match suite.client.fetch_status("/debug/pprof/heap") {
            Ok(response) => {
                assert_eq!(response.status, 200);
                assert!(
                    !response.body.is_empty(),
                    "the heap diagnostic must contain process information"
                );
                return;
            }
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    panic!("failed to get a pprof-equivalent heap diagnostic: {last_error:?}");

    // Go 原始签名: func TestPprof(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // 状态准备: retryTime := 100
    // 循环遍历: for range retryTime {
    // HTTP 请求: resp, err := ts.FetchStatus("/debug/pprof/heap")
    // 关键分支: if err == nil {
    // IO 读取写入: _, err = io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 返回值: return
    // 迁移语句: 结束上一层 Go 代码块。
    // 时间相关: time.Sleep(time.Millisecond * 10)
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: log.Fatal("failed to get profile for %d retries in every 10 ms", zap.Int("retryTime", retryTime))
}

#[test]
// TestHotRegionInfo 对应 Go 函数 `func TestHotRegionInfo(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestHotRegionInfo 对应 Go 函数 `func TestHotRegionInfo(t *testing.T) {`。
pub fn test_hot_region_info() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/regions/hot")
        .expect("hot-region route must reach its RegionHandler");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("hot region metrics are unavailable")
    );

    // Go 原始签名: func TestHotRegionInfo(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/regions/hot")
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)
}

#[test]
// TestDebugZip 对应 Go 函数 `func TestDebugZip(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestDebugZip 对应 Go 函数 `func TestDebugZip(t *testing.T) {`。
pub fn test_debug_zip() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/debug/zip?seconds=1")
        .expect("debug zip request must reach the status server");
    assert_eq!(response.status, 200);
    let body = response.text().expect("debug snapshot must be UTF-8");
    assert!(body.contains("AsterSQL debug snapshot"));
    assert!(body.contains("requested_seconds=1"));

    // Go 原始签名: func TestDebugZip(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/debug/zip?seconds=1")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: b, err := httputil.DumpResponse(resp, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: require.Greater(t, len(b), 0)
    // 错误处理: require.NoError(t, resp.Body.Close())
}

#[test]
// TestCheckCN 对应 Go 函数 `func TestCheckCN(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestCheckCN 对应 Go 函数 `func TestCheckCN(t *testing.T) {`。
pub fn test_check_cn() {
    let certificate_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../tests/cncheckcert");
    let config = TlsConfig {
        ca_path: Some(
            certificate_dir
                .join("ca-tidb-test-1.crt")
                .to_string_lossy()
                .into_owned(),
        ),
        verify_common_names: vec!["tidb-client-1 ".into(), "tidb-client-2".into()],
        ..TlsConfig::default()
    };
    let client_one = std::fs::read(certificate_dir.join("client-cert-1.pem"))
        .expect("CN test client certificate must be available");
    let client_two = std::fs::read(certificate_dir.join("client-cert-2.pem"))
        .expect("CN test client certificate must be available");
    let server = std::fs::read(certificate_dir.join("server-cert.pem"))
        .expect("CN test server certificate must be available");
    assert!(config.verify_peer_common_name(&client_one).is_ok());
    assert!(config.verify_peer_common_name(&client_two).is_ok());
    let error = config
        .verify_peer_common_name(&server)
        .expect_err("unlisted CN must be rejected");
    assert!(error.contains("not found"));

    // Go 原始签名: func TestCheckCN(t *testing.T) {
    // 状态准备: cfg := &config.Config{Security: config.Security{ClusterVerifyCN: []string{"a ", "b", "c"}}}
    // 状态准备: s := server2.NewTestServer(cfg)
    // 状态准备: tlsConfig := &tls.Config{}
    // 迁移语句: s.SetCNChecker(tlsConfig)
    // 断言: require.NotNil(t, tlsConfig.VerifyPeerCertificate)
    // 状态准备: err := tlsConfig.VerifyPeerCertificate(nil, [][]*x509.Certificate{{{Subject: pkix.Name{CommonName: "a"}}}})
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = tlsConfig.VerifyPeerCertificate(nil, [][]*x509.Certificate{{{Subject: pkix.Name{CommonName: "b"}}}})
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = tlsConfig.VerifyPeerCertificate(nil, [][]*x509.Certificate{{{Subject: pkix.Name{CommonName: "d"}}}})
    // 错误断言: require.Error(t, err)
}

#[test]
// TestDDLHookHandler 对应 Go 函数 `func TestDDLHookHandler(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestDDLHookHandler 对应 Go 函数 `func TestDDLHookHandler(t *testing.T) {`。
pub fn test_ddl_hook_handler() {
    let suite = create_basic_http_handler_test_suite();
    let get = suite
        .client
        .fetch_status("/test/ddl/hook")
        .expect("GET DDL hook request must reach the handler");
    assert_eq!(get.status, 400);
    assert!(get.text().unwrap().contains("only support POST"));

    for (form, expected) in [
        (b"ddl_hook=ctc_hook".as_slice(), true),
        (b"ddl_hook=default_hook", false),
    ] {
        let response = suite
            .client
            .post_status("/test/ddl/hook", "application/x-www-form-urlencoded", form)
            .expect("POST DDL hook request must reach the handler");
        assert_eq!(response.status, 200);
        assert_eq!(response.text().unwrap(), r#""success!""#);
        assert_eq!(
            super::http_handler_serial_test::CTC_DDL_HOOK_ENABLED
                .load(std::sync::atomic::Ordering::Acquire),
            expected
        );
    }

    // Go 原始签名: func TestDDLHookHandler(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()

    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/test/ddl/hook")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // HTTP 请求: resp, err = ts.PostStatus("/test/ddl/hook", "application/x-www-form-urlencoded", bytes.NewBuffer([]byte(`ddl_hook=ctc_hook`)))
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // IO 读取写入: body, err := io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "\"success!\"", string(body))
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)

    // HTTP 请求: resp, err = ts.PostStatus("/test/ddl/hook", "application/x-www-form-urlencoded", bytes.NewBuffer([]byte(`ddl_hook=default_hook`)))
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // IO 读取写入: body, err = io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "\"success!\"", string(body))
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
}

#[test]
// TestWriteDBTablesData 对应 Go 函数 `func TestWriteDBTablesData(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestWriteDBTablesData 对应 Go 函数 `func TestWriteDBTablesData(t *testing.T) {`。
pub fn test_write_db_tables_data() {
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;
    use astersql_server_handler_tikvhandler::{TableInfo, WriteDBTablesData};

    let mut writer = ResponseWriter::default();
    WriteDBTablesData(&mut writer, Vec::<TableInfo>::new());
    assert_eq!(writer.status, Some(200));
    assert_eq!(writer.data.len(), 1);
    assert!(writer.data[0].downcast_ref::<Vec<TableInfo>>().is_some());

    let mut writer = ResponseWriter::default();
    let tables = vec![
        TableInfo {
            id: 1,
            name: "signed".into(),
            indices: Vec::new(),
            partitions: Vec::new(),
            is_common_handle: false,
            tiflash_replica: None,
            tiflash_replica_infos: Vec::new(),
        },
        TableInfo {
            id: 2,
            name: "unsigned".into(),
            indices: Vec::new(),
            partitions: Vec::new(),
            is_common_handle: false,
            tiflash_replica: None,
            tiflash_replica_infos: Vec::new(),
        },
    ];
    WriteDBTablesData(&mut writer, tables);
    let written = writer.data[0]
        .downcast_ref::<Vec<TableInfo>>()
        .expect("table response must remain one JSON array value");
    assert_eq!(written.len(), 2);
    assert_eq!(written[0].id, 1);
    assert_eq!(written[0].name, "signed");
    assert_eq!(written[1].id, 2);
    assert_eq!(written[1].name, "unsigned");

    // Go 原始签名: func TestWriteDBTablesData(t *testing.T) {
    // 保留 Go 注释: // No table in a schema.
    // 状态准备: info := infoschema.MockInfoSchema([]*model.TableInfo{})
    // 状态准备: rc := httptest.NewRecorder()
    // 上下文: tbs, err := info.SchemaTableInfos(context.Background(), ast.NewCIStr("test"))
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 0, len(tbs))
    // 迁移语句: tikvhandler.WriteDBTablesData(rc, tbs)
    // 迁移语句: var ti []*model.TableInfo
    // JSON 编解码: decoder := json.NewDecoder(rc.Body)
    // 状态准备: err = decoder.Decode(&ti)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 0, len(ti))

    // 保留 Go 注释: // One table in a schema.
    // 状态准备: info = infoschema.MockInfoSchema([]*model.TableInfo{coretestsdk.MockSignedTable()})
    // 状态准备: rc = httptest.NewRecorder()
    // 上下文: tbs, err = info.SchemaTableInfos(context.Background(), ast.NewCIStr("test"))
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 1, len(tbs))
    // 迁移语句: tikvhandler.WriteDBTablesData(rc, tbs)
    // JSON 编解码: decoder = json.NewDecoder(rc.Body)
    // 状态准备: err = decoder.Decode(&ti)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 1, len(ti))
    // 断言: require.Equal(t, ti[0].ID, tbs[0].ID)
    // 断言: require.Equal(t, ti[0].Name.String(), tbs[0].Name.String())

    // 保留 Go 注释: // Two tables in a schema.
    // 状态准备: info = infoschema.MockInfoSchema([]*model.TableInfo{coretestsdk.MockSignedTable(), coretestsdk.MockUnsignedTable()})
    // 状态准备: rc = httptest.NewRecorder()
    // 上下文: tbs, err = info.SchemaTableInfos(context.Background(), ast.NewCIStr("test"))
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 2, len(tbs))
    // 迁移语句: tikvhandler.WriteDBTablesData(rc, tbs)
    // JSON 编解码: decoder = json.NewDecoder(rc.Body)
    // 状态准备: err = decoder.Decode(&ti)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 2, len(ti))
    // 断言: require.Equal(t, ti[0].ID, tbs[0].ID)
    // 断言: require.Equal(t, ti[1].ID, tbs[1].ID)
    // 断言: require.Equal(t, ti[0].Name.String(), tbs[0].Name.String())
    // 断言: require.Equal(t, ti[1].Name.String(), tbs[1].Name.String())
}

#[test]
// TestSetLabels 对应 Go 函数 `func TestSetLabels(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestSetLabels 对应 Go 函数 `func TestSetLabels(t *testing.T) {`。
pub fn test_set_labels() {
    let _guard = labels_test_lock().lock().expect("labels test lock");
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| config.labels.clear());
    let suite = create_basic_http_handler_test_suite();
    for (payload, expected_zone, expected_test) in [
        (r#"{"zone":"us-west-1","test":"123"}"#, "us-west-1", "123"),
        (r#"{"zone":"bj-1"}"#, "bj-1", "123"),
    ] {
        let response = suite
            .client
            .post_status("/labels", "application/json", payload.as_bytes())
            .expect("label update must reach the status handler");
        assert_eq!(response.status, 200);
        assert_eq!(response.text().unwrap(), r#"{"message":"success"}"#);
        let labels = &astersql_config::get_global_config().labels;
        assert_eq!(labels.get("zone"), Some(&expected_zone.to_owned()));
        assert_eq!(labels.get("test"), Some(&expected_test.to_owned()));
    }
    restore();

    // Go 原始签名: func TestSetLabels(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()

    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // 状态准备: testUpdateLabels := func(labels, expected map[string]string) {
    // 状态准备: buffer := bytes.NewBuffer([]byte{})
    // JSON 编解码: require.Nil(t, json.NewEncoder(buffer).Encode(labels))
    // HTTP 请求: resp, err := ts.PostStatus("/labels", "application/json", buffer)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: newLabels := config.GetGlobalConfig().Labels
    // 断言: require.Equal(t, newLabels, expected)
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: labels := map[string]string{
    // 迁移语句: "zone": "us-west-1",
    // 迁移语句: "test": "123",
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: testUpdateLabels(labels, labels)

    // 状态准备: updated := map[string]string{
    // 迁移语句: "zone": "bj-1",
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: labels["zone"] = "bj-1"
    // 迁移语句: testUpdateLabels(updated, labels)

    // 保留 Go 注释: // reset the global variable
    // 迁移语句: config.UpdateGlobal(func(conf *config.Config) {
    // 状态准备: conf.Labels = map[string]string{}
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestSetLabelsWithEtcd 对应 Go 函数 `func TestSetLabelsWithEtcd(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestSetLabelsWithEtcd 对应 Go 函数 `func TestSetLabelsWithEtcd(t *testing.T) {`。
pub fn test_set_labels_with_etcd() {
    use astersql_domain_serverinfo::{
        Context as ServerInfoContext, MemoryEtcdClient, NewSyncer, NoopMinStartTSReporter,
    };

    let _guard = labels_test_lock().lock().expect("labels test lock");
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| config.labels.clear());
    let etcd = Arc::new(MemoryEtcdClient::default());
    let mut syncer = NewSyncer(
        "handler-labels".into(),
        Arc::new(|| 1),
        Some(etcd),
        Arc::new(NoopMinStartTSReporter),
    );
    syncer
        .NewSessionAndStoreServerInfo(ServerInfoContext::Background())
        .expect("in-memory etcd server-info session must start");
    let syncer = Arc::new(Mutex::new(syncer));
    let _syncer_guard =
        super::http_handler_serial_test::install_label_server_info_syncer(Arc::clone(&syncer));
    let suite = create_basic_http_handler_test_suite();
    let running = Arc::new(AtomicBool::new(true));
    let topology_running = Arc::clone(&running);
    let topology_server = Arc::clone(&suite.server);
    let topology = std::thread::spawn(move || {
        while topology_running.load(Ordering::Acquire) {
            assert!(topology_server.listener_addr().is_some());
            assert!(topology_server.status_listener_addr().is_some());
            assert!(topology_server.domain().is_some());
            let _ = astersql_config::get_global_config().labels.len();
        }
    });

    for index in 0..100 {
        let payload = format!(r#"{{"zone":"topology-{index}","rack":"r-{index}"}}"#);
        let response = suite
            .client
            .post_status("/labels", "application/json", payload.as_bytes())
            .expect("label update must stay live while server topology is read");
        assert_eq!(response.status, 200);
        assert_eq!(response.text().unwrap(), r#"{"message":"success"}"#);
    }
    running.store(false, Ordering::Release);
    topology
        .join()
        .expect("concurrent server topology reader must not panic");
    let labels = &astersql_config::get_global_config().labels;
    assert_eq!(labels.get("zone"), Some(&"topology-99".to_owned()));
    assert_eq!(labels.get("rack"), Some(&"r-99".to_owned()));
    let server_infos = syncer
        .lock()
        .expect("server-info syncer lock")
        .GetAllServerInfo(ServerInfoContext::Background())
        .expect("labels must be persisted to in-memory etcd server info");
    assert_eq!(server_infos.len(), 1);
    let server_info = server_infos
        .get("handler-labels")
        .expect("handler server-info must be present in etcd");
    assert_eq!(
        server_info.DynamicInfo.Labels.get("zone"),
        Some(&"topology-99".to_owned())
    );
    assert_eq!(
        server_info.DynamicInfo.Labels.get("rack"),
        Some(&"r-99".to_owned())
    );
    restore();

    // Go 原始签名: func TestSetLabelsWithEtcd(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 上下文: ctx, cancel := context.WithCancel(context.Background())
    // 资源收尾: defer cancel()

    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // 时间相关: time.Sleep(time.Second)
    // 迁移语句: integration.BeforeTestExternal(t)
    // 状态准备: cluster := integration.NewClusterV3(t, &integration.ClusterConfig{Size: 1})
    // 资源收尾: defer cluster.Terminate(t)
    // 状态准备: client := cluster.RandClient()
    // 迁移语句: infosync.SetEtcdClient(client)
    // 迁移语句: ts.domain.InfoSyncer().ServerInfoSyncer().Restart(ctx)

    // 状态准备: testUpdateLabels := func(labels, expected map[string]string) {
    // 状态准备: buffer := bytes.NewBuffer([]byte{})
    // JSON 编解码: require.Nil(t, json.NewEncoder(buffer).Encode(labels))
    // HTTP 请求: resp, err := ts.PostStatus("/labels", "application/json", buffer)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: newLabels := config.GetGlobalConfig().Labels
    // 断言: require.Equal(t, newLabels, expected)
    // 状态准备: servers, err := infosync.GetAllServerInfo(ctx)
    // 错误处理: require.NoError(t, err)
    // 循环遍历: for _, server := range servers {
    // 循环遍历: for k, expectV := range expected {
    // 状态准备: v, ok := server.Labels[k]
    // 断言: require.True(t, ok)
    // 断言: require.Equal(t, expectV, v)
    // 迁移语句: 结束上一层 Go 代码块。
    // 返回值: return
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: require.Fail(t, "no server found")
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: labels := map[string]string{
    // 迁移语句: "zone": "us-west-1",
    // 迁移语句: "test": "123",
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: testUpdateLabels(labels, labels)

    // 状态准备: updated := map[string]string{
    // 迁移语句: "zone": "bj-1",
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: labels["zone"] = "bj-1"
    // 迁移语句: testUpdateLabels(updated, labels)

    // 保留 Go 注释: // reset the global variable
    // 迁移语句: config.UpdateGlobal(func(conf *config.Config) {
    // 状态准备: conf.Labels = map[string]string{}
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestSetLabelsConcurrentWithGetLabel 对应 Go 函数 `func TestSetLabelsConcurrentWithGetLabel(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestSetLabelsConcurrentWithGetLabel 对应 Go 函数 `func TestSetLabelsConcurrentWithGetLabel(t *testing.T) {`。
pub fn test_set_labels_concurrent_with_get_label() {
    let _guard = labels_test_lock().lock().expect("labels test lock");
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| config.labels.clear());
    let suite = create_basic_http_handler_test_suite();
    let reading = Arc::new(AtomicBool::new(true));
    let reader_stop = Arc::clone(&reading);
    let reader = std::thread::spawn(move || {
        while reader_stop.load(Ordering::Acquire) {
            let _ = astersql_config::get_global_config().labels.len();
        }
    });

    for index in 0..20 {
        let payload = format!(r#"{{"zone":"z-{index}"}}"#);
        let response = suite
            .client
            .post_status("/labels", "application/json", payload.as_bytes())
            .expect("concurrent label update must reach the status handler");
        assert_eq!(response.status, 200);
        assert_eq!(response.text().unwrap(), r#"{"message":"success"}"#);
        assert_eq!(
            astersql_config::get_global_config().labels.get("zone"),
            Some(&format!("z-{index}"))
        );
    }
    reading.store(false, Ordering::Release);
    reader
        .join()
        .expect("concurrent config reader must not panic");
    restore();

    // Go 原始签名: func TestSetLabelsConcurrentWithGetLabel(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()

    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // 状态准备: testUpdateLabels := func() {
    // 状态准备: labels := map[string]string{}
    // 格式化参数: labels["zone"] = fmt.Sprintf("z-%v", rand.Intn(100000))
    // 状态准备: buffer := bytes.NewBuffer([]byte{})
    // JSON 编解码: require.Nil(t, json.NewEncoder(buffer).Encode(labels))
    // HTTP 请求: resp, err := ts.PostStatus("/labels", "application/json", buffer)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: newLabels := config.GetGlobalConfig().Labels
    // 断言: require.Equal(t, newLabels, labels)
    // 迁移语句: 结束上一层 Go 代码块。
    // 并发通道: done := make(chan struct{})
    // 并发/异步: go func() {
    // 循环遍历: for {
    // 迁移语句: select {
    // 分支项: case <-done:
    // 返回值: return
    // 分支项: default:
    // 迁移语句: config.GetGlobalConfig().GetTiKVConfig()
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: }()
    // 循环遍历: for range 100 {
    // 迁移语句: testUpdateLabels()
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: close(done)

    // 保留 Go 注释: // reset the global variable
    // 迁移语句: config.UpdateGlobal(func(conf *config.Config) {
    // 状态准备: conf.Labels = map[string]string{}
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestUpgrade 对应 Go 函数 `func TestUpgrade(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestUpgrade 对应 Go 函数 `func TestUpgrade(t *testing.T) {`。
pub fn test_upgrade() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/upgrade/start")
        .expect("upgrade GET must reach the handler");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("only support POST"));

    let response = suite
        .client
        .post_status("/upgrade/start", "application/x-www-form-urlencoded", &[])
        .expect("upgrade start must transition the cluster state");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#""success!""#);

    let response = suite
        .client
        .post_status("/upgrade/start", "application/x-www-form-urlencoded", &[])
        .expect("duplicate upgrade start must be observable");
    assert_eq!(response.status, 200);
    assert!(
        response
            .text()
            .unwrap()
            .contains("already in upgrading state")
    );

    let response = suite
        .client
        .post_status("/upgrade/finish", "application/x-www-form-urlencoded", &[])
        .expect("upgrade finish must restore normal state");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#""success!""#);

    let response = suite
        .client
        .post_status("/upgrade/show", "application/x-www-form-urlencoded", &[])
        .expect("upgrade show must report normal state after finish");
    assert_eq!(response.status, 200);
    assert!(response.text().unwrap().contains("cluster state is normal"));
    test_upgrade_show();

    // Go 原始签名: func TestUpgrade(t *testing.T) {
    // 关键分支: if kerneltype.IsNextGen() {
    // 迁移语句: t.Skip("Skip this case because there is no upgrade in the first release of next-gen kernel")
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // HTTP 请求: resp, err := ts.FetchStatus("/upgrade/start")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // 保留 Go 注释: // test upgrade start
    // HTTP 请求: resp, err = ts.PostStatus("/upgrade/start", "application/x-www-form-urlencoded", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: b, err := httputil.DumpResponse(resp, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: require.Greater(t, len(b), 0)
    // IO 读取写入: body, err := io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "\"success!\"", string(body))
    // 保留 Go 注释: // check the result
    // 状态准备: se, err := session.CreateSession(ts.store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: isUpgrading, err := session.IsUpgradingClusterState(se)
    // 错误处理: require.NoError(t, err)
    // 断言: require.True(t, isUpgrading)

    // 保留 Go 注释: // Do start upgrade again.
    // HTTP 请求: resp, err = ts.PostStatus("/upgrade/start", "application/x-www-form-urlencoded", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: b, err = httputil.DumpResponse(resp, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: require.Greater(t, len(b), 0)
    // IO 读取写入: body, err = io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "\"It's a duplicated operation and the cluster is already in upgrading state.\"", string(body))
    // 保留 Go 注释: // check the result
    // 状态准备: se, err = session.CreateSession(ts.store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: isUpgrading, err = session.IsUpgradingClusterState(se)
    // 错误处理: require.NoError(t, err)
    // 断言: require.True(t, isUpgrading)

    // 保留 Go 注释: // test upgrade show
    // 迁移语句: testUpgradeShow(t, ts)
    // 保留 Go 注释: // check the cluster state
    // 状态准备: se, err = session.CreateSession(ts.store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: isUpgrading, err = session.IsUpgradingClusterState(se)
    // 错误处理: require.NoError(t, err)
    // 断言: require.True(t, isUpgrading)

    // 保留 Go 注释: // test upgrade finish
    // HTTP 请求: resp, err = ts.PostStatus("/upgrade/finish", "application/x-www-form-urlencoded", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: b, err = httputil.DumpResponse(resp, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: require.Greater(t, len(b), 0)
    // IO 读取写入: body, err = io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "\"success!\"", string(body))
    // 保留 Go 注释: // check the result
    // 状态准备: se, err = session.CreateSession(ts.store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: isUpgrading, err = session.IsUpgradingClusterState(se)
    // 错误处理: require.NoError(t, err)
    // 断言: require.False(t, isUpgrading)

    // 保留 Go 注释: // test upgrade show failed
    // HTTP 请求: resp, err = ts.PostStatus("/upgrade/show", "application/x-www-form-urlencoded", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: b, err = httputil.DumpResponse(resp, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: require.Greater(t, len(b), 0)
    // IO 读取写入: body, err = io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "\"The cluster state is normal.\"\"success!\"", string(body))

    // 保留 Go 注释: // Do finish upgrade again.
    // HTTP 请求: resp, err = ts.PostStatus("/upgrade/finish", "application/x-www-form-urlencoded", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: b, err = httputil.DumpResponse(resp, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: require.Greater(t, len(b), 0)
    // IO 读取写入: body, err = io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "\"It's a duplicated operation and the cluster is already in normal state.\"", string(body))
    // 保留 Go 注释: // check the result
    // 状态准备: se, err = session.CreateSession(ts.store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: isUpgrading, err = session.IsUpgradingClusterState(se)
    // 错误处理: require.NoError(t, err)
    // 断言: require.False(t, isUpgrading)
}

// testUpgradeShow 对应 Go 函数 `func testUpgradeShow(t *testing.T, ts *basicHTTPHandlerTestSuite) {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// testUpgradeShow 对应 Go 函数 `func testUpgradeShow(t *testing.T, ts *basicHTTPHandlerTestSuite) {`。
pub fn test_upgrade_show() {
    let suite = create_basic_http_handler_test_suite();
    let response = suite
        .client
        .post_status("/upgrade/show", "application/x-www-form-urlencoded", &[])
        .expect("upgrade show must reach ClusterUpgradeHandler");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.text().unwrap(),
        r#""The cluster state is normal.""success!""#
    );

    // Go 原始签名: func testUpgradeShow(t *testing.T, ts *basicHTTPHandlerTestSuite) {
    // 状态准备: do, err := session.GetDomain(ts.store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: ddlID := do.DDL().GetID()
    // 保留 Go 注释: // check the result for upgrade show
    // 状态准备: mockedAllServerInfos := map[string]*serverinfo.ServerInfo{
    // 迁移语句: "s0": {
    // 迁移语句: StaticInfo: serverinfo.StaticInfo{
    // 迁移语句: ID: ddlID,
    // 迁移语句: IP: "127.0.0.1",
    // 迁移语句: Port: 4000,
    // 迁移语句: JSONServerID: 0,
    // 迁移语句: VersionInfo: serverinfo.VersionInfo{
    // 迁移语句: Version: "ver",
    // 迁移语句: GitHash: "hash",
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: "s2": {
    // 迁移语句: StaticInfo: serverinfo.StaticInfo{
    // 迁移语句: ID: "ID2",
    // 迁移语句: IP: "127.0.0.1",
    // 迁移语句: Port: 4002,
    // 迁移语句: JSONServerID: 2,
    // 迁移语句: VersionInfo: serverinfo.VersionInfo{
    // 迁移语句: Version: "ver2",
    // 迁移语句: GitHash: "hash2",
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: "s1": {
    // 迁移语句: StaticInfo: serverinfo.StaticInfo{
    // 迁移语句: ID: "ID1",
    // 迁移语句: IP: "127.0.0.1",
    // 迁移语句: Port: 4001,
    // 迁移语句: JSONServerID: 1,
    // 迁移语句: VersionInfo: serverinfo.VersionInfo{
    // 迁移语句: Version: "ver",
    // 迁移语句: GitHash: "hash",
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: makeFailpointRes := func(v any) string {
    // JSON 编解码: bytes, err := json.Marshal(v)
    // 错误处理: require.NoError(t, err)
    // 返回值: return fmt.Sprintf("return(`%s`)", string(bytes))
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: checkSimpleServerInfo := func(sInfo handler.SimpleServerInfo) {
    // 格式化参数: key := fmt.Sprintf("s%d", sInfo.JSONServerID)
    // 状态准备: val, ok := mockedAllServerInfos[key]
    // 断言: require.True(t, ok)
    // 断言: require.Equal(t, val.Version, sInfo.Version)
    // 断言: require.Equal(t, val.GitHash, sInfo.GitHash)
    // 断言: require.Equal(t, val.IP, sInfo.IP)
    // 断言: require.Equal(t, val.Port, sInfo.Port)
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: checkUpgradeShow := func(serverNum, upgradedPercent, diffInfos int) {
    // HTTP 请求: resp, err := ts.PostStatus("/upgrade/show", "application/x-www-form-urlencoded", nil)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: b, err := httputil.DumpResponse(resp, true)
    // 错误处理: require.NoError(t, err)
    // 迁移语句: require.Greater(t, len(b), 0)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 状态准备: clusterInfo := handler.ClusterUpgradeInfo{}
    // 状态准备: err = decoder.Decode(&clusterInfo)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, ddlID, clusterInfo.OwnerID)
    // 断言: require.Equal(t, serverNum, clusterInfo.ServersNum)
    // 断言: require.Equal(t, upgradedPercent, clusterInfo.UpgradedPercent)
    // 断言: require.Equal(t, diffInfos, len(clusterInfo.AllServersDiffInfos))
    // 关键分支: if diffInfos > 0 {
    // 断言: require.False(t, clusterInfo.IsAllUpgraded)
    // 循环遍历: for _, info := range clusterInfo.AllServersDiffInfos {
    // 迁移语句: checkSimpleServerInfo(info)
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: } else {
    // 断言: require.True(t, clusterInfo.IsAllUpgraded)
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // test upgrade show for 1 server
    // 迁移语句: checkUpgradeShow(1, 100, 0)
    // 保留 Go 注释: // test upgrade show for 3 servers
    // 错误处理: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/domain/serverinfo/mockGetAllServerInfo", makeFailpointRes(mockedAllServerInfos)))
    // 资源收尾: defer failpoint.Disable("github.com/pingcap/tidb/pkg/domain/serverinfo/mockGetAllServerInfo")
    // 保留 Go 注释: // test upgrade show again with 3 different version servers
    // 迁移语句: checkUpgradeShow(3, 33, 3)
    // 保留 Go 注释: // test upgrade show again with 3 servers of the same version
    // 状态准备: mockedAllServerInfos["s2"].Version = mockedAllServerInfos["s0"].Version
    // 状态准备: mockedAllServerInfos["s2"].GitHash = mockedAllServerInfos["s0"].GitHash
    // 错误处理: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/domain/serverinfo/mockGetAllServerInfo", makeFailpointRes(mockedAllServerInfos)))
    // 迁移语句: checkUpgradeShow(3, 100, 0)
}

#[test]
// TestIssue52608 对应 Go 函数 `func TestIssue52608(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestIssue52608 对应 Go 函数 `func TestIssue52608(t *testing.T) {`。
pub fn test_issue52608() {
    let suite = create_basic_http_handler_test_suite();
    let (server_on, address) = InstanceMPPCoordinatorManager.server_address();
    assert!(server_on);
    assert!(address.starts_with("127.0.0.1:"));
    assert_ne!(address, "127.0.0.1:0");
    assert!(suite.server.listener_addr().is_some());

    // Go 原始签名: func TestIssue52608(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()

    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // 状态准备: on, addr := mppcoordmanager.InstanceMPPCoordinatorManager.GetServerAddr()
    // 断言: require.Equal(t, on, true)
    // 断言: require.Equal(t, addr[:10], "127.0.0.1:")
}

#[test]
// TestSetLabelsConcurrentWithStoreTopology 对应 Go 函数 `func TestSetLabelsConcurrentWithStoreTopology(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestSetLabelsConcurrentWithStoreTopology 对应 Go 函数 `func TestSetLabelsConcurrentWithStoreTopology(t *testing.T) {`。
pub fn test_set_labels_concurrent_with_store_topology() {
    let _guard = labels_test_lock().lock().expect("labels test lock");
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| config.labels.clear());
    let suite = create_basic_http_handler_test_suite();
    let running = Arc::new(AtomicBool::new(true));
    let topology_running = Arc::clone(&running);
    let topology_server = Arc::clone(&suite.server);
    let topology = std::thread::spawn(move || {
        while topology_running.load(Ordering::Acquire) {
            let sql = topology_server
                .listener_addr()
                .expect("SQL listener must remain published");
            let status = topology_server
                .status_listener_addr()
                .expect("status listener must remain published");
            assert_ne!(sql.port(), 0);
            assert_ne!(status.port(), 0);
            assert!(topology_server.health());
        }
    });
    for index in 0..100 {
        let payload = format!(r#"{{"zone":"z-{index}"}}"#);
        let response = suite
            .client
            .post_status("/labels", "application/json", payload.as_bytes())
            .expect("label update must succeed while listener topology is observed");
        assert_eq!(response.status, 200);
    }
    running.store(false, Ordering::Release);
    topology
        .join()
        .expect("concurrent listener-topology reader must not panic");
    assert_eq!(
        astersql_config::get_global_config().labels.get("zone"),
        Some(&"z-99".to_owned())
    );
    restore();

    // Go 原始签名: func TestSetLabelsConcurrentWithStoreTopology(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 上下文: ctx, cancel := context.WithCancel(context.Background())
    // 资源收尾: defer cancel()

    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // 时间相关: time.Sleep(time.Second)
    // 迁移语句: integration.BeforeTestExternal(t)
    // 状态准备: cluster := integration.NewClusterV3(t, &integration.ClusterConfig{Size: 1})
    // 资源收尾: defer cluster.Terminate(t)
    // 状态准备: client := cluster.RandClient()
    // 迁移语句: infosync.SetEtcdClient(client)

    // 迁移语句: ts.domain.InfoSyncer().ServerInfoSyncer().Restart(ctx)
    // 迁移语句: ts.domain.InfoSyncer().ServerInfoSyncer().RestartTopology(ctx)

    // 状态准备: testUpdateLabels := func() {
    // 状态准备: labels := map[string]string{}
    // 格式化参数: labels["zone"] = fmt.Sprintf("z-%v", rand.Intn(100000))
    // 状态准备: buffer := bytes.NewBuffer([]byte{})
    // JSON 编解码: require.Nil(t, json.NewEncoder(buffer).Encode(labels))
    // HTTP 请求: resp, err := ts.PostStatus("/labels", "application/json", buffer)
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 状态准备: newLabels := config.GetGlobalConfig().Labels
    // 断言: require.Equal(t, newLabels, labels)
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: testStoreTopology := func() {
    // 错误处理: require.NoError(t, ts.domain.InfoSyncer().ServerInfoSyncer().StoreTopologyInfo(context.Background()))
    // 迁移语句: 结束上一层 Go 代码块。

    // 并发通道: done := make(chan struct{})
    // 迁移语句: var wg sync.WaitGroup
    // 迁移语句: wg.Add(1)
    // 并发/异步: go func() {
    // 资源收尾: defer wg.Done()
    // 循环遍历: for {
    // 迁移语句: select {
    // 分支项: case <-done:
    // 返回值: return
    // 分支项: default:
    // 迁移语句: testStoreTopology()
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: }()
    // 循环遍历: for range 100 {
    // 迁移语句: testUpdateLabels()
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: close(done)
    // 迁移语句: wg.Wait()

    // 保留 Go 注释: // reset the global variable
    // 迁移语句: config.UpdateGlobal(func(conf *config.Config) {
    // 状态准备: conf.Labels = map[string]string{}
    // 迁移语句: 结束上一层 Go 代码块。
}
