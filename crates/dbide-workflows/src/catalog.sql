BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '__TIMEOUT_MS__ms';
SET LOCAL lock_timeout = '2000ms';
SET LOCAL standard_conforming_strings = on;
SET LOCAL search_path = pg_catalog;
SELECT jsonb_build_object('kind','identity','database',current_database(),'version',current_setting('server_version'),'version_num',current_setting('server_version_num')::integer)::text AS catalog_record;
SELECT jsonb_build_object('kind','schema','name',n.nspname)::text AS catalog_record FROM pg_catalog.pg_namespace n WHERE n.nspname IN (__SCOPE__) ORDER BY n.nspname LIMIT __LIMIT__;
SELECT jsonb_build_object('kind','relation','schema',n.nspname,'name',c.relname,'relation_kind',c.relkind::text,'relation_oid',c.oid::bigint,'ordinary',c.relkind='r' AND NOT c.relispartition AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_inherits inh WHERE inh.inhrelid=c.oid OR inh.inhparent=c.oid),
 'definition',CASE WHEN c.relkind IN ('v','m') THEN pg_catalog.pg_get_viewdef(c.oid,true) ELSE NULL END,
 'features',to_jsonb(array_remove(ARRAY[
   CASE WHEN c.relkind IN ('p','f') THEN 'nonordinary table kind: '||c.relkind::text END,
   CASE WHEN c.relpersistence <> 'p' THEN 'nonpersistent relation' END,
   CASE WHEN c.relispartition OR EXISTS(SELECT 1 FROM pg_catalog.pg_inherits h WHERE h.inhrelid=c.oid OR h.inhparent=c.oid) THEN 'inheritance/partition dependency' END,
   CASE WHEN c.relrowsecurity OR c.relforcerowsecurity THEN 'row security' END,
   CASE WHEN c.relreplident <> 'd' THEN 'custom replica identity' END,
   CASE WHEN c.reloptions IS NOT NULL THEN 'relation options: '||c.reloptions::text END,
   CASE WHEN c.reltablespace <> 0 THEN 'custom tablespace' END,
   CASE WHEN c.relacl IS NOT NULL THEN 'explicit relation privileges' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint fk JOIN pg_catalog.pg_class child ON child.oid=fk.conrelid JOIN pg_catalog.pg_namespace cn ON cn.oid=child.relnamespace WHERE fk.contype='f' AND fk.confrelid=c.oid AND cn.nspname NOT IN (__SCOPE__)) THEN 'incoming foreign key outside scope' END,
   CASE WHEN __TARGETED_CAPTURE__ AND EXISTS(SELECT 1 FROM pg_catalog.pg_constraint fk WHERE fk.contype='f' AND fk.confrelid=c.oid AND (fk.confdeltype IN ('c','n','d') OR fk.confupdtype IN ('c','n','d'))) THEN 'incoming mutating foreign key' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_depend dep JOIN pg_catalog.pg_rewrite rw ON dep.classid='pg_catalog.pg_rewrite'::regclass AND dep.objid=rw.oid JOIN pg_catalog.pg_class dependent ON dependent.oid=rw.ev_class JOIN pg_catalog.pg_namespace dn ON dn.oid=dependent.relnamespace WHERE dep.refclassid='pg_catalog.pg_class'::regclass AND dep.refobjid=c.oid AND dn.nspname NOT IN (__SCOPE__)) THEN 'dependent view outside scope' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.refclassid='pg_catalog.pg_class'::regclass AND dep.refobjid=c.oid AND dep.classid='pg_catalog.pg_proc'::regclass) THEN 'dependent stored routine' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_publication_rel pr WHERE pr.prrelid=c.oid) OR EXISTS(SELECT 1 FROM pg_catalog.pg_statistic_ext st WHERE st.stxrelid=c.oid) THEN 'publication membership/extended statistics' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type ty ON ty.oid=a.atttypid JOIN pg_catalog.pg_namespace tn ON tn.oid=ty.typnamespace WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND tn.nspname<>'pg_catalog') THEN 'user-defined column type dependency' END,
   CASE WHEN c.relowner <> (SELECT oid FROM pg_catalog.pg_roles WHERE rolname=current_user) THEN 'noncurrent relation owner' END,
   CASE WHEN pg_catalog.obj_description(c.oid,'pg_class') IS NOT NULL THEN 'relation comment' END,
   CASE WHEN c.relam<>0 AND (SELECT amname FROM pg_catalog.pg_am WHERE oid=c.relam)<>'heap' THEN 'custom table access method' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_trigger t WHERE t.tgrelid=c.oid AND NOT t.tgisinternal) THEN 'user triggers' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_depend d WHERE d.classid='pg_catalog.pg_class'::regclass AND d.objid=c.oid AND d.deptype='e') THEN 'extension-owned relation' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND a.attidentity<>'') THEN 'column identity' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND a.attgenerated<>'') THEN 'column generated' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND a.attacl IS NOT NULL) THEN 'column privileges' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND pg_catalog.col_description(c.oid,a.attnum) IS NOT NULL) THEN 'column comment' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type ty ON ty.oid=a.atttypid WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND a.attcollation<>ty.typcollation) THEN 'column custom collation' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_collation co ON co.oid=a.attcollation WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND NOT co.collisdeterministic) THEN 'column nondeterministic collation' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype='c') THEN 'constraint check' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype='u') THEN 'constraint unique' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype='x') THEN 'constraint exclusion' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype='n') THEN 'constraint not null' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype NOT IN ('p','f','c','u','x','n')) THEN 'constraint unknown kind' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND NOT con.convalidated) THEN 'constraint unvalidated' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND NOT COALESCE((to_jsonb(con)->>'conenforced')::boolean,true)) THEN 'constraint not enforced' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype NOT IN ('p','f') AND con.condeferrable) THEN 'constraint deferrable' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype='p' AND con.condeferrable) THEN 'primary key deferrable' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_index ix ON ix.indexrelid=con.conindid WHERE con.conrelid=c.oid AND con.contype='p' AND ix.indnatts<>ix.indnkeyatts) THEN 'primary key include columns' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype='f' AND con.confmatchtype NOT IN (' ','s')) THEN 'foreign key non-simple match' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND COALESCE((to_jsonb(con)->>'conperiod')::boolean,false)) THEN 'constraint temporal/period' END,
   CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype='f' AND (to_jsonb(con)->>'confdelsetcols') IS NOT NULL) THEN 'foreign key delete-column subset' END
 ]::text[],NULL)))::text AS catalog_record
FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
WHERE n.nspname IN (__SCOPE__) AND (__RELATION_FILTER__) AND c.relkind IN ('r','p','f','v','m') ORDER BY n.nspname,c.relname LIMIT __LIMIT__;
SELECT jsonb_build_object('kind','column','schema',n.nspname,'table',c.relname,
 'column',jsonb_build_object('name',a.attname,'data_type',pg_catalog.format_type(a.atttypid,a.atttypmod),'nullable',NOT a.attnotnull,
 'default',CASE WHEN a.attgenerated='' AND a.attidentity='' THEN pg_catalog.pg_get_expr(d.adbin,d.adrelid) ELSE NULL END,
 'generated',CASE WHEN a.attidentity<>'' THEN 'identity:'||a.attidentity::text WHEN a.attgenerated<>'' THEN 'generated:'||a.attgenerated::text||':'||pg_catalog.pg_get_expr(d.adbin,d.adrelid) ELSE NULL END))::text AS catalog_record
FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
WHERE n.nspname IN (__SCOPE__) AND (__RELATION_FILTER__) AND c.relkind IN ('r','p','f') ORDER BY n.nspname,c.relname,a.attnum LIMIT __LIMIT__;
SELECT jsonb_build_object('kind','primary_key','schema',n.nspname,'table',c.relname,
 'key',jsonb_build_object('name',con.conname,'columns',(SELECT jsonb_agg(a.attname ORDER BY k.ordinality) FROM unnest(con.conkey) WITH ORDINALITY k(attnum,ordinality) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.attnum)))::text AS catalog_record
FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class c ON c.oid=con.conrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname IN (__SCOPE__) AND (__RELATION_FILTER__) AND con.contype='p' ORDER BY n.nspname,c.relname,con.conname LIMIT __LIMIT__;
SELECT jsonb_build_object('kind','foreign_key','schema',n.nspname,'table',c.relname,
 'key',jsonb_build_object('name',con.conname,'columns',(SELECT jsonb_agg(a.attname ORDER BY k.ordinality) FROM unnest(con.conkey) WITH ORDINALITY k(attnum,ordinality) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.attnum),
 'references',jsonb_build_object('schema',rn.nspname,'name',rc.relname),'referenced_columns',(SELECT jsonb_agg(a.attname ORDER BY k.ordinality) FROM unnest(con.confkey) WITH ORDINALITY k(attnum,ordinality) JOIN pg_catalog.pg_attribute a ON a.attrelid=rc.oid AND a.attnum=k.attnum),
 'on_delete',CASE con.confdeltype WHEN 'a' THEN 'no_action' WHEN 'r' THEN 'restrict' WHEN 'c' THEN 'cascade' WHEN 'n' THEN 'set_null' WHEN 'd' THEN 'set_default' ELSE 'unknown' END,
 'on_update',CASE con.confupdtype WHEN 'a' THEN 'no_action' WHEN 'r' THEN 'restrict' WHEN 'c' THEN 'cascade' WHEN 'n' THEN 'set_null' WHEN 'd' THEN 'set_default' ELSE 'unknown' END,
 'deferrable',con.condeferrable,'initially_deferred',con.condeferred))::text AS catalog_record
FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class c ON c.oid=con.conrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_class rc ON rc.oid=con.confrelid JOIN pg_catalog.pg_namespace rn ON rn.oid=rc.relnamespace
WHERE n.nspname IN (__SCOPE__) AND (__RELATION_FILTER__) AND con.contype='f' ORDER BY n.nspname,c.relname,con.conname LIMIT __LIMIT__;
SELECT jsonb_build_object('kind','index','schema',n.nspname,'table',c.relname,
 'index',jsonb_build_object('name',ic.relname,'columns',COALESCE((SELECT jsonb_agg(a.attname ORDER BY k.ordinality) FROM unnest(ix.indkey) WITH ORDINALITY k(attnum,ordinality) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.attnum WHERE k.ordinality<=ix.indnkeyatts),'[]'::jsonb),
 'unique',ix.indisunique,'method',am.amname,'predicate',pg_catalog.pg_get_expr(ix.indpred,ix.indrelid),
 'expressions',CASE WHEN ix.indexprs IS NOT NULL THEN jsonb_build_array(pg_catalog.pg_get_expr(ix.indexprs,ix.indrelid)) ELSE '[]'::jsonb END,
 'include',COALESCE((SELECT jsonb_agg(a.attname ORDER BY k.ordinality) FROM unnest(ix.indkey) WITH ORDINALITY k(attnum,ordinality) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.attnum WHERE k.ordinality>ix.indnkeyatts),'[]'::jsonb),
 'constraint_owned',EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con WHERE con.conindid=ix.indexrelid AND con.contype IN ('p','u','x')),
 'unsupported_features',to_jsonb(array_remove(ARRAY[
  CASE WHEN NOT ix.indisvalid OR NOT ix.indisready OR NOT ix.indislive THEN 'invalid/not ready index' END,
  CASE WHEN COALESCE((to_jsonb(ix)->>'indnullsnotdistinct')::boolean,false) THEN 'NULLS NOT DISTINCT' END,
  CASE WHEN EXISTS(SELECT 1 FROM unnest(ix.indoption) opt WHERE opt<>0) THEN 'index sort/null order' END,
  CASE WHEN EXISTS(SELECT 1 FROM unnest(ix.indclass) op JOIN pg_catalog.pg_opclass oc ON oc.oid=op WHERE NOT oc.opcdefault) THEN 'custom operator class' END,
  CASE WHEN ic.reloptions IS NOT NULL OR ic.reltablespace<>0 THEN 'index storage options/tablespace' END,
  CASE WHEN pg_catalog.obj_description(ic.oid,'pg_class') IS NOT NULL THEN 'index comment' END,
  CASE WHEN EXISTS(SELECT 1 FROM unnest(ix.indcollation) WITH ORDINALITY co(coll,ord) JOIN unnest(ix.indkey) WITH ORDINALITY k(attnum,ord) USING(ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.attnum WHERE co.coll<>a.attcollation) THEN 'index custom collation' END
 ]::text[],NULL))))::text AS catalog_record
FROM pg_catalog.pg_index ix JOIN pg_catalog.pg_class c ON c.oid=ix.indrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_class ic ON ic.oid=ix.indexrelid JOIN pg_catalog.pg_am am ON am.oid=ic.relam
WHERE n.nspname IN (__SCOPE__) AND (__RELATION_FILTER__) ORDER BY n.nspname,c.relname,ic.relname LIMIT __LIMIT__;
__SCHEMA_OBJECTS__
ROLLBACK;
