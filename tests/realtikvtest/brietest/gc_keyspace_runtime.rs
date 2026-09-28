// Copyright 2026 AsterSQL.
//! Real PD GC and TiKV backup transport for the keyspace integration scenario.
//! Wire fields follow kvproto pdpb/keyspacepb/backup; unknown response fields
//! remain forward compatible. No success signals are synthesized here.
#![allow(non_snake_case)]
use astersql_br_pkg_gc::{self as gc, GCBarrierInfo, GCState, GCStatesClient, PdClient};
use astersql_br_pkg_task::stubs::{self as task, BackupClient, Storage};
use gc::safepoint::SharedError;
use prost::Message;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::transport::{Channel, Endpoint};

fn error(message: impl ToString) -> SharedError {
    Box::new(std::io::Error::other(message.to_string()))
}
fn task_error(message: impl ToString) -> task::Error {
    task::Error::new(message.to_string())
}

#[derive(Clone, PartialEq, Message)]
struct Header {
    #[prost(uint64, tag = "1")]
    cluster_id: u64,
    #[prost(message, optional, tag = "2")]
    error: Option<WireError>,
}

#[derive(Clone, PartialEq, Message)]
struct WireError {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
}

#[derive(Clone, PartialEq, Message)]
struct MembersRequest {
    #[prost(message, optional, tag = "1")]
    header: Option<Header>,
}

#[derive(Clone, PartialEq, Message)]
struct MembersResponse {
    #[prost(message, optional, tag = "1")]
    header: Option<Header>,
}

#[derive(Clone, PartialEq, Message)]
struct Scope {
    #[prost(uint32, tag = "1")]
    keyspace_id: u32,
}

#[derive(Clone, PartialEq, Message)]
struct KeyspaceRequest {
    #[prost(message, optional, tag = "1")]
    header: Option<Header>,
    #[prost(string, tag = "2")]
    name: String,
}

#[derive(Clone, PartialEq, Message)]
struct KeyspaceMeta {
    #[prost(uint32, tag = "1")]
    id: u32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(int32, tag = "3")]
    state: i32,
}

#[derive(Clone, PartialEq, Message)]
struct KeyspaceResponse {
    #[prost(message, optional, tag = "1")]
    header: Option<Header>,
    #[prost(message, optional, tag = "2")]
    keyspace: Option<KeyspaceMeta>,
}

#[derive(Clone, PartialEq, Message)]
struct StateRequest {
    #[prost(message, optional, tag = "1")]
    header: Option<Header>,
    #[prost(message, optional, tag = "2")]
    scope: Option<Scope>,
}

#[derive(Clone, PartialEq, Message)]
struct Barrier {
    #[prost(string, tag = "1")]
    id: String,
    #[prost(uint64, tag = "2")]
    ts: u64,
    #[prost(int64, tag = "3")]
    ttl: i64,
}

#[derive(Clone, PartialEq, Message)]
struct State {
    #[prost(uint64, tag = "3")]
    txn_safe_point: u64,
    #[prost(uint64, tag = "4")]
    gc_safe_point: u64,
    #[prost(message, repeated, tag = "5")]
    barriers: Vec<Barrier>,
}

#[derive(Clone, PartialEq, Message)]
struct StateResponse {
    #[prost(message, optional, tag = "1")]
    header: Option<Header>,
    #[prost(message, optional, tag = "2")]
    state: Option<State>,
}

#[derive(Clone, PartialEq, Message)]
struct BarrierRequest {
    #[prost(message, optional, tag = "1")]
    header: Option<Header>,
    #[prost(message, optional, tag = "2")]
    scope: Option<Scope>,
    #[prost(string, tag = "3")]
    id: String,
    #[prost(uint64, tag = "4")]
    ts: u64,
    #[prost(int64, tag = "5")]
    ttl: i64,
}

#[derive(Clone, PartialEq, Message)]
struct BarrierResponse {
    #[prost(message, optional, tag = "1")]
    header: Option<Header>,
    #[prost(message, optional, tag = "2")]
    barrier: Option<Barrier>,
}

#[derive(Clone, PartialEq, Message)]
struct Local {
    #[prost(string, tag = "1")]
    path: String,
}

#[derive(Clone, PartialEq, Message)]
struct Backend {
    #[prost(message, optional, tag = "2")]
    local: Option<Local>,
}

#[derive(Clone, PartialEq, Message)]
struct BackupContext {
    #[prost(int32, tag = "21")]
    api_version: i32,
    #[prost(uint32, tag = "32")]
    keyspace_id: u32,
}

