//! 强制模式：休息时全屏遮罩。显示与隐藏全由后端决定，前端只负责画倒计时。

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, WebviewWindow};

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

/// 遮罩归谁所有：微休息与短/长休息是两条独立的线程，靠它避免互相把对方的遮罩关掉。
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Owner {
    Micro,
    Break,
}

/// 建窗失败后的冷却时间：失败一次就别每 250ms 重试，否则 stderr 会被刷满。
const RETRY_AFTER: Duration = Duration::from_secs(2);

/// 和遮罩同一置顶层的窗口。遮罩期间必须把它们降下去：它们和遮罩都在置顶层，
/// 点一下它们在屏幕上的位置就会被抬到遮罩之上，落下的那一下点击能直接结束休息。
const PEERS: [&str; 3] = ["mini", "notes", "micro-break"];

const LABEL: &str = "break-screen";

#[derive(Default)]
pub struct Slot {
    pub owner: Option<Owner>,
    failed_at: Option<Instant>,
}

pub struct Screen(pub Mutex<Slot>);

pub fn init(app: &mut tauri::App) {
    app.manage(Screen(Mutex::new(Slot::default())));
}

/// 记录归属。返回 true 表示「此前不是它」，调用方需要真正显示并聚焦一次。
pub fn claim(cur: &mut Option<Owner>, want: Owner) -> bool {
    if *cur == Some(want) {
        false
    } else {
        *cur = Some(want);
        true
    }
}

/// 只有当前持有者才能收回遮罩；返回 true 表示确实该隐藏窗口。
pub fn release(cur: &mut Option<Owner>, owner: Owner) -> bool {
    if *cur == Some(owner) {
        *cur = None;
        true
    } else {
        false
    }
}

fn build(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("break-screen.html".into()))
        .title("休息")
        .fullscreen(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(true)
        // 页面首帧是深色，窗口底色也用深色，免得弹出瞬间闪一下白屏
        .background_color(tauri::window::Color(17, 17, 17, 255))
        .build()
}

/// 显示遮罩。窗口第一次用时创建，之后每 250ms 复用同一次（只刷新可见状态，不再抢焦点）。
pub fn show(app: &AppHandle, owner: Owner) {
    let state = app.state::<Screen>();
    // 锁内只做判定与置位：下面的窗口操作会阻塞在主线程 dispatcher 上，
    // 拿着 Screen 的锁去等它，主线程一旦也来取锁就会死锁。
    let fresh = {
        let mut slot = state.0.lock().unwrap();
        if slot.failed_at.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
            return;
        }
        claim(&mut slot.owner, owner)
    };

    let window = match app.get_webview_window(LABEL) {
        Some(w) => w,
        None => match build(app) {
            Ok(w) => w,
            Err(e) => {
                // 遮罩建不出来不影响计时，清掉归属，过一会儿再试
                eprintln!("break-screen 创建失败：{e}");
                let mut slot = state.0.lock().unwrap();
                slot.owner = None;
                slot.failed_at = Some(Instant::now());
                return;
            }
        },
    };
    state.0.lock().unwrap().failed_at = None;

    let _ = window.show();
    let _ = window.unminimize();
    if fresh {
        // 只在这一轮第一次显示时抢焦点。每 250ms 抢一次会把托盘菜单顶掉，
        // 而托盘「退出」是遮罩期间唯一的逃生口。
        let _ = window.set_focus();
    }
    for label in PEERS {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.set_always_on_top(false);
        }
    }
}

/// 只有当前 owner 才能把遮罩关掉，避免微休息与休息互相串台。
pub fn hide_if(app: &AppHandle, owner: Owner) {
    let state = app.state::<Screen>();
    let hide = {
        let mut slot = state.0.lock().unwrap();
        release(&mut slot.owner, owner)
    };
    if !hide {
        return;
    }
    for label in PEERS {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.set_always_on_top(true);
        }
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

    #[test]
    fn claim_is_idempotent_and_switches_owner() {
        let mut cur = None;
        assert!(claim(&mut cur, Owner::Micro));
        assert_eq!(cur, Some(Owner::Micro));
        // 同一轮里每次 tick 都调一次，重复调用不该再触发聚焦
        assert!(!claim(&mut cur, Owner::Micro));
        assert!(claim(&mut cur, Owner::Break));
        assert_eq!(cur, Some(Owner::Break));
    }

    #[test]
    fn release_only_clears_its_own_owner() {
        let mut cur = Some(Owner::Break);
        // 微休息结束不能把正在跑的短休息遮罩关掉
        assert!(!release(&mut cur, Owner::Micro));
        assert_eq!(cur, Some(Owner::Break));
        assert!(release(&mut cur, Owner::Break));
        assert_eq!(cur, None);
        // 已经收起来后再收一次，不该再动窗口
        assert!(!release(&mut cur, Owner::Break));
    }

    #[test]
    fn released_overlay_can_be_claimed_again() {
        let mut cur = Some(Owner::Micro);
        assert!(release(&mut cur, Owner::Micro));
        assert!(claim(&mut cur, Owner::Micro));
    }
}
