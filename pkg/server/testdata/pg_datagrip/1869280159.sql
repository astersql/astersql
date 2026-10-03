with system_languages as ( select oid as lang
                           from pg_catalog.pg_language
                           where lanname in ('c','internal') )
select oid as id,
       pg_catalog.pg_get_function_arguments(oid) as arguments_def,
       pg_catalog.pg_get_function_result(oid) as result_def,
       pg_catalog.pg_get_function_sqlbody(oid) /* null */ as sqlbody_def,
       prosrc as source_text
from pg_catalog.pg_proc
where pronamespace = ?::oid
  --  and pg_proc.proname in ( :[*f_names] )
  --  and pg_catalog.age(xmin) <= #SRCTXAGE
  and not (prokind = 'a') /* proisagg */
  and prolang not in (select lang from system_languages)
  and prosrc is not null
