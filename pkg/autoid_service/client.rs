// Copyright 2026 AsterSQL.
//! Production AutoID leader discovery and RPC transport.
use autoid_dependency::{
    AutoIdClient, AutoIdClientConnector, AutoIdError, AutoIdRequest, AutoIdResponse,
    ClientConnection, ClientDiscover, Context, LeaderDiscovery, RebaseRequest, RebaseResponse,
    Result,
};
use etcd_client::{
    Certificate, Client, ConnectOptions, GetOptions, Identity, SortOrder, SortTarget, TlsOptions,
};
use grpcio::{CallOption, ChannelBuilder, ChannelCredentialsBuilder, Environment};
use kvproto::autoid::{
    AutoIDRequest_oneof_keyspace, AutoIdAllocClient, AutoIdRequest as WireRequest,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
fn err(e: impl ToString) -> AutoIdError {
    AutoIdError::Storage(e.to_string())
}
#[derive(Clone, Default)]
pub struct ClientTls {
    pub ca: Vec<u8>,
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}
struct Discovery {
    runtime: tokio::runtime::Runtime,
    client: Mutex<Client>,
    namespace: String,
}
impl LeaderDiscovery for Discovery {
    fn leader(&self, ctx: &Context, path: &str) -> Result<Option<String>> {
        ctx.check()?;
        let options = GetOptions::new()
            .with_prefix()
            .with_sort(SortTarget::Create, SortOrder::Ascend)
            .with_limit(1);
        let result = self
            .runtime
            .block_on(
                self.client
                    .lock()
                    .map_err(err)?
                    .get(format!("{}{path}", self.namespace), Some(options)),
            )
            .map_err(err)?;
        result
            .kvs()
            .first()
            .map(|v| {
                std::str::from_utf8(v.value())
                    .map(str::to_owned)
                    .map_err(err)
            })
            .transpose()
    }
}
struct Connector {
    env: Arc<Environment>,
    tls: Option<ClientTls>,
}
struct RpcClient {
    client: AutoIdAllocClient,
}
struct Connection(Mutex<Option<grpcio::Channel>>);
impl ClientConnection for Connection {
    fn close(&self) -> Result<()> {
        self.0.lock().map_err(err)?.take();
        Ok(())
    }
}
impl AutoIdClientConnector for Connector {
    fn connect(&self, address: &str) -> Result<(Arc<dyn AutoIdClient>, Arc<dyn ClientConnection>)> {
        let builder = ChannelBuilder::new(self.env.clone());
        let channel = if let Some(t) = &self.tls {
            let mut credentials = ChannelCredentialsBuilder::new().root_cert(t.ca.clone());
            if !t.cert.is_empty() || !t.key.is_empty() {
                credentials = credentials.cert(t.cert.clone(), t.key.clone());
            }
            builder
                .set_credentials(credentials.build())
                .connect(address)
        } else {
            builder.connect(address)
        };
        Ok((
            Arc::new(RpcClient {
                client: AutoIdAllocClient::new(channel.clone()),
            }),
            Arc::new(Connection(Mutex::new(Some(channel)))),
        ))
    }
}
impl AutoIdClient for RpcClient {
    fn alloc_auto_id(&self, ctx: &Context, r: AutoIdRequest) -> Result<AutoIdResponse> {
        ctx.check()?;
        let mut wire = WireRequest::new();
        wire.set_db_id(r.database_id);
        wire.set_tbl_id(r.table_id);
        wire.set_n(r.n);
        wire.set_increment(r.increment);
        wire.set_offset(r.offset);
        wire.set_is_unsigned(r.is_unsigned);
        wire.keyspace = Some(AutoIDRequest_oneof_keyspace::KeyspaceId(r.keyspace_id));
        let result = self
            .client
            .alloc_auto_id_opt(
                &wire,
                CallOption::default().timeout(Duration::from_secs(30)),
            )
            .map_err(err)?;
        Ok(AutoIdResponse {
            min: result.get_min(),
            max: result.get_max(),
            errmsg: String::from_utf8(result.get_errmsg().to_vec()).map_err(err)?,
        })
    }
    fn rebase(&self, ctx: &Context, r: RebaseRequest) -> Result<RebaseResponse> {
        ctx.check()?;
        let mut wire = kvproto::autoid::RebaseRequest::new();
        wire.set_db_id(r.database_id);
        wire.set_tbl_id(r.table_id);
        wire.set_base(r.base);
        wire.set_force(r.force);
        wire.set_is_unsigned(r.is_unsigned);
        let result = self
            .client
            .rebase_opt(
                &wire,
                CallOption::default().timeout(Duration::from_secs(30)),
            )
            .map_err(err)?;
        Ok(RebaseResponse {
            errmsg: String::from_utf8(result.get_errmsg().to_vec()).map_err(err)?,
        })
    }
}
/// Connect to the serving store's etcd endpoints; keyspace namespaces are exact.
pub fn client_discover(
    endpoints: Vec<String>,
    tls: Option<ClientTls>,
    namespace: String,
) -> Result<Arc<ClientDiscover>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(err)?;
    let mut options = ConnectOptions::new()
        .with_connect_timeout(Duration::from_secs(5))
        .with_timeout(Duration::from_secs(30));
    if let Some(t) = &tls {
        let mut config = TlsOptions::new().ca_certificate(Certificate::from_pem(t.ca.clone()));
        if !t.cert.is_empty() || !t.key.is_empty() {
            config = config.identity(Identity::from_pem(t.cert.clone(), t.key.clone()));
        }
        options = options.with_tls(config);
    }
    let client = runtime
        .block_on(Client::connect(endpoints, Some(options)))
        .map_err(err)?;
    Ok(Arc::new(ClientDiscover::new(
        Arc::new(Discovery {
            runtime,
            client: Mutex::new(client),
            namespace,
        }),
        Arc::new(Connector {
            env: Arc::new(Environment::new(1)),
            tls,
        }),
    )))
}

#[cfg(test)]
#[path = "client_test.rs"]
mod tests;
