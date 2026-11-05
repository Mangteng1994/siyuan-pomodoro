//! 随机微休息：后台调度，专注计时继续，不写入番茄或长短休息记录。
use std::hash::{BuildHasher, Hasher};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub min_minutes: f64,
    pub max_minutes: f64,
    pub duration_seconds: u32,
    pub popup: bool,
    pub sound: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self { enabled: false, min_minutes: 3.0, max_minutes: 5.0,
            duration_seconds: 10, popup: true, sound: true }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled && !self.popup && !self.sound {
            return Err("启用微休息时，请至少开启弹窗或提示音".into());
        }
        if !self.min_minutes.is_finite() || !self.max_minutes.is_finite()
            || self.min_minutes < 0.1 || self.max_minutes < self.min_minutes
            || self.max_minutes > 1440.0 || !(1..=3600).contains(&self.duration_seconds) {
            return Err("微休息间隔需为 0.1～1440 分钟，最大值不能小于最小值；时长需为 1～3600 秒".into());
        }
        Ok(())
    }
    fn interval(&self) -> Duration {
        // RandomState 每次创建时使用随机种子；这里只需随机提醒间隔。
        let sample = std::collections::hash_map::RandomState::new().build_hasher().finish();
        let fraction = sample as f64 / u64::MAX as f64;
        Duration::from_secs_f64((self.min_minutes + (self.max_minutes - self.min_minutes) * fraction) * 60.0)
    }
}

#[derive(Clone, Default, Serialize)]
pub struct View { pub remaining_seconds: u32, pub active: bool }
pub struct Handle(Mutex<View>);

#[tauri::command]
pub fn micro_break_get(state: State<Handle>) -> View { state.0.lock().unwrap().clone() }

#[tauri::command]
pub fn micro_break_dismiss(app: AppHandle, state: State<Handle>) {
    *state.0.lock().unwrap() = View::default();
    if let Some(w) = app.get_webview_window("micro-break") { let _ = w.hide(); }
}

fn publish(app: &AppHandle, view: View) {
    *app.state::<Handle>().0.lock().unwrap() = view.clone();
    let _ = app.emit("micro-break-changed", &view);
    if !view.active {
        if let Some(w) = app.get_webview_window("micro-break") { let _ = w.hide(); }
    }
}

fn beep() {
    #[link(name = "user32")]
    extern "system" { fn MessageBeep(kind: u32) -> i32; }
    unsafe { MessageBeep(0x40); }
}

pub fn init(app: &mut tauri::App) {
    app.manage(Handle(Mutex::new(View::default())));
    let app = app.handle().clone();
    std::thread::spawn(move || {
        let mut settings = Settings::default();
        let mut session = None;
        let mut wait = settings.interval();
        let mut end: Option<Instant> = None;
        let mut last = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let now = Instant::now();
            // 唤醒后不补发多次提醒，也不把休眠时间当作持续工作。
            let delta = now.duration_since(last).min(Duration::from_secs(1));
            last = now;
            let config = crate::read_config(&app).micro_break;
            if config != settings {
                settings = config;
                if settings.validate().is_err() { settings = Settings::default(); }
                wait = settings.interval();
                end = None;
                publish(&app, View::default());
            }
            let snap = app.state::<crate::timer::TimerHandle>().0.lock().unwrap().clone();
            let current = snap.as_ref().map(|s| (s.started_at, s.task.id.clone()));
            if current != session {
                session = current;
                wait = settings.interval();
                end = None;
                publish(&app, View::default());
            }
            let running = settings.enabled && snap.as_ref().map_or(false, |s|
                s.kind == "work" && s.status == crate::timer::Status::Running);
            if !running {
                if end.take().is_some() { publish(&app, View::default()); wait = settings.interval(); }
                continue;
            }
            if let Some(deadline) = end {
                let dismissed = !app.state::<Handle>().0.lock().unwrap().active;
                if now >= deadline || dismissed {
                    end = None;
                    wait = settings.interval();
                    publish(&app, View::default());
                    if !dismissed && settings.sound { beep(); }
                } else {
                    publish(&app, View { active: true,
                        remaining_seconds: deadline.duration_since(now).as_secs_f64().ceil() as u32 });
                }
            } else {
                wait = wait.saturating_sub(delta);
                if !wait.is_zero() { continue; }
                end = Some(now + Duration::from_secs(settings.duration_seconds as u64));
                publish(&app, View { active: true, remaining_seconds: settings.duration_seconds });
                if settings.sound { beep(); }
                if settings.popup {
                    let result = if let Some(w) = app.get_webview_window("micro-break") { Ok(w) } else {
                        tauri::WebviewWindowBuilder::new(&app, "micro-break", tauri::WebviewUrl::App("micro-break.html".into()))
                            .title("微休息").skip_taskbar(true)
                            .inner_size(300.0, 220.0).resizable(false).always_on_top(true).build()
                    };
                    if let Ok(w) = result {
                        let _ = w.show(); let _ = w.unminimize(); let _ = w.set_focus();
                    }
                }
            }
        }
    });
}
