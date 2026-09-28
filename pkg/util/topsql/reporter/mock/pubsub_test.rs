// Copyright 2026 AsterSQL.

use std::pin::Pin;

use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status};

use crate::pubsub::NewMockPubSubServer;
use crate::tipb::top_sql_pub_sub_client::TopSqlPubSubClient;
use crate::tipb::top_sql_pub_sub_server::TopSqlPubSub;
use crate::tipb::{TopSqlRecord, TopSqlSubRequest, TopSqlSubResponse, top_sql_sub_response};

#[derive(Default)]
struct EchoPubSub;

#[tonic::async_trait]
impl TopSqlPubSub for EchoPubSub {
    type SubscribeStream =
        Pin<Box<dyn Stream<Item = Result<TopSqlSubResponse, Status>> + Send + 'static>>;

    async fn subscribe(
        &self,
        _request: Request<TopSqlSubRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let response = TopSqlSubResponse {
            resp_oneof: Some(top_sql_sub_response::RespOneof::Record(TopSqlRecord {
                sql_digest: b"registered-service".to_vec(),
                ..Default::default()
            })),
        };
        Ok(Response::new(Box::pin(tokio_stream::iter([Ok(response)]))))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registered_service_handles_real_grpc_subscription_and_stops() {
    let mut server = NewMockPubSubServer().expect("bind mock server");
    let handle = server.Server();
    handle
        .register_top_sql_pub_sub(EchoPubSub)
        .expect("register pubsub service");
    server.Serve().expect("serve registered service");

    let endpoint = format!("http://{}", server.Address());
    let mut client = TopSqlPubSubClient::connect(endpoint)
        .await
        .expect("connect to real gRPC server");
    let mut stream = client
        .subscribe(TopSqlSubRequest::default())
        .await
        .expect("subscribe")
        .into_inner();
    let response = stream
        .next()
        .await
        .expect("one response")
        .expect("response");
    let top_sql_sub_response::RespOneof::Record(record) = response.resp_oneof.unwrap() else {
        panic!("expected TopSQL record")
    };
    assert_eq!(record.sql_digest, b"registered-service");

    handle.Stop();
    tokio::task::yield_now().await;
    assert!(handle.is_stopped());
}

#[test]
fn duplicate_service_registration_is_rejected() {
    let server = NewMockPubSubServer().expect("bind mock server");
    let handle = server.Server();
    handle
        .register_top_sql_pub_sub(EchoPubSub)
        .expect("first registration");
    let error = handle
        .register_top_sql_pub_sub(EchoPubSub)
        .expect_err("duplicate registration must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
}
