//! 检查 GitHub 是否有新版本，并在软件内下载、替换、重启。
//!
//! 流程：定时（启动后 30 秒、之后每 6 小时）或手动调 `releases/latest`，版本比当前新就记下
//! 安装包地址并通知界面；用户点“下载并安装”后下载到配置目录，核对 SHA-256，然后把正在运行
//! 的 exe 改名为 `.old`（Windows 允许改名正在运行的文件）、复制新 exe 到原位置，
//! 由一个隐藏的 cmd 稍后重新拉起新版，旧进程退出。`.old` 在下次启动时清理。
//!
//! 开发版（debug 构建）只检查、不安装，免得把 `target\debug` 里的 exe 换掉。

use serde::Serialize;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

const REPO: &str = "Mangteng1994/siyuan-pomodoro";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Serialize, Clone, Default)]
pub struct UpdateState {
    pub current: String,
    /// idle | checking | available | downloading | ready | installing | error
    pub status: String,
    pub latest: String,
    pub notes: String,
    pub page_url: String,
    pub size: u64,
    pub downloaded: u64,
    pub error: Option<String>,
    /// 最近一次检查成功的时间（毫秒）
    pub checked_at: i64,
    /// 开发版不能自动安装
    pub can_install: bool,
    #[serde(skip)]
    asset_url: String,
    #[serde(skip)]
    sha256: Option<String>,
    #[serde(skip)]
    sums_url: Option<String>,
}

pub struct Handle(Arc<Mutex<UpdateState>>);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// "v0.1.8" / "0.1.8" → [0,1,8]；解析不了的段按 0 算
fn parse_version(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

fn is_newer(latest: &str, current: &str) -> bool {
    let (mut a, mut b) = (parse_version(latest), parse_version(current));
    a.resize(3, 0);
    b.resize(3, 0);
    a > b
}

/// 环境变量里的代理（GitHub 在国内常要代理）；没设就直连
fn agent(timeout: Duration) -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new().timeout(timeout);
    for key in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(url) = std::env::var(key) {
            if let Ok(p) = ureq::Proxy::new(url.trim()) {
                b = b.proxy(p);
                break;
            }
        }
    }
    b.build()
}

fn current_version() -> String {
    // 调试用：不改代码就能模拟“当前是旧版本”
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("POMODORO_FAKE_CURRENT") {
        return v;
    }
    CURRENT.to_string()
}

fn fetch_latest() -> Result<serde_json::Value, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = agent(Duration::from_secs(15))
        .get(&url)
        .set("User-Agent", "siyuan-pomodoro-updater")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(403 | 429, _) => "GitHub 访问次数受限，稍后再试".to_string(),
            ureq::Error::Status(404, _) => "还没有发布过版本".to_string(),
            other => format!("连不上 GitHub（国内可能需要代理，可设置 HTTPS_PROXY 环境变量）：{other}"),
        })?;
    let text = resp.into_string().map_err(|e| format!("读取 GitHub 返回失败：{e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析 GitHub 返回失败：{e}"))
}

