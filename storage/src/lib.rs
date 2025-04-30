//! storage crate – SQLite + r2d2 pool
//! -------------------------------------------------
//! Хранит метаданные файлов/папок и частей в БД, даёт удобный API.
//!
//! **`storage/Cargo.toml`:**
//! ```toml
//! [dependencies]
//! rusqlite        = { version = "0.31", features = ["bundled", "chrono"] }
//! r2d2            = "0.8"
//! r2d2_sqlite     = "0.24"
//! serde           = { version = "1.0", features = ["derive"] }
//! thiserror       = "1"
//! ```
//!
//! Для начала реализуем:
//! * `init_db(path)` – создаёт файл и выполняет миграцию 001
//! * `DbPool` – общедоступный пул соединений (`r2d2_sqlite::SqliteConnectionManager`)
//! * CRUD для `Folder`, `File`, `FilePart`, `Thumbnail`.
//!
//! > **Schema v1**
//! > ```sql
//! > CREATE TABLE folders (
//! >     id              INTEGER PRIMARY KEY AUTOINCREMENT,
//! >     parent_id       INTEGER REFERENCES folders(id) ON DELETE CASCADE,
//! >     name            TEXT NOT NULL,
//! >     created_at      INTEGER NOT NULL DEFAULT (strftime('%s','now'))
//! > );
//! >
//! > CREATE TABLE files (
//! >     id              INTEGER PRIMARY KEY AUTOINCREMENT,
//! >     folder_id       INTEGER REFERENCES folders(id) ON DELETE CASCADE,
//! >     orig_name       TEXT NOT NULL,
//! >     size            INTEGER NOT NULL,
//! >     mime            TEXT,
//! >     hash_full       BLOB NOT NULL,
//! >     salt BLOB NOT NULL DEFAULT x'',
//! >     created_at      INTEGER NOT NULL DEFAULT (strftime('%s','now'))
//! > );
//! >
//! > CREATE TABLE file_parts (
//! >     id              INTEGER PRIMARY KEY AUTOINCREMENT,
//! >     file_id         INTEGER REFERENCES files(id) ON DELETE CASCADE,
//! >     part_no         INTEGER NOT NULL,
//! >     size            INTEGER NOT NULL,
//! >     file_id_tg      TEXT NOT NULL,
//! >     sha256          BLOB NOT NULL,
//! >     salt BLOB NOT NULL DEFAULT x'',
//! >     UNIQUE(file_id, part_no)
//! > );
//! >
//! > CREATE TABLE thumbnails (
//! >     id              INTEGER PRIMARY KEY AUTOINCREMENT,
//! >     file_id         INTEGER REFERENCES files(id) ON DELETE CASCADE,
//! >     file_id_tg      TEXT NOT NULL,
//! >     width           INTEGER,
//! >     height          INTEGER
//! > );
//! > ```
//!
//! Ниже – минимальная реализация `lib.rs`.

use std::path::Path;

use rusqlite::{params, Connection};
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use thiserror::Error;
use serde::{Serialize, Deserialize};

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("r2d2: {0}")]
    R2d2(#[from] r2d2::Error),
    #[error("not found")]
    NotFound,    // ← добавили этот вариант
}

pub type DbPool = Pool<SqliteConnectionManager>;

pub fn init_db<P: AsRef<Path>>(path: P) -> Result<DbPool, StorageError> {
    let manager = SqliteConnectionManager::file(path)
        .with_init(|c| {
            // WAL + foreign_keys ON
            c.pragma_update(None, "journal_mode", &"WAL")?;
            c.pragma_update(None, "foreign_keys", &"ON")
        });
    let pool = Pool::builder().max_size(8).build(manager)?;

    let conn = pool.get()?;
    run_migration(&conn)?;
    Ok(pool)
}

fn run_migration(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(include_str!("migrations/001_base.sql"))
}

/* ---------------------------------------------------------------------
 *  Data models
 * ------------------------------------------------------------------*/

#[derive(Debug, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMeta {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub orig_name: String,
    pub size: i64,
    pub mime: Option<String>,
    pub hash_full: Vec<u8>,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FilePart {
    pub id: i64,
    pub file_id: i64,
    pub part_no: i32,
    pub size: i64,
    pub file_id_tg: String,
    pub sha256: Vec<u8>,
}

