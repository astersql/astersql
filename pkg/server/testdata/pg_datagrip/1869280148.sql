select oid as id,
       xmin as state_number,
       collname as name,
       collcollate as lc_collate,
       collctype as lc_ctype,
       pg_catalog.pg_get_userbyid(collowner) as "owner"
from pg_catalog.pg_collation
where collnamespace = ?::oid
  --  and collname in ( :[*f_names] )
  --  and pg_catalog.age(xmin) <= #TXAGE
