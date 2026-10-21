SELECT jsonb_build_object('kind','unsupported_object','schema',x.schema_name,'description',x.description)::text AS catalog_record FROM (
 SELECT n.nspname AS schema_name,'relation '||c.relkind::text||': '||c.relname AS description FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind NOT IN ('r','p','f','v','m','i','I','t')
 UNION ALL SELECT n.nspname,'routine: '||p.oid::regprocedure::text FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
 UNION ALL SELECT n.nspname,'type: '||t.typname FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace WHERE t.typrelid=0 AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_type base WHERE base.typarray=t.oid)
 UNION ALL SELECT n.nspname,'extension: '||e.extname FROM pg_catalog.pg_extension e JOIN pg_catalog.pg_namespace n ON n.oid=e.extnamespace
 UNION ALL SELECT n.nspname,'schema explicit privileges/owner' FROM pg_catalog.pg_namespace n WHERE n.nspacl IS NOT NULL OR n.nspowner<>(SELECT oid FROM pg_catalog.pg_roles WHERE rolname=current_user)
 UNION ALL SELECT n.nspname,'schema comment' FROM pg_catalog.pg_namespace n WHERE pg_catalog.obj_description(n.oid,'pg_namespace') IS NOT NULL
 UNION ALL SELECT n.nspname,'default privileges' FROM pg_catalog.pg_default_acl a JOIN pg_catalog.pg_namespace n ON n.oid=a.defaclnamespace
 UNION ALL SELECT n.nspname,'collation: '||c.collname FROM pg_catalog.pg_collation c JOIN pg_catalog.pg_namespace n ON n.oid=c.collnamespace
 UNION ALL SELECT n.nspname,'conversion: '||c.conname FROM pg_catalog.pg_conversion c JOIN pg_catalog.pg_namespace n ON n.oid=c.connamespace
 UNION ALL SELECT n.nspname,'operator: '||o.oprname FROM pg_catalog.pg_operator o JOIN pg_catalog.pg_namespace n ON n.oid=o.oprnamespace
 UNION ALL SELECT n.nspname,'operator family: '||o.opfname FROM pg_catalog.pg_opfamily o JOIN pg_catalog.pg_namespace n ON n.oid=o.opfnamespace
 UNION ALL SELECT n.nspname,'operator class: '||o.opcname FROM pg_catalog.pg_opclass o JOIN pg_catalog.pg_namespace n ON n.oid=o.opcnamespace
 UNION ALL SELECT n.nspname,'text search configuration: '||o.cfgname FROM pg_catalog.pg_ts_config o JOIN pg_catalog.pg_namespace n ON n.oid=o.cfgnamespace
 UNION ALL SELECT n.nspname,'text search dictionary: '||o.dictname FROM pg_catalog.pg_ts_dict o JOIN pg_catalog.pg_namespace n ON n.oid=o.dictnamespace
 UNION ALL SELECT n.nspname,'text search parser: '||o.prsname FROM pg_catalog.pg_ts_parser o JOIN pg_catalog.pg_namespace n ON n.oid=o.prsnamespace
 UNION ALL SELECT n.nspname,'text search template: '||o.tmplname FROM pg_catalog.pg_ts_template o JOIN pg_catalog.pg_namespace n ON n.oid=o.tmplnamespace
) x WHERE x.schema_name IN (__SCOPE__) ORDER BY x.schema_name,x.description LIMIT __LIMIT__;
