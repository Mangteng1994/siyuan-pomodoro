#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tauri::Manager;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const KERNEL_IMAGE: &str = "SiYuan-Kernel.exe";

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Config {
    pub kernel_path: String,
    pub workspace: String,
    pub port: u16,
    /// 思源 API token；内核设了访问授权码时必填
    #[serde(default)]
    pub api_token: String,
    /// 定时触发思源原生同步（间隔跟随思源自己的设置，见 docs/spike-sync.md）
    #[serde(default = "default_true")]
    pub sync_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        Config {
            kernel_path: format!(r"{local}\Programs\SiYuan\resources\kernel\{KERNEL_IMAGE}"),
            workspace: String::new(),
            port: 6806,
            api_token: String::new(),
            sync_enabled: true,
        }
    }
}

/// 后台线程用：读配置，读不到就给默认值（不让线程崩）。
pub(crate) fn read_config(app: &tauri::AppHandle) -> Config {
    let config = config_dir(app)
        .ok()
        .and_then(|d| fs::read_to_string(d.join("config.json")).ok())
        .and_then(|t| serde_json::from_str::<Config>(&t).ok())
        .unwrap_or_default();
    mcp::set_api_token(&config.api_token);
    config
}

#[derive(Serialize, Deserialize)]
struct PidFile {
    pid: u32,
    port: u16,
}

#[derive(Serialize)]
struct Status {
    running: bool,
    version: Option<String>,
    /// 内核是不是由本工具启动的（只有这种内核才允许本工具停止）
    owned: bool,
}

pub(crate) fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn no_window(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(CREATE_NO_WINDOW)
}

/// 探测端口上是否有思源内核，返回内核版本。
fn probe(port: u16) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(1500))
        .build();
    let resp = mcp::auth(agent.post(&format!("http://127.0.0.1:{port}/api/system/version")))
        .set("Content-Type", "application/json")
        .send_string("{}")
        .ok()?;
    let v: serde_json::Value = serde_json::from_str(&resp.into_string().ok()?).ok()?;
    v.get("data")?.as_str().map(|s| s.to_string())
}

/// 该 PID 对应的进程是否确实是思源内核（防止 PID 被复用后误杀别的程序）。
fn pid_is_kernel(pid: u32) -> bool {
    let mut cmd = Command::new("tasklist");
    no_window(&mut cmd).args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .to_ascii_lowercase()
            .contains(&KERNEL_IMAGE.to_ascii_lowercase()),
        Err(_) => false,
    }
}

fn pid_path(dir: &PathBuf, port: u16) -> PathBuf {
    dir.join(format!("kernel-{port}.pid.json"))
}

/// 本工具在该端口启动的、仍在运行的内核。
fn owned_kernel(dir: &PathBuf, port: u16) -> Option<PidFile> {
    let text = fs::read_to_string(pid_path(dir, port)).ok()?;
    let p: PidFile = serde_json::from_str(&text).ok()?;
    if pid_is_kernel(p.pid) {
        Some(p)
    } else {
        let _ = fs::remove_file(pid_path(dir, port));
        None
    }
}

#[tauri::command]
fn get_config(app: tauri::AppHandle) -> Result<Config, String> {
    let path = config_dir(&app)?.join("config.json");
    let config = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("配置文件损坏：{e}"))?,
        Err(_) => Config::default(),
    };
    mcp::set_api_token(&config.api_token);
    Ok(config)
}

#[tauri::command]
fn save_config(app: tauri::AppHandle, config: Config) -> Result<(), String> {
    mcp::set_api_token(&config.api_token);
    let path = config_dir(&app)?.join("config.json");
    let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| e.to_string())
}

