//! 强制模式：休息时全屏遮罩。显示与隐藏全由后端决定，前端只负责画倒计时。

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// 三个独立的强制模式开关；老配置里没有这一段时全部按关闭处理。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ForceBreak {
    pub micro: bool,
    pub short: bool,
    pub long: bool,
}

/// 当前计时是否该显示全屏遮罩：只有休息正在跑、且对应开关开着才算。
pub fn break_wants_overlay(kind: &str, running: bool, force: &ForceBreak) -> bool {
    if !running {
        return false;
    }
    match kind {
        "shortBreak" => force.short,
        "longBreak" => force.long,
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Owner {
    Micro,
    Break,
}

pub struct Screen(pub Mutex<Option<Owner>>);

pub fn init(app: &mut tauri::App) {
    app.manage(Screen(Mutex::new(None)));
}

const LABEL: &str = "break-screen";

/// 显示遮罩。窗口第一次用时创建，之后复用。
pub fn show(app: &AppHandle, owner: Owner) {
    let state = app.state::<Screen>();
    {
        let mut cur = state.0.lock().unwrap();
        if *cur == Some(owner) {
            if let Some(w) = app.get_webview_window(LABEL) {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            return;
        }
        *cur = Some(owner);
    }
    let built = match app.get_webview_window(LABEL) {
        Some(w) => Ok(w),
        None => tauri::WebviewWindowBuilder::new(
            app,
            LABEL,
            tauri::WebviewUrl::App("break-screen.html".into()),
        )
        .title("休息")
        .fullscreen(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(true)
        .build(),
    };
    match built {
        Ok(w) => {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        Err(e) => {
            // 遮罩建不出来不影响计时，清掉 owner 下一次还能重试
            eprintln!("break-screen 创建失败：{e}");
            *state.0.lock().unwrap() = None;
        }
    }
}

/// 只有当前 owner 才能把遮罩关掉，避免微休息与休息互相串台。
pub fn hide_if(app: &AppHandle, owner: Owner) {
    let state = app.state::<Screen>();
    {
        let mut cur = state.0.lock().unwrap();
        if *cur != Some(owner) {
            return;
        }
        *cur = None;
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_all_off() {
        let f = ForceBreak::default();
        assert!(!f.micro && !f.short && !f.long);
    }

    #[test]
    fn old_config_without_force_break_still_parses() {
        let json = r#"{"kernel_path":"","workspace":"","port":6806}"#;
        let c: crate::Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.force_break, ForceBreak::default());
    }

    #[test]
    fn overlay_only_for_running_break_with_its_switch_on() {
        let short_on = ForceBreak { micro: false, short: true, long: false };
        assert!(break_wants_overlay("shortBreak", true, &short_on));
        assert!(!break_wants_overlay("longBreak", true, &short_on));
        assert!(!break_wants_overlay("work", true, &short_on));

        let long_on = ForceBreak { micro: false, short: false, long: true };
        assert!(break_wants_overlay("longBreak", true, &long_on));
        assert!(!break_wants_overlay("longBreak", false, &long_on));
        assert!(!break_wants_overlay("shortBreak", false, &short_on));
    }
}
