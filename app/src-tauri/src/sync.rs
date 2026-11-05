//! 定时触发思源「原生同步」。
//!
//! 背景：由本工具单独启动的后台内核**不会自己定时同步**（实测 11/04 全天 0 次，
//! 且 `siyuan sync status` 显示 `Mode: manual`），所以手机端拿不到新的番茄记录。
//! 这里只做一件事：按**思源自己的同步间隔**（conf.json 的 `sync.interval`）定时调
//! `POST /api/sync/performSync`，外加一个「立即同步」按钮。
//!
//! 代理按「方案丁」不处理：只把检测到的代理状态显示出来，提醒同步可能变慢。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::mcp;

/// 同步进行中（只在内存里）
static SYNCING: AtomicBool = AtomicBool::new(false);
/// 正在写番茄记录的线程数；同步开始前等它归零，避免和同步的 checkout 抢文件
static WRITING: AtomicUsize = AtomicUsize::new(0);

/// 定时器轮询间隔（只是检查频率，不是同步间隔；思源间隔最小 30 秒，这里 10 秒检查一次才跟得上）
const TICK: Duration = Duration::from_secs(10);
/// 同步开始前最多等写记录收尾多久
const WRITE_DRAIN: Duration = Duration::from_secs(60);
/// 同步失败后的重试间隔
const RETRY_AFTER_FAIL: i64 = 5 * 60 * 1000;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SyncState {
    /// 上次同步成功的时间（毫秒）
    #[serde(default)]
    pub last_sync_at: i64,
    /// 上次尝试的时间（成功或失败都算），调度用
    #[serde(default)]
    pub last_attempt_at: i64,
    #[serde(default)]
    pub last_ok: bool,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub duration_ms: i64,
    /// 内核返回的同步摘要（已把 <br> 之类换成可读文本）
    #[serde(default)]
    pub last_stat: String,
    /// 本次同步开始时刻，只在内存里给界面显示「同步中 12s」
    #[serde(skip)]
    pub started_at: i64,
}

pub struct Handle(pub Arc<Mutex<SyncState>>);

/// 思源自己 conf.json 里的同步设置
struct SiyuanSync {
    enabled: bool,
    /// 秒。思源官方文档：同步间隔默认 30 秒，范围 30–43200 秒；<=0 视为没设置
    interval_sec: i64,
}

/// 思源自己的最小间隔就是 30 秒，跟它对齐，避免配置被改坏后疯狂同步
const MIN_INTERVAL_SEC: i64 = 30;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn state_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::config_dir(app)?.join("sync-state.json"))
}

fn persist(app: &AppHandle, s: &SyncState) -> Result<(), String> {
    let path = state_path(app)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn conf_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("conf").join("conf.json")
}

