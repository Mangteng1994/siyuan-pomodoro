//! 番茄计时：状态机放在后端，窗口（主窗口、迷你窗口）只是视图。
//! 后台线程负责到点结束、落盘、写回番茄记录，所以主窗口关掉计时也照常走。
//! 状态每次变化都写 timer.json（先写临时文件再改名），重启后恢复。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::mcp;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskRef {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub block_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Paused,
    Finished,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TimerState {
    pub task: TaskRef,
    pub planned_ms: i64,
    pub started_at: i64,
    pub paused_total: i64,
    pub paused_at: Option<i64>,
    pub status: Status,
    pub actual_ms: i64,
    pub completed: bool,
    pub ended_at: i64,
    pub written: bool,
    pub session_id: Option<String>,
    pub duration: Option<u32>,
    /// 写记录用的内核端口
    pub port: u16,
    /// 以下两项只在内存里，不落盘
    #[serde(default)]
    pub writing: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// 是否是程序重启后从文件恢复的（只在内存里，供界面提示）
    #[serde(default)]
    pub recovered: bool,
}

/// 记录写入失败后的自动重试间隔
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

pub type Shared = Arc<Mutex<Option<TimerState>>>;

pub struct TimerHandle(pub Shared);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn elapsed_ms(s: &TimerState, now: i64) -> i64 {
    now - s.started_at - s.paused_total - s.paused_at.map(|p| now - p).unwrap_or(0)
}

fn timer_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::config_dir(app)?.join("timer.json"))
}

