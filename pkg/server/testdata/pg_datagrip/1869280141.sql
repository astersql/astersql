select T.oid as type_id,
       T.xmin as type_state_number,
       T.typname as type_name,
       T.typtype as type_sub_kind,
       T.typcategory as type_category,
       T.typrelid as class_id,
       T.typbasetype as base_type_id,
       case when T.typtype in ('c','e') then null
            else pg_catalog.format_type(T.typbasetype, T.typtypmod) end as type_def,
       T.typndims as dimensions_number,
       T.typdefault as default_expression,
       T.typnotnull as mandatory,
       pg_catalog.pg_get_userbyid(T.typowner) as "owner"
from pg_catalog.pg_type T
         left outer join pg_catalog.pg_class C
             on T.typrelid = C.oid
where T.typnamespace = ?::oid
  --  and T.typname in ( :[*f_names] )
  --  and pg_catalog.age(T.xmin) <= #TXAGE
  and (T.typtype in ('d','e') or
       C.relkind = 'c'::"char" or
       (T.typtype = 'b' and (T.typelem = 0 OR T.typcategory <> 'A')) or
       T.typtype = 'p' and not T.typisdefined)
order by 1
