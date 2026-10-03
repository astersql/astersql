select O.oid as id,
       O.xmin as state_number,
       opcname as name,
       opcintype::regtype::varchar as in_type,
       case when opckeytype = 0 then null else opckeytype::regtype::varchar end as key_type,
       opcdefault as is_default,
       opcfamily as family_id,
       opfname as family,
       opcmethod as access_method_id,
       pg_catalog.pg_get_userbyid(O.opcowner) as "owner"
from pg_catalog.pg_opclass O join pg_catalog.pg_opfamily F on F.oid = opcfamily
where opcnamespace = ?::oid
  --  and opcname in ( :[*f_names] )
  --  and pg_catalog.age(O.xmin) <= #TXAGE