fn read_conf(workspace: &str) -> Option<serde_json::Value> {
    if workspace.trim().is_empty() {
        return None;
    }
    let text = fs::read_to_string(conf_path(workspace)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 读思源自己的同步设置：是否启用 + 同步间隔（秒）
fn read_siyuan_sync(workspace: &str) -> Option<SiyuanSync> {
    let v = read_conf(workspace)?;
    Some(SiyuanSync {
        enabled: v["sync"]["enabled"].as_bool().unwrap_or(false),
        interval_sec: v["sync"]["interval"].as_i64().unwrap_or(0),
    })
}

/// 内核当前用没用代理（只做提示，不修改）。返回 Some(说明) 表示确实在走代理。
fn proxy_warning(workspace: &str) -> Option<String> {
    let v = read_conf(workspace)?;
    let p = &v["system"]["networkProxy"];
    let scheme = p["scheme"].as_str().unwrap_or("").trim();
    if scheme.is_empty() {
        return None; // 直连
    }
    if scheme == "system" {
        return Some("思源设为「使用系统代理」，同步会走代理，大同步可能明显变慢".to_string());
    }
    let host = p["host"].as_str().unwrap_or("");
    let port = p["port"].as_str().unwrap_or("");
    Some(format!(
        "思源网络代理为 {scheme}://{host}:{port}，同步会走代理，大同步可能明显变慢（思源 设置 → 关于 → 网络代理 可改直连）"
    ))
}

/// 内核返回的摘要里带 HTML 换行，转成一行可读文本
fn pretty_stat(raw: &str) -> String {
    raw.replace("<br>", "；")
        .replace("&emsp;", " ")
        .replace("&nbsp;", " ")
        .trim()
        .trim_start_matches('；')
        .to_string()
}

/// 写记录前调用：同步进行中时等一会儿（最多 max），避免和同步的 checkout 抢文件。
/// 等不到也放行——实测并发写入不会丢记录，宁可能写进去别卡住记录。
pub fn wait_write_slot(max: Duration) {
    let mut waited = Duration::ZERO;
    while SYNCING.load(Ordering::SeqCst) && waited < max {
        std::thread::sleep(Duration::from_millis(200));
        waited += Duration::from_millis(200);
    }
}

pub fn begin_write() {
    WRITING.fetch_add(1, Ordering::SeqCst);
}

pub fn end_write() {
    WRITING.fetch_sub(1, Ordering::SeqCst);
}

/// 真正跑一次同步（阻塞，可能几百秒）。同一时刻只允许一个。
fn run_sync(app: &AppHandle, port: u16) -> Result<String, String> {
    if SYNCING.swap(true, Ordering::SeqCst) {
        return Err("已经有一次同步在进行".to_string());
    }
    let shared = app.state::<Handle>().0.clone();
    {
        let mut s = shared.lock().unwrap();
        s.started_at = now_ms();
        s.last_attempt_at = s.started_at;
    }
    let _ = app.emit("sync-changed", ());

    let t0 = Instant::now();
    // 等正在写的番茄记录写完
    let mut waited = Duration::ZERO;
    while WRITING.load(Ordering::SeqCst) > 0 && waited < WRITE_DRAIN {
        std::thread::sleep(Duration::from_millis(200));
        waited += Duration::from_millis(200);
    }

    let before = mcp::sync_info(port).ok().map(|i| i.synced);
    let result = mcp::perform_sync(port).and_then(|_| mcp::sync_info(port));
    let duration_ms = t0.elapsed().as_millis() as i64;

    let (msg, outcome) = match result {
        Ok(info) => {
            let advanced = before.map(|b| info.synced > b).unwrap_or(true);
            let mut m = if advanced {
                "同步完成".to_string()
            } else {
                "同步完成（内核的同步时间没有推进：可能没有变更）".to_string()
            };
            if !info.stat.is_empty() {
                m.push_str(&format!("：{}", pretty_stat(&info.stat)));
            }
            (m, Ok(info))
        }
        Err(e) => (e.clone(), Err(e)),
    };

    {
        let mut s = shared.lock().unwrap();
        s.started_at = 0;
        s.duration_ms = duration_ms;
        match &outcome {
            Ok(info) => {
                s.last_ok = true;
                s.last_error = None;
                s.last_sync_at = now_ms();
                s.last_stat = pretty_stat(&info.stat);
            }
            Err(e) => {
                s.last_ok = false;
                s.last_error = Some(e.clone());
            }
        }
        let _ = persist(app, &s);
    }
    SYNCING.store(false, Ordering::SeqCst);
    let _ = app.emit("sync-changed", ());

    match outcome {
        Ok(_) => Ok(msg),
        Err(e) => Err(e),
    }
}

pub fn init(app: &mut tauri::App) -> Result<(), String> {
    let handle = app.handle().clone();
    let mut state = SyncState::default();
    if let Ok(path) = state_path(&handle) {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(s) = serde_json::from_str::<SyncState>(&text) {
                state = s;
            }
        }
    }
    let shared = Arc::new(Mutex::new(state));
    app.manage(Handle(shared.clone()));

    // 定时器：间隔完全跟随思源自己的设置（conf.json 的 sync.interval，单位分钟）
    std::thread::spawn(move || loop {
        std::thread::sleep(TICK);
        let cfg = crate::read_config(&handle);
        if !cfg.sync_enabled {
            continue;
        }
        let Some(ys) = read_siyuan_sync(&cfg.workspace) else {
            continue;
        };
        if !ys.enabled || ys.interval_sec <= 0 {
            continue; // 思源没开同步 / 没设间隔，就不自动触发
        }
        if SYNCING.load(Ordering::SeqCst) {
            continue;
        }
        let (last_ok, base) = {
            let s = shared.lock().unwrap();
            (s.last_ok, s.last_attempt_at.max(s.last_sync_at))
        };
        let interval = if last_ok {
            ys.interval_sec.max(MIN_INTERVAL_SEC) * 1000
        } else {
            RETRY_AFTER_FAIL
        };
        if now_ms() - base < interval {
            continue;
        }
        // 内核没在跑就安静跳过，等下一轮
        if mcp::sync_info(cfg.port).is_err() {
            continue;
        }
        let _ = run_sync(&handle, cfg.port);
    });
    Ok(())
}

#[derive(Serialize)]
pub struct Status {
    pub syncing: bool,
    pub started_at: i64,
    pub elapsed_ms: i64,
    pub last_sync_at: i64,
    pub last_ok: bool,
    pub last_error: Option<String>,
    pub duration_ms: i64,
    pub last_stat: String,
    /// 思源自己的同步设置（只读展示）
    pub siyuan_enabled: bool,
    /// 思源配置的同步间隔（秒）
    pub siyuan_interval_sec: i64,
    /// 检测到内核走代理时的提示（方案丁：只提示不修改）
    pub proxy_warning: Option<String>,
}

#[tauri::command]
pub fn sync_status(state: State<Handle>) -> Status {
    let s = state.0.lock().unwrap().clone();
    let syncing = SYNCING.load(Ordering::SeqCst);
    // 状态查询没有 AppHandle，直接读配置文件；
    // 结果缓存 60 秒，避免界面每 3 秒轮询都去读 conf.json
    let (siyuan_enabled, siyuan_interval_sec, proxy_warning) = SiyuanCache::get();
    Status {
        syncing,
        started_at: s.started_at,
        elapsed_ms: if s.started_at > 0 { now_ms() - s.started_at } else { 0 },
        last_sync_at: s.last_sync_at,
        last_ok: s.last_ok,
        last_error: s.last_error,
        duration_ms: s.duration_ms,
        last_stat: s.last_stat,
        siyuan_enabled,
        siyuan_interval_sec,
        proxy_warning,
    }
}

/// 思源同步设置的只读缓存
struct SiyuanCache;

impl SiyuanCache {
    fn get() -> (bool, i64, Option<String>) {
        static CACHE: Mutex<Option<(i64, (bool, i64, Option<String>))>> = Mutex::new(None);
        let mut c = CACHE.lock().unwrap();
        if let Some((at, v)) = c.as_ref() {
            if now_ms() - at < 60_000 {
                return v.clone();
            }
        }
        let workspace = std::env::var("APPDATA")
            .ok()
            .map(|d| PathBuf::from(d).join("com.local.siyuan-pomodoro").join("config.json"))
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["workspace"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let ys = read_siyuan_sync(&workspace);
        let v = (
            ys.as_ref().map(|y| y.enabled).unwrap_or(false),
            ys.map(|y| y.interval_sec).unwrap_or(0),
            proxy_warning(&workspace),
        );
        *c = Some((now_ms(), v.clone()));
        v
    }
}

#[tauri::command]
pub fn sync_now(app: AppHandle, port: u16) -> Result<String, String> {
    run_sync(&app, port)
}