/// 在状态里记下检查结果；返回是否有新版本
fn apply_release(st: &mut UpdateState, rel: &serde_json::Value) {
    let tag = rel["tag_name"].as_str().unwrap_or("").to_string();
    st.checked_at = now_ms();
    st.error = None;
    st.current = current_version();
    if tag.is_empty() || !is_newer(&tag, &st.current) {
        st.status = "idle".into();
        st.latest = tag;
        return;
    }
    let assets = rel["assets"].as_array().cloned().unwrap_or_default();
    let exe = assets.iter().find(|a| {
        a["name"].as_str().map(|n| n.ends_with("windows-x64.exe")).unwrap_or(false)
    });
    let Some(exe) = exe else {
        st.status = "error".into();
        st.error = Some(format!("新版本 {tag} 没有找到 Windows 安装包"));
        return;
    };
    st.latest = tag;
    st.notes = rel["body"].as_str().unwrap_or("").trim().to_string();
    st.page_url = rel["html_url"].as_str().unwrap_or("").to_string();
    st.asset_url = exe["browser_download_url"].as_str().unwrap_or("").to_string();
    st.size = exe["size"].as_u64().unwrap_or(0);
    st.sha256 = exe["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(|s| s.to_lowercase());
    st.sums_url = assets
        .iter()
        .find(|a| a["name"].as_str().map(|n| n.starts_with("SHA256SUMS")).unwrap_or(false))
        .and_then(|a| a["browser_download_url"].as_str().map(String::from));
    st.downloaded = 0;
    // 已经下载好了同一个版本就不用退回“可下载”
    if st.status != "ready" {
        st.status = "available".into();
    }
}

fn publish(app: &AppHandle, shared: &Arc<Mutex<UpdateState>>) {
    let snapshot = shared.lock().unwrap().clone();
    let _ = app.emit("update-changed", &snapshot);
}

fn do_check(app: &AppHandle, shared: &Arc<Mutex<UpdateState>>) {
    {
        let mut st = shared.lock().unwrap();
        if matches!(st.status.as_str(), "checking" | "downloading" | "installing") {
            return;
        }
        st.status = "checking".into();
        st.error = None;
    }
    publish(app, shared);
    let result = fetch_latest();
    {
        let mut st = shared.lock().unwrap();
        st.current = current_version();
        match result {
            Ok(rel) => {
                apply_release(&mut st, &rel);
            }
            Err(e) => {
                st.status = "error".into();
                st.error = Some(e);
            }
        }
    }
    publish(app, shared);
}

pub fn init(app: &mut tauri::App) -> Result<(), String> {
    let handle = app.handle().clone();
    // 清理上次更新留下的旧 exe
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(exe.with_extension("old"));
    }
    let shared = Arc::new(Mutex::new(UpdateState {
        current: current_version(),
        status: "idle".into(),
        can_install: !cfg!(debug_assertions),
        ..Default::default()
    }));
    app.manage(Handle(shared.clone()));
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK);
        loop {
            do_check(&handle, &shared);
            std::thread::sleep(CHECK_EVERY);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn update_get(state: State<Handle>) -> UpdateState {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub async fn update_check(app: AppHandle, state: State<'_, Handle>) -> Result<(), String> {
    let shared = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_check(&app, &shared))
        .await
        .map_err(|e| e.to_string())
}

fn sha256_of(path: &PathBuf) -> Result<String, String> {
    let out = Command::new("certutil")
        .arg("-hashfile")
        .arg(path)
        .arg("SHA256")
        .creation_flags_hidden()
        .output()
        .map_err(|e| format!("无法计算校验值：{e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .map(|l| l.trim().replace(' ', "").to_lowercase())
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| "无法计算校验值".to_string())
}

trait HiddenCmd {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}
impl HiddenCmd for Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(0x0800_0000)
    }
}

fn expected_hash(st: &UpdateState) -> Option<String> {
    if let Some(h) = &st.sha256 {
        return Some(h.clone());
    }
    let url = st.sums_url.as_ref()?;
    let text = agent(Duration::from_secs(15))
        .get(url)
        .set("User-Agent", "siyuan-pomodoro-updater")
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let name = st.asset_url.rsplit('/').next()?;
    text.lines().find(|l| l.contains(name)).and_then(|l| l.split_whitespace().next()).map(|s| s.to_lowercase())
}

fn download(app: &AppHandle, shared: &Arc<Mutex<UpdateState>>) -> Result<PathBuf, String> {
    let (url, size, expected) = {
        let st = shared.lock().unwrap();
        (st.asset_url.clone(), st.size, expected_hash(&st))
    };
    if url.is_empty() {
        return Err("没有可下载的安装包，请先检查更新".into());
    }
    let dir = crate::config_dir(app)?.join("update");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join("new.exe");
    let part = dir.join("new.exe.part");
    let resp = agent(Duration::from_secs(600))
        .get(&url)
        .set("User-Agent", "siyuan-pomodoro-updater")
        .call()
        .map_err(|e| format!("下载失败：{e}"))?;
    let mut reader = resp.into_reader();
    let mut file = fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("下载中断：{e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if last_emit.elapsed() > Duration::from_millis(300) {
            shared.lock().unwrap().downloaded = done;
            publish(app, shared);
            last_emit = std::time::Instant::now();
        }
    }
    drop(file);
    if size > 0 && done != size {
        let _ = fs::remove_file(&part);
        return Err(format!("下载不完整（{done}/{size} 字节），请重试"));
    }
    if let Some(want) = expected {
        let got = sha256_of(&part)?;
        if got != want {
            let _ = fs::remove_file(&part);
            return Err("安装包校验失败（SHA-256 不一致），已丢弃，请重试".into());
        }
    }
    let _ = fs::remove_file(&target);
    fs::rename(&part, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// 下载并校验；成功后状态变 ready，等用户点“安装并重启”
#[tauri::command]
pub async fn update_download(app: AppHandle, state: State<'_, Handle>) -> Result<(), String> {
    let shared = state.0.clone();
    {
        let mut st = shared.lock().unwrap();
        if st.status != "available" && st.status != "error" {
            return Ok(());
        }
        st.status = "downloading".into();
        st.downloaded = 0;
        st.error = None;
    }
    publish(&app, &shared);
    let a = app.clone();
    let s = shared.clone();
    let result = tauri::async_runtime::spawn_blocking(move || download(&a, &s))
        .await
        .map_err(|e| e.to_string())?;
    {
        let mut st = shared.lock().unwrap();
        match &result {
            Ok(_) => {
                st.status = "ready".into();
                st.downloaded = st.size;
            }
            Err(e) => {
                st.status = if st.asset_url.is_empty() { "error".into() } else { "available".into() };
                st.error = Some(e.clone());
            }
        }
    }
    publish(&app, &shared);
    result.map(|_| ())
}

/// 替换当前 exe 并重启。计时状态在 timer.json 里，重启后自动恢复。
#[tauri::command]
pub fn update_install(app: AppHandle, state: State<Handle>) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("开发版不自动安装，请用发布版测试更新".into());
    }
    {
        let st = state.0.lock().unwrap();
        if st.status != "ready" {
            return Err("还没有下载好安装包".into());
        }
    }
    let new_exe = crate::config_dir(&app)?.join("update").join("new.exe");
    if !new_exe.is_file() {
        return Err("找不到已下载的安装包，请重新下载".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = exe.with_extension("old");
    let _ = fs::remove_file(&old);
    fs::rename(&exe, &old).map_err(|e| format!("无法替换当前程序（所在目录可能没有写入权限）：{e}"))?;
    if let Err(e) = fs::copy(&new_exe, &exe) {
        let _ = fs::remove_file(&exe);
        let _ = fs::rename(&old, &exe);
        return Err(format!("复制新版本失败，已恢复旧版本：{e}"));
    }
    state.0.lock().unwrap().status = "installing".into();
    publish(&app, &state.0);
    let _ = fs::remove_file(&new_exe);
    // 稍等旧进程退出（释放单实例锁）后再拉起新版
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("cmd")
            .raw_arg(format!("/C \"ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"\"", exe.display()))
            .creation_flags_hidden()
            .spawn();
    }
    app.exit(0);
    Ok(())
}

