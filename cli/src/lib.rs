use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use rand::thread_rng;
use rand::RngCore; // ← bring fill_bytes into scope
use std::sync::LazyLock;
use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};
use storage::{DbPool, FileMeta, FilePart};
use tg_core::{decrypt_chunk, derive_key, encrypt_chunk, merge_chunks, Chunk, DEFAULT_CHUNK_SIZE};
use tg_gateway::Gateway;

pub static TEMP_FOLDER: LazyLock<&str> = LazyLock::new(|| "temp");
#[derive(Args, Clone, Debug)]
pub struct DownloadArgs {
    pub file_id: i64,
    #[arg(short, long)]
    pub password: String,
    #[arg(long)]
    pub out: PathBuf,
}
#[derive(Args, Clone)]
pub struct DownloadFolderArgs {
    pub folder_id: i64,
    #[arg(short, long)]
    pub password: String,
    #[arg(long)]
    pub out: PathBuf,
}
#[derive(Args, Debug, Clone)]
pub struct UploadArgs {
    pub path: String,
    #[arg(short, long)]
    pub password: String,
    #[arg(long)]
    pub folder: Option<i64>,
}
const SALT_LEN: usize = 16;
// pub fn count_files_in_real_folder(path: &Path) -> Result<usize, std::io::Error> {
//     let mut count = 0;
//     for entry_res in std::fs::read_dir(path)? {
//         let entry = entry_res?;
//         let entry_path = entry.path();
//         if entry_path.is_file() {
//             count += 1;
//         } else if entry_path.is_dir() {
//             count += count_files_in_real_folder(&entry_path)?;
//         }
//     }
//     Ok(count)
// }

