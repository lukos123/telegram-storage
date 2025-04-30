use cli::{
    download_file, download_folder, download_folder_step, download_thumbnail, get_download_folder_steps, get_upload_path_steps, upload_path, upload_path_step, DownloadArgs, UploadArgs
};
use dotenvy::dotenv;
use iced::widget::{button, column, image, row, scrollable, text, Container, ProgressBar, Space, TextInput};
use iced::window;
use iced::window::close;
use iced::Settings;
use iced::{Alignment, Application, Command, Element, Length, Subscription, Theme};
use storage::init_db;
use storage::{self, StorageError};
use tg_gateway::Gateway;

fn human_readable_size(size: i64) -> String {
    let size = size as f64;
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = size;
    let mut unit = 0;
    while size >= 1024.0 && unit < units.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:.0} {}", size, units[unit])
    } else {
        format!("{:.2} {}", size, units[unit])
    }
}

#[derive(Debug, Clone)]

enum Message {
    RefreshParentFolders,
    SelectParentFolder,
    SetParentFolders(Vec<(i64, Option<i64>, String)>),
    SetFolders(Vec<(i64, Option<i64>, String)>),
    SetFiles(Vec<(i64, String, i64, Vec<u8>)>),
    SetFilesWithThumbnail(Vec<(i64, String, i64, Vec<u8>)>, i64),
    GetFolderName,
    GetThumbnails,
    DeleteFolder(i64),
    DeleteFile(i64),
    GetAbsolutPath(String),
    RefreshAbsolutPath,
    SelectFolderNone,
    SetFolderName(String),
    SelectFolder(i64),
    RefreshFiles,
    UploadFile,
    UploadFileProgress(Vec<UploadArgs>, usize),
    DownloadFile(i64),
    DownloadFolder(i64),
    DownloadFolderProgress(Vec<DownloadArgs>, usize),
    PasswordChanged(String),
    PathChanged(String),
    SelectPathFile,
    SelectPathFolder,
    CreateFolder,
    StartUpload,
    Progress(f32), // новое сообщение для UI
    FinishUpload,
    None,
    SetAllSize(i64), // новое сообщение для установки размера
}

struct StorageGui {
    parent_folders: Vec<(i64, Option<i64>, String)>,
    folders: Vec<(i64, Option<i64>, String)>,
    selected_folder: Option<i64>,
    selected_folder_name: String,
    files: Vec<(i64, String, i64, Vec<u8>)>,
    password_input: String,
    path_input: String,
    gateway: Gateway,
    db: storage::DbPool,
    is_busy: bool,
    progress: f32,
    progress_message: String,
    all_size: Option<i64>, // новое поле для хранения размера
}

