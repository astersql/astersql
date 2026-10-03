select O.oid as id,
       O.xmin as state_number,
       opfname as name,
       opfmethod as access_method_id,
       pg_catalog.pg_get_userbyid(O.opfowner) as "owner"
from pg_catalog.pg_opfamily O
where opfnamespace = ?::oid
  --  and opfname in ( :[*f_names] )
  --  and pg_catalog.age(xmin) <= #TXAGE
