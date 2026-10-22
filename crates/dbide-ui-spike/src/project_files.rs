//! File saves retain explicit project ownership even after switching workspaces.
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct SavedFile {
    pub path: PathBuf,
    pub project_root: Option<PathBuf>,
}

pub fn save_file(
    path: &Path,
    sql: &str,
    previous: Option<&Path>,
    baseline: &str,
    known_roots: &[PathBuf],
) -> Result<SavedFile, String> {
    if !crate::sql_files::is_sql_path(path) {
        return Err(
            "Choose a file name ending in .sql, then save again. No file was written.".into(),
        );
    }
    if sql.len() > dbide_project::MAX_SQL_BYTES {
        return Err("SQL file exceeds64KiB".into());
    }
    // Native Save As supplies an existing parent. Resolve aliases before choosing
    // ownership, so a symlinked external folder cannot bypass a known project.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let destination = std::fs::canonicalize(parent)
        .map_err(|e| e.to_string())?
        .join(path.file_name().ok_or("File name required")?);
    let lexical_owner = known_roots
        .iter()
        .filter(|r| path.starts_with(r))
        .max_by_key(|r| r.components().count());
    if let Some(owner) = lexical_owner
        && !destination.starts_with(owner)
    {
        return Err("Project path now resolves outside its workspace. Original preserved.".into());
    }
    // Keep the owned lexical path so project IO can also reject links retargeted
    // to another directory inside the same workspace. External aliases are only
    // normalized inward to discover an additional explicit owner.
    let destination = if lexical_owner.is_some() {
        path.to_path_buf()
    } else {
        destination
    };
    let same = previous.is_some_and(|p| p == destination);
    let root = known_roots
        .iter()
        .filter(|r| destination.starts_with(r))
        .max_by_key(|r| r.components().count());
    if let Some(root) = root {
        let project = dbide_project::Project::open(root).map_err(|e| e.to_string())?;
        let relative = destination
            .strip_prefix(project.root())
            .map_err(|_| "File is outside its workspace")?;
        let expected = same.then(|| dbide_project::digest_sql(baseline));
        let file = project
            .write_sql(relative, sql, expected.as_deref())
            .map_err(|e| format!("{}: {}", e.code(), e))?;
        return Ok(SavedFile {
            path: file.absolute_path,
            project_root: Some(root.clone()),
        });
    }
    if same {
        let current = crate::sql_files::read_sql_file(&destination).map_err(|e| e.to_string())?;
        if current.sql != baseline {
            return Err("File changed outside the editor. Original preserved; use Save As or reopen the file.".into());
        }
    }
    crate::write_atomic(&destination, |out| {
        std::io::Write::write_all(out, sql.as_bytes())
    })
    .map_err(|e| e.to_string())?;
    Ok(SavedFile {
        path: destination,
        project_root: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_save_extensions_never_create_or_replace_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["query", "query.txt"] {
            let path = dir.path().join(name);
            let error = save_file(&path, "SELECT 1;", None, "", &[]).unwrap_err();
            assert!(error.contains("ending in .sql"));
            assert!(!path.exists());
            std::fs::write(&path, "existing content").unwrap();
            assert!(save_file(&path, "SELECT 2;", None, "", &[]).is_err());
            assert_eq!(std::fs::read_to_string(path).unwrap(), "existing content");
        }
    }
    #[test]
    fn uppercase_sql_save_as_remains_saveable_after_editing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("query.SQL");
        let saved = save_file(&path, "SELECT 1;", None, "", &[]).unwrap();
        save_file(
            &saved.path,
            "SELECT 2;",
            Some(&saved.path),
            "SELECT 1;",
            &[],
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "SELECT 2;");
    }
    #[test]
    fn save_as_to_inactive_project_stays_create_only_and_keeps_owner() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let a = dbide_project::Project::init(&a, "A").unwrap();
        let b = dbide_project::Project::init(&b, "B").unwrap();
        let old = a
            .write_sql(Path::new("queries/original.sql"), "old", None)
            .unwrap();
        let existing = a
            .write_sql(Path::new("queries/other.sql"), "protected", None)
            .unwrap();
        let roots = vec![b.root().to_path_buf(), a.root().to_path_buf()];
        assert!(
            save_file(
                &existing.absolute_path,
                "changed",
                Some(&old.absolute_path),
                "old",
                &roots
            )
            .unwrap_err()
            .contains("already_exists")
        );
        assert_eq!(
            std::fs::read_to_string(&existing.absolute_path).unwrap(),
            "protected"
        );
        let saved = save_file(
            &a.root().join("queries/new.sql"),
            "new",
            Some(&old.absolute_path),
            "old",
            &roots,
        )
        .unwrap();
        assert_eq!(saved.project_root, Some(a.root().to_path_buf()));
        let external = a
            .write_sql(
                Path::new("queries/original.sql"),
                "external",
                Some(&old.sha256),
            )
            .unwrap();
        assert!(
            save_file(
                &old.absolute_path,
                "stale editor",
                Some(&old.absolute_path),
                "old",
                &roots
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(external.absolute_path).unwrap(),
            "external"
        );
    }
    #[cfg(unix)]
    #[test]
    fn changed_parent_link_never_loses_source_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let outside = dir.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let p = dbide_project::Project::init(&root, "P").unwrap();
        let file = p
            .write_sql(Path::new("queries/x.sql"), "old", None)
            .unwrap();
        std::fs::write(outside.join("x.sql"), "outside sentinel").unwrap();
        std::fs::rename(p.root().join("queries"), p.root().join("queries-old")).unwrap();
        std::os::unix::fs::symlink(&outside, p.root().join("queries")).unwrap();
        assert!(
            save_file(
                &file.absolute_path,
                "editor",
                Some(&file.absolute_path),
                "old",
                &[p.root().to_path_buf()]
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(outside.join("x.sql")).unwrap(),
            "outside sentinel"
        );
        assert_eq!(
            std::fs::read_to_string(p.root().join("queries-old/x.sql")).unwrap(),
            "old"
        );
    }
    #[cfg(unix)]
    #[test]
    fn aliased_parent_cannot_bypass_known_project_create_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let p = dbide_project::Project::init(&root, "P").unwrap();
        p.write_sql(Path::new("queries/keep.sql"), "keep", None)
            .unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(p.root().join("queries"), &alias).unwrap();
        assert!(
            save_file(
                &alias.join("keep.sql"),
                "replace",
                None,
                "",
                &[p.root().to_path_buf()]
            )
            .is_err()
        );
        assert_eq!(
            p.read_sql(Path::new("queries/keep.sql")).unwrap().sql,
            "keep"
        );
    }
}
