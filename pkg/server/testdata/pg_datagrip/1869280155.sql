select
       P.oid id,
       P.xmin as state_number,
       polname policyname,
       polrelid table_id,
       polpermissive /* true */ as permissive,
       polroles roles,
       polcmd cmd,
       pg_get_expr(polqual, polrelid) qual,
       pg_get_expr(polwithcheck, polrelid) with_check
from pg_catalog.pg_policy P
       join pg_catalog.pg_class C on polrelid = C.oid
where relnamespace = ?::oid
  --  and C.relname in ( :[*f_names] )
  --  and pg_catalog.age(P.xmin) <= #TXAGE
order by polrelid