#[derive(Clone, PartialEq, Message)]
struct BackupRequest {
    #[prost(uint64, tag = "1")]
    cluster_id: u64,
    #[prost(bytes = "vec", tag = "2")]
    start_key: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    end_key: Vec<u8>,
    #[prost(uint64, tag = "4")]
    start_version: u64,
    #[prost(uint64, tag = "5")]
    end_version: u64,
    #[prost(uint64, tag = "7")]
    rate_limit: u64,
    #[prost(uint32, tag = "8")]
    concurrency: u32,
    #[prost(message, optional, tag = "9")]
    backend: Option<Backend>,
    #[prost(int32, tag = "12")]
    compression: i32,
    #[prost(int32, tag = "13")]
    compression_level: i32,
    #[prost(int32, tag = "15")]
    dst_api_version: i32,
    #[prost(message, optional, tag = "20")]
    context: Option<BackupContext>,
}

#[derive(Clone, PartialEq, Message)]
struct BackupError {
    #[prost(string, tag = "1")]
    message: String,
    #[prost(bytes = "vec", optional, tag = "3")]
    cluster_error: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "4")]
    kv_error: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "5")]
    region_error: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct BackupFile {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(uint64, tag = "8")]
    total_kvs: u64,
    #[prost(uint64, tag = "9")]
    total_bytes: u64,
    #[prost(uint64, tag = "11")]
    size: u64,
}

#[derive(Clone, PartialEq, Message)]
struct BackupResponse {
    #[prost(message, optional, tag = "1")]
    error: Option<BackupError>,
    #[prost(message, repeated, tag = "4")]
    files: Vec<BackupFile>,
}

#[derive(Clone)]
pub struct Rpc {
    runtime: Arc<tokio::runtime::Runtime>,
    channel: Channel,
    pub cluster_id: u64,
}
impl Rpc {
    pub fn connect(address: &str) -> Result<Self, SharedError> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?,
        );
        let url = if address.contains("://") {
            address.to_string()
        } else {
            format!("http://{address}")
        };
        let channel = runtime.block_on(
            Endpoint::from_shared(url)?
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .connect(),
        )?;
        let mut rpc = Self {
            runtime,
            channel,
            cluster_id: 0,
        };
        let response: MembersResponse =
            rpc.call("/pdpb.PD/GetMembers", MembersRequest::default())?;
        rpc.cluster_id = Self::check_header(response.header)?.cluster_id;
        Ok(rpc)
    }
    fn header(&self) -> Option<Header> {
        Some(Header {
            cluster_id: self.cluster_id,
            error: None,
        })
    }
    fn check_header(header: Option<Header>) -> Result<Header, SharedError> {
        let header = header.ok_or_else(|| error("PD returned no response header"))?;
        if let Some(err) = &header.error {
            return Err(error(format!("PD error {}: {}", err.code, err.message)));
        }
        Ok(header)
    }
    fn call<
        Q: Message + Default + Send + Sync + 'static,
        R: Message + Default + Send + Sync + 'static,
    >(
        &self,
        path: &'static str,
        request: Q,
    ) -> Result<R, SharedError> {
        self.runtime.block_on(async {
            let mut client = tonic::client::Grpc::new(self.channel.clone());
            client.ready().await.map_err(error)?;
            let response: tonic::Response<R> = client
                .unary(
                    tonic::Request::new(request),
                    tonic::codegen::http::uri::PathAndQuery::from_static(path),
                    tonic::codec::ProstCodec::default(),
                )
                .await?;
            Ok(response.into_inner())
        })
    }
    pub fn keyspace_id(&self, name: &str) -> Result<u32, SharedError> {
        let response: KeyspaceResponse = self.call(
            "/keyspacepb.Keyspace/LoadKeyspace",
            KeyspaceRequest {
                header: self.header(),
                name: name.into(),
            },
        )?;
        Self::check_header(response.header)?;
        let meta = response
            .keyspace
            .ok_or_else(|| error("keyspace metadata missing"))?;
        if meta.state != 0 {
            return Err(error(format!("keyspace {name} is not enabled")));
        }
        Ok(meta.id)
    }
    pub fn state(&self, id: u32) -> Result<GCState, SharedError> {
        self.GetGCStatesClient(id)
            .GetGCState(&gc::Context::Background())
    }
}
struct ScopedRpc {
    rpc: Rpc,
    id: u32,
}
impl GCStatesClient for ScopedRpc {
    fn GetGCState(&self, ctx: &gc::Context) -> Result<GCState, SharedError> {
        if ctx.Done() {
            return Err(error("context cancelled"));
        }
        let r: StateResponse = self.rpc.call(
            "/pdpb.PD/GetGCState",
            StateRequest {
                header: self.rpc.header(),
                scope: Some(Scope {
                    keyspace_id: self.id,
                }),
            },
        )?;
        Rpc::check_header(r.header)?;
        let s = r.state.ok_or_else(|| error("GC state missing"))?;
        Ok(GCState {
            GCSafePoint: s.gc_safe_point,
            TxnSafePoint: s.txn_safe_point,
            GCBarriers: s
                .barriers
                .into_iter()
                .map(|b| GCBarrierInfo {
                    BarrierID: b.id,
                    BarrierTS: b.ts,
                    TTL: b.ttl,
                })
                .collect(),
        })
    }
    fn SetGCBarrier(
        &self,
        ctx: &gc::Context,
        id: &str,
        ts: u64,
        ttl: i64,
    ) -> Result<GCBarrierInfo, SharedError> {
        if ctx.Done() {
            return Err(error("context cancelled"));
        }
        let r: BarrierResponse = self.rpc.call(
            "/pdpb.PD/SetGCBarrier",
            BarrierRequest {
                header: self.rpc.header(),
                scope: Some(Scope {
                    keyspace_id: self.id,
                }),
                id: id.into(),
                ts,
                ttl,
            },
        )?;
        Rpc::check_header(r.header)?;
        let b = r
            .barrier
            .ok_or_else(|| error("set GC barrier response missing"))?;
        Ok(GCBarrierInfo {
            BarrierID: b.id,
            BarrierTS: b.ts,
            TTL: b.ttl,
        })
    }
    fn DeleteGCBarrier(
        &self,
        ctx: &gc::Context,
        id: &str,
    ) -> Result<Option<GCBarrierInfo>, SharedError> {
        if ctx.Done() {
            return Err(error("context cancelled"));
        }
        let r: BarrierResponse = self.rpc.call(
            "/pdpb.PD/DeleteGCBarrier",
            BarrierRequest {
                header: self.rpc.header(),
                scope: Some(Scope {
                    keyspace_id: self.id,
                }),
                id: id.into(),
                ts: 0,
                ttl: 0,
            },
        )?;
        Rpc::check_header(r.header)?;
        Ok(r.barrier.map(|b| GCBarrierInfo {
            BarrierID: b.id,
            BarrierTS: b.ts,
            TTL: b.ttl,
        }))
    }
}
impl PdClient for Rpc {
    fn UpdateGCSafePoint(&self, _: &gc::Context, _: u64) -> Result<u64, SharedError> {
        Err(error(
            "keyspace integration must not call the deprecated global GC API",
        ))
    }
    fn UpdateServiceGCSafePoint(
        &self,
        _: &gc::Context,
        _: &str,
        _: i64,
        _: u64,
    ) -> Result<u64, SharedError> {
        Err(error(
            "keyspace integration must not call the deprecated global service GC API",
        ))
    }
    fn GetGCStatesClient(&self, id: u32) -> Arc<dyn GCStatesClient> {
        Arc::new(ScopedRpc {
            rpc: self.clone(),
            id,
        })
    }
}

