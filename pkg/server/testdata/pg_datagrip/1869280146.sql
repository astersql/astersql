select P.oid as aggregate_id,
       P.xmin as state_number,
       P.proname as aggregate_name,
       P.proargnames as arg_names,
       P.proargmodes as arg_modes,
       P.proargtypes::int[] as in_arg_types,
       P.proallargtypes::int[] as all_arg_types,
       A.aggtransfn::oid as transition_function_id,
       A.aggtransfn::regproc::text as transition_function_name,
       A.aggtranstype as transition_type,
       A.aggfinalfn::oid as final_function_id,
       case when A.aggfinalfn::oid = 0 then null else A.aggfinalfn::regproc::varchar end as final_function_name,
       case when A.aggfinalfn::oid = 0 then 0 else P.prorettype end as final_return_type,
       A.agginitval as initial_value,
       A.aggsortop as sort_operator_id,
       case when A.aggsortop = 0 then null else A.aggsortop::regoper::varchar end as sort_operator_name,
       pg_catalog.pg_get_userbyid(P.proowner) as "owner"
       ,
       A.aggfinalextra as final_extra,
       A.aggtransspace as state_size,
       A.aggmtransfn::oid as moving_transition_id,
       case when A.aggmtransfn::oid = 0 then null else A.aggmtransfn::regproc::varchar end as moving_transition_name,
       A.aggminvtransfn::oid as inverse_transition_id,
       case when A.aggminvtransfn::oid = 0 then null else A.aggminvtransfn::regproc::varchar end as inverse_transition_name,
       A.aggmtranstype::oid as moving_state_type,
       A.aggmtransspace as moving_state_size,
       A.aggmfinalfn::oid as moving_final_id,
       case when A.aggmfinalfn::oid = 0 then null else A.aggmfinalfn::regproc::varchar end as moving_final_name,
       A.aggmfinalextra as moving_final_extra,
       A.aggminitval as moving_initial_value,
       A.aggkind as aggregate_kind,
       A.aggnumdirectargs as direct_args
       
       ,
       A.aggcombinefn::oid as combine_function_id,
       case when A.aggcombinefn::oid = 0 then null else A.aggcombinefn::regproc::varchar end as combine_function_name,
       A.aggserialfn::oid as serialization_function_id,
       case when A.aggserialfn::oid = 0 then null else A.aggserialfn::regproc::varchar end as serialization_function_name,
       A.aggdeserialfn::oid as deserialization_function_id,
       case when A.aggdeserialfn::oid = 0 then null else A.aggdeserialfn::regproc::varchar end as deserialization_function_name,
       P.proparallel as concurrency_kind
       
from pg_catalog.pg_aggregate A
join pg_catalog.pg_proc P
  on A.aggfnoid = P.oid
where P.pronamespace = ?::oid
--  and P.proname in ( :[*f_names] )
--  and (pg_catalog.age(A.xmin) <= #TXAGE or pg_catalog.age(P.xmin) <= #TXAGE)
order by P.oid
