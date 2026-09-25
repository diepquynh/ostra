//! Files the user uploads as context (Rule C3). A file is staged in the workspace first, because a
//! new task uploads before its session exists, then claimed into the session's `uploads/` folder.

use crate::runner::EngineError;
use ostra_core::api::UploadRef;
use ostra_core::event::UploadedFile;
use ostra_core::paths;
use std::path::Path;

/// Largest single upload.
pub const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;
/// Uploads one request or addition may claim.
pub const MAX_UPLOADS: usize = 20;

/// A file name safe to write in one folder: the last path segment, without control characters,
/// never hidden, never empty.
pub fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    let cleaned: String = cleaned.chars().take(120).collect();
    if cleaned.is_empty() {
        "upload".into()
    } else {
        cleaned
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Write an upload to the staging area.
pub fn stage(workspace: &Path, name: &str, bytes: &[u8]) -> Result<UploadRef, EngineError> {
    if bytes.is_empty() {
        return Err(EngineError::Invalid("The file is empty.".into()));
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(EngineError::Invalid(format!(
            "Upload files of at most {} MB, because every agent reads them.",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        )));
    }
    let root = paths::upload_staging(workspace);
    let id = uuid::Uuid::new_v4().simple().to_string();
    let name = safe_name(name);
    let dir = root.join(&id);
    let io = |e: std::io::Error| EngineError::Invalid(format!("Saving the upload failed: {e}"));
    std::fs::create_dir_all(&dir).map_err(io)?;
    let ignore = root.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(ignore, "*\n");
    }
    std::fs::write(dir.join(&name), bytes).map_err(io)?;
    Ok(UploadRef {
        id,
        name,
        size: bytes.len() as u64,
    })
}

/// Check every id names a staged upload, before anything is created.
pub fn check(workspace: &Path, ids: &[String]) -> Result<(), EngineError> {
    if ids.len() > MAX_UPLOADS {
        return Err(EngineError::Invalid(format!(
            "Upload at most {MAX_UPLOADS} files at a time."
        )));
    }
    for id in ids {
        staged_file(workspace, id)?;
    }
    Ok(())
}

fn staged_file(workspace: &Path, id: &str) -> Result<std::path::PathBuf, EngineError> {
    let missing =
        || EngineError::Invalid("An upload is missing. Upload the file again, then send.".into());
    if !valid_id(id) {
        return Err(missing());
    }
    let dir = paths::upload_staging(workspace).join(id);
    std::fs::read_dir(&dir)
        .map_err(|_| missing())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file())
        .ok_or_else(missing)
}

/// Move staged uploads into the session's `uploads/` folder. A name already taken there gets a
/// number, so no upload overwrites another.
pub fn claim(
    workspace: &Path,
    session_root: &Path,
    ids: &[String],
) -> Result<Vec<UploadedFile>, EngineError> {
    check(workspace, ids)?;
    let dest = paths::session_uploads(session_root);
    let io = |e: std::io::Error| EngineError::Invalid(format!("Keeping the upload failed: {e}"));
    let mut out = vec![];
    for id in ids {
        let src = staged_file(workspace, id)?;
        std::fs::create_dir_all(&dest).map_err(io)?;
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "upload".into());
        let mut target = dest.join(&name);
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
            _ => (name.clone(), String::new()),
        };
        let mut n = 2;
        while target.exists() {
            target = dest.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        if std::fs::rename(&src, &target).is_err() {
            std::fs::copy(&src, &target).map_err(io)?;
        }
        let _ = std::fs::remove_dir_all(src.parent().unwrap_or(&src));
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        out.push(UploadedFile {
            name: target
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or(name),
            path: target,
            size,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\Users\\me\\notes.txt"), "notes.txt");
        assert_eq!(safe_name(".env"), "env");
        assert_eq!(safe_name("a:b?.pdf"), "a_b_.pdf");
        assert_eq!(safe_name(""), "upload");
    }

    #[test]
    fn staged_uploads_move_into_the_session_without_overwriting() {
        let ws = tempfile::tempdir().unwrap();
        let session = ws.path().join("s");
        let a = stage(ws.path(), "spec.pdf", b"one").unwrap();
        let b = stage(ws.path(), "spec.pdf", b"two").unwrap();
        assert!(check(ws.path(), &["../x".into()]).is_err());
        let got = claim(ws.path(), &session, &[a.id.clone(), b.id]).unwrap();
        assert_eq!(
            got.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(),
            ["spec.pdf", "spec (2).pdf"]
        );
        assert_eq!(std::fs::read(&got[1].path).unwrap(), b"two");
        assert!(got[0].path.starts_with(paths::session_uploads(&session)));
        assert!(claim(ws.path(), &session, &[a.id]).is_err(), "claimed once");
        assert!(stage(ws.path(), "e", b"").is_err());
    }
}
