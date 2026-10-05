use anyhow::{Context, Result};
use std::{cell::RefCell, collections::HashSet, rc::Rc};
use tao::{
    dpi::LogicalSize,
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget},
    platform::run_return::EventLoopExtRunReturn,
    window::{Window, WindowBuilder},
};
use tokio::sync::watch;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
use wry::{NewWindowResponse, WebView, WebViewBuilder};
use zeroize::Zeroizing;

enum DesktopEvent {
    Open(Zeroizing<String>),
    Reopen,
    Licenses(String),
    Menu(MenuEvent),
    Tray(TrayIconEvent),
    Stopped(Result<()>),
    DownloadFailed,
}

#[derive(Clone)]
pub struct Handle {
    proxy: EventLoopProxy<DesktopEvent>,
    shutdown: watch::Sender<bool>,
}
impl Handle {
    pub fn open(&self, url: Zeroizing<String>) -> Result<()> {
        self.proxy
            .send_event(DesktopEvent::Open(url))
            .map_err(|_| anyhow::anyhow!("应用窗口已退出"))
    }
    pub fn reopen(&self) -> Result<()> {
        self.proxy
            .send_event(DesktopEvent::Reopen)
            .map_err(|_| anyhow::anyhow!("应用窗口已退出"))
    }
    pub fn shutdown(&self) -> watch::Sender<bool> {
        self.shutdown.clone()
    }
}

struct AppWindow {
    // Drop the webview before its parent window.
    _webview: WebView,
    window: Window,
}
impl AppWindow {
    fn active(&self, active: bool) {
        let script = if active {
            "window.__cipherwhisperActive = true; window.dispatchEvent(new Event('cipherwhisper:visibility'));"
        } else {
            "window.__cipherwhisperActive = false; window.dispatchEvent(new Event('cipherwhisper:visibility'));"
        };
        let _ = self._webview.evaluate_script(script);
    }
    fn show(&self, target: &EventLoopWindowTarget<DesktopEvent>) {
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
            target.set_activation_policy_at_runtime(ActivationPolicy::Regular);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = target;
        self.window.set_visible(true);
        self.window.set_minimized(false);
        self.window.set_focus();
        self.active(true);
    }
}

fn create_window(
    target: &EventLoopWindowTarget<DesktopEvent>,
    proxy: EventLoopProxy<DesktopEvent>,
    url: &str,
    licenses: bool,
) -> Result<AppWindow> {
    let origin = reqwest::Url::parse(url)?;
    let window = WindowBuilder::new()
        .with_title(if licenses {
            "CipherWhisper — 开源许可"
        } else {
            "CipherWhisper"
        })
        .with_inner_size(LogicalSize::new(1160.0, 780.0))
        .with_min_inner_size(LogicalSize::new(760.0, 540.0))
        .with_visible(false)
        .build(target)?;
    let navigation_origin = origin.clone();
    let popup_origin = origin.clone();
    let popup_proxy = proxy.clone();
    let download_proxy = proxy;
    let downloading = Rc::new(RefCell::new(HashSet::new()));
    let started = downloading.clone();
    let webview = WebViewBuilder::new()
        .with_url(url)
        // No persistent browser profile or session shared between workspaces.
        .with_incognito(true)
        .with_devtools(cfg!(debug_assertions))
        .with_clipboard(true)
        .with_initialization_script("window.__cipherwhisperActive = true;")
        .with_navigation_handler(move |url| super::navigation_allowed(&navigation_origin, &url))
        .with_new_window_req_handler(move |url, _| {
            if super::navigation_allowed(&popup_origin, &url)
                && reqwest::Url::parse(&url).is_ok_and(|u| u.path() == "/third-party-ui.txt")
            {
                let _ = popup_proxy.send_event(DesktopEvent::Licenses(url));
            }
            NewWindowResponse::Deny
        })
        .with_download_started_handler(move |url, destination| {
            let name = destination
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("download");
            match rfd::FileDialog::new()
                .set_title("保存文件")
                .set_file_name(name)
                .save_file()
            {
                Some(path) => {
                    *destination = path;
                    started.borrow_mut().insert(url);
                    true
                }
                None => false,
            }
        })
        .with_download_completed_handler(move |url, _, success| {
            if downloading.borrow_mut().remove(&url) && !success {
                let _ = download_proxy.send_event(DesktopEvent::DownloadFailed);
            }
        })
        .build(&window)
        .context("无法创建应用窗口；Windows 请确认已安装 Microsoft Edge WebView2 Runtime")?;
    Ok(AppWindow {
        _webview: webview,
        window,
    })
}

fn icon() -> Result<Icon> {
    // Rasterize the existing ◒ brand mark; macOS treats alpha as a template mask.
    let size = 32;
    let mut rgba = vec![0; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let distance = ((x as f64 - 15.5).powi(2) + (y as f64 - 15.5).powi(2)).sqrt();
            let outer = (14.0 - distance).clamp(0.0, 1.0);
            let shape = if y >= 16 {
                outer
            } else {
                outer * (distance - 10.5).clamp(0.0, 1.0)
            };
            let pixel = &mut rgba[(y * size + x) * 4..][..4];
            pixel.copy_from_slice(&[36, 170, 156, (shape * 255.0) as u8]);
        }
    }
    Ok(Icon::from_rgba(rgba, size as u32, size as u32)?)
}

