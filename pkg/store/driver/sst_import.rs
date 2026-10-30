// Copyright 2026 AsterSQL.

//! Physical TiKV import transport. Match the Go region-job protocol: locate
//! each encoded key range in PD, stream one SST UUID to every region peer,
//! then ingest the leader's returned SST metadata at one fixed commit TSO.
use std::{collections::HashMap, sync::Arc, time::Duration};
use tikv_client::proto::{import_sstpb as sst, kvrpcpb, metapb, pdpb};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

type Result<T> = std::result::Result<T, ImportError>;
type PdClient = pdpb::pd_client::PdClient<Channel>;
type ImportClient = sst::import_sst_client::ImportSstClient<Channel>;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ImportError {
    message: String,
    retryable: bool,
}
impl ImportError {
    fn permanent(error: impl std::fmt::Display) -> Self {
        Self {
            message: error.to_string(),
            retryable: false,
        }
    }
    fn retry(error: impl std::fmt::Display) -> Self {
        Self {
            message: error.to_string(),
            retryable: true,
        }
    }
}
impl From<tonic::Status> for ImportError {
    fn from(error: tonic::Status) -> Self {
        Self {
            retryable: matches!(
                error.code(),
                tonic::Code::Unavailable
                    | tonic::Code::DeadlineExceeded
                    | tonic::Code::ResourceExhausted
            ),
            message: error.to_string(),
        }
    }
}

#[derive(Default, Debug)]
pub struct ImportStats {
    pub keys: usize,
    pub bytes: usize,
    pub write_rpcs: usize,
    pub ingest_rpcs: usize,
}

/// TiKV/PD region boundaries use memcomparable bytes; WriteBatch pairs retain
/// the original user keys, as in Go codec.EncodeBytes + import_sstpb.Pair.
fn region_key(key: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity((key.len() / 8 + 1) * 9);
    for group in key.chunks_exact(8) {
        encoded.extend_from_slice(group);
        encoded.push(0xff);
    }
    let tail = &key[key.len() / 8 * 8..];
    encoded.extend_from_slice(tail);
    let padding = 8 - tail.len();
    encoded.resize(encoded.len() + padding, 0);
    encoded.push(0xff - padding as u8);
    encoded
}

async fn channel(address: &str, tls: Option<&crate::TlsConfig>) -> Result<Channel> {
    let uri = if address.contains("://") {
        address.to_owned()
    } else {
        format!(
            "{}://{address}",
            if tls.is_some() { "https" } else { "http" }
        )
    };
    let mut endpoint = Endpoint::from_shared(uri)
        .map_err(ImportError::permanent)?
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60));
    if let Some(tls) = tls {
        let mut config = ClientTlsConfig::new().ca_certificate(Certificate::from_pem(
            std::fs::read(&tls.ca_path).map_err(ImportError::permanent)?,
        ));
        if !tls.cert_path.is_empty() {
            config = config.identity(Identity::from_pem(
                std::fs::read(&tls.cert_path).map_err(ImportError::permanent)?,
                std::fs::read(&tls.key_path).map_err(ImportError::permanent)?,
            ));
        }
        endpoint = endpoint
            .tls_config(config)
            .map_err(ImportError::permanent)?;
    }
    endpoint.connect().await.map_err(ImportError::retry)
}

fn check_header(header: Option<&pdpb::ResponseHeader>) -> Result<()> {
    if let Some(error) = header.and_then(|header| header.error.as_ref()) {
        return Err(ImportError::retry(format!("PD: {}", error.message)));
    }
    Ok(())
}

