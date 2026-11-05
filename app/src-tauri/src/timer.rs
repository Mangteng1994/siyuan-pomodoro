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
    /// work（番茄）| shortBreak | longBreak（休息）。旧的 timer.json 没有这一项，按番茄处理
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 专注结束后等用户填备注：等到填完、点跳过或超时，才写记录
    #[serde(default)]
    pub awaiting_note: bool,
    #[serde(default)]
    pub note_deadline: i64,
    #[serde(default)]
    pub note: String,
    /// 写记录用的内核端口
    pub port: u16,
    /// 上一个专注的计划时长（毫秒）：休息结束后用它当接续番茄的默认时长
    #[serde(default)]
    pub work_ms: Option<i64>,
    /// 以下两项只在内存里，不落盘
    #[serde(default)]
    pub writing: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// 是否是程序重启后从文件恢复的（只在内存里，供界面提示）
    #[serde(default)]
    pub recovered: bool,
}

fn default_kind() -> String {
    "work".to_string()
}

/// 记录写入失败后的自动重试间隔
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// 专注结束后等用户填备注的时间，超时按空备注写入，不会卡住记录
const NOTE_WAIT_MS: i64 = 120_000;

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
                Some(s) if s.status == Status::Finished && !s.written && !s.writing && !s.awaiting_note => {
                    s.writing = true;
                    s.error = None;
                    s.clone()
                }
                _ => return,
            }
        };
        notify(&app);
        // 同步进行中时稍等，避免和同步的 checkout 抢同一批文件
        crate::sync::wait_write_slot(Duration::from_secs(60));
        crate::sync::begin_write();
        let result = mcp::record_focus(
            snap.port,
            &snap.task.id,
            &snap.task.title,
            snap.started_at,
            snap.ended_at,
            snap.actual_ms,
            snap.planned_ms as f64 / 60000.0,
            snap.completed,
            &snap.kind,
            &snap.note,
        );
        crate::sync::end_write();
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
        // 等备注超时：按当前备注（可能是空）写入
        let note_timeout = {
            let mut g = shared.lock().unwrap();
            match g.as_mut() {
                Some(s) if s.status == Status::Finished && s.awaiting_note && now_ms() >= s.note_deadline => {
                    s.awaiting_note = false;
                    let _ = persist(&handle, s);
                    true
                }
                _ => false,
            }
        };
        if note_timeout {
            notify(&handle);
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
                    // 只有专注结束才问备注，休息不问
                    if s.kind == "work" {
                        s.awaiting_note = true;
                        s.note_deadline = now_ms() + NOTE_WAIT_MS;
                    }
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
    let started_at = now_ms();
    // 自由专注：没选任务时用合成的事件 ID，记录照常写进统计，只是不挂任何任务或块
    let mut task = task;
    if task.id.trim().is_empty() {
        task.id = format!("free_{started_at}");
        task.block_id = None;
        if task.title.trim().is_empty() {
            task.title = "番茄专注".to_string();
        }
    }
    let planned_ms = (minutes * 60000.0).round() as i64;
    let s = TimerState {
        task,
        planned_ms,
        started_at,
        paused_total: 0,
        paused_at: None,
        status: Status::Running,
        actual_ms: 0,
        completed: false,
        ended_at: 0,
        written: false,
        session_id: None,
        duration: None,
        kind: default_kind(),
        awaiting_note: false,
        note_deadline: 0,
        note: String::new(),
        port,
        work_ms: Some(planned_ms),
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

/// 番茄记录写入后，接着开始休息倒计时（短休息或长休息），休息记在同一个任务名下。
#[tauri::command]
pub fn timer_start_break(
    app: AppHandle,
    state: State<TimerHandle>,
    minutes: f64,
    long: bool,
    port: u16,
) -> Result<(), String> {
    if !(minutes > 0.0) {
        return Err("时长必须大于 0".to_string());
    }
    let mut g = state.0.lock().unwrap();
    let prev = match g.as_ref() {
        Some(s) if s.status == Status::Finished && s.written && s.kind == "work" => s.clone(),
        Some(_) => return Err("要等番茄计时结束并写入记录后才能开始休息".to_string()),
        None => return Err("没有刚完成的番茄".to_string()),
    };
    let s = TimerState {
        task: prev.task,
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
        kind: if long { "longBreak" } else { "shortBreak" }.to_string(),
        awaiting_note: false,
        note_deadline: 0,
        note: String::new(),
        port,
        // 休息之后接续的番茄沿用这个时长
        work_ms: prev.work_ms.or(Some(prev.planned_ms)),
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

/// 「接续下一个番茄」的计划时长。抽出成纯函数，方便单元测试覆盖各种分支。
/// 只有「休息已结束且记录已写入」才允许接续；时长优先用调用方给的分钟数，
/// 否则沿用上一个番茄的 `work_ms`，老版本状态里没有就退回 25 分钟。
fn plan_next_work(prev: &TimerState, minutes: Option<f64>) -> Result<i64, String> {
    if !(prev.status == Status::Finished && prev.written) || prev.kind == "work" {
        return Err("要等休息结束后才能接续下一个番茄".to_string());
    }
    Ok(match minutes {
        Some(m) if m > 0.0 => (m * 60000.0).round() as i64,
        _ => prev.work_ms.unwrap_or(25 * 60_000),
    })
}

/// 休息结束后一键接续下一个番茄：清掉已写入的休息状态，用同一个任务、上次的专注时长再开一个。
/// 迷你窗口里点一下就能连着做，不用回主窗口。
#[tauri::command]
pub fn timer_next_work(
    app: AppHandle,
    state: State<TimerHandle>,
    minutes: Option<f64>,
    port: u16,
) -> Result<(), String> {
    let mut g = state.0.lock().unwrap();
    let prev = match g.as_ref() {
        Some(s) => s.clone(),
        None => return Err("没有刚结束的休息".to_string()),
    };
    let planned_ms = plan_next_work(&prev, minutes)?;
    let started_at = now_ms();
    let s = TimerState {
        task: prev.task,
        planned_ms,
        started_at,
        paused_total: 0,
        paused_at: None,
        status: Status::Running,
        actual_ms: 0,
        completed: false,
        ended_at: 0,
        written: false,
        session_id: None,
        duration: None,
        kind: default_kind(),
        awaiting_note: false,
        note_deadline: 0,
        note: String::new(),
        port,
        work_ms: Some(planned_ms),
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
        // 休息提前结束不记录（和原版一样，只记录完整休息），直接清掉
        if s.kind != "work" {
            remove_file(&app)?;
            *g = None;
            drop(g);
            notify(&app);
            return Ok(());
        }
        let now = now_ms();
        s.ended_at = now;
        s.actual_ms = elapsed_ms(s, now).clamp(0, s.planned_ms);
        s.completed = false;
        s.paused_at = None;
        s.status = Status::Finished;
        s.awaiting_note = true;
        s.note_deadline = now + NOTE_WAIT_MS;
        persist(&app, s)?;
    }
    notify(&app);
    Ok(())
}

/// 提交备注并写入记录；传空字符串就是“跳过”。
#[tauri::command]
pub fn timer_submit_note(app: AppHandle, state: State<TimerHandle>, note: String) -> Result<(), String> {
    {
        let mut g = state.0.lock().unwrap();
        let s = g.as_mut().ok_or("没有计时")?;
        if s.status != Status::Finished || s.written {
            return Err("没有需要备注的记录".to_string());
        }
        s.note = note.trim().to_string();
        s.awaiting_note = false;
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
        _ => (250.0, 200.0, true),
    };
    // 普通模式可以拖边缘改大（备注输入框随之变大），紧凑样式尺寸固定
    w.set_resizable(decorations).map_err(|e| e.to_string())?;
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
        .skip_taskbar(true)
        .inner_size(250.0, 200.0)
        .min_inner_size(230.0, 170.0)
        .resizable(true)
        .always_on_top(true)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(status: Status, kind: &str, written: bool, work_ms: Option<i64>) -> TimerState {
        TimerState {
            task: TaskRef {
                id: "reminder_1".to_string(),
                title: "测试任务".to_string(),
                block_id: None,
            },
            planned_ms: 5 * 60_000,
            started_at: 0,
            paused_total: 0,
            paused_at: None,
            status,
            actual_ms: 0,
            completed: true,
            ended_at: 0,
            written,
            session_id: None,
            duration: None,
            kind: kind.to_string(),
            awaiting_note: false,
            note_deadline: 0,
            note: String::new(),
            port: 6806,
            work_ms,
            writing: false,
            error: None,
            recovered: false,
        }
    }

    #[test]
    fn next_work_reuses_previous_work_duration() {
        let s = st(Status::Finished, "shortBreak", true, Some(40 * 60_000));
        assert_eq!(plan_next_work(&s, None).unwrap(), 40 * 60_000);
    }

    #[test]
    fn next_work_explicit_minutes_wins() {
        let s = st(Status::Finished, "longBreak", true, Some(40 * 60_000));
        assert_eq!(plan_next_work(&s, Some(10.0)).unwrap(), 10 * 60_000);
    }

    #[test]
    fn next_work_ignores_invalid_minutes() {
        let s = st(Status::Finished, "shortBreak", true, Some(30 * 60_000));
        assert_eq!(plan_next_work(&s, Some(0.0)).unwrap(), 30 * 60_000);
        assert_eq!(plan_next_work(&s, Some(-5.0)).unwrap(), 30 * 60_000);
    }

    #[test]
    fn next_work_defaults_to_25_without_history() {
        // 老版本 timer.json 没有 work_ms
        let s = st(Status::Finished, "shortBreak", true, None);
        assert_eq!(plan_next_work(&s, None).unwrap(), 25 * 60_000);
    }

    #[test]
    fn next_work_rejected_unless_finished_written_break() {
        let running = st(Status::Running, "shortBreak", false, None);
        assert!(plan_next_work(&running, None).is_err());
        let not_written = st(Status::Finished, "shortBreak", false, None);
        assert!(plan_next_work(&not_written, None).is_err());
        let work = st(Status::Finished, "work", true, Some(25 * 60_000));
        assert!(plan_next_work(&work, None).is_err());
    }
}