impl Application for StorageGui {
    type Message = Message;
    type Executor = iced::executor::Default;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Self::Message>) {
        let bot_token = std::env::var("BOT_TOKEN").unwrap();
        let chat_id: i64 = std::env::var("CHAT_ID").unwrap().parse().unwrap();
        let gw = Gateway::new(&bot_token, chat_id, None).unwrap();
        let db = init_db("storage.db").unwrap();

        (
            Self {
                parent_folders: vec![],
                folders: vec![],
                selected_folder: None,
                files: vec![],
                selected_folder_name: String::new(),
                password_input: String::new(),
                path_input: String::new(),
                progress_message: String::from("Hello!"),
                gateway: gw,
                db,
                is_busy: false,
                progress: 0.0,
                all_size: None,
            },
            Command::batch(vec![
                Command::perform(async {}, |_| Message::RefreshFiles),
                Command::perform(async {}, |_| Message::RefreshParentFolders),
                Command::perform(async {}, |_| Message::RefreshAbsolutPath),
            ]),
        )
    }

    fn title(&self) -> String {
        String::from("Telegram Storage GUI")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::CreateFolder => {
                let db = self.db.clone();
                let selected_folder = self.selected_folder;
                let folder_name = self.path_input.clone().to_string();

                return Command::perform(
                    async move {
                        // Prompt for folder name (for now, use a default or random name)
                        // In a real GUI, you'd want to show a dialog to enter the name.
                        let parent_id = selected_folder;
                        match storage::insert_folder(&db, parent_id,&folder_name) {
                            Ok(_folder_id) => Ok(()),
                            Err(e) => Err(e),
                        }
                    },
                    |_| Message::RefreshFiles,
                );

            }
            Message::StartUpload => {
                self.is_busy = true;
                self.progress = 0.0;
            }
            Message::Progress(p) => {
                self.progress = p;
            }
            Message::FinishUpload => {
                self.is_busy = false;
                self.progress = 1.0;
                return Command::perform(async {}, |_| Message::RefreshFiles);
            }
            Message::DeleteFile(file_id) => {
                let db = self.db.clone();
                return Command::perform(
                    async move {
                        cli::delete_file(&db, file_id)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    |_| Message::RefreshFiles,
                );
            }
            Message::DeleteFolder(folder_id) => {
                let db = self.db.clone();
                return Command::perform(
                    async move {
                        cli::delete_folder(&db, folder_id)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    |_| Message::RefreshFiles,
                );
            }
            Message::RefreshAbsolutPath => {
                let db = self.db.clone();
                let selected_folder = self.selected_folder;
                return Command::perform(
                    async move {
                        if let Some(folder_id) = selected_folder {
                            let path = storage::get_absolut_path_by_folder(&db, folder_id)?;
                            Ok(path)
                        } else {
                            Ok(String::new())
                        }
                    },
                    |res: Result<String, StorageError>| match res {
                        Ok(path) => Message::GetAbsolutPath(path),
                        Err(_) => Message::None,
                    },
                );
            }
            Message::GetAbsolutPath(path) => {
                if path.is_empty() {
                    self.selected_folder_name = String::from("root");
                    return Command::none();
                }
                self.selected_folder_name = path;
            }
            Message::RefreshParentFolders => {
                // TODO: call storage::list_folders and update self.folders
                let selected_folder = self.selected_folder;
                return Command::perform(
                    async move {
                        let db = init_db("storage.db")?;
                        // Replace with your actual folder listing logic
                        let parent_id = if let Some(selected) = selected_folder {
                            storage::get_parent(&db, selected)?
                        } else {
                            None
                        };
                        let folders = storage::list_folders_by_parent_folder_id(&db, parent_id)?;
                        Ok(folders)
                    },
                    |res: Result<Vec<(i64, Option<i64>, String)>, StorageError>| match res {
                        Ok(folders) => Message::SetParentFolders(folders),
                        Err(_) => Message::None, // or handle error as you wish
                    },
                );
            }
            Message::SetParentFolders(folders) => {
                self.parent_folders = folders;
            }
            Message::SelectFolder(id) => {
                self.selected_folder = Some(id);
                // Обновляем и папки, и файлы, и размер
                return Command::batch(vec![
                    Command::perform(async {}, |_| Message::RefreshFiles),
                    Command::perform(async {}, |_| Message::RefreshParentFolders),
                    Command::perform(async {}, |_| Message::RefreshAbsolutPath),
                    {
                        let db = self.db.clone();
                        Command::perform(async move {
                            let size = storage::get_all_size(&db, id).unwrap_or(0);
                            size
                        }, Message::SetAllSize)
                    },
                ]);
            }
            Message::SelectParentFolder => {
                let selected_folder = self.selected_folder;
                return Command::perform(
                    async move {
                        let db = init_db("storage.db")?;
                        let selected_folder = selected_folder;
                        if let Some(folder_id) = selected_folder {
                            // Get the parent of the currently selected folder
                            match storage::get_parent(&db, folder_id) {
                                Ok(Some(parent_id)) => Ok(parent_id),
                                Ok(None) => Ok(-1), // Already at root, no parent
                                Err(e) => Err(e),
                            }
                        } else {
                            Err(StorageError::NotFound) // No folder selected
                        }
                    },
                    |res: Result<i64, StorageError>| match res {
                        Ok(id) => {
                            if id == -1 {
                                return Message::SelectFolderNone;
                            }
                            Message::SelectFolder(id)
                        }
                        Err(_) => Message::None, // or handle error as you wish
                    },
                );
            }
            Message::SelectFolderNone => {
                self.selected_folder = None;
                self.selected_folder_name.clear();
                self.all_size = None;
                return Command::batch(vec![
                    Command::perform(async {}, |_| Message::RefreshFiles),
                    Command::perform(async {}, |_| Message::RefreshParentFolders),
                ]);
            }
            Message::GetFolderName => {
                let folder_id = self.selected_folder.unwrap();
                return Command::perform(
                    async move {
                        let db = init_db("storage.db")?;
                        let name = storage::get_folder_name(&db, folder_id)?;
                        Ok(name)
                    },
                    |res: Result<String, StorageError>| match res {
                        Ok(name) => Message::SetFolderName(name),
                        Err(_) => Message::None, // or handle error as you wish
                    },
                );
            }
            Message::SetFolderName(new_name) => {
                self.selected_folder_name = new_name;
            }
            Message::RefreshFiles => {
                let selected_folder = self.selected_folder;
                if let Some(folder_id) = selected_folder {
                    self.files.clear();
                }
                let db1 = self.db.clone();
                let db2 = self.db.clone();
                let db3 = self.db.clone();
                let selected_folder_for_size = self.selected_folder;
                return Command::batch(vec![
                    Command::perform(
                        async move {
                            let folders =
                                storage::list_folders_by_parent_folder_id(&db1, selected_folder)?;
                            Ok(folders)
                        },
                        |res: Result<Vec<(i64, Option<i64>, String)>, StorageError>| match res {
                            Ok(folders) => Message::SetFolders(folders),
                            Err(_) => Message::None, // or handle error as you wish
                        },
                    ),
                    Command::perform(
                        async move {
                            let files = match storage::get_files_by_folder_id(&db2, selected_folder)
                            {
                                Ok(files) => files,
                                Err(e) => {
                                    println!("get_files_by_parent_id error: {:?}", e);
                                    return Err(e.into());
                                }
                            };
                            Ok(files)
                        },
                        |res: Result<Vec<(i64, String, i64, Vec<u8>)>, StorageError>| match res {
                            Ok(files) => Message::SetFiles(files),
                            Err(_) => Message::None,
                        },
                    ),
                    Command::perform(
                        async move {
                            if let Some(folder_id) = selected_folder_for_size {
                                storage::get_all_size(&db3, folder_id).unwrap_or(0)
                            } else {
                                0
                            }
                        },
                        Message::SetAllSize,
                    ),
                ]);
            }
            Message::SetFolders(folders) => {
                self.folders = folders;
            }
            Message::SetFiles(files_temp) => {
                self.files = files_temp;
                let mut files = self.files.clone();
                let password_input = self.password_input.to_string();
                let selected_folder = self.selected_folder;
                let db = self.db.clone();
                let gateway = self.gateway.clone();

                return Command::perform(
                    async move {
                        

                        // Example: Download thumbnails for each file in the folder (if password is set)
                        if !password_input.is_empty() {
                            for (file_id, _name, _size, ref mut data) in &mut files.iter_mut() {
                                match download_thumbnail(
                                    // You may need to provide a Gateway instance here; this is a placeholder
                                    &gateway, // Replace with actual Gateway instance
                                    &db,
                                    *file_id,
                                    &password_input,
                                )
                                .await
                                {
                                    Ok(Some(thumbnail_bytes)) => {
                                        *data = thumbnail_bytes;
                                    }
                                    Ok(None) => {
                                        // No thumbnail for this file
                                    }
                                    Err(e) => {
                                        println!(
                                            "download_thumbnail error for file {}: {:?}",
                                            file_id, e
                                        );
                                    }
                                }
                            }
                        }
                        Ok(files)
                    },
                    move |res: Result<Vec<(i64, String, i64, Vec<u8>)>, StorageError>| match res {
                        Ok(files) => {
                            Message::SetFilesWithThumbnail(files, selected_folder.unwrap_or(-1))
                        }
                        Err(_) => Message::None,
                    },
                );
            }
            Message::SetFilesWithThumbnail(files, folder_id) => {
                if folder_id != self.selected_folder.unwrap_or(-1) {
                    return Command::none();
                }

                self.files = files;
            }
            Message::PasswordChanged(p) => {
                self.password_input = p;
            }
            Message::PathChanged(p) => {
                self.path_input = p;
            }
            Message::UploadFile => {
                // TODO: call upload_path for path_input + password_input
                self.is_busy = true;
                self.progress = 0.0;
                self.progress_message = String::from("Начало загрузки..");
                let path = self.path_input.clone();
                let password = self.password_input.clone();
                let selected_folder = self.selected_folder;
                let db = self.db.clone();
                let args = cli::UploadArgs {
                    path,
                    password,
                    folder: selected_folder,
                };
                return Command::perform(
                    async move {
                        let mut arr: Vec<UploadArgs> = Vec::new();
                        get_upload_path_steps(&db, args, &mut arr).unwrap();

                        let total = arr.len();
                        (arr, total)
                    },
                    |temp| Message::UploadFileProgress(temp.0, temp.1),
                );
            }
            Message::UploadFileProgress(mut arr, total) => {
                if arr.len() == 0 {
                    self.is_busy = false;
                    self.progress_message =
                        String::from(format!("{} файлов было загружено", total));
                    return Command::perform(async {}, |_| Message::RefreshFiles);
                }
                self.progress = (total as f32 - arr.len() as f32) / total as f32;
                let el = arr.remove(0);
                self.progress_message = String::from(format!("Файл {} в процессе..", el.path));
                let gw = self.gateway.clone();
                let db = self.db.clone();
                return Command::perform(
                    async move {
                        use std::time::Duration;
                        use tokio::time::sleep;

                        loop {
                            match upload_path_step(&gw, &db, el.clone()).await {
                                Ok(_) => break,
                                Err(e) => {
                                    let err_str = format!("{:?}", e);
                                    println!("upload_path_step error: {:?}", e);
                                    if let Some(pos) = err_str.find("Retry after ") {
                                        // Try to parse "Retry after Xs" from the error description
                                        let after_str = &err_str[pos + "Retry after ".len()..];
                                        let mut secs = 0u64;
                                        for c in after_str.chars() {
                                            if c.is_digit(10) {
                                                secs = secs * 10 + c.to_digit(10).unwrap() as u64;
                                            } else {
                                                break;
                                            }
                                        }
                                        
                                        if secs > 0 {
                                            println!("Telegram rate limit hit, waiting {} seconds...", secs);
                                            sleep(Duration::from_secs(secs)).await;
                                            continue;
                                        }
                                    }
                                    // If not a retry-after error, just break and propagate
                                    panic!("upload_path_step error: {:?}", e);
                                }
                            }
                        }
                        arr
                    },
                    move |arr_get| Message::UploadFileProgress(arr_get, total),
                );
            }
            Message::DownloadFile(file_id) => {
                // TODO: call download_folder or download_file
                let password = self.password_input.clone();
                
                return Command::perform(
                    async move {
                        let bot_token = std::env::var("BOT_TOKEN").unwrap();
                        let chat_id: i64 = std::env::var("CHAT_ID").unwrap().parse().unwrap();
                        let gw = Gateway::new(&bot_token, chat_id, None).unwrap();
                        let db = init_db("storage.db").unwrap();
                        let mut path = String::new();
                        #[cfg(target_os = "windows")]
                        {
                            use rfd::FileDialog;
                            if let Some(path_temp) = FileDialog::new().pick_folder() {
                                path = path_temp.display().to_string();
                            }
                        }

                        let file_name = match storage::get_file_meta(&db, file_id) {
                            Ok(meta) => meta.orig_name,
                            Err(_) => "downloaded_file".to_string(),
                        };
                        let out_path = std::path::PathBuf::from(path.clone()).join(&file_name);
                        let args = cli::DownloadArgs {
                            file_id,
                            password,
                            out: out_path,
                        };
                        match download_file(&gw, &db, args).await {
                            Ok(_) => (),
                            Err(e) => {
                                println!("download_file error: {:?}", e);
                            }
                        }
                    },
                    |_| Message::RefreshFiles,
                );
            }
            Message::DownloadFolder(folder_id) => {
                // TODO: call download_folder or download_file
                self.is_busy = true;
                self.progress = 0.0;
                self.progress_message = String::from("Начало загрузки..");
                let password = self.password_input.clone();
                let gw = self.gateway.clone();
                let db =self.db.clone();
                return Command::perform(
                    async move {
                        
                        let mut path = String::new();
                        #[cfg(target_os = "windows")]
                        {
                            use rfd::FileDialog;
                            if let Some(path_temp) = FileDialog::new().pick_folder() {
                                path = path_temp.display().to_string();
                            }
                        }
                        if path.is_empty() {
                            return (Vec::new(), 0);
                        }
                        let folder_name = match storage::get_folder_name(&db, folder_id) {
                            Ok(name) => name,
                            Err(_) => "downloaded_file".to_string(),
                        };
                        let out_path = std::path::PathBuf::from(path.clone()).join(&folder_name);
                        let args: cli::DownloadFolderArgs = cli::DownloadFolderArgs {
                            folder_id,
                            password,
                            out: out_path,
                        };
                        let mut arr = Vec::new();
                        get_download_folder_steps(&gw, &db, args,&mut arr).unwrap();
                        let total = arr.len();
                        (arr,total)
                    },
                    |temp| Message::DownloadFolderProgress(temp.0,temp.1),
                );
            }
            Message::DownloadFolderProgress(mut arr, total) =>{
                if total == 0{
                    self.is_busy = false;
                    self.progress = 0.0;
                    self.progress_message = String::from("Что то не так");
                    return Command::none();
                }
                if arr.len() == 0 {
                    self.progress_message = String::from(format!("Было загружено {} файлов",total));
                    return Command::none();
                }
                self.progress = (total as f32 - arr.len() as f32) / total as f32;
                let el = arr.remove(0);
                self.progress_message = String::from(format!("Файл {} в процессе..", el.out.to_string_lossy()));
                let gw = self.gateway.clone();
                let db = self.db.clone();
                return Command::perform(
                    async move {
                        download_folder_step(&gw, &db, el).await.unwrap();
                        arr
                    },
                    move |arr_get| Message::DownloadFolderProgress(arr_get, total),
                );
            }
            Message::SelectPathFile => {
                // вызов окна для выбора папки или файла
                return Command::perform(
                    async {
                        #[cfg(target_os = "windows")]
                        {
                            use rfd::FileDialog;
                            if let Some(path) = FileDialog::new().pick_file() {
                                return path.display().to_string();
                            }
                            String::new()
                        }
                    },
                    |path| Message::PathChanged(path),
                );
            }
            Message::SelectPathFolder => {
                // вызов окна для выбора папки или файла
                return Command::perform(
                    async {
                        #[cfg(target_os = "windows")]
                        {
                            use rfd::FileDialog;
                            if let Some(path) = FileDialog::new().pick_folder() {
                                return path.display().to_string();
                            }
                            String::new()
                        }
                    },
                    |path| Message::PathChanged(path),
                );
            }
            Message::SetAllSize(size) => {
                self.all_size = Some(size);
            }
            _ => {}
        }
        Command::none()
    }

    fn view(&self) -> Element<Message> {
        let folder_parent_list = if self.selected_folder.is_none() {
            column![].spacing(10)
        } else {
            self.parent_folders.iter().fold(
                column![button(
                    text(String::from("..")).style(iced::theme::Text::Color(iced::Color::WHITE))
                )
                .on_press(Message::SelectParentFolder)
                .width(Length::Fill)
                .style(iced::theme::Button::Secondary)]
                .spacing(10),
                |col, (id, _parent, name)| {
                    let is_current = Some(*id) == self.selected_folder;
                    let btn = button(
                        text(name.clone()).style(iced::theme::Text::Color(iced::Color::WHITE)),
                    )
                    .on_press(Message::SelectFolder(*id))
                    .width(Length::Fill)
                    .style(if is_current {
                        iced::theme::Button::Primary
                    } else {
                        iced::theme::Button::Secondary
                    });
                    col.push(btn)
                },
            )
        };

        let folder_list = self.folders.iter().fold(
            column![].spacing(8),
            |col: iced::widget::Column<'_, Message>, (id, _parent, name)| {
                col.push(
                    row![
                        button(text(name.clone()))
                            .on_press(Message::SelectFolder(*id))
                            .width(Length::FillPortion(4)),
                        button("Delete")
                            .on_press(Message::DeleteFolder(*id))
                            .width(Length::FillPortion(1)),
                    ]
                    .spacing(8)
                    .width(Length::Fill),
                )
            },
        );
        let file_list = self.files.iter().fold(
            column![].spacing(10),
            |col: iced::widget::Column<'_, Message>, (id, name, size, bytes)| {
                // Определяем, является ли файл изображением по наличию байтов (если байты не равны None)
                let is_image = !bytes.is_empty();

                let mut row = row![];

                if is_image {
                    // Пытаемся загрузить изображение из файла
                    use iced::widget::image::{self, Handle};
                    let image_handle = Handle::from_memory(bytes.clone());
                    row = row.push(
                        image(image_handle)
                            .width(Length::Fixed(40.0))
                            .height(Length::Fixed(40.0)),
                    );
                }

                
                row = row.push(
                    text(format!("Name: {}", name))
                        .size(16)
                        .width(Length::FillPortion(2)),
                );

                row = row.push(
                    text(format!("Size: {} bytes", size))
                        .size(14)
                        .width(Length::Fixed(120.0)),
                );
                row = row.push(
                    button("Download")
                        .on_press(Message::DownloadFile(*id))
                        .width(Length::Fixed(120.0)),
                );
                row = row.push(
                    button("Delete")
                        .on_press(Message::DeleteFile(*id))
                        .width(Length::Fixed(80.0)),
                );
                let row = row.spacing(10);

                col.push(row)
            },
        );

        let controls = column![
            text(self.selected_folder_name.to_string())
                .size(14)
                .width(Length::Fill),
            if let Some(size) = self.all_size {
                text(format!("Total size: {}", human_readable_size(size))).size(14).width(Length::Fill)
            } else {
                text("").size(14).width(Length::Fill)
            },
            TextInput::new("Password", &self.password_input)
                .on_input(Message::PasswordChanged)
                .padding(10)
                .width(Length::FillPortion(2)),
            TextInput::new("Path", &self.path_input)
                .on_input(Message::PathChanged)
                .padding(10)
                .width(Length::FillPortion(3)),
            row![
                button("Select File").on_press(Message::SelectPathFile),
                button("Select Folder").on_press(Message::SelectPathFolder),
                button("Upload").on_press(Message::UploadFile),
                button("Download")
                    .on_press_maybe(self.selected_folder.map(Message::DownloadFolder)),
                button("Create Folder")
                    .on_press(Message::CreateFolder),
            ]
        ]
        .spacing(10)
        .align_items(Alignment::Start);

        let footer = column![
            Container::new(text(self.progress_message.to_string()))
                .width(Length::Fill)
                .padding(10),
            ProgressBar::new(0.0..=1.0, self.progress).width(Length::Fill)
        ];
        column![
            row![
                scrollable(folder_parent_list).width(Length::Fixed(200.0)),
                column![controls, scrollable(column![folder_list, file_list])]
                    .spacing(20)
                    .padding(20),
            ].height(Length::Fill)
            .padding(20),
            footer
        ]
        
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

fn main() -> iced::Result {
    dotenv().ok();
    StorageGui::run(Settings {
        window: window::Settings {
            size: (800, 600),
            resizable: true,
            ..window::Settings::default()
        },
        ..Settings::default()
    })
}