struct Connection {
    pd: PdClient,
    cluster_id: u64,
    tls: Option<crate::TlsConfig>,
    stores: HashMap<u64, ImportClient>,
}
impl Connection {
    async fn connect(endpoints: &[String], tls: Option<crate::TlsConfig>) -> Result<Self> {
        let mut last = ImportError::permanent("no PD endpoints configured");
        for endpoint in endpoints {
            let connection = async {
                let mut pd = PdClient::new(channel(endpoint, tls.as_ref()).await?);
                let members = pd
                    .get_members(pdpb::GetMembersRequest::default())
                    .await?
                    .into_inner();
                check_header(members.header.as_ref())?;
                let cluster_id = members
                    .header
                    .ok_or_else(|| ImportError::permanent("PD response has no header"))?
                    .cluster_id;
                if let Some(leader_url) = members
                    .leader
                    .and_then(|leader| leader.client_urls.into_iter().next())
                {
                    pd = PdClient::new(channel(&leader_url, tls.as_ref()).await?);
                }
                Ok(Self {
                    pd,
                    cluster_id,
                    tls: tls.clone(),
                    stores: HashMap::new(),
                })
            }
            .await;
            match connection {
                Ok(connection) => return Ok(connection),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
    fn header(&self) -> Option<pdpb::RequestHeader> {
        Some(pdpb::RequestHeader {
            cluster_id: self.cluster_id,
            ..Default::default()
        })
    }
    async fn store(&mut self, id: u64) -> Result<ImportClient> {
        if let Some(client) = self.stores.get(&id) {
            return Ok(client.clone());
        }
        let response = self
            .pd
            .get_store(pdpb::GetStoreRequest {
                header: self.header(),
                store_id: id,
            })
            .await?
            .into_inner();
        check_header(response.header.as_ref())?;
        let store = response
            .store
            .ok_or_else(|| ImportError::retry(format!("PD store {id} missing")))?;
        let client = ImportClient::new(channel(&store.address, self.tls.as_ref()).await?);
        self.stores.insert(id, client.clone());
        Ok(client)
    }
    async fn import_region(
        &mut self,
        pairs: &[(Vec<u8>, Vec<u8>)],
        commit_ts: u64,
        stats: &mut ImportStats,
        keyspace_id: Option<u32>,
        options: &astersql_kv::SSTImportOptions,
    ) -> Result<usize> {
        let response = self
            .pd
            .get_region(pdpb::GetRegionRequest {
                header: self.header(),
                region_key: region_key(&pairs[0].0),
                need_buckets: false,
            })
            .await?
            .into_inner();
        check_header(response.header.as_ref())?;
        let region = response
            .region
            .ok_or_else(|| ImportError::retry("PD region missing"))?;
        let leader = response
            .leader
            .ok_or_else(|| ImportError::retry("PD region has no leader"))?;
        let count = pairs.partition_point(|(key, _)| {
            region.end_key.is_empty() || region_key(key) < region.end_key
        });
        if count == 0 {
            return Err(ImportError::retry("PD returned a stale region range"));
        }
        let pairs = Arc::new(pairs[..count].to_vec());
        let meta = sst::SstMeta {
            uuid: uuid::Uuid::new_v4().as_bytes().to_vec(),
            region_id: region.id,
            region_epoch: region.region_epoch.clone(),
            api_version: if keyspace_id.is_some() {
                kvrpcpb::ApiVersion::V2 as i32
            } else {
                kvrpcpb::ApiVersion::V1 as i32
            },
            range: Some(sst::Range {
                start: region_key(&pairs[0].0),
                end: region_key(&pairs[count - 1].0),
            }),
            ..Default::default()
        };
        let mut writes = Vec::new();
        for peer in &region.peers {
            let mut client = self.store(peer.store_id).await?;
            let pairs = pairs.clone();
            let context = context(&region, peer, keyspace_id);
            let first = sst::WriteRequest {
                context: Some(context.clone()),
                chunk: Some(sst::write_request::Chunk::Meta(meta.clone())),
            };
            let options = options.clone();
            let write_failure = Arc::new(std::sync::Mutex::new(None));
            let failure = write_failure.clone();
            let store_id = peer.store_id;
            let batches = futures::stream::unfold((pairs, 0), move |(pairs, start)| {
                let context = context.clone();
                let options = options.clone();
                let failure = failure.clone();
                async move {
                    if start == pairs.len() {
                        return None;
                    }
                    let mut end = start;
                    let mut size = 0;
                    while end < pairs.len() && (end == start || size < 1024 * 1024) {
                        size += pairs[end].0.len() + pairs[end].1.len();
                        end += 1;
                    }
                    if options.context.is_cancelled() {
                        *failure.lock().unwrap() =
                            Some(ImportError::permanent("SST import cancelled"));
                        return None;
                    }
                    if let Some(limiter) = options.write_limiter {
                        let cancel = options.context.clone();
                        let waiting = tokio::task::spawn_blocking(move || {
                            limiter.WaitN(&cancel, store_id, size)
                        });
                        match waiting.await {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                *failure.lock().unwrap() = Some(ImportError::permanent(error));
                                return None;
                            }
                            Err(error) => {
                                *failure.lock().unwrap() = Some(ImportError::permanent(error));
                                return None;
                            }
                        }
                    }
                    let batch = sst::WriteBatch {
                        commit_ts,
                        pairs: pairs[start..end]
                            .iter()
                            .map(|(key, value)| sst::Pair {
                                key: key.clone(),
                                value: value.clone(),
                                ..Default::default()
                            })
                            .collect(),
                    };
                    Some((
                        sst::WriteRequest {
                            context: Some(context),
                            chunk: Some(sst::write_request::Chunk::Batch(batch)),
                        },
                        (pairs, end),
                    ))
                }
            });
            let stream = futures::StreamExt::chain(futures::stream::once(async { first }), batches);
            let peer_id = peer.id;
            writes.push(async move {
                let response = client.write(stream).await?.into_inner();
                if let Some(error) = write_failure.lock().unwrap().take() {
                    return Err(error);
                }
                if let Some(error) = response.error {
                    return Err(ImportError::retry(format!("TiKV SST write: {error:?}")));
                }
                Ok::<_, ImportError>((peer_id, response.metas))
            });
        }
        stats.write_rpcs += writes.len();
        let responses = futures::future::try_join_all(writes).await?;
        let metas = responses
            .into_iter()
            .find(|(id, _)| *id == leader.id)
            .ok_or_else(|| ImportError::retry("leader is absent from region peers"))?
            .1;
        if metas.is_empty() {
            return Err(ImportError::permanent(
                "leader returned no SST metadata for nonempty input",
            ));
        }
        let mut client = self.store(leader.store_id).await?;
        stats.ingest_rpcs += 1;
        let response = client
            .multi_ingest(sst::MultiIngestRequest {
                context: Some(context(&region, &leader, keyspace_id)),
                ssts: metas,
            })
            .await?
            .into_inner();
        if let Some(error) = response.error {
            return Err(ImportError::retry(format!("TiKV SST ingest: {error:?}")));
        }
        stats.keys += count;
        stats.bytes += pairs
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>();
        Ok(count)
    }
}

fn context(
    region: &metapb::Region,
    peer: &metapb::Peer,
    keyspace_id: Option<u32>,
) -> kvrpcpb::Context {
    kvrpcpb::Context {
        region_id: region.id,
        region_epoch: region.region_epoch.clone(),
        peer: Some(peer.clone()),
        request_source: "internal_lightning:import".into(),
        txn_source: 1,
        api_version: if keyspace_id.is_some() {
            kvrpcpb::ApiVersion::V2 as i32
        } else {
            kvrpcpb::ApiVersion::V1 as i32
        },
        keyspace_id: keyspace_id.unwrap_or_default(),
        ..Default::default()
    }
}

/// Physically import strictly ordered unique KV pairs. Retries keep the same
/// commit timestamp and relocate the unfinished range, including after a split
/// or an ambiguous ingest response. No transactional Put/Commit fallback exists.
pub fn write_and_ingest(
    endpoints: &[String],
    tls: Option<crate::TlsConfig>,
    commit_ts: u64,
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<ImportStats> {
    write_and_ingest_with_options(
        endpoints,
        tls,
        commit_ts,
        pairs,
        None,
        astersql_kv::SSTImportOptions::default(),
    )
}

/// Runtime import controls and actual API V2 keyspace identity.
pub fn write_and_ingest_with_options(
    endpoints: &[String],
    tls: Option<crate::TlsConfig>,
    commit_ts: u64,
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
    keyspace_id: Option<u32>,
    options: astersql_kv::SSTImportOptions,
) -> Result<ImportStats> {
    if commit_ts == 0 || commit_ts > i64::MAX as u64 {
        return Err(ImportError::permanent(
            "SST import requires a valid PD commit timestamp",
        ));
    }
    if pairs.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return Err(ImportError::permanent(
            "SST input keys must be strictly increasing",
        ));
    }
    if pairs.is_empty() {
        return Ok(ImportStats::default());
    }
    let codec = match keyspace_id {
        Some(id) => astersql_store_copr::network_backend::KeyCodec::v2(String::new(), id)
            .map_err(ImportError::permanent)?,
        None => astersql_store_copr::network_backend::KeyCodec::v1(),
    };
    let pairs = pairs
        .into_iter()
        .map(|(key, value)| (codec.encode_key(&key), value))
        .collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ImportError::permanent)?;
    runtime.block_on(async {
        let importing = async {
            let mut connection = Connection::connect(endpoints, tls.clone()).await?;
            let mut stats = ImportStats::default();
            let mut cursor = 0;
            let mut attempts = 0;
            while cursor < pairs.len() {
                match connection
                    .import_region(
                        &pairs[cursor..],
                        commit_ts,
                        &mut stats,
                        keyspace_id,
                        &options,
                    )
                    .await
                {
                    Ok(count) => {
                        cursor += count;
                        attempts = 0;
                    }
                    Err(error) if error.retryable && attempts < 29 => {
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis((100 * attempts).min(1000))).await;
                        connection = Connection::connect(endpoints, tls.clone()).await?;
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(stats)
        };
        tokio::select! {
            _ = options.context.cancelled() => Err(ImportError::permanent("SST import cancelled")),
            result = importing => result,
        }
    })
}

#[cfg(test)]
#[path = "sst_import_test.rs"]
mod tests;

/// NextGen physical import. Region lookup remains PD-backed; a disk-full
/// response escapes intact to DXF instead of being exhausted inside transport.
pub fn write_and_ingest_nextgen(
    endpoints: &[String],
    tls: Option<crate::TlsConfig>,
    timestamp: u64,
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
    keyspace_id: Option<u32>,
    options: astersql_kv::SSTImportOptions,
) -> std::result::Result<ImportStats, astersql_kv::errors::SharedError> {
    use astersql_ingestor_ingestcli as http;
    use astersql_kv::errors::SharedError;
    struct Split(String);
    impl http::SplitClient for Split {
        fn get_store(
            &self,
            _: &dyn http::RequestContext,
            _: u64,
        ) -> std::result::Result<http::Store, http::Error> {
            Ok(http::Store {
                status_address: self.0.clone(),
            })
        }
    }
    struct Context(astersql_kv::Context);
    impl http::RequestContext for Context {
        fn is_cancelled(&self) -> bool {
            self.0.is_cancelled()
        }
    }
    struct Transport {
        client: reqwest::Client,
        runtime: std::sync::Mutex<tokio::runtime::Runtime>,
        context: astersql_kv::Context,
    }
    impl Transport {
        fn execute(
            &self,
            request: reqwest::RequestBuilder,
        ) -> std::result::Result<http::HttpResponse, http::Error> {
            if self.context.is_cancelled() {
                return Err(http::Error::Canceled);
            }
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| http::Error::WorkerPanicked)?;
            runtime.block_on(async {
                let request = async {
                    let response = request.send().await.map_err(|error| {
                        http::Error::Io(std::io::Error::other(error.without_url()))
                    })?;
                    let status_code = response.status().as_u16();
                    let body = response
                        .bytes()
                        .await
                        .map_err(|error| {
                            http::Error::Io(std::io::Error::other(error.without_url()))
                        })?
                        .to_vec();
                    Ok(http::HttpResponse { status_code, body })
                };
                tokio::pin!(request);
                loop {
                    tokio::select! {
                        response = &mut request => return response,
                        _ = tokio::time::sleep(Duration::from_millis(10)) => {
                            if self.context.is_cancelled() { return Err(http::Error::Canceled); }
                        }
                    }
                }
            })
        }
    }
    impl http::HttpTransport for Transport {
        fn put_stream(
            &self,
            url: &str,
            chunks: std::sync::mpsc::Receiver<Vec<u8>>,
        ) -> std::result::Result<http::HttpResponse, http::Error> {
            // Do not put a blocking recv inside the HTTP runtime. Dropping the
            // cancelled request must drop this receiver and wake its producer.
            let stream = futures::stream::unfold(chunks, |receiver| async move {
                loop {
                    match receiver.try_recv() {
                        Ok(chunk) => return Some((Ok::<_, std::io::Error>(chunk), receiver)),
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return None,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            tokio::time::sleep(Duration::from_millis(1)).await
                        }
                    }
                }
            });
            self.execute(
                self.client
                    .put(url)
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(reqwest::Body::wrap_stream(stream)),
            )
        }
        fn post(
            &self,
            url: &str,
            body: &[u8],
        ) -> std::result::Result<http::HttpResponse, http::Error> {
            self.execute(self.client.post(url).body(body.to_vec()))
        }
    }
    if timestamp == 0 || pairs.windows(2).any(|w| w[0].0 >= w[1].0) {
        return Err(astersql_kv::errors::New(
            "invalid NextGen SST timestamp or key order",
        ));
    }
    let codec = match keyspace_id {
        Some(id) => astersql_store_copr::network_backend::KeyCodec::v2(String::new(), id)
            .map_err(|e| astersql_kv::errors::New(e.to_string()))?,
        None => astersql_store_copr::network_backend::KeyCodec::v1(),
    };
    let pairs = pairs
        .into_iter()
        .map(|(k, v)| (codec.encode_key(&k), v))
        .collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(SharedError::new)?;
    let cancellation = options.context.clone();
    runtime.block_on(async {
        let importing = async {
            let mut connection = Connection::connect(endpoints, tls.clone())
                .await
                .map_err(SharedError::new)?;
            let mut stats = ImportStats::default();
            let mut cursor = 0;
            while cursor < pairs.len() {
                if options.context.is_cancelled() {
                    return Err(astersql_kv::errors::New("NextGen SST import cancelled"));
                }
                let response = connection
                    .pd
                    .get_region(pdpb::GetRegionRequest {
                        header: connection.header(),
                        region_key: region_key(&pairs[cursor].0),
                        need_buckets: false,
                    })
                    .await
                    .map_err(SharedError::new)?
                    .into_inner();
                check_header(response.header.as_ref()).map_err(SharedError::new)?;
                let region = response
                    .region
                    .ok_or_else(|| astersql_kv::errors::New("PD region missing"))?;
                let leader = response
                    .leader
                    .ok_or_else(|| astersql_kv::errors::New("PD region leader missing"))?;
                let count = pairs[cursor..].partition_point(|(k, _)| {
                    region.end_key.is_empty() || region_key(k) < region.end_key
                });
                if count == 0 {
                    return Err(astersql_kv::errors::New("stale PD Region range"));
                }
                let store = connection
                    .pd
                    .get_store(pdpb::GetStoreRequest {
                        header: connection.header(),
                        store_id: leader.store_id,
                    })
                    .await
                    .map_err(SharedError::new)?
                    .into_inner();
                check_header(store.header.as_ref()).map_err(SharedError::new)?;
                let address = store
                    .store
                    .ok_or_else(|| astersql_kv::errors::New("PD store missing"))?
                    .status_address;
                let batch = pairs[cursor..cursor + count].to_vec();
                let bytes = batch.iter().map(|(k, v)| k.len() + v.len()).sum();
                if let Some(limiter) = &options.write_limiter {
                    limiter.WaitN(&options.context, leader.store_id, bytes)?;
                }
                let cluster_id = connection.cluster_id;
                let tls = tls.clone();
                let context = Context(options.context.clone());
                let worker = astersql_config::get_global_config().tikv_worker_url.clone();
                tokio::task::spawn_blocking(move || -> std::result::Result<(), SharedError> {
                    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(60));
                    if let Some(tls) = &tls {
                        let ca = std::fs::read(&tls.ca_path).map_err(SharedError::new)?;
                        builder = builder.add_root_certificate(
                            reqwest::Certificate::from_pem(&ca).map_err(SharedError::new)?,
                        );
                        if !tls.cert_path.is_empty() {
                            let mut pem =
                                std::fs::read(&tls.cert_path).map_err(SharedError::new)?;
                            pem.extend(std::fs::read(&tls.key_path).map_err(SharedError::new)?);
                            builder = builder.identity(
                                reqwest::Identity::from_pem(&pem).map_err(SharedError::new)?,
                            );
                        }
                    }
                    let transport = builder.build().map_err(SharedError::new)?;
                    let client = http::ClientImpl::new(
                        worker,
                        cluster_id,
                        tls.is_some(),
                        Arc::new(Transport {
                            client: transport,
                            runtime: std::sync::Mutex::new(
                                tokio::runtime::Builder::new_multi_thread()
                                    .worker_threads(1)
                                    .enable_all()
                                    .build()
                                    .map_err(SharedError::new)?,
                            ),
                            context: context.0.clone(),
                        }),
                        Arc::new(Split(address)),
                    );
                    let epoch = region.region_epoch.map(|e| http::RegionEpoch {
                        conf_ver: e.conf_ver,
                        version: e.version,
                    });
                    http::WriteAndIngestRegion(
                        &client,
                        &context,
                        timestamp,
                        batch
                            .into_iter()
                            .map(|(key, value)| http::Pair { key, value })
                            .collect(),
                        http::RegionInfo {
                            region: http::Region {
                                id: region.id,
                                region_epoch: epoch,
                                start_key: region.start_key,
                                end_key: region.end_key,
                                ..Default::default()
                            },
                            leader: Some(http::Peer {
                                id: leader.id,
                                store_id: leader.store_id,
                            }),
                        },
                    )
                    .map_err(SharedError::new)
                })
                .await
                .map_err(SharedError::new)??;
                stats.keys += count;
                stats.bytes += bytes;
                stats.write_rpcs += 1;
                stats.ingest_rpcs += 1;
                cursor += count;
            }
            Ok(stats)
        };
        tokio::select! {
            result = importing => result,
            _ = async {
                while !cancellation.is_cancelled() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            } => Err(SharedError::new(http::Error::Canceled)),
        }
    })
}
