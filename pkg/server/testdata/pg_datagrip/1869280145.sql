with languages as (select oid as lang_oid, lanname as lang
                   from pg_catalog.pg_language),
     routines as (select proname as r_name,
                         prolang as lang_oid,
                         oid as r_id,
                         xmin as r_state_number,
                         proargnames as arg_names,
                         proargmodes as arg_modes,
                         proargtypes::int[] as in_arg_types,
                         proallargtypes::int[] as all_arg_types,
                         pg_catalog.pg_get_expr(proargdefaults, 0) as arg_defaults,
                         provariadic as arg_variadic_id,
                         prorettype as ret_type_id,
                         proretset as ret_set,
                         prokind /* case when proiswindow then 'w'
                                                when proisagg then 'a'
                                                else 'f'
                                           end */ as kind,
                         provolatile as volatile_kind,
                         proisstrict as is_strict,
                         prosecdef as is_security_definer,
                         proconfig as configuration_parameters,
                         procost as cost,
                         pg_catalog.pg_get_userbyid(proowner) as "owner",
                         prorows as rows ,
                         proleakproof as is_leakproof  ,
                         proparallel as concurrency_kind 
                  from pg_catalog.pg_proc
                  where pronamespace = ?::oid
                    --  and proname in ( :[*f_names] )
                    and not (prokind = 'a') /* proisagg */
                    /* and pg_catalog.age(xmin) <= #TXAGE */)
select *
from routines natural join languages
