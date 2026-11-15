//! 强制模式：休息时全屏遮罩。显示与隐藏全由后端决定，前端只负责画倒计时。

use serde::{Deserialize, Serialize};

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