pub struct LocalStorage(pub PathBuf);
impl Storage for LocalStorage {
    fn ReadFile(&self, name: &str) -> task::Result<Vec<u8>> {
        std::fs::read(self.0.join(name)).map_err(task_error)
    }
    fn WriteFile(&self, name: &str, data: &[u8]) -> task::Result<()> {
        std::fs::write(self.0.join(name), data).map_err(task_error)
    }
    fn FileExists(&self, name: &str) -> task::Result<bool> {
        self.0.join(name).try_exists().map_err(task_error)
    }
    fn WalkDir(
        &self,
        sub: &str,
        f: &mut dyn FnMut(&str, i64) -> task::Result<()>,
    ) -> task::Result<()> {
        fn walk(
            root: &Path,
            path: &Path,
            f: &mut dyn FnMut(&str, i64) -> task::Result<()>,
        ) -> task::Result<()> {
            for entry in std::fs::read_dir(path).map_err(task_error)? {
                let entry = entry.map_err(task_error)?;
                let meta = entry.metadata().map_err(task_error)?;
                if meta.is_dir() {
                    walk(root, &entry.path(), f)?;
                } else {
                    f(
                        &entry
                            .path()
                            .strip_prefix(root)
                            .map_err(task_error)?
                            .to_string_lossy(),
                        meta.len() as i64,
                    )?;
                }
            }
            Ok(())
        }
        walk(&self.0, &self.0.join(sub), f)
    }
}