// thumbnail опционально, поэтому миним. набор полей
#[derive(Debug, Serialize, Deserialize)]
pub struct Thumbnail {
    pub id: i64,
    pub file_id: i64,
    pub file_id_tg: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/* ---------------------------------------------------------------------
 *  CRUD helpers (минимум для MVP)
 * ------------------------------------------------------------------*/

pub fn insert_file(pool: &DbPool, meta: &FileMeta) -> Result<i64, StorageError> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO files (folder_id, orig_name, size, mime, hash_full) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![meta.folder_id, meta.orig_name, meta.size, meta.mime, meta.hash_full],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_part(pool: &DbPool, part: &FilePart) -> Result<i64, StorageError> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO file_parts (file_id, part_no, size, file_id_tg, sha256) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![part.file_id, part.part_no, part.size, part.file_id_tg, part.sha256],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_parts(pool: &DbPool, file_id: i64) -> Result<Vec<FilePart>, StorageError> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("SELECT id, file_id, part_no, size, file_id_tg, sha256 FROM file_parts WHERE file_id = ? ORDER BY part_no")?;
    let rows = stmt.query_map(params![file_id], |row| {
        Ok(FilePart {
            id: row.get(0)?,
            file_id: row.get(1)?,
            part_no: row.get(2)?,
            size: row.get(3)?,
            file_id_tg: row.get(4)?,
            sha256: row.get(5)?,
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}
pub fn insert_folder(
    pool: &DbPool,
    parent_id: Option<i64>,
    name: &str,
) -> Result<i64, StorageError> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO folders (parent_id, name) VALUES (?1, ?2)",
        params![parent_id, name],
    )?;
    Ok(conn.last_insert_rowid())
}
pub fn get_folder(
    pool: &DbPool,
    folder_id: i64,
) -> Result<(Vec<i64>, Vec<i64>), StorageError> {
    let conn = pool.get()?;

    // Получаем id файлов в папке
    let mut stmt_files = conn.prepare("SELECT id FROM files WHERE folder_id = ?1")?;
    let file_ids_iter = stmt_files.query_map(params![folder_id], |row| row.get(0))?;
    let file_ids: Vec<i64> = file_ids_iter.filter_map(Result::ok).collect();

    // Получаем id подпапок в папке
    let mut stmt_folders = conn.prepare("SELECT id FROM folders WHERE parent_id = ?1")?;
    let folder_ids_iter = stmt_folders.query_map(params![folder_id], |row| row.get(0))?;
    let folder_ids: Vec<i64> = folder_ids_iter.filter_map(Result::ok).collect();

    Ok((file_ids, folder_ids))
}

pub fn get_file_meta(pool: &DbPool, file_id: i64) -> Result<FileMeta, StorageError> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, folder_id, orig_name, size, mime, hash_full, created_at FROM files WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![file_id])?;
    if let Some(row) = rows.next()? {
        Ok(FileMeta {
            id: row.get(0)?,
            folder_id: row.get(1)?,
            orig_name: row.get(2)?,
            size: row.get(3)?,
            mime: row.get(4)?,
            hash_full: row.get(5)?,
            created_at: row.get(6)?,
        })
    } else {
        Err(StorageError::NotFound)
    }
}
pub fn get_folder_name(pool: &DbPool, folder_id: i64) -> Result<String, StorageError> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("SELECT name FROM folders WHERE id = ?1")?;
    let mut rows = stmt.query(params![folder_id])?;
    if let Some(row) = rows.next()? {
        Ok(row.get(0)?)
    } else {
        Ok(String::from("All"))
    }
}
pub fn get_folder_info(pool: &DbPool, folder_id: i64) -> Result<(i64, Option<i64>, String), StorageError> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("SELECT id, parent_id, name FROM folders WHERE id = ?1")?;
    let mut rows = stmt.query(params![folder_id])?;
    if let Some(row) = rows.next()? {
        Ok((
            row.get(0)?, // id
            row.get(1)?, // parent_id
            row.get(2)?, // name
        ))
    } else {
        Err(StorageError::NotFound)
    }
}
/// Returns a list of folders (id, parent_id, name) that have the given parent_id.
/// If parent_id is None, returns root folders.
pub fn list_folders_by_parent_folder_id(
    pool: &DbPool,
    parent_id: Option<i64>,
) -> Result<Vec<(i64, Option<i64>, String)>, StorageError> {
    let conn = pool.get()?;
    let mut stmt = match parent_id {
        Some(_) => conn.prepare("SELECT id, parent_id, name FROM folders WHERE parent_id = ?1")?,
        None => conn.prepare("SELECT id, parent_id, name FROM folders WHERE parent_id IS NULL")?,
    };
    let mut rows = match parent_id {
        Some(pid) => stmt.query(params![pid])?,
        None => stmt.query([])?,
    };
    let mut folders = Vec::new();
    while let Some(row) = rows.next()? {
        folders.push((
            row.get(0)?, // id
            row.get(1)?, // parent_id
            row.get(2)?, // name
        ));
    }
    Ok(folders)
}

