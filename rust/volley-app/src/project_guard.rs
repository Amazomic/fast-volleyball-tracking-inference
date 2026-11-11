//! Per-project cross-process write guard for GPU/CPU and metadata updates.
//! This is independent of any removed archive or legacy import feature.
use crate::{
    projects,
    storage::{self, ApiError},
    AppState,
};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io,
};

pub(crate) fn write_guard(state: &AppState, id: &str) -> Result<File, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Изменения проекта требуют отдельного рабочего хранилища",
        ));
    }
    storage::project_id(id)?;
    let root = projects::project_root(state);
    fs::create_dir_all(&root).map_err(|e| storage::internal(format!("Project root: {e}")))?;
    let root_metadata = fs::symlink_metadata(&root)
        .map_err(|e| storage::internal(format!("Check project root: {e}")))?;
    let canonical = root
        .canonicalize()
        .map_err(|e| storage::internal(format!("Resolve project root: {e}")))?;
    if root_metadata.file_type().is_symlink()
        || !root_metadata.is_dir()
        || !canonical.starts_with(
            &state
                .data_root
                .canonicalize()
                .map_err(|e| storage::internal(format!("Resolve data root: {e}")))?,
        )
    {
        return Err(storage::bad_request(
            "Каталог проектов вне рабочего хранилища",
        ));
    }
    let locks = root.join(".project-locks");
    if let Ok(meta) = fs::symlink_metadata(&locks) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request("Папка блокировок проекта небезопасна"));
        }
    } else {
        fs::create_dir(&locks)
            .map_err(|e| storage::internal(format!("Project lock directory: {e}")))?;
    }
    let path = locks.join(format!("{id}.lock"));
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(storage::bad_request("Файл блокировки проекта небезопасен"));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| storage::internal(format!("Open project lock: {e}")))?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(lock),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            Err(storage::conflict("Проект занят другой операцией студии"))
        }
        Err(e) => Err(storage::internal(format!("Lock project: {e}"))),
    }
}