/// The transport records real SST statistics and observes the barrier while
/// TiKV is about to read the snapshot. It never creates GC signal files.
pub struct RealBackupClient {
    pub rpc: Rpc,
    pub keyspace_id: u32,
    pub tikv: Vec<String>,
    pub timestamp: u64,
    pub storage: Arc<LocalStorage>,
    pub database: String,
    pub table_id: i64,
    pub files: Mutex<Vec<(String, u64)>>,
}
impl BackupClient for RealBackupClient {
    fn GetClusterID(&self) -> u64 {
        self.rpc.cluster_id
    }
    fn GetCurrentTS(&self) -> task::Result<u64> {
        Ok(self.timestamp)
    }
    fn GetStorageBackend(&self) -> Option<task::backuppb::StorageBackend> {
        Some(task::backuppb::StorageBackend {
            Scheme: "local".into(),
            Path: self.storage.0.to_string_lossy().into_owned(),
            ..Default::default()
        })
    }
    fn GetApiVersion(&self) -> i32 {
        2
    }
    fn SetStorageAndCheckNotInUse(
        &self,
        _: &task::backuppb::StorageBackend,
        _: &task::StorageOptions,
    ) -> task::Result<()> {
        std::fs::create_dir_all(&self.storage.0).map_err(task_error)?;
        if self.storage.FileExists("backupmeta")? {
            return Err(task_error("backup storage already contains backupmeta"));
        }
        Ok(())
    }
    fn BuildBackupRanges(
        &self,
        filter: &[String],
        _: u64,
        _: bool,
    ) -> task::Result<Vec<task::KeyRange>> {
        if filter != [format!("{}.*", self.database)] {
            return Err(task_error("fixture filter does not select its database"));
        }
        let encode = |id: i64| {
            let mut key = vec![b'x'];
            key.extend_from_slice(&self.keyspace_id.to_be_bytes()[1..]);
            key.push(b't');
            key.extend_from_slice(&((id as u64) ^ (1 << 63)).to_be_bytes());
            key
        };
        Ok(vec![task::KeyRange {
            StartKey: encode(self.table_id),
            EndKey: encode(self.table_id + 1),
        }])
    }
    fn BackupRanges(
        &self,
        ranges: &[task::KeyRange],
        request: &task::backuppb::BackupRequest,
    ) -> task::Result<u64> {
        let state = self.rpc.state(self.keyspace_id).map_err(task_error)?;
        if !state
            .GCBarriers
            .iter()
            .any(|b| b.BarrierTS == request.EndVersion - 1)
        {
            return Err(task_error(
                "snapshot is not protected by its keyspace GC barrier",
            ));
        }
        let mut total = 0;
        for address in &self.tikv {
            let address = address.clone();
            for range in ranges {
                let request = BackupRequest {
                    cluster_id: request.ClusterId,
                    start_key: range.StartKey.clone(),
                    end_key: range.EndKey.clone(),
                    start_version: request.StartVersion,
                    end_version: request.EndVersion,
                    rate_limit: request.RateLimit,
                    concurrency: request.Concurrency,
                    backend: Some(Backend {
                        local: Some(Local {
                            path: self.storage.0.to_string_lossy().into_owned(),
                        }),
                    }),
                    compression: request.CompressionType as i32,
                    compression_level: request.CompressionLevel,
                    dst_api_version: 2,
                    context: Some(BackupContext {
                        api_version: 2,
                        keyspace_id: self.keyspace_id,
                    }),
                };
                let files = self
                    .rpc
                    .runtime
                    .block_on(async {
                        let channel = Endpoint::from_shared(format!("http://{address}"))?
                            .connect_timeout(Duration::from_secs(5))
                            .timeout(Duration::from_secs(30))
                            .connect()
                            .await?;
                        let mut client = tonic::client::Grpc::new(channel);
                        client.ready().await.map_err(error)?;
                        let response: tonic::Response<tonic::Streaming<BackupResponse>> = client
                            .server_streaming(
                                tonic::Request::new(request),
                                tonic::codegen::http::uri::PathAndQuery::from_static(
                                    "/backup.Backup/backup",
                                ),
                                tonic::codec::ProstCodec::default(),
                            )
                            .await?;
                        let mut stream = response.into_inner();
                        let mut files = Vec::new();
                        while let Some(response) = stream.message().await? {
                            if let Some(e) = response.error {
                                return Err(error(format!("TiKV backup error: {e:?}")));
                            }
                            files.extend(response.files);
                        }
                        Ok::<_, SharedError>(files)
                    })
                    .map_err(task_error)?;
                for file in files {
                    total += file.size;
                    self.files.lock().unwrap().push((file.name, file.total_kvs));
                }
            }
        }
        Ok(total)
    }
    fn GetStorage(&self) -> Arc<dyn Storage> {
        self.storage.clone()
    }
}
