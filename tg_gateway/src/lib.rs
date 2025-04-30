//! tg_gateway – взаимодействие с Telegram (teloxide 0.12)
//! -----------------------------------------------------
//! MVP: отправка/скачка одного чанка (<= 49 MiB) и получение `file_id`.
//!
//! **Cargo.toml (tg_gateway)**
//! ```toml
//! [dependencies]
//! teloxide = { version = "0.12", features = ["macros"] }
//! tokio    = { version = "1", features = ["rt-multi-thread", "macros"] }
//! serde    = { version = "1.0", features = ["derive"] }
//! thiserror = "1"
//! tg_core  = { path = "../core" }
//! ```

use teloxide::{
    net::Download, prelude::*, requests::Payload, types::InputFile, Bot, DownloadError,
};
use tg_core::Chunk;
use thiserror::Error;
#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("telegram: {0}")]
    Telegram(#[from] teloxide::RequestError),
    #[error("download: {0}")]
    Download(#[from] DownloadError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct Gateway {
    bot: Bot,
    chat_id: ChatId,
}

impl Gateway {
    /// `api_url` – например, `http://localhost:8081` для локального Bot API.
    pub fn new(bot_token: &str, chat_id: i64, api_url: Option<&str>) -> Result<Self, GatewayError> {
        let client = reqwest::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|e| GatewayError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?; // или сделай свой вариант

        let mut bot = Bot::with_client(bot_token, client);
        if let Some(url) = api_url {
            bot = bot.set_api_url(url.parse().unwrap());
        }
        Ok(Self {
            bot,
            chat_id: ChatId(chat_id),
        })
    }

    /// Загружаем один `Chunk`, получаем `file_id`.
    pub async fn upload_chunk(&self, chunk: &Chunk) -> Result<String, GatewayError> {
        let form = InputFile::memory(chunk.data.clone()).file_name("part.bin");
        let msg = self
            .bot
            .send_document(self.chat_id, form)
            .disable_notification(true)
            .await?;
        let file_id = msg.document().unwrap().file.id.clone();
        Ok(file_id)
    }
    pub async fn upload_bytes(&self, bytes: Vec<u8>) -> Result<String, GatewayError> {
        use std::time::Duration;
        use tokio::time::sleep;

        let form = InputFile::memory(bytes).file_name("part.bin");
        let mut attempt = 0;
        loop {
            attempt += 1;
            sleep(Duration::from_secs_f32(1.2)).await;
            let send_result = self
                .bot
                .send_document(self.chat_id, form.clone())
                .disable_notification(true)
                .await;

            match send_result {
                Ok(msg) => {
                    return Ok(msg.document().unwrap().file.id.clone());
                }
                Err(teloxide::RequestError::Api(api_err)) => {
                    println!("Telegram API error: {}", api_err);
                    // Try to parse "Retry after Xs" from the error description
                    let desc = api_err.to_string();
                    println!("{}", desc);
                    if let Some(pos) = desc.find("Retry after ") {
                        println!("Retry after detected in Telegram API error: {}", desc);
                        let after_str = &desc[pos + "Retry after ".len()..];
                        let mut secs = 0u64;
                        for c in after_str.chars() {
                            if c.is_digit(10) {
                                secs = secs * 10 + c.to_digit(10).unwrap() as u64;
                            } else {
                                break;
                            }
                        }
                        println!("Retry after {} seconds, sleeping...", secs);
                        if secs > 0 {
                            // Optionally, print/log the wait
                            // eprintln!("Telegram rate limit hit, waiting {} seconds...", secs);
                            sleep(Duration::from_secs(secs)).await;
                            continue;
                        }
                    }
                    // If not a retry-after error, return as usual
                    return Err(GatewayError::Telegram(teloxide::RequestError::Api(api_err)));
                }
                Err(e) => {
                    println!("Other error in upload_bytes: {:?}", e);
                    return Err(GatewayError::Telegram(e));
                }
            }
        }
    }

    /// Скачиваем чанк по `file_id`.
    pub async fn download_chunk(&self, file_id: &str) -> Result<Vec<u8>, GatewayError> {
        let file = self.bot.get_file(file_id).await?;
        let path = &file.path; // ← строка вида "C:/Users/…/file_2.bin"
                               // ──▶ локальный Bot API (режим --local) отдаёт абсолютный путь на диске
        if path.contains(":\\") {
            // простая проверка Windows-пути
            return Ok(std::fs::read(path)?); // читаем файл напрямую
        }

        // иначе обычный облачный Bot API
        let mut buf = Vec::<u8>::new();
        self.bot.download_file(path, &mut buf).await?;
        Ok(buf)
    }
}

/* ---------------------------------------------------------------------
 * Интеграционный тест (нужны BOT_TOKEN + CHAT_ID)
 * ------------------------------------------------------------------*/

#[cfg(test)]
mod tests {
    use super::*;
    use tg_core::{split_bytes, DEFAULT_CHUNK_SIZE};
    use tokio::runtime::Runtime;

    #[test]
    fn upload_download_roundtrip() {
        dotenvy::dotenv().ok();

        if std::env::var("BOT_TOKEN").is_err() {
            return;
        }
        let bot_token = std::env::var("BOT_TOKEN").unwrap();
        let chat_id: i64 = std::env::var("CHAT_ID").unwrap().parse().unwrap();
        // let api_url = std::env::var("BOT_API_URL").ok();
        let gw = Gateway::new(&bot_token, chat_id, None).unwrap();

        Runtime::new().unwrap().block_on(async move {
            let data = vec![42u8; 9000];
            let chunk = split_bytes(&data, DEFAULT_CHUNK_SIZE).pop().unwrap();
            let file_id = gw.upload_chunk(&chunk).await.unwrap();
            let bytes = gw.download_chunk(&file_id).await.unwrap();
            assert_eq!(bytes, data);
        });
    }
}