pub  fn get_upload_path_steps( db: &storage::DbPool, u: UploadArgs, arr:&mut Vec<UploadArgs>  ) -> Result<()> {
    
    let path: PathBuf = PathBuf::from(&u.path);
    if path.is_file() {
        arr.push(u);
        
    } else if path.is_dir() {
        let folder_name = path
            .file_name()
            .and_then(|os_str| os_str.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid folder name"))?;
        let id_folder = storage::insert_folder(db, u.folder, folder_name)?;

        for entry_res in std::fs::read_dir(&path)? {
            let entry = entry_res?;
            let entry_path = entry.path();

            let sub_args = UploadArgs {
                path: entry_path.to_string_lossy().into(),
                password: u.password.clone(),
                folder: Some(id_folder),
            };
            
            get_upload_path_steps(db, sub_args,arr)?;
        }
    } else {
        anyhow::bail!("path not found");
    }
    Ok(())
    
}
pub async fn upload_path_step(gw: &Gateway, db: &storage::DbPool, u: UploadArgs) -> Result<()> {
    let path: PathBuf = PathBuf::from(&u.path);
    upload_single(&path, gw, db, &u.password, u.folder).await?;
    Ok(())
}
pub async fn upload_path(gw: &Gateway, db: &storage::DbPool, u: UploadArgs) -> Result<()> {
    let path: PathBuf = PathBuf::from(&u.path);
    if path.is_file() {
        upload_single(&path, gw, db, &u.password, u.folder).await?;
    } else if path.is_dir() {
        let folder_name = path
            .file_name()
            .and_then(|os_str| os_str.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid folder name"))?;
        let id_folder = storage::insert_folder(db, u.folder, folder_name)?;

        for entry_res in std::fs::read_dir(&path)? {
            let entry = entry_res?;
            let entry_path = entry.path();

            let sub_args = UploadArgs {
                path: entry_path.to_string_lossy().into(),
                password: u.password.clone(),
                folder: Some(id_folder),
            };
            Box::pin(upload_path(gw, db, sub_args)).await?;
        }
    } else {
        anyhow::bail!("path not found");
    }
    Ok(())
}
pub async fn upload_thumbnail(
    gw: &Gateway,
    db: &DbPool,
    db_file_id: i64,
    pw: &str,
    salt: &[u8; SALT_LEN],
    thumb_bytes: Vec<u8>,
    width: i32,
    height: i32,
) -> anyhow::Result<()> {
    // 1) Деривим ключ по тому же password+salt
    let key = derive_key(pw, salt)?;

    // 2) Упаковываем thumbnail как единственный чанк index=0, total=1
    let chunk = Chunk::new(0, 1, thumb_bytes);
    let (ct, nonce) = encrypt_chunk(&chunk, &key)?;

    // 3) Формируем payload = [nonce | ciphertext]
    let mut payload = Vec::with_capacity(12 + ct.len());
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&ct);
    // Сохраняем payload во временную папку с использованием переменной TEMP_FOLDER
    {
        use std::fs;
        use std::path::Path;

        // Получаем путь к временной папке из переменной TEMP_FOLDER
        let temp_folder = TEMP_FOLDER.to_string();
        let temp_dir = Path::new(&temp_folder);

        // Создаем временную папку, если она не существует
        if !temp_dir.exists() {
            let _ = fs::create_dir_all(&temp_dir);
        }

        // Формируем имя файла: thumbnail_{db_file_id}.bin
        let file_name = format!("thumbnail_{}.bin", db_file_id);
        let file_path = temp_dir.join(file_name);

        // Сохраняем payload в файл
        let _ = fs::write(&file_path, &payload);
    }

    // 4) Заливаем в Телеграм
    let tg_id = gw.upload_bytes(payload).await?;

    // 5) Пишем в БД
    storage::insert_thumbnail(
        db,
        storage::Thumbnail {
            id: 0,
            file_id: db_file_id,
            file_id_tg: tg_id,
            width: Some(width),
            height: Some(height),
        },
    )?;

    Ok(())
}

pub async fn upload_single(
    full: &Path,
    gw: &Gateway,
    db: &storage::DbPool,
    pw: &str,
    folder: Option<i64>,
) -> Result<()> {
    let meta = std::fs::metadata(full)?;
    // если это изображение, то создать миниатюру,
    let is_image = {
        let ext = full
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp"
        )
    };

    let mut thumb_width = None;
    let mut thumb_height = None;

    let total_parts = ((meta.len() as usize + DEFAULT_CHUNK_SIZE - 1) / DEFAULT_CHUNK_SIZE) as u32;

    // генерируем соль один раз на файл
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let key = derive_key(pw, &salt)?;

    // файл‑мета
    let file_row = FileMeta {
        id: 0,
        folder_id: folder,
        orig_name: full.file_name().unwrap().to_string_lossy().into(),
        size: meta.len() as i64,
        mime: None,
        hash_full: vec![],
        created_at: chrono::Utc::now().timestamp(),
    };
    let db_id = storage::insert_file(db, &file_row)?;

    let mut reader = BufReader::new(File::open(full)?);
    let mut index = 0u32;
    loop {
        let mut buf = vec![0u8; DEFAULT_CHUNK_SIZE];
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        buf.truncate(n);

        let chunk_plain = Chunk::new(index, total_parts, buf);
        let (ct, nonce) = encrypt_chunk(&chunk_plain, &key)?;

        // payload = [salt? | nonce | ciphertext]
        let mut payload = Vec::with_capacity(SALT_LEN + 12 + ct.len());
        if index == 0 {
            payload.extend_from_slice(&salt);
        }
        payload.extend_from_slice(&nonce);
        payload.extend_from_slice(&ct);

        let tg_id = gw.upload_bytes(payload).await?; // добавь в Gateway метод upload_bytes

        let part_row = FilePart {
            id: 0,
            file_id: db_id,
            part_no: index as i32,
            size: chunk_plain.data.len() as i64,
            file_id_tg: tg_id,
            sha256: chunk_plain.hash.to_vec(),
        };
        storage::insert_part(db, &part_row)?;
        println!("   uploaded {}/{}", index + 1, total_parts);
        index += 1;
    }
    println!("✅ {} uploaded (id={})", full.display(), db_id);
    if is_image {
        // Попробуем создать миниатюру (128x128, JPEG)
        if let Ok(img) = image::open(full) {
            let thumb = img.thumbnail(128, 128);
            thumb_width = Some(thumb.width() as i32);
            thumb_height = Some(thumb.height() as i32);
            let mut buf = Vec::new();
            if thumb
                .write_to(
                    &mut std::io::Cursor::new(&mut buf),
                    image::ImageOutputFormat::Jpeg(80),
                )
                .is_ok()
            {
                if let Err(e) = upload_thumbnail(
                    gw,

                    db,
                    db_id,
                    pw,
                    &salt,
                    buf,
                    thumb_width.unwrap_or(128),
                    thumb_height.unwrap_or(128),
                )
                .await
                {
                    eprintln!("⚠️ thumbnail upload failed: {}", e);
                };
            }
        }
    }
    Ok(())
}
pub async fn download_thumbnail(
    gw: &Gateway,
    db: &DbPool,
    file_id: i64,
    password: &str,
) -> Result<Option<Vec<u8>>> {
    // 1) Сначала вытаскиваем соль из первого чанка файла
    let parts = storage::list_parts(db, file_id)?;
    if parts.is_empty() {
        return Ok(None);
    }
    // скачать первый чанк (encrypted: [salt|nonce|ciphertext])
    let first = gw.download_chunk(&parts[0].file_id_tg).await?;
    let (salt_slice, rest) = first.split_at(SALT_LEN);
    let (_, _ct0) = rest.split_at(12);
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(salt_slice);

    // 2) Деривим ключ
    let key = derive_key(password, &salt)?;

    // 3) Забираем запись о миниатюре
    //    (предполагаем, что get_thumbnail возвращает Option<storage::Thumbnail>)
    let thumb = match storage::get_thumbnail_by_file_id(db, file_id)? {
        Some(t) => t,
        None => return Ok(None),
    };

    // 4) Скачиваем payload = [nonce|ciphertext]
    // если есть в кеше, то берем его, иначе скачиваем из Telegram
    let payload = {
        use std::fs;
        use std::path::Path;

        let temp_folder = TEMP_FOLDER.to_string();
        let temp_dir = Path::new(&temp_folder);
        let file_name = format!("thumbnail_{}.bin", thumb.file_id);
        let file_path = temp_dir.join(file_name);

        if file_path.exists() {
            fs::read(&file_path)?
        } else {
            let data = gw.download_chunk(&thumb.file_id_tg).await?;
            // пробуем создать папку если не существует
            if !temp_dir.exists() {
                let _ = fs::create_dir_all(&temp_dir);
            }
            let _ = fs::write(&file_path, &data);
            data
        }
    };
    let (nonce_bytes, ct) = payload.split_at(12);

    // 5) Расшифровываем
    let chunk = decrypt_chunk(
        0, // index
        1, // total
        ct,
        &nonce_bytes.try_into().unwrap(),
        &key,
    )?;

    // 6) Возвращаем чистые байты миниатюры
    Ok(Some(chunk.data))
}
/* ============================================================ */
pub async fn download_file(gw: &Gateway, db: &storage::DbPool, d: DownloadArgs) -> Result<()> {
    let parts = storage::list_parts(db, d.file_id)?;
    if parts.is_empty() {
        // Если частей нет, создаём пустой файл
        if let Some(dir) = d.out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&d.out, &[])?; // создаём пустой файл
        println!("✅ saved empty file to {}", d.out.display());
        return Ok(());
    }

    // first part → получить соль
    let first = gw.download_chunk(&parts[0].file_id_tg).await?;
    let (salt, rest) = first.split_at(SALT_LEN);
    let (nonce0, ct0) = rest.split_at(12);
    let key = derive_key(&d.password, salt)?;

    let mut chunks = Vec::with_capacity(parts.len());
    // расшифровываем первую
    let plain0 =
        tg_core::decrypt_chunk(0, parts.len() as u32, ct0, nonce0.try_into().unwrap(), &key)?;
    chunks.push(plain0);

    // остальные
    for p in &parts[1..] {
        let data = gw.download_chunk(&p.file_id_tg).await?;
        let (nonce, ct) = data.split_at(12);
        let plain = tg_core::decrypt_chunk(
            p.part_no as u32,
            parts.len() as u32,
            ct,
            nonce.try_into().unwrap(),
            &key,
        )?;
        chunks.push(plain);
        println!("downloaded part {}", p.part_no);
    }

    let merged = merge_chunks(chunks)?;
    if let Some(dir) = d.out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&d.out, merged)?;
    println!("✅ saved to {}", d.out.display());
    Ok(())
}
pub fn get_download_folder_steps(
    gw: &Gateway,
    db: &storage::DbPool,
    df: DownloadFolderArgs,
    arr:&mut Vec<DownloadArgs>
) -> Result<()> {
    let res = storage::get_folder(db, df.folder_id)?;
    let file_ids = res.0;
    let folder_ids = res.1;
    // Рекурсивно скачиваем все файлы и папки внутри данной папки
    // Сначала создаём текущую папку, если её нет
    std::fs::create_dir_all(&df.out)?;

    // Скачиваем все файлы в этой папке
    for file_id in file_ids {
        let meta = storage::get_file_meta(db, file_id)?;
        let file_args = DownloadArgs {
            file_id: meta.id,
            password: df.password.clone(),
            out: df.out.join(&meta.orig_name),
        };

        
        arr.push(file_args);
    }

    // Рекурсивно обрабатываем подпапки
    for folder_id in folder_ids {
        let mut folder_args = df.clone();
        // Имя папки нужно получить из БД
        let folder_name = storage::get_folder_name(db, folder_id)?;
        folder_args.folder_id = folder_id;
        folder_args.out = df.out.join(&folder_name);
        get_download_folder_steps(gw, db, folder_args,arr)?;
    }
    Ok(())
}
pub async fn download_folder_step(
    gw: &Gateway,
    db: &storage::DbPool,
    d: DownloadArgs,
) -> Result<()> {
    
    download_file(gw, db, d).await?;
    Ok(())
}
pub async fn download_folder(
    gw: &Gateway,
    db: &storage::DbPool,
    df: DownloadFolderArgs,
) -> Result<()> {
    let res = storage::get_folder(db, df.folder_id)?;
    let file_ids = res.0;
    let folder_ids = res.1;
    // Рекурсивно скачиваем все файлы и папки внутри данной папки
    // Сначала создаём текущую папку, если её нет
    std::fs::create_dir_all(&df.out)?;

    // Скачиваем все файлы в этой папке
    for file_id in file_ids {
        let meta = storage::get_file_meta(db, file_id)?;
        let file_args = DownloadArgs {
            file_id: meta.id,
            password: df.password.clone(),
            out: df.out.join(&meta.orig_name),
        };

        download_file(gw, db, file_args).await?;
    }

    // Рекурсивно обрабатываем подпапки
    for folder_id in folder_ids {
        let mut folder_args = df.clone();
        // Имя папки нужно получить из БД
        let folder_name = storage::get_folder_name(db, folder_id)?;
        folder_args.folder_id = folder_id;
        folder_args.out = df.out.join(&folder_name);
        Box::pin(download_folder(gw, db, folder_args)).await?;
    }
    Ok(())
}
pub async fn delete_file(db: &storage::DbPool, file_id: i64) -> Result<()> {
    // Remove the file record from the database
    storage::delete_file(db, file_id)?;
    Ok(())
}
pub async fn delete_folder(db: &storage::DbPool, folder_id: i64) -> Result<()> {
    // Get all files and subfolders in this folder
    let (file_ids, subfolder_ids) = storage::get_folder(db, folder_id)?;

    // Delete all files in this folder
    for file_id in file_ids {
        storage::delete_file(db, file_id)?;
    }

    // Recursively delete all subfolders
    for subfolder_id in subfolder_ids {
        Box::pin(delete_folder(db, subfolder_id)).await?;
    }

    // Delete the folder itself
    storage::delete_folder(db, folder_id)?;
    Ok(())
}
