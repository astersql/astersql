// Copyright 2026 AsterSQL.
use super::*;
#[test]
fn normal_ddl_plan_create_table_autoid_rpc_rebase_and_allocate() {
    let store = Arc::new(crate::autoid_test::MemoryStore::new(
        "task12-rpc",
        autoid_dependency::NULLSPACE_ID,
    ));
    let service = crate::Service::new_mock(store);
    let env = Arc::new(Environment::new(1));
    let mut server = grpcio::ServerBuilder::new(env.clone())
        .register_service(crate::create_grpc_service(service))
        .build()
        .unwrap();
    let port = server
        .add_listening_port("127.0.0.1:0", grpcio::ServerCredentials::insecure())
        .unwrap();
    server.start();
    let connector = Connector { env, tls: None };
    let (client, connection) = connector.connect(&format!("127.0.0.1:{port}")).unwrap();
    let ctx = Context::default();
    let response = client
        .rebase(
            &ctx,
            RebaseRequest {
                database_id: 17,
                table_id: 99,
                base: 110,
                force: false,
                is_unsigned: false,
            },
        )
        .unwrap();
    assert!(response.errmsg.is_empty());
    let response = client
        .alloc_auto_id(
            &ctx,
            AutoIdRequest {
                database_id: 17,
                table_id: 99,
                n: 1,
                increment: 1,
                offset: 1,
                is_unsigned: false,
                keyspace_id: autoid_dependency::NULLSPACE_ID,
            },
        )
        .unwrap();
    assert!(response.errmsg.is_empty());
    assert_eq!((response.min, response.max), (110, 111));
    connection.close().unwrap();
    drop(client);
    futures::executor::block_on(server.shutdown()).unwrap();
}
