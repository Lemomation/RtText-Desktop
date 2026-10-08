#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;

use slint::Model;

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RtText Desktop initializing, backend: {}", config::CONFIG.supabase_url);

    #[cfg(target_os = "windows")]
    if !check_single_instance() {
        eprintln!("RtText is already running in the system tray.");
        return Ok(());
    }

    // Initialize Tokio runtime for background Supabase networking
    let _rt = tokio::runtime::Runtime::new()?;

    // Create Main Slint Window
    let main_window = MainWindow::new()?;

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
    main_window.on_tab_clicked(|tab| {
        println!("Switched to tab: {}", tab);
    });

    main_window.on_conversation_selected(|id, title, subtitle, _avatar, is_bot, _color| {
        println!("Selected conversation {}: {} ({}, is_bot: {})", id, title, subtitle, is_bot);
    });

    let window_for_send = main_window.as_weak();
    main_window.on_send_message(move |text| {
        println!("Send message triggered: {}", text);
        if let Some(w) = window_for_send.upgrade() {
            let msgs_model = w.get_messages();
            let mut msgs: Vec<MessageItem> = (0..msgs_model.row_count())
                .filter_map(|i| msgs_model.row_data(i))
                .collect();
            msgs.push(MessageItem {
                id: format!("m-{}", msgs.len() + 1).into(),
                content: text,
                timestamp: "Just now".into(),
                is_me: true,
                bubble_color: slint::Color::from_argb_u8(255, 251, 191, 36),
            });
            let model = std::rc::Rc::new(slint::VecModel::from(msgs));
            w.set_messages(model.into());
        }
    });

    // Background Tray & Menu Event Polling Timer + Working Set Trimmer
    let timer = slint::Timer::default();
    let window_for_tray = main_window.as_weak();
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
