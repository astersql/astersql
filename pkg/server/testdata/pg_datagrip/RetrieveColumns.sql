with T as ( select
                  T.oid as table_id, T.relname as table_name
            from pg_catalog.pg_class T
            where T.relnamespace = ?::oid
              and T.relkind in ('r', 'm', 'v', 'f', 'p')
            )
select T.table_id,
       C.attnum as column_position,
       C.attname as column_name,
       C.xmin as column_state_number,
       C.atttypmod as type_mod,
       C.attndims as dimensions_number,
       pg_catalog.format_type(C.atttypid, C.atttypmod) as type_spec,
       C.atttypid as type_id,
       C.attnotnull as mandatory,
       pg_catalog.pg_get_expr(D.adbin, T.table_id) as column_default_expression,
       not C.attislocal as column_is_inherited,
        C.attfdwoptions as options,
       C.attisdropped as column_is_dropped,
       C.attidentity as identity_kind,
       C.attgenerated as generated
from T
  join pg_catalog.pg_attribute C on T.table_id = C.attrelid
  left join pg_catalog.pg_attrdef D on (C.attrelid, C.attnum) = (D.adrelid, D.adnum)
where attnum > 0
order by table_id, attnum
;