/// 更新日志（和仓库根目录的 CHANGELOG.md 同一份，编译时打进 exe，离线也能看）
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

#[derive(Serialize, PartialEq, Debug)]
pub struct ChangeEntry {
    /// "0.1.8"，未发布的写成 "未发布"
    pub version: String,
    pub date: String,
    pub notes: String,
}

fn parse_changelog(text: &str) -> Vec<ChangeEntry> {
    let mut out: Vec<ChangeEntry> = Vec::new();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            let (v, d) = head.split_once('·').map(|(a, b)| (a, b.trim())).unwrap_or((head, ""));
            out.push(ChangeEntry {
                version: v.trim().trim_start_matches('v').to_string(),
                date: d.to_string(),
                notes: String::new(),
            });
        } else if let Some(last) = out.last_mut() {
            last.notes.push_str(line);
            last.notes.push('\n');
        }
    }
    for e in &mut out {
        e.notes = e.notes.trim().to_string();
    }
    out
}

#[tauri::command]
pub fn changelog_get() -> Vec<ChangeEntry> {
    parse_changelog(CHANGELOG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.1.9", "0.1.8"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.8", "0.1.8"));
        assert!(!is_newer("v0.1.7", "0.1.8"));
        assert!(!is_newer("garbage", "0.1.8"));
    }

    #[test]
    fn picks_windows_asset() {
        let rel = serde_json::json!({
            "tag_name": "v9.9.9", "body": "说明", "html_url": "https://x/r",
            "assets": [
                {"name": "SHA256SUMS-v9.9.9.txt", "browser_download_url": "https://x/sums", "size": 10},
                {"name": "siyuan-pomodoro-v9.9.9-windows-x64.exe", "browser_download_url": "https://x/a.exe",
                 "size": 123, "digest": "sha256:ABCD"}
            ]
        });
        let mut st = UpdateState::default();
        apply_release(&mut st, &rel);
        assert_eq!(st.status, "available");
        assert_eq!(st.asset_url, "https://x/a.exe");
        assert_eq!(st.sha256.as_deref(), Some("abcd"));
        assert_eq!(st.sums_url.as_deref(), Some("https://x/sums"));
    }

    #[test]
    fn parses_changelog() {
        let list = parse_changelog("# 更新日志\n说明\n\n## 未发布\n\n- 甲\n\n## v0.1.8 · 2026-11-07\n\n- 乙\n- 丙\n");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].version, "未发布");
        assert_eq!(list[0].notes, "- 甲");
        assert_eq!(list[1].version, "0.1.8");
        assert_eq!(list[1].date, "2026-11-07");
        assert_eq!(list[1].notes, "- 乙\n- 丙");
        // 仓库里真实的日志必须能解析出当前版本
        let real = changelog_get();
        assert!(real.iter().any(|e| e.version == "0.1.8"));
    }

    #[test]
    fn same_version_is_idle() {
        let rel = serde_json::json!({"tag_name": format!("v{CURRENT}"), "assets": []});
        let mut st = UpdateState::default();
        apply_release(&mut st, &rel);
        assert_eq!(st.status, "idle");
    }
}
