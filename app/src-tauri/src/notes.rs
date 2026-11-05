//! 悬浮笔记：番茄期间随手记做了什么，带复选框。
//! 内容每次改动都由前端交给后端，原子写入 notes.json，窗口被关掉或程序崩溃都不会丢。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

const MAX_ITEMS: usize = 500;
const MAX_TEXT_CHARS: usize = 2000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NoteItem {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

fn notes_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::config_dir(app)?.join("notes.json"))
}

#[tauri::command]
pub fn notes_get(app: AppHandle) -> Result<Vec<NoteItem>, String> {
    let path = notes_path(&app)?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    match serde_json::from_str::<Vec<NoteItem>>(&text) {
        Ok(items) => Ok(items),
        Err(e) => {
            // 文件损坏：改名保留，不覆盖，界面从空白开始
            let _ = fs::rename(&path, path.with_extension("json.corrupt"));
            Err(format!("悬浮笔记文件损坏，已改名保留为 notes.json.corrupt：{e}"))
        }
    }
}

#[tauri::command]
pub fn notes_save(app: AppHandle, items: Vec<NoteItem>) -> Result<(), String> {
    if items.len() > MAX_ITEMS {
        return Err(format!("悬浮笔记最多 {MAX_ITEMS} 条"));
    }
    let items: Vec<NoteItem> = items
        .into_iter()
        .map(|mut i| {
            i.text = i.text.chars().take(MAX_TEXT_CHARS).collect();
            i
        })
        .collect();
    let path = notes_path(&app)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string(&items).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    let _ = app.emit("notes-changed", ());
    Ok(())
}

/// 打开（或唤起）悬浮笔记窗口：置顶、可缩放，和迷你计时窗口互相独立。
pub fn show_notes(app: &AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("notes") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(app, "notes", tauri::WebviewUrl::App("notes.html".into()))
        .title("悬浮笔记")
        .skip_taskbar(true)
        .inner_size(300.0, 380.0)
        .min_inner_size(220.0, 160.0)
        .resizable(true)
        .always_on_top(true)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_notes(app: AppHandle) -> Result<(), String> {
    show_notes(&app)
}
