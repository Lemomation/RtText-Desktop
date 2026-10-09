#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unused_imports, dead_code, unused_variables)]

mod api;
mod config;
mod models;

use api::{decode_image_rgba, decode_slint_image, format_bubble_time, format_timestamp, parse_hex_color, SupabaseClient};
use models::*;
use slint::{ComponentHandle, Model};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

slint::include_modules!();

#[cfg(target_os = "windows")]
fn check_single_instance() -> bool {
    extern "system" {
        fn CreateMutexW(
            lp_mutex_attributes: *const std::ffi::c_void,
            b_initial_owner: i32,
            lp_name: *const u16,
        ) -> *mut std::ffi::c_void;
        fn GetLastError() -> u32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;

    let name: Vec<u16> = "Global\\RtText_Desktop_SingleInstance_Mutex\0"
        .encode_utf16()
        .collect();

    unsafe {
        let _handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return false;
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn trim_working_set() {
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn SetProcessWorkingSetSize(
            h_process: *mut std::ffi::c_void,
            minimum_working_set_size: usize,
            maximum_working_set_size: usize,
        ) -> i32;
    }
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

fn create_tray_icon() -> Result<tray_icon::Icon, Box<dyn std::error::Error>> {
    let img_bytes = include_bytes!("../assets/icons/bead.png");
    let img = image::load_from_memory(img_bytes)?.to_rgba8();
    let resized = image::imageops::resize(&img, 32, 32, image::imageops::FilterType::Lanczos3);
    let (width, height) = resized.dimensions();
    let rgba = resized.into_raw();

    Ok(tray_icon::Icon::from_rgba(rgba, width, height)?)
}

fn get_app_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "Lemomation", "RtText")
        .map(|dirs| dirs.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load_saved_session() -> Option<SessionData> {
    let path = get_app_dir().join("session.json");
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_session(session: &SessionData) {
    let dir = get_app_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(data) = serde_json::to_string(session) {
        let _ = std::fs::write(dir.join("session.json"), data);
    }
}

fn delete_session() {
    let _ = std::fs::remove_file(get_app_dir().join("session.json"));
}

fn load_saved_accent() -> Option<(String, String)> {
    let path = get_app_dir().join("settings.json");
    let content = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    let id = val.get("accent_id")?.as_str()?.to_string();
    let hex = val.get("accent_hex")?.as_str()?.to_string();
    Some((id, hex))
}

fn save_accent(accent_id: &str, accent_hex: &str) {
    let dir = get_app_dir();
    let _ = std::fs::create_dir_all(&dir);
    let data = serde_json::json!({
        "accent_id": accent_id,
        "accent_hex": accent_hex,
    });
    let _ = std::fs::write(dir.join("settings.json"), data.to_string());
}

#[cfg(target_os = "windows")]
fn register_custom_protocol() {
    if let Ok(exe_path) = std::env::current_exe() {
        let exe_str = exe_path.to_string_lossy().to_string();
        let cmd = format!("\"{}\" \"%1\"", exe_str);

        let _ = std::process::Command::new("reg")
            .args(["add", "HKCU\\Software\\Classes\\io.supabase.flutterdeepauth", "/ve", "/d", "URL:RtText Protocol", "/f"])
            .status();
        let _ = std::process::Command::new("reg")
            .args(["add", "HKCU\\Software\\Classes\\io.supabase.flutterdeepauth", "/v", "URL Protocol", "/d", "", "/f"])
            .status();
        let _ = std::process::Command::new("reg")
            .args(["add", "HKCU\\Software\\Classes\\io.supabase.flutterdeepauth\\shell\\open\\command", "/ve", "/d", &cmd, "/f"])
            .status();
    }
}

async fn fetch_and_build_messages(
    client: &SupabaseClient,
    cid: &str,
    my_uid: &str,
    is_bot_chat: bool,
    is_group: bool,
    bot_bubble_color: &str,
    user_accent: &str,
) -> Vec<RawMsgItem> {
    let msgs = match client.fetch_messages(cid).await {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };

    let group_members = if is_group {
        client.fetch_group_members(cid).await.unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut items = Vec::new();
    let mut prev_time: Option<chrono::DateTime<chrono::Local>> = None;
    let msgs_len = msgs.len();

    for (i, m) in msgs.into_iter().enumerate() {
        let msg_time = m.created_at.as_deref().and_then(|t| {
            chrono::DateTime::parse_from_rfc3339(t).ok().map(|dt| dt.with_timezone(&chrono::Local))
        });

        if let Some(curr) = msg_time {
            let needs_sep = match prev_time {
                None => true,
                Some(p) => {
                    if p.date_naive() != curr.date_naive() {
                        true
                    } else {
                        (curr - p).num_minutes().abs() > 30
                    }
                }
            };

            if needs_sep {
                let now = chrono::Local::now();
                let today = now.date_naive();
                let curr_date = curr.date_naive();
                let yesterday = today - chrono::Duration::days(1);

                let label = if curr_date == today {
                    "Today".to_string()
                } else if curr_date == yesterday {
                    "Yesterday".to_string()
                } else {
                    curr.format("%b %-d, %-I:%M %p").to_string()
                };

                items.push(RawMsgItem {
                    id: format!("sep_{}", m.id),
                    content: String::new(),
                    timestamp: String::new(),
                    is_me: false,
                    bubble_color_hex: String::new(),
                    media_img: None,
                    show_sender: false,
                    sender_name: String::new(),
                    is_read: false,
                    is_separator: true,
                    separator_text: label,
                });
            }
            prev_time = Some(curr);
        }

        let is_me = m.role == "user" && (m.sender_id.as_deref() == Some(my_uid) || m.sender_id.is_none());

        let is_read = if is_me {
            if is_bot_chat {
                i < msgs_len - 1
            } else {
                false
            }
        } else {
            false
        };

        let (show_sender, sender_name) = if is_group && !is_me {
            let name = if let Some(bid) = m.bot_id.as_deref() {
                group_members
                    .iter()
                    .find(|gm| gm.bot_id.as_deref() == Some(bid))
                    .and_then(|gm| gm.name.clone())
                    .unwrap_or_else(|| "Bot".to_string())
            } else if let Some(sid) = m.sender_id.as_deref() {
                group_members
                    .iter()
                    .find(|gm| gm.user_id.as_deref() == Some(sid))
                    .and_then(|gm| gm.name.clone())
                    .unwrap_or_else(|| "Member".to_string())
            } else {
                "Member".to_string()
            };
            (true, name)
        } else {
            (false, String::new())
        };

        let mut media_img = None;
        if let Some(murl) = m.media_url.as_deref() {
            if let Some(bytes) = client.fetch_and_cache_image(murl).await {
                media_img = decode_image_rgba(&bytes);
            }
        }

        let bubble = if is_me {
            user_accent.to_string()
        } else if is_bot_chat {
            bot_bubble_color.to_string()
        } else {
            "#1E2530".to_string()
        };

        let formatted_time = format_bubble_time(m.created_at.as_deref().unwrap_or(""));

        items.push(RawMsgItem {
            id: m.id,
            content: m.content.unwrap_or_default(),
            timestamp: formatted_time,
            is_me,
            bubble_color_hex: bubble,
            media_img,
            show_sender,
            sender_name,
            is_read,
            is_separator: false,
            separator_text: String::new(),
        });
    }

    items
}

fn raw_to_message_items(raw: Vec<RawMsgItem>) -> Vec<MessageItem> {
    raw.into_iter()
        .map(|m| {
            let has_media = m.media_img.is_some();
            let img = m.media_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
            let bubble = if m.is_separator {
                slint::Color::from_argb_u8(0, 0, 0, 0)
            } else {
                parse_hex_color(&m.bubble_color_hex)
            };
            MessageItem {
                id: m.id.into(),
                content: m.content.into(),
                timestamp: m.timestamp.into(),
                is_me: m.is_me,
                bubble_color: bubble,
                has_media,
                media_image: img,
                show_sender: m.show_sender,
                sender_name: m.sender_name.into(),
                is_read: m.is_read,
                is_separator: m.is_separator,
                separator_text: m.separator_text.into(),
            }
        })
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RtText Desktop initializing, backend: {}", config::CONFIG.supabase_url);

    let args: Vec<String> = std::env::args().collect();
    for arg in &args[1..] {
        if arg.starts_with("io.supabase.flutterdeepauth://") {
            if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:54321") {
                use std::io::Write;
                let req = format!(
                    "GET /auth/forward?url={} HTTP/1.1\r\nHost: localhost:54321\r\nConnection: close\r\n\r\n",
                    api::urlencoding_encode(arg)
                );
                let _ = stream.write_all(req.as_bytes());
            }
            return Ok(());
        }
    }

    #[cfg(target_os = "windows")]
    register_custom_protocol();

    #[cfg(target_os = "windows")]
    if !check_single_instance() {
        eprintln!("RtText is already running in the system tray.");
        return Ok(());
    }

    // Initialize Tokio runtime for background Supabase networking
    let rt = tokio::runtime::Runtime::new()?;
    let _rt_guard = rt.enter();

    // Initialize Supabase Client
    let client = Arc::new(SupabaseClient::new());

    // Create Main Slint Window
    let main_window = MainWindow::new()?;

    // Shared State
    let active_conv_id = Arc::new(RwLock::new(String::new()));
    let active_bot_id = Arc::new(RwLock::new(Option::<String>::None));
    let my_user_id = Arc::new(RwLock::new(String::new()));
    let conv_to_bot: Arc<RwLock<HashMap<String, Option<String>>>> = Arc::new(RwLock::new(HashMap::new()));
    let bot_to_conv: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));
    let conv_to_dm: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));
    let dm_to_conv: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));
    let conv_is_group: Arc<RwLock<HashMap<String, bool>>> = Arc::new(RwLock::new(HashMap::new()));
    let all_bots_cache: Arc<RwLock<Vec<RawBotCard>>> = Arc::new(RwLock::new(Vec::new()));
    let pending_bot_pfp: Arc<RwLock<Option<(Vec<u8>, String)>>> = Arc::new(RwLock::new(None));

    // Restore saved accent if available
    if let Some((acc_id, acc_hex)) = load_saved_accent() {
        main_window.set_user_accent_id(acc_id.into());
        main_window.set_user_accent_color(parse_hex_color(&acc_hex));
    }

    // Set app version property
    main_window.set_app_version(config::CONFIG.app_version.into());

    // Setup System Tray Menu
    let tray_menu = muda::Menu::new();
    let item_open = muda::MenuItem::new("Open RtText", true, None);
    let item_quit = muda::MenuItem::new("Exit RtText", true, None);
    let _ = tray_menu.append(&item_open);
    let _ = tray_menu.append(&item_quit);

    let open_menu_id = item_open.id().clone();
    let quit_menu_id = item_quit.id().clone();

    // Setup System Tray Icon
    let _tray_icon = if let Ok(icon) = create_tray_icon() {
        tray_icon::TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("RtText • Running in System Tray")
            .with_icon(icon)
            .build()
            .ok()
    } else {
        None
    };

    // Close button intercepts window close and hides to system tray
    main_window.window().on_close_requested(move || {
        slint::CloseRequestResponse::HideWindow
    });

    // Helper to load application data after login
    let load_app_data = {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let active_id_clone = active_conv_id.clone();
        let active_bot_clone = active_bot_id.clone();
        let conv_to_bot_clone = conv_to_bot.clone();
        let bot_to_conv_clone = bot_to_conv.clone();
        let conv_to_dm_clone = conv_to_dm.clone();
        let dm_to_conv_clone = dm_to_conv.clone();
        let conv_is_group_clone = conv_is_group.clone();
        let all_bots_clone = all_bots_cache.clone();

        Arc::new(move |uid: String| {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let active_id = active_id_clone.clone();
            let active_bot = active_bot_clone.clone();
            let conv_to_bot = conv_to_bot_clone.clone();
            let bot_to_conv = bot_to_conv_clone.clone();
            let conv_to_dm = conv_to_dm_clone.clone();
            let dm_to_conv = dm_to_conv_clone.clone();
            let conv_is_group = conv_is_group_clone.clone();
            let all_bots = all_bots_clone.clone();

            tokio::spawn(async move {
                // 1. Profile & Avatar
                if let Ok(profile) = client.fetch_profile(&uid).await {
                    let uname = profile.username.clone().unwrap_or_else(|| "User".into());
                    let beads = profile.beads.unwrap_or(0);
                    let mut avatar_img = None;
                    if let Some(a_url) = profile.avatar_url.as_deref() {
                        if let Some(bytes) = client.fetch_and_cache_image(a_url).await {
                            avatar_img = decode_image_rgba(&bytes);
                        }
                    }

                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            w.set_current_username(uname.into());
                            w.set_bead_balance(beads);
                            if let Some(ref d) = avatar_img {
                                w.set_user_has_avatar(true);
                                w.set_user_avatar_image(d.to_slint_image());
                            }
                        }
                    });
                }

                // 2. Bots Directory
                if let Ok(bots) = client.fetch_bots().await {
                    let mut bot_items = Vec::new();
                    for b in bots {
                        let mut avatar_img = None;
                        if let Some(pfp) = b.pfp_url.as_deref() {
                            if let Some(bytes) = client.fetch_and_cache_image(pfp).await {
                                avatar_img = decode_image_rgba(&bytes);
                            }
                        }

                        bot_items.push(RawBotCard {
                            id: b.id,
                            owner: b.owner,
                            name: b.name.clone(),
                            bio: b.bio.unwrap_or_default(),
                            description: b.description.unwrap_or_default(),
                            sys_prompt: b.sys_prompt.unwrap_or_default(),
                            avatar_letter: b.name.chars().next().unwrap_or('B').to_uppercase().to_string(),
                            avatar_img,
                            bubble_color: b.bubble_color.unwrap_or_else(|| "#FB7185".into()),
                        });
                    }

                    *all_bots.write().await = bot_items.clone();

                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let items: Vec<BotCardItem> = bot_items
                                .into_iter()
                                .map(|b| {
                                    let has_avatar = b.avatar_img.is_some();
                                    let img = b.avatar_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                                    BotCardItem {
                                        id: b.id.into(),
                                        name: b.name.into(),
                                        bio: b.bio.into(),
                                        avatar_letter: b.avatar_letter.into(),
                                        has_avatar,
                                        avatar_image: img,
                                        bubble_color: parse_hex_color(&b.bubble_color),
                                    }
                                })
                                .collect();
                            let model = std::rc::Rc::new(slint::VecModel::from(items));
                            w.set_bot_directory(model.into());

                            // People directory is hidden/empty by default; only populated via live search
                            let empty_people = std::rc::Rc::new(slint::VecModel::<PersonCardItem>::default());
                            w.set_people_directory(empty_people.into());
                        }
                    });
                }

                // 4. Conversations List
                if let Ok(convs) = client.fetch_conversations(&uid).await {
                    {
                        let mut c2b = conv_to_bot.write().await;
                        let mut b2c = bot_to_conv.write().await;
                        let mut c2d = conv_to_dm.write().await;
                        let mut d2c = dm_to_conv.write().await;
                        let mut c2g = conv_is_group.write().await;
                        for c in &convs {
                            c2b.insert(c.id.clone(), c.bot_id.clone());
                            if let Some(bid) = &c.bot_id {
                                b2c.insert(bid.clone(), c.id.clone());
                            }
                            if let Some(dmid) = &c.dm_user_id {
                                c2d.insert(c.id.clone(), dmid.clone());
                                d2c.insert(dmid.clone(), c.id.clone());
                            }
                            c2g.insert(c.id.clone(), c.is_group);
                        }
                    }

                    // Select first conversation by default
                    if let Some(first) = convs.first() {
                        *active_id.write().await = first.id.clone();
                        *active_bot.write().await = first.bot_id.clone();

                        let title = first.title.clone();
                        let subtitle = first.subtitle.clone();
                        let avatar_letter = first.avatar_letter.clone();
                        let is_bot = first.is_bot;
                        let bubble_color = parse_hex_color(&first.bubble_color);
                        let cid = first.id.clone();

                        let mut header_avatar = None;
                        if let Some(a_url) = first.avatar_url.as_deref() {
                            if let Some(bytes) = client.fetch_and_cache_image(a_url).await {
                                header_avatar = decode_image_rgba(&bytes);
                            }
                        }
                        let has_hdr_avatar = header_avatar.is_some();

                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_active_chat_id(cid.into());
                                w.set_active_chat_title(title.into());
                                w.set_active_chat_subtitle(subtitle.into());
                                w.set_active_chat_avatar(avatar_letter.into());
                                w.set_active_chat_has_avatar(has_hdr_avatar);
                                let img = header_avatar.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                                w.set_active_chat_avatar_image(img);
                                w.set_active_chat_is_bot(is_bot);
                                w.set_active_chat_bubble_color(bubble_color);
                            }
                        });
                    }

                    // Convert and download avatars for conversation items
                    let mut conv_items = Vec::new();
                    for c in convs {
                        let mut avatar_img = None;
                        if let Some(a_url) = c.avatar_url.as_deref() {
                            if let Some(bytes) = client.fetch_and_cache_image(a_url).await {
                                avatar_img = decode_image_rgba(&bytes);
                            }
                        }

                        conv_items.push(RawConvItem {
                            id: c.id,
                            title: c.title,
                            subtitle: c.subtitle,
                            avatar_letter: c.avatar_letter,
                            avatar_img,
                            last_message: c.last_message,
                            timestamp: c.timestamp,
                            is_bot: c.is_bot,
                            is_online: c.is_online,
                            bubble_color: c.bubble_color,
                        });
                    }

                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let items: Vec<ConversationItem> = conv_items
                                .into_iter()
                                .map(|c| {
                                    let has_avatar = c.avatar_img.is_some();
                                    let img = c.avatar_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                                    ConversationItem {
                                        id: c.id.into(),
                                        title: c.title.into(),
                                        subtitle: c.subtitle.into(),
                                        avatar_letter: c.avatar_letter.into(),
                                        has_avatar,
                                        avatar_image: img,
                                        last_message: c.last_message.into(),
                                        timestamp: c.timestamp.into(),
                                        is_bot: c.is_bot,
                                        is_online: c.is_online,
                                        bubble_color: parse_hex_color(&c.bubble_color),
                                    }
                                })
                                .collect();
                            let model = std::rc::Rc::new(slint::VecModel::from(items));
                            w.set_conversations(model.into());
                        }
                    });

                    // 5. Load Messages for Active Conversation
                    let cur_cid = active_id.read().await.clone();
                    if !cur_cid.is_empty() {
                        let is_bot = active_bot.read().await.is_some();
                        let is_grp = conv_is_group.read().await.get(&cur_cid).copied().unwrap_or(false);
                        let raw_items = fetch_and_build_messages(&client, &cur_cid, &uid, is_bot, is_grp, "#FB7185", "#F59E0B").await;
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                let items = raw_to_message_items(raw_items);
                                let model = std::rc::Rc::new(slint::VecModel::from(items));
                                w.set_messages(model.into());
                                w.invoke_scroll_to_bottom();
                            }
                        });
                    }
                }
            });
        })
    };

    // Check saved session on launch
    if let Some(session) = load_saved_session() {
        println!("Restoring saved session for user: {}", session.user_id);
        let my_user_id = my_user_id.clone();
        let client = client.clone();
        let session_clone = session.clone();
        let load_fn = load_app_data.clone();
        let window_weak = main_window.as_weak();
        tokio::spawn(async move {
            *my_user_id.write().await = session_clone.user_id.clone();
            client.set_session(&session_clone).await;
            // Verify session works
            match client.fetch_profile(&session_clone.user_id).await {
                Ok(_) => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = window_weak.upgrade() {
                            w.set_is_logged_in(true);
                        }
                    });
                    load_fn(session_clone.user_id);
                }
                Err(e) => {
                    eprintln!("Saved session invalid or expired: {}", e);
                    delete_session();
                }
            }
        });
    }

    // AUTH CALLBACKS
    // 0. Google Login Requested (Official OAuth Flow)
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let my_uid = my_user_id.clone();
        let load_fn = load_app_data.clone();

        main_window.on_google_login_requested(move || {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let my_uid = my_uid.clone();
            let load_fn = load_fn.clone();

            if let Some(w) = window_weak.upgrade() {
                w.set_auth_busy(true);
                w.set_auth_error("".into());
            }

            tokio::spawn(async move {
                match client.start_google_oauth_flow().await {
                    Ok(session) => {
                        save_session(&session);
                        *my_uid.write().await = session.user_id.clone();

                        let uid = session.user_id.clone();
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_auth_busy(false);
                                w.set_is_logged_in(true);
                                w.set_auth_error("".into());
                            }
                        });

                        load_fn(uid);
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_auth_busy(false);
                                w.set_auth_error(err_msg.into());
                            }
                        });
                    }
                }
            });
        });
    }

    // 1. Login Requested (Developer / Password Fallback)
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let my_uid = my_user_id.clone();
        let load_fn = load_app_data.clone();

        main_window.on_login_requested(move |email, password| {
            let email_str = email.to_string();
            let pass_str = password.to_string();
            let client = client.clone();
            let window_weak = window_weak.clone();
            let my_uid = my_uid.clone();
            let load_fn = load_fn.clone();

            if let Some(w) = window_weak.upgrade() {
                w.set_auth_busy(true);
                w.set_auth_error("".into());
            }

            tokio::spawn(async move {
                match client.login(&email_str, &pass_str).await {
                    Ok(session) => {
                        save_session(&session);
                        *my_uid.write().await = session.user_id.clone();

                        let uid = session.user_id.clone();
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_auth_busy(false);
                                w.set_is_logged_in(true);
                                w.set_auth_error("".into());
                            }
                        });

                        load_fn(uid);
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        let display_err = if err_msg.contains("Invalid login") {
                            "Invalid email or password. Please check your credentials.".to_string()
                        } else {
                            err_msg
                        };
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_auth_busy(false);
                                w.set_auth_error(display_err.into());
                            }
                        });
                    }
                }
            });
        });
    }

    // 2. Signup Requested
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let my_uid = my_user_id.clone();
        let load_fn = load_app_data.clone();

        main_window.on_signup_requested(move |email, password, username| {
            let email_str = email.to_string();
            let pass_str = password.to_string();
            let uname_str = username.to_string();
            let client = client.clone();
            let window_weak = window_weak.clone();
            let my_uid = my_uid.clone();
            let load_fn = load_fn.clone();

            if let Some(w) = window_weak.upgrade() {
                w.set_auth_busy(true);
                w.set_auth_error("".into());
            }

            tokio::spawn(async move {
                match client.signup(&email_str, &pass_str, &uname_str).await {
                    Ok(session) => {
                        save_session(&session);
                        *my_uid.write().await = session.user_id.clone();

                        let uid = session.user_id.clone();
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_auth_busy(false);
                                w.set_is_logged_in(true);
                                w.set_auth_error("".into());
                            }
                        });

                        load_fn(uid);
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        let display_err = if err_msg.contains("already registered") {
                            "An account with this email already exists.".to_string()
                        } else {
                            err_msg
                        };
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_auth_busy(false);
                                w.set_auth_error(display_err.into());
                            }
                        });
                    }
                }
            });
        });
    }

    // 3. Logout Requested
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let my_uid = my_user_id.clone();
        let active_id = active_conv_id.clone();

        main_window.on_logout_requested(move || {
            delete_session();
            let client = client.clone();
            let window_weak = window_weak.clone();
            let my_uid = my_uid.clone();
            let active_id = active_id.clone();

            tokio::spawn(async move {
                client.clear_session().await;
                *my_uid.write().await = String::new();
                *active_id.write().await = String::new();

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = window_weak.upgrade() {
                        w.set_is_logged_in(false);
                        w.set_conversations(std::rc::Rc::new(slint::VecModel::default()).into());
                        w.set_messages(std::rc::Rc::new(slint::VecModel::default()).into());
                        w.set_active_chat_title("".into());
                        w.set_active_chat_id("".into());
                    }
                });
            });
        });
    }

    // DISCOVER CALLBACK: Person Selected (Start/Open DM)
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let my_uid_clone = my_user_id.clone();
        let active_id = active_conv_id.clone();
        let active_bot = active_bot_id.clone();
        let dm_to_conv = dm_to_conv.clone();
        let conv_to_dm = conv_to_dm.clone();

        main_window.on_person_selected(move |person_id, person_username| {
            let pid = person_id.to_string();
            let uname = person_username.to_string();
            let client = client.clone();
            let window_weak = window_weak.clone();
            let my_uid = my_uid_clone.clone();
            let active_id = active_id.clone();
            let active_bot = active_bot.clone();
            let dm_to_conv = dm_to_conv.clone();
            let conv_to_dm = conv_to_dm.clone();

            tokio::spawn(async move {
                let uid = my_uid.read().await.clone();
                if uid.is_empty() { return; }

                if let Ok(conv) = client.get_or_create_dm_conversation(&uid, &pid).await {
                    let cid = conv.id.clone();
                    *active_id.write().await = cid.clone();
                    *active_bot.write().await = None;

                    dm_to_conv.write().await.insert(pid.clone(), cid.clone());
                    conv_to_dm.write().await.insert(cid.clone(), pid.clone());

                    let w_clone = window_weak.clone();
                    let letter = uname.chars().next().unwrap_or('U').to_uppercase().to_string();
                    let title = uname.clone();

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            w.set_active_chat_id(cid.into());
                            w.set_active_chat_title(title.into());
                            w.set_active_chat_subtitle("Direct message".into());
                            w.set_active_chat_avatar(letter.into());
                            w.set_active_chat_has_avatar(false);
                            w.set_active_chat_is_bot(false);
                            w.set_active_chat_bubble_color(parse_hex_color("#1E2530"));
                        }
                    });

                    // Fetch messages for this DM
                    let raw_items = fetch_and_build_messages(&client, &conv.id, &uid, false, false, "#1E2530", "#F59E0B").await;
                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let items = raw_to_message_items(raw_items);
                            let model = std::rc::Rc::new(slint::VecModel::from(items));
                            w.set_messages(model.into());
                            w.invoke_scroll_to_bottom();
                        }
                    });
                }
            });
        });
    }

    // UPDATE CHECKER CALLBACKS
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();

        main_window.on_check_updates_requested(move || {
            let client = client.clone();
            let window_weak = window_weak.clone();

            if let Some(w) = window_weak.upgrade() {
                w.set_update_status("checking".into());
            }

            tokio::spawn(async move {
                match client.check_latest_github_release().await {
                    Ok(rel) => {
                        let latest_tag = rel.tag_name;
                        let cur_ver = config::CONFIG.app_version;

                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                if latest_tag == cur_ver {
                                    w.set_update_status("up-to-date".into());
                                } else {
                                    w.set_update_status("available".into());
                                    w.set_latest_version_tag(latest_tag.into());
                                }
                            }
                        });
                    }
                    Err(_) => {
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_update_status("up-to-date".into());
                            }
                        });
                    }
                }
            });
        });

        main_window.on_perform_update_requested(|| {
            // Open GitHub releases download page in user's browser
            let url = format!("https://github.com/{}/releases/latest", config::CONFIG.github_repo);
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", &url])
                .spawn();
        });
    }

    // ATTACH IMAGE CALLBACK (Native Windows File Picker)
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let active_id = active_conv_id.clone();
        let my_uid = my_user_id.clone();

        main_window.on_attach_image_clicked(move || {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let active_id = active_id.clone();
            let my_uid = my_uid.clone();

            // Native Windows File Dialog
            let file_opt = rfd::FileDialog::new()
                .add_filter("Image", &["png", "jpg", "jpeg", "webp"])
                .set_title("Select Image to Send")
                .pick_file();

            if let Some(file_path) = file_opt {
                tokio::spawn(async move {
                    let cid = active_id.read().await.clone();
                    let uid = my_uid.read().await.clone();
                    if cid.is_empty() { return; }

                    if let Ok(bytes) = std::fs::read(&file_path) {
                        let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("jpg");

                        // Optimistically render in UI
                        let decoded_img = decode_image_rgba(&bytes);
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                let msgs_model = w.get_messages();
                                let mut msgs: Vec<MessageItem> = (0..msgs_model.row_count())
                                    .filter_map(|i| msgs_model.row_data(i))
                                    .collect();
                                let user_accent = w.get_user_accent_color();
                                let img = decoded_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();

                                msgs.push(MessageItem {
                                    id: format!("temp-media-{}", msgs.len() + 1).into(),
                                    content: "".into(),
                                    timestamp: "Just now".into(),
                                    is_me: true,
                                    bubble_color: user_accent,
                                    has_media: true,
                                    media_image: img,
                                    show_sender: false,
                                    sender_name: "".into(),
                                    is_read: false,
                                    is_separator: false,
                                    separator_text: "".into(),
                                });

                                let model = std::rc::Rc::new(slint::VecModel::from(msgs));
                                w.set_messages(model.into());
                                w.invoke_scroll_to_bottom();
                            }
                        });

                        // Upload to Supabase Storage and insert message
                        if let Ok(public_url) = client.upload_chat_image(&cid, bytes, ext).await {
                            let _ = client.send_message(&cid, "", Some(&uid), Some(&public_url)).await;
                        }
                    }
                });
            }
        });
    }

    // CONVERSATION SELECTED CALLBACK
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let active_id = active_conv_id.clone();
        let active_bot = active_bot_id.clone();
        let my_uid = my_user_id.clone();
        let conv_to_bot = conv_to_bot.clone();
        let bot_to_conv = bot_to_conv.clone();
        let conv_is_group = conv_is_group.clone();

        main_window.on_conversation_selected(move |id, title, subtitle, _avatar, is_bot, color| {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let active_id = active_id.clone();
            let active_bot = active_bot.clone();
            let my_uid = my_uid.clone();
            let conv_to_bot = conv_to_bot.clone();
            let bot_to_conv = bot_to_conv.clone();
            let conv_is_group = conv_is_group.clone();
            let sel_id = id.to_string();
            let bot_color_hex = format!("#{:02x}{:02x}{:02x}", color.red(), color.green(), color.blue());
            let is_bot_chat = is_bot;

            tokio::spawn(async move {
                let uid = my_uid.read().await.clone();
                if uid.is_empty() { return; }

                let (cid, bid) = if is_bot_chat {
                    let b_map = bot_to_conv.read().await;
                    if let Some(existing_cid) = b_map.get(&sel_id) {
                        (existing_cid.clone(), Some(sel_id.clone()))
                    } else {
                        let c_map = conv_to_bot.read().await;
                        if let Some(Some(existing_bid)) = c_map.get(&sel_id) {
                            (sel_id.clone(), Some(existing_bid.clone()))
                        } else {
                            drop(b_map);
                            drop(c_map);
                            if let Ok(c) = client.get_or_create_bot_conversation(&uid, &sel_id).await {
                                conv_to_bot.write().await.insert(c.id.clone(), Some(sel_id.clone()));
                                bot_to_conv.write().await.insert(sel_id.clone(), c.id.clone());
                                (c.id, Some(sel_id.clone()))
                            } else {
                                (sel_id.clone(), Some(sel_id.clone()))
                            }
                        }
                    }
                } else {
                    (sel_id.clone(), None)
                };

                *active_id.write().await = cid.clone();
                *active_bot.write().await = bid;

                let is_grp = conv_is_group.read().await.get(&cid).copied().unwrap_or(false);
                let raw_items = fetch_and_build_messages(&client, &cid, &uid, is_bot_chat, is_grp, &bot_color_hex, "#F59E0B").await;
                let w_clone = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_clone.upgrade() {
                        let items = raw_to_message_items(raw_items);
                        let model = std::rc::Rc::new(slint::VecModel::from(items));
                        w.set_messages(model.into());
                        w.invoke_scroll_to_bottom();
                    }
                });
            });
        });
    }

    // SEND MESSAGE CALLBACK
    {
        let client = client.clone();
        let window_for_send = main_window.as_weak();
        let active_id = active_conv_id.clone();
        let active_bot = active_bot_id.clone();
        let my_uid = my_user_id.clone();

        main_window.on_send_message(move |text| {
            if let Some(w) = window_for_send.upgrade() {
                let msgs_model = w.get_messages();
                let mut msgs: Vec<MessageItem> = (0..msgs_model.row_count())
                    .filter_map(|i| msgs_model.row_data(i))
                    .collect();
                let user_accent = w.get_user_accent_color();
                msgs.push(MessageItem {
                    id: format!("temp-{}", msgs.len() + 1).into(),
                    content: text.clone(),
                    timestamp: "Just now".into(),
                    is_me: true,
                    bubble_color: user_accent,
                    has_media: false,
                    media_image: slint::Image::default(),
                    show_sender: false,
                    sender_name: "".into(),
                    is_read: false,
                    is_separator: false,
                    separator_text: "".into(),
                });
                let model = std::rc::Rc::new(slint::VecModel::from(msgs));
                w.set_messages(model.into());
                w.invoke_scroll_to_bottom();
            }

            let client = client.clone();
            let window_weak = window_for_send.clone();
            let active_id = active_id.clone();
            let active_bot = active_bot.clone();
            let my_uid = my_uid.clone();
            let text_content = text.to_string();

            tokio::spawn(async move {
                let cid = active_id.read().await.clone();
                let bid = active_bot.read().await.clone();
                let uid = my_uid.read().await.clone();

                if cid.is_empty() { return; }

                // Post message
                let _ = client.send_message(&cid, &text_content, Some(&uid), None).await;

                // If bot, trigger AI reply
                if bid.is_some() {
                    match client.trigger_ai_reply(&cid, bid.as_deref()).await {
                        Ok(reply) => {
                            if let Some(content) = reply.content {
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = window_weak.upgrade() {
                                        let msgs_model = w.get_messages();
                                        let mut msgs: Vec<MessageItem> = (0..msgs_model.row_count())
                                            .filter_map(|i| msgs_model.row_data(i))
                                            .collect();
                                        let bot_bubble = w.get_active_chat_bubble_color();
                                        msgs.push(MessageItem {
                                            id: format!("reply-{}", msgs.len() + 1).into(),
                                            content: content.into(),
                                            timestamp: "Just now".into(),
                                            is_me: false,
                                            bubble_color: bot_bubble,
                                            has_media: false,
                                            media_image: slint::Image::default(),
                                            show_sender: false,
                                            sender_name: "".into(),
                                            is_read: false,
                                            is_separator: false,
                                            separator_text: "".into(),
                                        });
                                        let model = std::rc::Rc::new(slint::VecModel::from(msgs));
                                        w.set_messages(model.into());
                                        w.invoke_scroll_to_bottom();

                                        if let Some(beads) = reply.beads {
                                            w.set_bead_balance(beads);
                                        }
                                    }
                                });
                            }
                        }
                        Err(e) => {
                            let err_msg = e.to_string();
                            let display_msg = if err_msg.contains("out_of_beads") || err_msg.contains("402") {
                                "You are out of beads! Please claim your daily beads in Profile."
                            } else {
                                "AI character could not reply at this moment."
                            };
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = window_weak.upgrade() {
                                    let msgs_model = w.get_messages();
                                    let mut msgs: Vec<MessageItem> = (0..msgs_model.row_count())
                                        .filter_map(|i| msgs_model.row_data(i))
                                        .collect();
                                    msgs.push(MessageItem {
                                        id: format!("err-{}", msgs.len() + 1).into(),
                                        content: display_msg.into(),
                                        timestamp: "Just now".into(),
                                        is_me: false,
                                        bubble_color: parse_hex_color("#EF4444"),
                                        has_media: false,
                                        media_image: slint::Image::default(),
                                        show_sender: false,
                                        sender_name: "".into(),
                                        is_read: false,
                                        is_separator: false,
                                        separator_text: "".into(),
                                    });
                                    let model = std::rc::Rc::new(slint::VecModel::from(msgs));
                                    w.set_messages(model.into());
                                    w.invoke_scroll_to_bottom();
                                }
                            });
                        }
                    }
                }
            });
        });
    }

    // BOT PROFILE, EDITOR & AVATAR CALLBACKS
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let my_uid = my_user_id.clone();

        main_window.on_open_bot_profile(move |bot_id| {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let my_uid = my_uid.clone();
            let bid = bot_id.to_string();

            tokio::spawn(async move {
                let uid = my_uid.read().await.clone();
                if let Ok(Some(bot)) = client.fetch_bot_by_id(&bid).await {
                    let is_owner = bot.owner.as_deref() == Some(&uid);
                    let mut avatar_img = None;
                    if let Some(pfp) = bot.pfp_url.as_deref() {
                        if let Some(bytes) = client.fetch_and_cache_image(pfp).await {
                            avatar_img = decode_image_rgba(&bytes);
                        }
                    }
                    let has_avatar = avatar_img.is_some();
                    let letter = bot.name.chars().next().unwrap_or('B').to_uppercase().to_string();
                    let b_color_hex = bot.bubble_color.unwrap_or_else(|| "#FB7185".into());

                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let img = avatar_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                            let b_color = parse_hex_color(&b_color_hex);
                            w.set_bot_profile_id(bot.id.into());
                            w.set_bot_profile_name(bot.name.into());
                            w.set_bot_profile_bio(bot.bio.unwrap_or_default().into());
                            w.set_bot_profile_desc(bot.description.unwrap_or_default().into());
                            w.set_bot_profile_avatar_letter(letter.into());
                            w.set_bot_profile_has_avatar(has_avatar);
                            w.set_bot_profile_avatar_image(img);
                            w.set_bot_profile_bubble_color(b_color);
                            w.set_bot_profile_is_owner(is_owner);
                            w.set_show_bot_profile(true);
                        }
                    });
                }
            });
        });
    }

    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let pending_pfp = pending_bot_pfp.clone();

        main_window.on_open_bot_editor(move |id, is_edit, name, bio, desc, prompt, color| {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let pending_pfp = pending_pfp.clone();
            let bot_id = id.to_string();
            let e_name = name.to_string();
            let e_bio = bio.to_string();
            let e_desc = desc.to_string();
            let e_prompt = prompt.to_string();
            let e_color = color.to_string();

            tokio::spawn(async move {
                *pending_pfp.write().await = None;

                let mut existing_pfp_img = None;
                let mut existing_pfp_url = String::new();
                let mut actual_name = e_name.clone();
                let mut actual_bio = e_bio.clone();
                let mut actual_desc = e_desc.clone();
                let mut actual_prompt = e_prompt.clone();
                let mut actual_color = e_color.clone();

                if is_edit && !bot_id.is_empty() {
                    if let Ok(Some(bot)) = client.fetch_bot_by_id(&bot_id).await {
                        actual_name = bot.name;
                        actual_bio = bot.bio.unwrap_or_default();
                        actual_desc = bot.description.unwrap_or_default();
                        actual_prompt = bot.sys_prompt.unwrap_or_default();
                        actual_color = bot.bubble_color.unwrap_or_else(|| "#FB7185".into());
                        if let Some(purl) = bot.pfp_url {
                            existing_pfp_url = purl.clone();
                            if let Some(bytes) = client.fetch_and_cache_image(&purl).await {
                                existing_pfp_img = decode_image_rgba(&bytes);
                            }
                        }
                    }
                }

                let letter = actual_name.chars().next().unwrap_or('B').to_uppercase().to_string();
                let has_pfp = existing_pfp_img.is_some();

                let w_clone = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_clone.upgrade() {
                        let img = existing_pfp_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                        w.set_bot_editor_id(bot_id.into());
                        w.set_bot_editor_is_edit(is_edit);
                        w.set_bot_editor_name(actual_name.into());
                        w.set_bot_editor_bio(actual_bio.into());
                        w.set_bot_editor_desc(actual_desc.into());
                        w.set_bot_editor_sys_prompt(actual_prompt.into());
                        w.set_bot_editor_color_hex(actual_color.into());
                        w.set_bot_editor_has_pfp(has_pfp);
                        w.set_bot_editor_pfp_image(img);
                        w.set_bot_editor_pfp_url(existing_pfp_url.into());
                        w.set_bot_editor_avatar_letter(letter.into());
                        w.set_show_bot_editor(true);
                    }
                });
            });
        });
    }

    {
        let window_weak = main_window.as_weak();
        let pending_pfp = pending_bot_pfp.clone();

        main_window.on_pick_bot_avatar(move || {
            let window_weak = window_weak.clone();
            let pending_pfp = pending_pfp.clone();

            let file_opt = rfd::FileDialog::new()
                .add_filter("Image", &["png", "jpg", "jpeg", "webp"])
                .set_title("Select Bot Avatar")
                .pick_file();

            if let Some(file_path) = file_opt {
                if let Ok(bytes) = std::fs::read(&file_path) {
                    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_string();
                    let decoded_opt = decode_image_rgba(&bytes);

                    tokio::spawn(async move {
                        *pending_pfp.write().await = Some((bytes, ext));
                    });

                    if let Some(d) = decoded_opt {
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                w.set_bot_editor_has_pfp(true);
                                w.set_bot_editor_pfp_image(d.to_slint_image());
                            }
                        });
                    }
                }
            }
        });
    }

    {
        let client = client.clone();
        let my_uid = my_user_id.clone();
        let load_fn = load_app_data.clone();
        let pending_pfp = pending_bot_pfp.clone();

        main_window.on_save_bot(move |id, is_edit, name, bio, desc, prompt, color| {
            let client = client.clone();
            let my_uid = my_uid.clone();
            let load_fn = load_fn.clone();
            let pending_pfp = pending_pfp.clone();
            let bot_id = id.to_string();
            let b_name = name.to_string();
            let b_bio = if bio.is_empty() { None } else { Some(bio.to_string()) };
            let b_desc = if desc.is_empty() { None } else { Some(desc.to_string()) };
            let b_prompt = prompt.to_string();
            let b_color = if color.is_empty() { None } else { Some(color.to_string()) };

            tokio::spawn(async move {
                let maybe_upload = {
                    let mut p = pending_pfp.write().await;
                    p.take()
                };

                let mut uploaded_url = None;
                if let Some((bytes, ext)) = maybe_upload {
                    if let Ok(purl) = client.upload_bot_pfp(bytes, &ext).await {
                        uploaded_url = Some(purl);
                    }
                }

                let res = if is_edit {
                    client.update_bot(
                        &bot_id,
                        &b_name,
                        &b_prompt,
                        b_bio.as_deref(),
                        b_desc.as_deref(),
                        uploaded_url.as_deref(),
                        b_color.as_deref(),
                    ).await
                } else {
                    client.create_bot(
                        &b_name,
                        &b_prompt,
                        b_bio.as_deref(),
                        b_desc.as_deref(),
                        uploaded_url.as_deref(),
                        b_color.as_deref(),
                    ).await
                };

                if res.is_ok() {
                    let uid = my_uid.read().await.clone();
                    load_fn(uid);
                }
            });
        });
    }

    {
        let client = client.clone();
        let my_uid = my_user_id.clone();
        let load_fn = load_app_data.clone();

        main_window.on_delete_bot(move |id| {
            let client = client.clone();
            let my_uid = my_uid.clone();
            let load_fn = load_fn.clone();
            let bot_id = id.to_string();

            tokio::spawn(async move {
                if client.delete_bot(&bot_id).await.is_ok() {
                    let uid = my_uid.read().await.clone();
                    load_fn(uid);
                }
            });
        });
    }

    // SEARCH QUERY CHANGED CALLBACK
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let all_bots = all_bots_cache.clone();

        main_window.on_search_query_changed(move |query| {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let all_bots = all_bots.clone();
            let q = query.to_string();

            tokio::spawn(async move {
                let q_trimmed = q.trim().to_string();

                // 1. Live People Search via Supabase API ilike prefix
                if q_trimmed.is_empty() {
                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let empty = std::rc::Rc::new(slint::VecModel::<PersonCardItem>::default());
                            w.set_people_directory(empty.into());
                        }
                    });
                } else {
                    if let Ok(people) = client.search_people(&q_trimmed).await {
                        let mut raw_people = Vec::new();
                        for p in people {
                            let mut avatar_img = None;
                            if let Some(a_url) = p.avatar_url.as_deref() {
                                if let Some(bytes) = client.fetch_and_cache_image(a_url).await {
                                    avatar_img = decode_image_rgba(&bytes);
                                }
                            }
                            let uname = p.username.unwrap_or_else(|| "User".into());
                            let letter = uname.chars().next().unwrap_or('U').to_uppercase().to_string();
                            raw_people.push(RawPersonCard {
                                id: p.id,
                                username: uname,
                                subtitle: "Member of RtText".into(),
                                avatar_letter: letter,
                                avatar_img,
                            });
                        }

                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                let items: Vec<PersonCardItem> = raw_people
                                    .into_iter()
                                    .map(|p| {
                                        let has_avatar = p.avatar_img.is_some();
                                        let img = p.avatar_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                                        PersonCardItem {
                                            id: p.id.into(),
                                            username: p.username.into(),
                                            subtitle: p.subtitle.into(),
                                            avatar_letter: p.avatar_letter.into(),
                                            has_avatar,
                                            avatar_image: img,
                                            is_online: false,
                                        }
                                    })
                                    .collect();
                                let model = std::rc::Rc::new(slint::VecModel::from(items));
                                w.set_people_directory(model.into());
                            }
                        });
                    }
                }

                // 2. Characters In-Memory Filter
                let bots = all_bots.read().await.clone();
                let q_lower = q_trimmed.to_lowercase();
                let filtered: Vec<_> = if q_lower.is_empty() {
                    bots
                } else {
                    bots.into_iter()
                        .filter(|b| {
                            b.name.to_lowercase().contains(&q_lower)
                                || b.bio.to_lowercase().contains(&q_lower)
                                || b.description.to_lowercase().contains(&q_lower)
                        })
                        .collect()
                };

                let w_clone = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_clone.upgrade() {
                        let items: Vec<BotCardItem> = filtered
                            .into_iter()
                            .map(|b| {
                                let has_avatar = b.avatar_img.is_some();
                                let img = b.avatar_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                                BotCardItem {
                                    id: b.id.into(),
                                    name: b.name.into(),
                                    bio: b.bio.into(),
                                    avatar_letter: b.avatar_letter.into(),
                                    has_avatar,
                                    avatar_image: img,
                                    bubble_color: parse_hex_color(&b.bubble_color),
                                }
                            })
                            .collect();
                        let model = std::rc::Rc::new(slint::VecModel::from(items));
                        w.set_bot_directory(model.into());
                    }
                });
            });
        });
    }

    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let active_id = active_conv_id.clone();
        let active_bot = active_bot_id.clone();
        let my_uid = my_user_id.clone();
        let bot_to_conv = bot_to_conv.clone();
        let conv_to_bot = conv_to_bot.clone();

        main_window.on_bot_start_chat(move |bot_id| {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let active_id = active_id.clone();
            let active_bot = active_bot.clone();
            let my_uid = my_uid.clone();
            let bot_to_conv = bot_to_conv.clone();
            let conv_to_bot = conv_to_bot.clone();
            let bid = bot_id.to_string();

            tokio::spawn(async move {
                let uid = my_uid.read().await.clone();
                if uid.is_empty() { return; }

                if let Ok(c) = client.get_or_create_bot_conversation(&uid, &bid).await {
                    let cid = c.id.clone();
                    *active_id.write().await = cid.clone();
                    *active_bot.write().await = Some(bid.clone());
                    bot_to_conv.write().await.insert(bid.clone(), cid.clone());
                    conv_to_bot.write().await.insert(cid.clone(), Some(bid.clone()));

                    let (b_name, b_bio, b_letter, b_has_avatar, avatar_img, b_color_hex) = if let Ok(Some(b)) = client.fetch_bot_by_id(&bid).await {
                        let mut a_img = None;
                        if let Some(pfp) = b.pfp_url.as_deref() {
                            if let Some(bytes) = client.fetch_and_cache_image(pfp).await {
                                a_img = decode_image_rgba(&bytes);
                            }
                        }
                        let has_a = a_img.is_some();
                        let l = b.name.chars().next().unwrap_or('B').to_uppercase().to_string();
                        let col = b.bubble_color.unwrap_or_else(|| "#FB7185".into());
                        (b.name, b.bio.unwrap_or_default(), l, has_a, a_img, col)
                    } else {
                        ("Bot".into(), "".into(), "B".into(), false, None, "#FB7185".into())
                    };

                    let w_clone = window_weak.clone();
                    let b_col_copy = b_color_hex.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let b_avatar = avatar_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                            let b_color = parse_hex_color(&b_col_copy);
                            w.set_active_chat_id(cid.clone().into());
                            w.set_active_chat_title(b_name.into());
                            w.set_active_chat_subtitle(b_bio.into());
                            w.set_active_chat_avatar(b_letter.into());
                            w.set_active_chat_has_avatar(b_has_avatar);
                            w.set_active_chat_avatar_image(b_avatar);
                            w.set_active_chat_is_bot(true);
                            w.set_active_chat_bubble_color(b_color);
                            w.set_active_nav("chats".into());
                        }
                    });

                    // Load messages & scroll to bottom
                    let raw_items = fetch_and_build_messages(&client, &c.id, &uid, true, false, &b_color_hex, "#F59E0B").await;
                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let items = raw_to_message_items(raw_items);
                            let model = std::rc::Rc::new(slint::VecModel::from(items));
                            w.set_messages(model.into());
                            w.invoke_scroll_to_bottom();
                        }
                    });
                }
            });
        });
    }

    // PEER PROFILE CALLBACKS
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();

        main_window.on_open_peer_profile(move |peer_id, username| {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let pid = peer_id.to_string();
            let uname = username.to_string();

            tokio::spawn(async move {
                let (has_avatar, decoded_avatar, subtitle) = if let Ok(profile) = client.fetch_profile(&pid).await {
                    let mut a_img = None;
                    if let Some(a_url) = profile.avatar_url.as_deref() {
                        if let Some(bytes) = client.fetch_and_cache_image(a_url).await {
                            a_img = decode_image_rgba(&bytes);
                        }
                    }
                    let has_a = a_img.is_some();
                    (has_a, a_img, "Member of RtText".to_string())
                } else {
                    (false, None, "Member of RtText".to_string())
                };

                let letter = uname.chars().next().unwrap_or('U').to_uppercase().to_string();
                let w_clone = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_clone.upgrade() {
                        let avatar_img = decoded_avatar.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                        w.set_peer_profile_id(pid.into());
                        w.set_peer_profile_username(uname.into());
                        w.set_peer_profile_subtitle(subtitle.into());
                        w.set_peer_profile_avatar_letter(letter.into());
                        w.set_peer_profile_has_avatar(has_avatar);
                        w.set_peer_profile_avatar_image(avatar_img);
                        w.set_show_peer_profile(true);
                    }
                });
            });
        });
    }

    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let active_id = active_conv_id.clone();
        let active_bot = active_bot_id.clone();
        let my_uid = my_user_id.clone();
        let dm_to_conv = dm_to_conv.clone();
        let conv_to_dm = conv_to_dm.clone();

        main_window.on_peer_start_chat(move |peer_id| {
            let client = client.clone();
            let window_weak = window_weak.clone();
            let active_id = active_id.clone();
            let active_bot = active_bot.clone();
            let my_uid = my_uid.clone();
            let dm_to_conv = dm_to_conv.clone();
            let conv_to_dm = conv_to_dm.clone();
            let pid = peer_id.to_string();

            tokio::spawn(async move {
                let uid = my_uid.read().await.clone();
                if uid.is_empty() { return; }

                if let Ok(c) = client.get_or_create_dm_conversation(&uid, &pid).await {
                    let cid = c.id.clone();
                    *active_id.write().await = cid.clone();
                    *active_bot.write().await = None;
                    dm_to_conv.write().await.insert(pid.clone(), cid.clone());
                    conv_to_dm.write().await.insert(cid.clone(), pid.clone());

                    let (uname, letter, has_avatar, avatar_img) = if let Ok(profile) = client.fetch_profile(&pid).await {
                        let mut a_img = None;
                        if let Some(a_url) = profile.avatar_url.as_deref() {
                            if let Some(bytes) = client.fetch_and_cache_image(a_url).await {
                                a_img = decode_image_rgba(&bytes);
                            }
                        }
                        let u = profile.username.unwrap_or_else(|| "User".into());
                        let l = u.chars().next().unwrap_or('U').to_uppercase().to_string();
                        let has_a = a_img.is_some();
                        (u, l, has_a, a_img)
                    } else {
                        ("User".into(), "U".into(), false, None)
                    };

                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let img = avatar_img.as_ref().map(|d| d.to_slint_image()).unwrap_or_default();
                            w.set_active_chat_id(cid.clone().into());
                            w.set_active_chat_title(uname.into());
                            w.set_active_chat_subtitle("Direct message".into());
                            w.set_active_chat_avatar(letter.into());
                            w.set_active_chat_has_avatar(has_avatar);
                            w.set_active_chat_avatar_image(img);
                            w.set_active_chat_is_bot(false);
                            w.set_active_chat_bubble_color(parse_hex_color("#1E2530"));
                            w.set_active_nav("chats".into());
                        }
                    });

                    // Load messages & scroll to bottom
                    let raw_items = fetch_and_build_messages(&client, &c.id, &uid, false, false, "#1E2530", "#F59E0B").await;
                    let w_clone = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_clone.upgrade() {
                            let items = raw_to_message_items(raw_items);
                            let model = std::rc::Rc::new(slint::VecModel::from(items));
                            w.set_messages(model.into());
                            w.invoke_scroll_to_bottom();
                        }
                    });
                }
            });
        });
    }

    // HEADER PROFILE CLICKED CALLBACK
    {
        let window_weak = main_window.as_weak();

        main_window.on_header_profile_clicked(move || {
            if let Some(w) = window_weak.upgrade() {
                if w.get_active_chat_is_bot() {
                    let bot_id = w.get_active_chat_id().to_string();
                    let title = w.get_active_chat_title().to_string();
                    let subtitle = w.get_active_chat_subtitle().to_string();
                    let letter = w.get_active_chat_avatar().to_string();
                    let has_avatar = w.get_active_chat_has_avatar();
                    let img = w.get_active_chat_avatar_image();
                    let col = w.get_active_chat_bubble_color();

                    w.set_bot_profile_id(bot_id.into());
                    w.set_bot_profile_name(title.into());
                    w.set_bot_profile_bio(subtitle.into());
                    w.set_bot_profile_desc("".into());
                    w.set_bot_profile_avatar_letter(letter.into());
                    w.set_bot_profile_has_avatar(has_avatar);
                    w.set_bot_profile_avatar_image(img);
                    w.set_bot_profile_bubble_color(col);
                    w.set_bot_profile_is_owner(false);
                    w.set_show_bot_profile(true);
                } else {
                    let title = w.get_active_chat_title().to_string();
                    let letter = w.get_active_chat_avatar().to_string();
                    let has_avatar = w.get_active_chat_has_avatar();
                    let img = w.get_active_chat_avatar_image();

                    w.set_peer_profile_id(w.get_active_chat_id().clone());
                    w.set_peer_profile_username(title.into());
                    w.set_peer_profile_subtitle("Member of RtText".into());
                    w.set_peer_profile_avatar_letter(letter.into());
                    w.set_peer_profile_has_avatar(has_avatar);
                    w.set_peer_profile_avatar_image(img);
                    w.set_show_peer_profile(true);
                }
            }
        });
    }

    // ACCENT CHANGED CALLBACK
    main_window.on_accent_changed(|id, _color| {
        let hex = match id.as_str() {
            "teal" => "#2DD4BF",
            "violet" => "#A78BFA",
            "rose" => "#FB7185",
            "blue" => "#60A5FA",
            _ => "#FBBF24",
        };
        save_accent(id.as_str(), hex);
    });

    // Background Tray & Menu Event Polling Timer + Working Set Trimmer + Realtime Sync
    let timer = slint::Timer::default();
    let window_for_tray = main_window.as_weak();
    let sync_client = client.clone();
    let sync_active_id = active_conv_id.clone();
    let sync_my_uid = my_user_id.clone();
    let sync_conv_is_group = conv_is_group.clone();
    let mut poll_count: u32 = 0;

    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(150),
        move || {
            poll_count = poll_count.wrapping_add(1);

            // Flush unneeded memory every ~4.5 seconds
            #[cfg(target_os = "windows")]
            if poll_count % 30 == 0 {
                trim_working_set();
            }

            // Periodic message sync every ~3 seconds
            if poll_count % 20 == 0 {
                let client = sync_client.clone();
                let active_id = sync_active_id.clone();
                let my_uid = sync_my_uid.clone();
                let w_clone = window_for_tray.clone();
                let conv_is_group = sync_conv_is_group.clone();

                tokio::spawn(async move {
                    let cid = active_id.read().await.clone();
                    let uid = my_uid.read().await.clone();

                    if !cid.is_empty() && !uid.is_empty() {
                        if let Ok(msgs) = client.fetch_messages(&cid).await {
                            let is_bot = if let Some(w) = w_clone.upgrade() {
                                w.get_active_chat_is_bot()
                            } else {
                                return;
                            };

                            let is_group = conv_is_group.read().await.get(&cid).copied().unwrap_or(false);

                            if let Some(latest) = msgs.last() {
                                let has_latest = if let Some(w) = w_clone.upgrade() {
                                    let model = w.get_messages();
                                    let n = model.row_count();
                                    if n > 0 {
                                        model.row_data(n - 1).map(|m| m.id.as_str() == latest.id.as_str()).unwrap_or(false)
                                    } else {
                                        false
                                    }
                                } else {
                                    true
                                };

                                if !has_latest {
                                    let raw_items = fetch_and_build_messages(
                                        &client,
                                        &cid,
                                        &uid,
                                        is_bot,
                                        is_group,
                                        "#FB7185",
                                        "#F59E0B",
                                    ).await;

                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(w) = w_clone.upgrade() {
                                            let items = raw_to_message_items(raw_items);
                                            let model = std::rc::Rc::new(slint::VecModel::from(items));
                                            w.set_messages(model.into());
                                            w.invoke_scroll_to_bottom();
                                        }
                                    });
                                }
                            }
                        }
                    }
                });
            }

            // Process system tray menu events
            if let Ok(event) = muda::MenuEvent::receiver().try_recv() {
                if event.id == open_menu_id {
                    if let Some(w) = window_for_tray.upgrade() {
                        let _ = w.show();
                    }
                } else if event.id == quit_menu_id {
                    let _ = slint::quit_event_loop();
                }
            }

            // Process system tray icon clicks (left-click restores window)
            if let Ok(tray_event) = tray_icon::TrayIconEvent::receiver().try_recv() {
                if let tray_icon::TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    ..
                } = tray_event
                {
                    if let Some(w) = window_for_tray.upgrade() {
                        let _ = w.show();
                    }
                }
            }
        },
    );

    // Initial memory working set flush
    #[cfg(target_os = "windows")]
    trim_working_set();

    // Run main application event loop (persists in system tray even when window is hidden)
    main_window.show()?;
    slint::run_event_loop_until_quit()?;

    Ok(())
}