#[tauri::command]
async fn kernel_status(app: tauri::AppHandle, port: u16) -> Result<Status, String> {
    let dir = config_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let version = probe(port);
        let owned = owned_kernel(&dir, port).is_some();
        Status {
            running: version.is_some(),
            version,
            owned,
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn kernel_start(app: tauri::AppHandle, config: Config) -> Result<String, String> {
    mcp::set_api_token(&config.api_token);
    let dir = config_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || start_kernel(&dir, &config))
        .await
        .map_err(|e| e.to_string())?
}

fn start_kernel(dir: &PathBuf, config: &Config) -> Result<String, String> {
    if let Some(v) = probe(config.port) {
        return Ok(format!(
            "端口 {} 上已有内核在运行（版本 {v}），已直接连接，没有重复启动",
            config.port
        ));
    }
    let kernel = PathBuf::from(&config.kernel_path);
    if !kernel.is_file() {
        return Err(format!("找不到内核程序：{}", config.kernel_path));
    }
    let wd = kernel
        .parent()
        .and_then(|p| p.parent())
        .ok_or("内核路径不合法，应形如 …\\resources\\kernel\\SiYuan-Kernel.exe")?
        .to_path_buf();
    if config.workspace.trim().is_empty() || !PathBuf::from(&config.workspace).is_dir() {
        return Err(format!("工作空间目录不存在：{}", config.workspace));
    }

    let log = fs::File::create(dir.join("kernel.log")).map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&kernel);
    no_window(&mut cmd)
        .arg("serve")
        .arg(format!("--port={}", config.port))
        .arg(format!("--wd={}", wd.display()))
        .arg(format!("--workspace={}", config.workspace))
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err));
    let mut child = cmd.spawn().map_err(|e| format!("启动内核失败：{e}"))?;
    let pid = child.id();

    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            let code = status.code().unwrap_or(-1);
            return Err(if code == 24 {
                "内核启动后立刻退出（退出码 24）：这个工作空间正被另一个内核占用，请先关闭桌面端或其他内核"
                    .to_string()
            } else {
                format!(
                    "内核启动后退出，退出码 {code}，日志见 {}",
                    dir.join("kernel.log").display()
                )
            });
        }
        if probe(config.port).is_some() {
            // 只有确认就绪后才记录 PID，失败的启动不会覆盖已有记录
            let pidfile = PidFile {
                pid,
                port: config.port,
            };
            fs::write(
                pid_path(dir, config.port),
                serde_json::to_string(&pidfile).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            // 内核 HTTP 就绪后，插件工具还要几秒才注册；等它就绪，启动才算真正完成
            let plugin_deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < plugin_deadline {
                if mcp::plugin_tool_ready(config.port, "task") {
                    return Ok(format!("内核已启动，任务插件已就绪（PID {pid}，端口 {}）", config.port));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            return Ok(format!(
                "内核已启动（PID {pid}，端口 {}），但 30 秒内没有等到任务插件：请确认工作空间 conf.json 里 bazaar.trust 为 true，且已启用任务笔记管理插件",
                config.port
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("等待 60 秒内核仍未就绪，已保留进程，请查看日志".to_string())
}

#[tauri::command]
async fn kernel_stop(app: tauri::AppHandle, port: u16) -> Result<String, String> {
    let dir = config_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(p) = owned_kernel(&dir, port) else {
            return Err(
                "没有由本工具启动的内核，未停止任何进程（其他方式启动的内核请在原处关闭）".to_string(),
            );
        };
        let mut cmd = Command::new("taskkill");
        no_window(&mut cmd).args(["/PID", &p.pid.to_string(), "/F"]);
        let out = cmd.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("停止失败：{}", String::from_utf8_lossy(&out.stdout)));
        }
        let _ = fs::remove_file(pid_path(&dir, port));
        Ok(format!("已停止内核（PID {}）", p.pid))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn open_ui(port: u16) -> Result<(), String> {
    let mut cmd = Command::new("cmd");
    no_window(&mut cmd).args(["/C", "start", "", &format!("http://127.0.0.1:{port}/")]);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

mod mcp;

#[tauri::command]
async fn search_tasks(
    port: u16,
    scope: String,
    keyword: String,
    include_completed: bool,
) -> Result<Vec<mcp::TaskItem>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        mcp::search_tasks(port, &scope, &keyword, include_completed)
    })
    .await
    .map_err(|e| e.to_string())?
}

mod notes;
mod sync;
mod timer;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "SiYuanPomodoro";

/// 开机自启：写当前用户的 Run 注册表项（只影响当前用户，可随时关掉）。启动时带 --hidden，只进托盘不弹主窗口。
#[tauri::command]
fn autostart_get() -> bool {
    let mut cmd = Command::new("reg");
    no_window(&mut cmd).args(["query", RUN_KEY, "/v", RUN_NAME]);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

#[tauri::command]
fn autostart_set(enabled: bool) -> Result<(), String> {
    let mut cmd = Command::new("reg");
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let value = format!("\"{}\" --hidden", exe.display());
        no_window(&mut cmd).args(["add", RUN_KEY, "/v", RUN_NAME, "/t", "REG_SZ", "/d", &value, "/f"]);
    } else {
        if !autostart_get() {
            return Ok(());
        }
        no_window(&mut cmd).args(["delete", RUN_KEY, "/v", RUN_NAME, "/f"]);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 托盘常驻：关窗口只是隐藏，计时、提醒、写记录照常；真正退出走托盘菜单。
fn init_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let open_main = MenuItem::with_id(app, "main", "打开主窗口", true, None::<&str>)?;
    let open_mini = MenuItem::with_id(app, "mini", "打开迷你窗口", true, None::<&str>)?;
    let open_notes = MenuItem::with_id(app, "notes", "打开悬浮笔记", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出（计时靠记录文件，下次启动恢复）", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_main, &open_mini, &open_notes, &quit])?;
    let mut builder = TrayIconBuilder::new()
        .tooltip("思源番茄钟")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "main" => show_main(app),
            "mini" => {
                let _ = timer::show_mini(app);
            }
            "notes" => {
                let _ = notes::show_notes(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // 先载入 API token，再启动计时线程（它会自动重试写记录）
            let _ = get_config(app.handle().clone());
            timer::init(app)?;
            sync::init(app)?;
            init_tray(app)?;
            // 开机自启时带 --hidden：只进托盘，不弹主窗口
            if std::env::args().any(|a| a == "--hidden") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            kernel_status,
            kernel_start,
            kernel_stop,
            open_ui,
            search_tasks,
            timer::timer_get,
            timer::timer_start,
            timer::timer_start_break,
            timer::timer_toggle_pause,
            timer::timer_end,
            timer::timer_submit_note,
            timer::timer_retry,
            timer::timer_clear,
            timer::timer_discard,
            autostart_get,
            autostart_set,
            notes::notes_get,
            notes::notes_save,
            notes::open_notes,
            timer::open_mini,
            timer::set_mini_mode,
            sync::sync_status,
            sync::sync_now
        ])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
