select T.relkind as table_kind,
       T.relname as table_name,
       T.oid as table_id,
       T.xmin as table_state_number,
       false /* T.relhasoids */ as table_with_oids,
       T.reltablespace as tablespace_id,
       T.reloptions as options,
       T.relpersistence as persistence,
       (select pg_catalog.array_agg(inhparent::bigint order by inhseqno)::varchar from pg_catalog.pg_inherits where T.oid = inhrelid) as ancestors,
       (select pg_catalog.array_agg(inhrelid::bigint order by inhrelid)::varchar from pg_catalog.pg_inherits where T.oid = inhparent) as successors,
       T.relispartition /* false */ as is_partition,
       pg_catalog.pg_get_partkeydef(T.oid) /* null */ as partition_key,
       pg_catalog.pg_get_expr(T.relpartbound, T.oid) /* null */ as partition_expression,
       T.relam am_id,
       pg_catalog.pg_get_userbyid(T.relowner) as "owner"
from pg_catalog.pg_class T
where relnamespace = ?::oid
       and relkind in ('r', 'm', 'v', 'f', 'p')
--  and pg_catalog.age(T.xmin) <= #TXAGE
--  and T.relname in ( :[*f_names] )
order by table_kind, table_id
