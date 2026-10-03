with schema_procs as (select prorettype, proargtypes, proallargtypes
                      from pg_catalog.pg_proc
                      where pronamespace = ?::oid
                        /* and pg_catalog.age(xmin) <= #TXAGE */ ),
     schema_opers as (select oprleft, oprright, oprresult
                      from pg_catalog.pg_operator
                      where oprnamespace = ?::oid
                        /* and pg_catalog.age(xmin) <= #TXAGE */ ),
     schema_aggregates as (select A.aggtranstype , A.aggmtranstype 
                           from pg_catalog.pg_aggregate A
                           join pg_catalog.pg_proc P
                             on A.aggfnoid = P.oid
                           where P.pronamespace = ?::oid
                           /* and (pg_catalog.age(A.xmin) <= #TXAGE or pg_catalog.age(P.xmin) <= #TXAGE) */),
     schema_arg_types as ( select prorettype as type_id
                           from schema_procs
                           union
                           select distinct unnest(proargtypes) as type_id
                           from schema_procs
                           union
                           select distinct unnest(proallargtypes) as type_id
                           from schema_procs
                           union
                           select oprleft as type_id
                           from schema_opers
                           where oprleft is not null
                           union
                           select oprright as type_id
                           from schema_opers
                           where oprright is not null
                           union
                           select oprresult as type_id
                           from schema_opers
                           where oprresult is not null
                           union
                           select aggtranstype::oid as type_id
                           from schema_aggregates
                           union
                           select aggmtranstype::oid as type_id
                           from schema_aggregates
                           
                           )
select type_id, pg_catalog.format_type(type_id, null) as type_spec
from schema_arg_types
where type_id <> 0 -- todo unclear how to frag
