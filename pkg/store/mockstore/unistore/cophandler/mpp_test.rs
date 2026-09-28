// Copyright 2026 AsterSQL.

use crate::cop_handler::{Datum, ExchangeType, Executor, Expr, KeyRange, MemoryReader};
use crate::mpp::{
    EstablishRequest, ExchangerTunnel, MppContext, MppExecBuilder, MppTaskHandler, TaskMeta,
    TunnelKey,
};

#[test]
fn hash_exchange_uses_only_partition_keys_like_go() {
    let mut reader = MemoryReader::default();
    for suffix in 0..8_i64 {
        reader.rows.insert(
            vec![suffix as u8],
            (vec![Datum::Int(7), Datum::Int(suffix)], 1),
        );
    }
    let ranges = [KeyRange {
        start: vec![],
        end: vec![],
    }];
    let handler = MppTaskHandler::default();
    let first = ExchangerTunnel::new();
    let second = ExchangerTunnel::new();
    for (receiver, tunnel) in [(2, first.clone()), (3, second.clone())] {
        handler
            .register_tunnel(
                TunnelKey {
                    sender_task_id: 1,
                    receiver_task_id: receiver,
                },
                tunnel,
            )
            .unwrap();
        handler
            .establish_conn(&EstablishRequest {
                sender: TaskMeta {
                    task_id: 1,
                    address: String::new(),
                },
                receiver: TaskMeta {
                    task_id: receiver,
                    address: String::new(),
                },
            })
            .unwrap();
    }

    MppExecBuilder::new(&reader, &ranges, 1)
        .with_context(MppContext {
            task: TaskMeta {
                task_id: 1,
                address: String::new(),
            },
            handler: &handler,
        })
        .build_and_execute(&Executor::ExchangeSender {
            exchange: ExchangeType::Hash,
            partition_keys: vec![Expr::Column(0)],
            child: Box::new(Executor::TableScan {
                columns: vec![0, 1],
                descending: false,
            }),
        })
        .unwrap();

    let first_rows = first.recv_chunk().unwrap().unwrap().data;
    let second_rows = second.recv_chunk().unwrap().unwrap().data;
    assert!(
        first_rows.is_empty() || second_rows.is_empty(),
        "equal partition keys must always select the same tunnel"
    );
    assert_eq!(first_rows.len() + second_rows.len(), 8);
}