fn persist(app: &AppHandle, s: &TimerState) -> Result<(), String> {
    let mut c = s.clone();
    c.writing = false;
    c.error = None;
    c.recovered = false;
    let path = timer_path(app)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string(&c).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn remove_file(app: &AppHandle) -> Result<(), String> {
    let path = timer_path(app)?;
    match fs::remove_file(&path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn notify(app: &AppHandle) {
    let _ = app.emit("timer-changed", ());
}

/// 把“已结束但还没写入”的计时写成番茄记录。同一时刻只会有一个写入在进行。
fn spawn_write(app: AppHandle, shared: Shared) {
    std::thread::spawn(move || {
        let snap = {
            let mut g = shared.lock().unwrap();
            match g.as_mut() {
                Some(s) if s.status == Status::Finished && !s.written && !s.writing => {
                    s.writing = true;
                    s.error = None;
                    s.clone()
                }
                _ => return,
            }
        };
        notify(&app);
        let result = mcp::record_focus(
            snap.port,
            &snap.task.id,
            &snap.task.title,
            snap.started_at,
            snap.ended_at,
            snap.actual_ms,
            snap.planned_ms as f64 / 60000.0,
            snap.completed,
        );
        {
            let mut g = shared.lock().unwrap();
            if let Some(s) = g.as_mut() {
                s.writing = false;
                match result {
                    Ok(v) => {
                        s.written = true;
                        s.error = None;
                        s.session_id = v["id"].as_str().map(|x| x.to_string());
                        s.duration = v["duration"].as_u64().map(|x| x as u32);
                        let _ = persist(&app, s);
                    }
                    Err(e) => s.error = Some(e),
                }
            }
        }
        notify(&app);
    });
}

/// 启动时调用：读回上次的计时，启动后台线程。
pub fn init(app: &mut tauri::App) -> Result<(), String> {
    let handle = app.handle().clone();
    let shared: Shared = Arc::new(Mutex::new(None));
    let mut recovery_error = None;
    if let Ok(path) = timer_path(&handle) {
        if let Ok(text) = fs::read_to_string(&path) {
            match serde_json::from_str::<TimerState>(&text) {
                Ok(mut s) => {
                    s.writing = false;
                    s.error = None;
                    s.recovered = true;
                    *shared.lock().unwrap() = Some(s);
                }
                Err(e) => {
                    let _ = fs::rename(&path, path.with_extension("json.corrupt"));
                    recovery_error = Some(format!("上次的计时状态文件损坏，已改名保留为 timer.json.corrupt：{e}"));
                }
            }
        }
    }
    app.manage(TimerHandle(shared.clone()));
    if let Some(e) = recovery_error {
        eprintln!("{e}");
    }

    // 恢复时已结束但没写入的，直接补写
    spawn_write(handle.clone(), shared.clone());

    // 到点检测线程；写入失败（比如内核没开）后每隔一段时间自动重试，内核一起来记录就补上
    std::thread::spawn(move || {
        let mut last_try = std::time::Instant::now();
        loop {
        std::thread::sleep(Duration::from_millis(250));
        let retry_due = {
            let g = shared.lock().unwrap();
            g.as_ref().map_or(false, |s| {
                s.status == Status::Finished && !s.written && !s.writing && s.error.is_some()
            })
        };
        if retry_due && last_try.elapsed() >= RETRY_INTERVAL {
            last_try = std::time::Instant::now();
            spawn_write(handle.clone(), shared.clone());
        }
        let finished = {
            let mut g = shared.lock().unwrap();
            match g.as_mut() {
                Some(s) if s.status == Status::Running && elapsed_ms(s, now_ms()) >= s.planned_ms => {
                    // 结束时刻按“起点 + 暂停总时长 + 计划时长”算，电脑休眠后才检测到也不会偏
                    s.status = Status::Finished;
                    s.actual_ms = s.planned_ms;
                    s.completed = true;
                    s.ended_at = s.started_at + s.paused_total + s.planned_ms;
                    let _ = persist(&handle, s);
                    true
                }
                _ => false,
            }
        };
        if finished {
            remind(&handle);
            notify(&handle);
            last_try = std::time::Instant::now();
            spawn_write(handle.clone(), shared.clone());
        }
        }
    });
    Ok(())
}

#[tauri::command]
pub fn timer_get(state: State<TimerHandle>) -> Option<TimerState> {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn timer_start(
    app: AppHandle,
    state: State<TimerHandle>,
    task: TaskRef,
    minutes: f64,
    port: u16,
) -> Result<(), String> {
    if !(minutes > 0.0) {
        return Err("时长必须大于 0".to_string());
    }
    let mut g = state.0.lock().unwrap();
    if g.is_some() {
        return Err("已经有一个计时，请先结束并清除".to_string());
    }
    let s = TimerState {
        task,
        planned_ms: (minutes * 60000.0).round() as i64,
        started_at: now_ms(),
        paused_total: 0,
        paused_at: None,
        status: Status::Running,
        actual_ms: 0,
        completed: false,
        ended_at: 0,
        written: false,
        session_id: None,
        duration: None,
        port,
        writing: false,
        error: None,
        recovered: false,
    };
    persist(&app, &s)?;
    *g = Some(s);
    drop(g);
    notify(&app);
    Ok(())
}

#[tauri::command]
pub fn timer_toggle_pause(app: AppHandle, state: State<TimerHandle>) -> Result<(), String> {
    let mut g = state.0.lock().unwrap();
    let s = g.as_mut().ok_or("没有正在进行的计时")?;
    let now = now_ms();
    match s.status {
        Status::Running => {
            s.paused_at = Some(now);
            s.status = Status::Paused;
        }
        Status::Paused => {
            s.paused_total += now - s.paused_at.unwrap_or(now);
            s.paused_at = None;
            s.status = Status::Running;
        }
        Status::Finished => return Err("计时已经结束".to_string()),
    }
    persist(&app, s)?;
    drop(g);
    notify(&app);
    Ok(())
}

#[tauri::command]
pub fn timer_end(app: AppHandle, state: State<TimerHandle>) -> Result<(), String> {
    {
        let mut g = state.0.lock().unwrap();
        let s = g.as_mut().ok_or("没有正在进行的计时")?;
        if s.status == Status::Finished {
            return Err("计时已经结束".to_string());
        }
        let now = now_ms();
        s.ended_at = now;
        s.actual_ms = elapsed_ms(s, now).clamp(0, s.planned_ms);
        s.completed = false;
        s.paused_at = None;
        s.status = Status::Finished;
        persist(&app, s)?;
    }
    notify(&app);
    spawn_write(app, state.0.clone());
    Ok(())
}

#[tauri::command]
pub fn timer_retry(app: AppHandle, state: State<TimerHandle>, port: u16) -> Result<(), String> {
    {
        let mut g = state.0.lock().unwrap();
        let s = g.as_mut().ok_or("没有计时")?;
        if s.status != Status::Finished || s.written {
            return Err("没有需要写入的记录".to_string());
        }
        s.port = port;
    }
    spawn_write(app, state.0.clone());
    Ok(())
}

/// 记录已写入后清除计时。
#[tauri::command]
pub fn timer_clear(app: AppHandle, state: State<TimerHandle>) -> Result<(), String> {
    let mut g = state.0.lock().unwrap();
    match g.as_ref() {
        None => return Ok(()),
        Some(s) if s.status != Status::Finished || !s.written => {
            return Err("记录还没有写入成功，不能清除（确实要丢弃请用“放弃记录”）".to_string())
        }
        _ => {}
    }
    remove_file(&app)?;
    *g = None;
    drop(g);
    notify(&app);
    Ok(())
}

/// 放弃一个已结束但写不进去的计时（比如内核一直起不来）。这条记录会丢。
#[tauri::command]
pub fn timer_discard(app: AppHandle, state: State<TimerHandle>) -> Result<(), String> {
    let mut g = state.0.lock().unwrap();
    match g.as_ref() {
        None => return Ok(()),
        Some(s) if s.status != Status::Finished => return Err("计时还没有结束".to_string()),
        Some(s) if s.writing => return Err("正在写入，请稍候".to_string()),
        _ => {}
    }
    remove_file(&app)?;
    *g = None;
    drop(g);
    notify(&app);
    Ok(())
}

/// 打开（或唤起）置顶的迷你计时窗口。
#[tauri::command]
pub async fn open_mini(app: AppHandle) -> Result<(), String> {
    show_mini(&app)
}

#[cfg(windows)]
extern "system" {
    fn MessageBeep(kind: u32) -> i32;
}

/// 自然到点的提醒：连响三声系统提示音，并把迷你窗口弹出到最前（没开则新开一个）。
fn remind(app: &AppHandle) {
    std::thread::spawn(|| {
        for _ in 0..3 {
            #[cfg(windows)]
            unsafe {
                MessageBeep(0x30); // MB_ICONEXCLAMATION
            }
            std::thread::sleep(Duration::from_millis(700));
        }
    });
    let _ = show_mini(app);
    if let Some(w) = app.get_webview_window("mini") {
        let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
    }
}

/// 迷你窗口的外观：normal 是带标题栏的普通小窗，其余是无边框的紧凑样式（尺寸对应原插件的三种迷你样式）。
#[tauri::command]
pub fn set_mini_mode(app: AppHandle, mode: String) -> Result<(), String> {
    let w = app.get_webview_window("mini").ok_or("迷你窗口未打开")?;
    let (width, height, decorations) = match mode.as_str() {
        "ring" => (64.0, 64.0, false),
        "bar" => (190.0, 46.0, false),
        "minimal" => (150.0, 22.0, false),
        _ => (250.0, 160.0, true),
    };
    w.set_decorations(decorations).map_err(|e| e.to_string())?;
    // 无边框时不要系统阴影/边框，否则圆环外面会露出一个方框
    w.set_shadow(decorations).map_err(|e| e.to_string())?;
    w.set_size(tauri::LogicalSize::new(width, height)).map_err(|e| e.to_string())
}

pub fn show_mini(app: &AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("mini") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(app, "mini", tauri::WebviewUrl::App("mini.html".into()))
        .title("番茄钟")
        .inner_size(250.0, 160.0)
        .resizable(false)
        .always_on_top(true)
        .transparent(true)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
