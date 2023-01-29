// at the top of main.rs - that will prevent the console from showing
#![windows_subsystem = "windows"]
extern crate image;

use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, File};
use std::io::Write;
use tauri_utils::config::{Config, WindowConfig};
use wry::{
    application::{
        event::{Event, StartCause, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        menu::MenuType,
        window::{Fullscreen, Window, WindowBuilder},
    },
    webview::WebViewBuilder,
    Error,
};

#[cfg(target_os = "macos")]
use wry::application::{
    accelerator::{Accelerator, SysMods},
    keyboard::KeyCode,
    menu::{MenuBar as Menu, MenuItem, MenuItemAttributes},
    platform::macos::WindowBuilderExtMacOS,
};

#[cfg(target_os = "windows")]
use wry::application::window::Icon;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use wry::webview::WebContext;

use dirs::download_dir;
use std::path::PathBuf;
use wry::application::dpi::{
    LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize, Position, Size,
};
use wry::application::monitor::MonitorHandle;

enum UserEvent {
    DownloadStarted(String, String),
    DownloadComplete(Option<PathBuf>, bool),
}

pub const STATE_FILENAME: &str = ".window-state";

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
struct WindowState {
    width: f64,
    height: f64,
    x: i32,
    y: i32,
    maximized: bool,
    visible: bool,
    decorated: bool,
    fullscreen: bool,
}

fn main() -> wry::Result<()> {
    #[cfg(target_os = "macos")]
    let (menu_bar_menu, close_item) = {
        let mut menu_bar_menu = Menu::new();
        let mut first_menu = Menu::new();
        first_menu.add_native_item(MenuItem::Hide);
        first_menu.add_native_item(MenuItem::EnterFullScreen);
        first_menu.add_native_item(MenuItem::Minimize);
        first_menu.add_native_item(MenuItem::Separator);
        first_menu.add_native_item(MenuItem::Copy);
        first_menu.add_native_item(MenuItem::Cut);
        first_menu.add_native_item(MenuItem::Paste);
        first_menu.add_native_item(MenuItem::Undo);
        first_menu.add_native_item(MenuItem::Redo);
        first_menu.add_native_item(MenuItem::SelectAll);
        first_menu.add_native_item(MenuItem::Separator);
        let close_item = first_menu.add_item(
            MenuItemAttributes::new("CloseWindow")
                .with_accelerators(&Accelerator::new(SysMods::Cmd, KeyCode::KeyW)),
        );
        first_menu.add_native_item(MenuItem::Quit);
        menu_bar_menu.add_submenu("App", true, first_menu);
        (menu_bar_menu, close_item)
    };

    let (
        package_name,
        WindowConfig {
            url,
            width,
            height,
            resizable,
            fullscreen,
            transparent,
            ..
        },
    ) = {
        let (package_name, windows_config) = get_windows_config();
        (
            package_name
                .expect("can't get package name in config file")
                .to_lowercase(),
            windows_config.unwrap_or_default(),
        )
    };

    let app_dir = dirs::config_dir().unwrap().join(&package_name);
    let state_path = app_dir.join(STATE_FILENAME);

    let window_state = if state_path.exists() {
        let file = File::open(&state_path)?;
        let state: WindowState = serde_json::from_reader(file)?;
        Some(state)
    } else {
        None
    };

    let event_loop: EventLoop<UserEvent> = EventLoop::with_user_event();
    let proxy = event_loop.create_proxy();
    let common_window = WindowBuilder::new()
        .with_title("")
        .with_resizable(resizable)
        .with_maximized(match window_state {
            Some(state) => state.maximized,
            None => false,
        })
        .with_position(match window_state {
            Some(state) => Position::Physical(PhysicalPosition::new(state.x, state.y)),
            None => Position::Logical(LogicalPosition::default()),
        });
    #[cfg(target_os = "windows")]
    let window = {
        let mut icon_path = format!("png/{}_32.ico", &package_name);
        // If there is no setting, use the default one.
        if !std::path::Path::new(&icon_path).exists() {
            icon_path = "png/icon_32.ico".to_string();
        }
        let icon = load_icon(std::path::Path::new(&icon_path));
        common_window
            .with_decorations(true)
            .with_window_icon(Some(icon))
            .build(&event_loop)
            .unwrap()
    };

    #[cfg(target_os = "linux")]
    let window = common_window.build(&event_loop).unwrap();

    #[cfg(target_os = "macos")]
    let window = common_window
        .with_fullsize_content_view(true)
        .with_titlebar_buttons_hidden(false)
        .with_titlebar_transparent(transparent)
        .with_title_hidden(true)
        .with_menu(menu_bar_menu)
        .build(&event_loop)
        .unwrap();

    match window_state {
        Some(state) => {
            if state.fullscreen {
                window.set_fullscreen(Some(Fullscreen::Borderless(window.current_monitor())));
            } else {
                window.set_fullscreen(None);
            }

            window.set_inner_size(Size::Logical(LogicalSize::new(state.width, state.height)))
        }
        None => {
            if fullscreen {
                window.set_fullscreen(Some(Fullscreen::Borderless(window.current_monitor())));
            } else {
                window.set_fullscreen(None);
            }
            window.set_inner_size(Size::Logical(LogicalSize::new(width, height)))
        }
    }

    // Handling events of JS -> Rust
    let handler = move |window: &Window, req: String| {
        if req == "drag_window" {
            let _ = window.drag_window();
        } else if req == "fullscreen" {
            let is_maximized = window.is_maximized();
            window.set_maximized(!is_maximized);
        } else if req.starts_with("open_browser") {
            let href = req.replace("open_browser:", "");
            webbrowser::open(&href).expect("no browser");
        }
    };

    let download_started = {
        let proxy = proxy.clone();
        move |uri: String, default_path: &mut PathBuf| {
            let path = download_dir()
                .unwrap()
                .join(default_path.display().to_string())
                .as_path()
                .to_path_buf();
            *default_path = path.clone();
            let submitted = proxy
                .send_event(UserEvent::DownloadStarted(uri, path.display().to_string()))
                .is_ok();
            submitted
        }
    };

    let download_completed = {
        move |_uri, path, success| {
            let _ = proxy.send_event(UserEvent::DownloadComplete(path, success));
        }
    };

    #[cfg(target_os = "macos")]
    let webview = {
        let user_agent_string = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.1 Safari/605.1.15";
        WebViewBuilder::new(window)?
            .with_user_agent(user_agent_string)
            .with_url(&url.to_string())?
            .with_devtools(cfg!(feature = "devtools"))
            .with_initialization_script(include_str!("pake.js"))
            .with_ipc_handler(handler)
            .with_back_forward_navigation_gestures(true)
            .with_download_started_handler(download_started)
            .with_download_completed_handler(download_completed)
            .build()?
    };

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let webview = {
        let home_dir = match home::home_dir() {
            Some(path1) => path1,
            None => panic!("Error, can't found you home dir!!"),
        };
        #[cfg(target_os = "windows")]
        let data_dir = home_dir.join("AppData").join("Roaming").join(&package_name);
        #[cfg(target_os = "linux")]
        let data_dir = home_dir.join(".config").join(&package_name);
        if !data_dir.exists() {
            std::fs::create_dir(&data_dir)
                .unwrap_or_else(|_| panic!("can't create dir {}", data_dir.display()));
        }
        let mut web_content = WebContext::new(Some(data_dir));
        #[cfg(target_os = "windows")]
        let user_agent_string = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36";
        #[cfg(target_os = "linux")]
        let user_agent_string = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36";
        WebViewBuilder::new(window)?
            .with_user_agent(user_agent_string)
            .with_url(&url.to_string())?
            .with_devtools(cfg!(feature = "devtools"))
            .with_initialization_script(include_str!("pake.js"))
            .with_ipc_handler(handler)
            .with_web_context(&mut web_content)
            .with_download_started_handler(download_started)
            .with_download_completed_handler(download_completed)
            .build()?
    };
    #[cfg(feature = "devtools")]
    {
        webview.open_devtools();
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => println!("Wry has started!"),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if app_dir.exists() {
                    let mut state = WindowState::default();

                    let window = webview.window();
                    let is_maximized = window.is_maximized();
                    state.maximized = is_maximized;
                    state.fullscreen = window.fullscreen().is_some();
                    state.decorated = window.is_decorated();
                    state.visible = window.is_visible();

                    let scale_factor = window
                        .current_monitor()
                        .map(|m| m.scale_factor())
                        .unwrap_or(1.);

                    let size = webview.inner_size().to_logical(scale_factor);
                    // It doesn't make sense to save a self with 0 height or width
                    if size.width > 0. && size.height > 0. && !is_maximized {
                        state.width = size.width;
                        state.height = size.height;
                    }

                    let position = window.inner_position().unwrap();

                    if let Some(monitor) = window.current_monitor() {
                        // save only window positions that are inside the current monitor
                        if monitor.contains(position) && !is_maximized {
                            state.x = position.x;
                            state.y = position.y;
                        }
                    }

                    create_dir_all(&app_dir)
                        .map_err(Error::Io)
                        .and_then(|_| File::create(&state_path).map_err(Into::into))
                        .and_then(|mut f| {
                            f.write_all(serde_json::to_string(&state).unwrap().as_ref())
                                .map_err(Into::into)
                        })
                        .expect("Can't save window state");
                };

                *control_flow = ControlFlow::Exit
            }
            Event::MenuEvent {
                menu_id,
                origin: MenuType::MenuBar,
                ..
            } => {
                #[cfg(target_os = "macos")]
                if menu_id == close_item.clone().id() {
                    webview.window().set_minimized(true);
                }
                println!("Clicked on {menu_id:?}");
            }
            Event::UserEvent(UserEvent::DownloadStarted(uri, temp_dir)) => {
                println!("Download: {uri}");
                println!("Will write to: {temp_dir:?}");
            }
            Event::UserEvent(UserEvent::DownloadComplete(_, success)) => {
                println!("Succeeded: {success}");
                if success {
                    let _ = webview.evaluate_script("window.pakeToast('Save in downloads folder')");
                } else {
                    println!("No output path")
                }
            }
            _ => (),
        }
    });
}

fn get_windows_config() -> (Option<String>, Option<WindowConfig>) {
    let config_file = include_str!("../tauri.conf.json");
    let config: Config = serde_json::from_str(config_file).expect("failed to parse windows config");
    (
        config.package.product_name.clone(),
        config.tauri.windows.first().cloned(),
    )
}

#[cfg(target_os = "windows")]
fn load_icon(path: &std::path::Path) -> Icon {
    let (icon_rgba, icon_width, icon_height) = {
        // alternatively, you can embed the icon in the binary through `include_bytes!` macro and use `image::load_from_memory`
        let image = image::open(path)
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("Failed to open icon")
}

trait MonitorExt {
    fn contains(&self, position: PhysicalPosition<i32>) -> bool;
}

impl MonitorExt for MonitorHandle {
    fn contains(&self, position: PhysicalPosition<i32>) -> bool {
        let PhysicalPosition { x, y } = self.position();
        let PhysicalSize { width, height } = self.size();

        x < position.x as _
            && position.x < (x + width as i32)
            && y < position.y as _
            && position.y < (y + height as i32)
    }
}