pub fn list_folders(pool: &DbPool) -> Result<Vec<(i64, Option<i64>, String)>, StorageError> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("SELECT id, parent_id, name FROM folders")?;
    let mut rows = stmt.query([])?;
    let mut folders = Vec::new();
    while let Some(row) = rows.next()? {
        folders.push((
            row.get(0)?, // id
            row.get(1)?, // parent_id
            row.get(2)?, // name
        ));
    }
    Ok(folders)
}
pub fn get_parent(pool: &DbPool, folder_id: i64) -> Result<Option<i64>, StorageError> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("SELECT parent_id FROM folders WHERE id = ?1")?;
    let mut rows = stmt.query(params![folder_id])?;
    if let Some(row) = rows.next()? {
        Ok(row.get(0)?)
    } else {
        Err(StorageError::NotFound)
    }
}
/// Returns the parent_id for a given folder, or None if the folder is root.
/// Returns StorageError::NotFound if the folder does not exist.
pub fn get_parent_for_folder(pool: &DbPool, folder_id: i64) -> Result<Option<i64>, StorageError> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("SELECT parent_id FROM folders WHERE id = ?1")?;
    let mut rows = stmt.query(params![folder_id])?;
    if let Some(row) = rows.next()? {
        Ok(row.get(0)?)
    } else {
        Err(StorageError::NotFound)
    }
}
pub fn get_files_by_folder_id(
    pool: &DbPool,
    parent_id: Option<i64>,
) -> Result<Vec<(i64, String, i64,Vec<u8>)>, StorageError> {
    let conn = pool.get()?;
    let mut stmt = match parent_id {
        Some(pid) => conn.prepare("SELECT id, orig_name, size FROM files WHERE folder_id = ?1")?,
        None => conn.prepare("SELECT id, orig_name, size FROM files WHERE folder_id IS NULL")?,
    };
    let mut rows = match parent_id {
        Some(pid) => stmt.query(params![pid])?,
        None => stmt.query([])?,
    };
    let mut files = Vec::new();
    while let Some(row) = rows.next()? {
        files.push((
            row.get(0)?, // id
            row.get(1)?, // name
            row.get(2)?, // size (вес)
            vec![]
        ));
    }
    Ok(files)
}





pub fn insert_thumbnail(pool: &DbPool, thumb: Thumbnail) -> Result<i64, StorageError> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO thumbnails (file_id, file_id_tg, width, height) VALUES (?1, ?2, ?3, ?4)",
        params![
            thumb.file_id,
            thumb.file_id_tg,
            thumb.width,
            thumb.height,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}
pub fn get_thumbnail_by_file_id(pool: &DbPool, file_id: i64) -> Result<Option<Thumbnail>, StorageError> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, file_id, file_id_tg, width, height FROM thumbnails WHERE file_id = ?1 LIMIT 1"
    )?;
    let mut rows = stmt.query(params![file_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(Thumbnail {
            id: row.get(0)?,
            file_id: row.get(1)?,
            file_id_tg: row.get(2)?,
            width: row.get(3)?,
            height: row.get(4)?,
        }))
    } else {
        Ok(None)
    }
}
pub fn get_absolut_path_by_folder(pool: &DbPool, folder_id: i64) -> Result<String, StorageError> {
    let conn = pool.get()?;
    let mut path_parts = Vec::new();
    let mut current_id = Some(folder_id);

    while let Some(fid) = current_id {
        let mut stmt = conn.prepare(
            "SELECT parent_id, name FROM folders WHERE id = ?1"
        )?;
        let mut rows = stmt.query(params![fid])?;
        if let Some(row) = rows.next()? {
            let parent_id: Option<i64> = row.get(0)?;
            let name: String = row.get(1)?;
            path_parts.push(name);
            current_id = parent_id;
        } else {
            break;
        }
    }

    path_parts.reverse();
    let path = path_parts.join("/");
    Ok(path)
}
pub fn delete_file(pool: &DbPool, file_id: i64) -> Result<(), StorageError> {
    let conn = pool.get()?;
    // Удаляем все части файла
    conn.execute("DELETE FROM file_parts WHERE file_id = ?1", params![file_id])?;
    // Удаляем миниатюры
    conn.execute("DELETE FROM thumbnails WHERE file_id = ?1", params![file_id])?;
    // Удаляем сам файл
    conn.execute("DELETE FROM files WHERE id = ?1", params![file_id])?;
    Ok(())
}

pub fn delete_folder(pool: &DbPool, folder_id: i64) -> Result<(), StorageError> {
    let conn = pool.get()?;
    // Удаляем только саму папку (строку из таблицы folders)
    conn.execute("DELETE FROM folders WHERE id = ?1", params![folder_id])?;
    Ok(())
}
/// Рекурсивно вычисляет общий размер всех файлов в папке (и подпапках)
pub fn get_all_size(pool: &DbPool, folder_id: i64) -> Result<i64, StorageError> {
    let conn = pool.get()?;

    // Получаем размер всех файлов в этой папке
    let mut stmt_files = conn.prepare("SELECT size FROM files WHERE folder_id = ?1")?;
    let file_sizes_iter = stmt_files.query_map(params![folder_id], |row| row.get::<_, i64>(0))?;
    let mut total_size: i64 = file_sizes_iter.filter_map(Result::ok).sum();

    // Получаем id всех подпапок
    let mut stmt_folders = conn.prepare("SELECT id FROM folders WHERE parent_id = ?1")?;
    let folder_ids_iter = stmt_folders.query_map(params![folder_id], |row| row.get::<_, i64>(0))?;

    for subfolder_id in folder_ids_iter.filter_map(Result::ok) {
        total_size += get_all_size(pool, subfolder_id)?;
    }

    Ok(total_size)
}