pub fn run(args: crate::launcher::Args) -> Result<()> {
    let mut event_loop = EventLoopBuilder::<DesktopEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    let (shutdown, _) = watch::channel(false);
    let handle = Handle {
        proxy: proxy.clone(),
        shutdown: shutdown.clone(),
    };
    let runtime = tokio::runtime::Runtime::new()?;
    let backend_proxy = proxy.clone();
    let backend = runtime.spawn(async move {
        let result = crate::launcher::run_with_window(args, Some(handle)).await;
        let _ = backend_proxy.send_event(DesktopEvent::Stopped(result));
    });

    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(DesktopEvent::Menu(event));
    }));
    let tray_proxy = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = tray_proxy.send_event(DesktopEvent::Tray(event));
    }));
    let menu = Menu::new();
    let show = MenuItem::new("打开 CipherWhisper", true, None);
    let hide = MenuItem::new("隐藏到后台", true, None);
    let quit = MenuItem::new("退出 CipherWhisper", true, None);
    menu.append_items(&[&show, &hide, &PredefinedMenuItem::separator(), &quit])?;

    // Cocoa's standard editing menu supplies Cmd+C/V/X/A and undo in WKWebView.
    #[cfg(target_os = "macos")]
    let _app_menu = {
        use tray_icon::menu::{
            Submenu,
            accelerator::{Accelerator, Code, Modifiers},
        };
        quit.set_accelerator(Some(Accelerator::new(Modifiers::META, Code::KeyQ)))?;
        let app = Submenu::new("CipherWhisper", true);
        app.append_items(&[&show, &hide, &PredefinedMenuItem::separator(), &quit])?;
        let edit = Submenu::new("编辑", true);
        edit.append_items(&[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ])?;
        let main = Menu::new();
        main.append_items(&[&app, &edit])?;
        main.init_for_nsapp();
        main
    };

    let mut tray: Option<TrayIcon> = None;
    let mut window: Option<AppWindow> = None;
    let mut licenses_window: Option<AppWindow> = None;
    let mut failure = None;
    let mut stopped = Ok(());
    event_loop.run_return(|event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let mut show_window = false;
        let mut hide_window = false;
        let mut quit_app = false;
        match event {
            Event::NewEvents(StartCause::Init) => {
                match icon().and_then(|icon| {
                    let builder = TrayIconBuilder::new()
                        .with_tooltip("CipherWhisper — 后台运行")
                        .with_menu(Box::new(menu.clone()))
                        .with_menu_on_left_click(cfg!(target_os = "macos"));
                    #[cfg(target_os = "macos")]
                    let builder = builder.with_icon_templated(icon);
                    #[cfg(target_os = "windows")]
                    let builder = builder.with_icon(icon);
                    Ok(builder.build()?)
                }) {
                    Ok(icon) => tray = Some(icon),
                    Err(error) => {
                        failure = Some(error);
                        quit_app = true;
                    }
                }
            }
            Event::UserEvent(DesktopEvent::Open(url)) => {
                match create_window(target, proxy.clone(), &url, false) {
                    Ok(created) => {
                        window = Some(created);
                        show_window = true;
                    }
                    Err(error) => {
                        failure = Some(error);
                        quit_app = true;
                    }
                }
            }
            Event::UserEvent(DesktopEvent::Reopen) => show_window = true,
            Event::Reopen { .. } => show_window = true,
            Event::UserEvent(DesktopEvent::Licenses(url)) => {
                if licenses_window.is_none() {
                    match create_window(target, proxy.clone(), &url, true) {
                        Ok(created) => licenses_window = Some(created),
                        Err(error) => {
                            rfd::MessageDialog::new()
                                .set_title("CipherWhisper")
                                .set_description(error.to_string())
                                .show();
                        }
                    }
                }
                if let Some(window) = &licenses_window {
                    window.show(target);
                }
            }
            Event::UserEvent(DesktopEvent::Menu(event)) => {
                show_window = event.id == show.id();
                hide_window = event.id == hide.id();
                quit_app = event.id == quit.id();
            }
            Event::UserEvent(DesktopEvent::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            })) => {
                #[cfg(target_os = "windows")]
                {
                    show_window = true;
                }
            }
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if window.as_ref().is_some_and(|w| w.window.id() == window_id) {
                    hide_window = true;
                } else if licenses_window
                    .as_ref()
                    .is_some_and(|w| w.window.id() == window_id)
                {
                    licenses_window = None;
                }
            }
            Event::WindowEvent {
                window_id,
                event: WindowEvent::Focused(focused),
                ..
            } => {
                if let Some(window) = &window
                    && window.window.id() == window_id
                {
                    window.active(
                        focused && window.window.is_visible() && !window.window.is_minimized(),
                    );
                }
            }
            Event::UserEvent(DesktopEvent::DownloadFailed) => {
                rfd::MessageDialog::new()
                    .set_title("CipherWhisper")
                    .set_description("文件未保存，请重试下载。")
                    .show();
            }
            Event::UserEvent(DesktopEvent::Stopped(result)) => {
                stopped = result;
                licenses_window = None;
                window = None;
                tray = None;
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
        if show_window && let Some(window) = &window {
            window.show(target);
        }
        if hide_window {
            if let Some(window) = &window {
                window.active(false);
                window.window.set_visible(false);
            }
            if let Some(window) = &licenses_window {
                window.window.set_visible(false);
            }
            #[cfg(target_os = "macos")]
            {
                use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
                target.set_activation_policy_at_runtime(ActivationPolicy::Accessory);
            }
        }
        if quit_app {
            show.set_enabled(false);
            hide.set_enabled(false);
            quit.set_enabled(false);
            shutdown.send_replace(true);
        }
    });
    MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    runtime.block_on(backend)?;
    let result = failure.map_or(stopped, Err);
    if let Err(error) = &result {
        rfd::MessageDialog::new()
            .set_title("CipherWhisper 启动失败")
            .set_description(format!("{error:#}"))
            .show();
    }
    result
}
