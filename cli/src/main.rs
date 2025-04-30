// cli/src/main.rs – рекурсивная загрузка + шифрование паролем
// ------------------------------------------------------------
// Запуск:
//   upload   <path>   --password <pwd>   [--folder <id>]   # файл или директория
//   download <fileId> --out <path>     --password <pwd>
//
// • Каждый файл режем на 19 MiB (DEFAULT_CHUNK_SIZE)  ➜  AES‑256‑GCM.
// • 16‑байтная соль + 12‑байтный nonce префиксуем к каждому чанку:
//   [ salt? (только в первом) | nonce | ciphertext ]
// • Папки создаём рекурсивно, сохраняя относительный путь.
// • WalkDir – обходим директорию.
mod lib;                // подключаем cli/src/lib.rs
use lib::{upload_path, download_file, download_folder, DownloadArgs, DownloadFolderArgs, UploadArgs};


use anyhow::{ Result};

use dotenvy::dotenv;

use clap::{ Parser, Subcommand};
use storage::{init_db};

use tg_gateway::Gateway;

const SALT_LEN: usize = 16; // первые 16 байт первого чанка

#[derive(Parser)]
#[command(author, version, about = "Telegram Storage CLI with AES‑GCM")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    Upload(UploadArgs),
    Download(DownloadArgs),
    Folder(DownloadFolderArgs),
    GetFolders,
    GetFoldersByParent {
        /// Parent folder id (optional, if not set, lists root folders)
        #[arg(long)]
        parent_id: Option<i64>,
    },
    GetFilesByFolder {
        /// Folder id (optional, if not set, lists files in root)
        #[arg(long)]
        folder_id: Option<i64>,
    },
}






#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let args = Cli::parse();

    let bot_token = std::env::var("BOT_TOKEN")?;
    let chat_id: i64 = std::env::var("CHAT_ID")?.parse()?;
    let gw = Gateway::new(&bot_token, chat_id, None)?;
    let db = init_db("storage.db")?;

    match args.cmd {
        /*──────────────────────── upload ───────────────────────*/
        
        Command::Upload(u) => upload_path(&gw, &db, u).await?,

        /*─────────────────────── download ─────────────────────*/
        
        Command::Folder(df) => download_folder(&gw, &db, df).await?,

        
        Command::Download(d) => download_file(&gw, &db, d).await?,

        
        Command::GetFolders => {
            let folders = storage::list_folders(&db)?;
            for (id, parent_id, name) in folders {
                println!("id: {}, parent_id: {:?}, name: {}", id, parent_id, name);
            }
        },

        
        Command::GetFoldersByParent { parent_id } => {
            let folders = storage::list_folders_by_parent_folder_id(&db, parent_id)?;
            for (id, parent_id, name) in folders {
                println!("{}\t{}\t{}", id, parent_id.unwrap_or_default(), name);
                
            }
        },
        
        Command::GetFilesByFolder { folder_id } => {
            let files = storage::get_files_by_folder_id(&db, folder_id)?;
            for (id, name, size, _bytes) in files {
                println!("{}\t{}\t{}", id, name, size);
            }
        },
    }
    Ok(())
}

/* ============================================================ */
