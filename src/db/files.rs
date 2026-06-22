//! File metadata and project metadata storage.

use super::FileMetadata;
use rusqlite::Connection;

/// Upsert file metadata into the database (INSERT OR REPLACE)
pub fn upsert_file_metadata(conn: &Connection, file: &FileMetadata) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO files (file_path, content_hash, language, size_bytes, last_modified)
         VALUES (?, ?, ?, ?, ?)",
        (
            &file.file_path,
            &file.content_hash,
            &file.language,
            file.size_bytes,
            file.last_modified,
        ),
    )?;
    Ok(())
}

/// Get file metadata by file path
pub fn get_file_metadata(
    conn: &Connection,
    file_path: &str,
) -> rusqlite::Result<Option<FileMetadata>> {
    let mut stmt = conn.prepare(
        "SELECT file_path, content_hash, language, size_bytes, last_modified FROM files WHERE file_path = ?"
    )?;
    let mut rows = stmt.query([file_path])?;
    if let Some(row) = rows.next()? {
        Ok(Some(FileMetadata {
            file_path: row.get(0)?,
            content_hash: row.get(1)?,
            language: row.get(2)?,
            size_bytes: row.get(3)?,
            last_modified: row.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

/// Upsert project metadata into the database
pub fn upsert_project_metadata(
    conn: &Connection,
    key: &str,
    value: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO project_metadata (key, value) VALUES (?, ?)",
        (key, value),
    )?;
    Ok(())
}

/// Retrieve project metadata value by key
pub fn get_project_metadata(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM project_metadata WHERE key = ?")?;
    let mut rows = stmt.query([key])?;
    if let Some(row) = rows.next()? {
        let val: Option<String> = row.get(0)?;
        Ok(val)
    } else {
        Ok(None)
    }
}
