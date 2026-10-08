#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unused_imports, dead_code, unused_variables)]

mod api;
mod config;
mod models;

use api::{format_bubble_time, parse_hex_color, SupabaseClient};
use models::*;
use slint::Model;
use std::collections::HashMap;
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
    let width: u32 = 32;
    let height: u32 = 32;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - 16.0;
            let dy = y as f32 - 16.0;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= 14.0 {
                // Amber accent: #FBBF24 (RGB: 251, 191, 36)
                rgba.extend_from_slice(&[251, 191, 36, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    Ok(tray_icon::Icon::from_rgba(rgba, width, height)?)
}

fn get_settings_file_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("com", "Lemomation", "RtText")
        .map(|dirs| dirs.config_dir().join("settings.json"))
}

fn load_saved_accent() -> Option<(String, String)> {
    let path = get_settings_file_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    let id = val.get("accent_id")?.as_str()?.to_string();
    let hex = val.get("accent_hex")?.as_str()?.to_string();
    Some((id, hex))
}

fn save_accent(accent_id: &str, accent_hex: &str) {
    if let Some(path) = get_settings_file_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let data = serde_json::json!({
            "accent_id": accent_id,
            "accent_hex": accent_hex,
        });
        let _ = std::fs::write(path, data.to_string());
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RtText Desktop initializing, backend: {}", config::CONFIG.supabase_url);

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

    // Restore saved accent if available
    if let Some((acc_id, acc_hex)) = load_saved_accent() {
        main_window.set_user_accent_id(acc_id.into());
        main_window.set_user_accent_color(parse_hex_color(&acc_hex));
    }

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

    // UI Callbacks
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let my_uid_clone = my_user_id.clone();
        main_window.on_tab_clicked(move |tab| {
            println!("Switched to tab: {}", tab);
            let client = client.clone();
            let window_weak = window_weak.clone();
            let my_uid_clone = my_uid_clone.clone();
            let tab_str = tab.to_string();

            tokio::spawn(async move {
                if tab_str == "profile" {
                    let uid = my_uid_clone.read().await.clone();
                    if !uid.is_empty() {
                        if let Ok(profile) = client.fetch_profile(&uid).await {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = window_weak.upgrade() {
                                    if let Some(uname) = profile.username {
                                        w.set_current_username(uname.into());
                                    }
                                    if let Some(beads) = profile.beads {
                                        w.set_bead_balance(beads);
                                    }
                                }
                            });
                        }
                    }
                }
            });
        });
    }

    // When a conversation is selected in the UI
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let active_id = active_conv_id.clone();
        let active_bot = active_bot_id.clone();
        let my_uid_clone = my_user_id.clone();
        let conv_to_bot = conv_to_bot.clone();
        let bot_to_conv = bot_to_conv.clone();

        main_window.on_conversation_selected(move |id, title, subtitle, _avatar, is_bot, color| {
            println!("Selected conversation {}: {} ({}, is_bot: {})", id, title, subtitle, is_bot);

            let client = client.clone();
            let window_weak = window_weak.clone();
            let active_id = active_id.clone();
            let active_bot = active_bot.clone();
            let my_uid_clone = my_uid_clone.clone();
            let conv_to_bot = conv_to_bot.clone();
            let bot_to_conv = bot_to_conv.clone();
            let sel_id = id.to_string();
            let bot_bubble_color = color;
            let is_bot_chat = is_bot;

            tokio::spawn(async move {
                let uid = my_uid_clone.read().await.clone();
                if uid.is_empty() {
                    return;
                }

                // Determine if sel_id is a conversation_id or a bot_id
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

                if let Ok(msgs) = client.fetch_messages(&cid).await {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = window_weak.upgrade() {
                            let user_accent = w.get_user_accent_color();
                            let items: Vec<MessageItem> = msgs
                                .into_iter()
                                .map(|m| {
                                    let is_me = m.role == "user"
                                        && (m.sender_id.as_deref() == Some(&uid) || m.sender_id.is_none());
                                    let bubble = if is_me {
                                        user_accent
                                    } else if is_bot_chat {
                                        bot_bubble_color
                                    } else {
                                        parse_hex_color("#1E2530")
                                    };

                                    MessageItem {
                                        id: m.id.into(),
                                        content: m.content.unwrap_or_default().into(),
                                        timestamp: format_bubble_time(
                                            m.created_at.as_deref().unwrap_or(""),
                                        )
                                        .into(),
                                        is_me,
                                        bubble_color: bubble,
                                    }
                                })
                                .collect();

                            let model = std::rc::Rc::new(slint::VecModel::from(items));
                            w.set_messages(model.into());
                        }
                    });
                }
            });
        });
    }

    // Accent changed handler: persists choice to settings
    main_window.on_accent_changed(|id, _color| {
        println!("User accent color changed to: {}", id);
        let hex = match id.as_str() {
            "teal" => "#2DD4BF",
            "violet" => "#A78BFA",
            "rose" => "#FB7185",
            "blue" => "#60A5FA",
            _ => "#FBBF24",
        };
        save_accent(id.as_str(), hex);
    });

    // Send Message Handler
    {
        let client = client.clone();
        let window_for_send = main_window.as_weak();
        let active_id = active_conv_id.clone();
        let active_bot = active_bot_id.clone();
        let my_uid_clone = my_user_id.clone();

        main_window.on_send_message(move |text| {
            println!("Send message triggered: {}", text);

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
                });
                let model = std::rc::Rc::new(slint::VecModel::from(msgs));
                w.set_messages(model.into());
            }

            let client = client.clone();
            let window_weak = window_for_send.clone();
            let active_id = active_id.clone();
            let active_bot = active_bot.clone();
            let my_uid_clone = my_uid_clone.clone();
            let text_content = text.to_string();

            tokio::spawn(async move {
                let cid = active_id.read().await.clone();
                let bid = active_bot.read().await.clone();
                let uid = my_uid_clone.read().await.clone();

                if cid.is_empty() {
                    return;
                }

                // 1. Post to Supabase messages
                let _ = client.send_message(&cid, &text_content, Some(&uid)).await;

                // 2. If talking to a bot, trigger AI reply edge function
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
                                        });
                                        let model = std::rc::Rc::new(slint::VecModel::from(msgs));
                                        w.set_messages(model.into());

                                        if let Some(beads) = reply.beads {
                                            w.set_bead_balance(beads);
                                        }
                                    }
                                });
                            }
                        }
                        Err(e) => {
                            eprintln!("AI reply error: {}", e);
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
                                    });
                                    let model = std::rc::Rc::new(slint::VecModel::from(msgs));
                                    w.set_messages(model.into());
                                }
                            });
                        }
                    }
                }
            });
        });
    }

    // INITIAL DATA LOADING TASK
    {
        let client = client.clone();
        let window_weak = main_window.as_weak();
        let my_uid_clone = my_user_id.clone();
        let active_id_clone = active_conv_id.clone();
        let active_bot_clone = active_bot_id.clone();
        let conv_to_bot_clone = conv_to_bot.clone();
        let bot_to_conv_clone = bot_to_conv.clone();

        tokio::spawn(async move {
            println!("Authenticating with Supabase...");
            match client.login_default().await {
                Ok(uid) => {
                    *my_uid_clone.write().await = uid.clone();

                    // 1. Load Profile
                    if let Ok(profile) = client.fetch_profile(&uid).await {
                        let w_clone = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                if let Some(uname) = profile.username {
                                    w.set_current_username(uname.into());
                                }
                                if let Some(beads) = profile.beads {
                                    w.set_bead_balance(beads);
                                }
                            }
                        });
                    }

                    // 2. Load Bots Directory
                    if let Ok(bots) = client.fetch_bots().await {
                        let w_clone = window_weak.clone();
                        let bot_items: Vec<BotCardItem> = bots
                            .into_iter()
                            .map(|b| BotCardItem {
                                id: b.id.into(),
                                name: b.name.clone().into(),
                                bio: b.bio.unwrap_or_default().into(),
                                avatar_letter: b
                                    .name
                                    .chars()
                                    .next()
                                    .unwrap_or('B')
                                    .to_uppercase()
                                    .to_string()
                                    .into(),
                                bubble_color: parse_hex_color(
                                    &b.bubble_color.unwrap_or_else(|| "#FB7185".into()),
                                ),
                            })
                            .collect();

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                let model = std::rc::Rc::new(slint::VecModel::from(bot_items));
                                w.set_bot_directory(model.into());
                            }
                        });
                    }

                    // 3. Load Conversations
                    if let Ok(convs) = client.fetch_conversations(&uid).await {
                        {
                            let mut c2b = conv_to_bot_clone.write().await;
                            let mut b2c = bot_to_conv_clone.write().await;
                            for c in &convs {
                                c2b.insert(c.id.clone(), c.bot_id.clone());
                                if let Some(bid) = &c.bot_id {
                                    b2c.insert(bid.clone(), c.id.clone());
                                }
                            }
                        }

                        // Select first conversation by default (or keep Nobara if present)
                        let selected_conv = convs
                            .iter()
                            .find(|c| c.title == "Nobara Kugisaki")
                            .or_else(|| convs.first());

                        if let Some(sel) = selected_conv {
                            *active_id_clone.write().await = sel.id.clone();
                            *active_bot_clone.write().await = sel.bot_id.clone();

                            let title = sel.title.clone();
                            let subtitle = sel.subtitle.clone();
                            let avatar = sel.avatar_letter.clone();
                            let is_bot = sel.is_bot;
                            let b_color = parse_hex_color(&sel.bubble_color);
                            let cid = sel.id.clone();

                            let w_clone = window_weak.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = w_clone.upgrade() {
                                    w.set_active_chat_id(cid.into());
                                    w.set_active_chat_title(title.into());
                                    w.set_active_chat_subtitle(subtitle.into());
                                    w.set_active_chat_avatar(avatar.into());
                                    w.set_active_chat_is_bot(is_bot);
                                    w.set_active_chat_bubble_color(b_color);
                                }
                            });
                        }

                        let w_clone = window_weak.clone();
                        let conv_items: Vec<ConversationItem> = convs
                            .iter()
                            .map(|c| ConversationItem {
                                id: c.id.clone().into(),
                                title: c.title.clone().into(),
                                subtitle: c.subtitle.clone().into(),
                                avatar_letter: c.avatar_letter.clone().into(),
                                last_message: c.last_message.clone().into(),
                                timestamp: c.timestamp.clone().into(),
                                is_bot: c.is_bot,
                                is_online: c.is_online,
                                bubble_color: parse_hex_color(&c.bubble_color),
                            })
                            .collect();

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_clone.upgrade() {
                                let model = std::rc::Rc::new(slint::VecModel::from(conv_items));
                                w.set_conversations(model.into());
                            }
                        });

                        // 4. Load messages for active conversation
                        let active_cid = active_id_clone.read().await.clone();
                        if !active_cid.is_empty() {
                            if let Ok(msgs) = client.fetch_messages(&active_cid).await {
                                let w_clone = window_weak.clone();
                                let user_id_copy = uid.clone();

                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = w_clone.upgrade() {
                                        let user_accent = w.get_user_accent_color();
                                        let is_bot = w.get_active_chat_is_bot();
                                        let bot_color = w.get_active_chat_bubble_color();

                                        let items: Vec<MessageItem> = msgs
                                            .into_iter()
                                            .map(|m| {
                                                let is_me = m.role == "user"
                                                    && (m.sender_id.as_deref() == Some(&user_id_copy)
                                                        || m.sender_id.is_none());
                                                let bubble = if is_me {
                                                    user_accent
                                                } else if is_bot {
                                                    bot_color
                                                } else {
                                                    parse_hex_color("#1E2530")
                                                };

                                                MessageItem {
                                                    id: m.id.into(),
                                                    content: m.content.unwrap_or_default().into(),
                                                    timestamp: format_bubble_time(
                                                        m.created_at.as_deref().unwrap_or(""),
                                                    )
                                                    .into(),
                                                    is_me,
                                                    bubble_color: bubble,
                                                }
                                            })
                                            .collect();

                                        let model = std::rc::Rc::new(slint::VecModel::from(items));
                                        w.set_messages(model.into());
                                    }
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to authenticate with Supabase: {}", e);
                }
            }
        });
    }

    // Background Tray & Menu Event Polling Timer + Working Set Trimmer + Realtime Sync
    let timer = slint::Timer::default();
    let window_for_tray = main_window.as_weak();
    let sync_client = client.clone();
    let sync_active_id = active_conv_id.clone();
    let sync_my_uid = my_user_id.clone();
    let mut poll_count: u32 = 0;

    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(150),
        move || {
            poll_count = poll_count.wrapping_add(1);

            // Periodically flush unneeded heap/pages to maintain ultra-low RAM
            #[cfg(target_os = "windows")]
            if poll_count % 30 == 0 {
                trim_working_set();
            }

            // Periodic sync check every ~3 seconds
            if poll_count % 20 == 0 {
                let client = sync_client.clone();
                let active_id = sync_active_id.clone();
                let my_uid = sync_my_uid.clone();
                let w_clone = window_for_tray.clone();

                tokio::spawn(async move {
                    let cid = active_id.read().await.clone();
                    let uid = my_uid.read().await.clone();

                    if !cid.is_empty() && !uid.is_empty() {
                        if let Ok(msgs) = client.fetch_messages(&cid).await {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = w_clone.upgrade() {
                                    let current_count = w.get_messages().row_count();
                                    if msgs.len() > current_count {
                                        let user_accent = w.get_user_accent_color();
                                        let is_bot = w.get_active_chat_is_bot();
                                        let bot_color = w.get_active_chat_bubble_color();

                                        let items: Vec<MessageItem> = msgs
                                            .into_iter()
                                            .map(|m| {
                                                let is_me = m.role == "user"
                                                    && (m.sender_id.as_deref() == Some(&uid)
                                                        || m.sender_id.is_none());
                                                let bubble = if is_me {
                                                    user_accent
                                                } else if is_bot {
                                                    bot_color
                                                } else {
                                                    parse_hex_color("#1E2530")
                                                };

                                                MessageItem {
                                                    id: m.id.into(),
                                                    content: m.content.unwrap_or_default().into(),
                                                    timestamp: format_bubble_time(
                                                        m.created_at.as_deref().unwrap_or(""),
                                                    )
                                                    .into(),
                                                    is_me,
                                                    bubble_color: bubble,
                                                }
                                            })
                                            .collect();

                                        let model = std::rc::Rc::new(slint::VecModel::from(items));
                                        w.set_messages(model.into());
                                    }
                                }
                            });
                        }
                    }
                });
            }

            // Process menu events
            if let Ok(event) = muda::MenuEvent::receiver().try_recv() {
                if event.id == open_menu_id {
                    if let Some(w) = window_for_tray.upgrade() {
                        let _ = w.show();
                    }
                } else if event.id == quit_menu_id {
                    let _ = slint::quit_event_loop();
                }
            }

            // Process tray click events (left click opens app)
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

    // Run main application event loop
    main_window.run()?;

    Ok(())
}
