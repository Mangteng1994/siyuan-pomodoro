//! 调用内核 /mcp 接口里的插件工具（MCP Streamable HTTP）。
//! 每次调用新建一个会话：initialize → initialized → tools/list（按名字片段找工具）→ tools/call → 关闭会话。

use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build()
}

fn post(port: u16, session: Option<&str>, body: &Value) -> Result<(Option<String>, Value), String> {
    let mut req = agent()
        .post(&format!("http://127.0.0.1:{port}/mcp"))
        .set("Content-Type", "application/json")
        .set("Accept", "application/json, text/event-stream");
    if let Some(s) = session {
        req = req.set("Mcp-Session-Id", s);
    }
    let resp = req
        .send_string(&body.to_string())
        .map_err(|e| format!("内核没有响应（端口 {port}）：{e}"))?;
    let sid = resp.header("Mcp-Session-Id").map(|s| s.to_string());
    let text = resp.into_string().map_err(|e| e.to_string())?;
    let text = text.trim();
    if text.is_empty() {
        return Ok((sid, Value::Null));
    }
    // 兼容 text/event-stream 形式：取 data: 行
    let json_text = text
        .lines()
        .find_map(|l| l.strip_prefix("data:").map(|s| s.trim()))
        .unwrap_or(text);
    let v: Value = serde_json::from_str(json_text).map_err(|e| format!("无法解析内核返回：{e}"))?;
    Ok((sid, v))
}

/// 调用名字里含 `__{kind}__` 的插件工具（kind 为 task / stats 等），返回工具返回的 JSON。
pub fn call_plugin_tool(port: u16, kind: &str, arguments: Value) -> Result<Value, String> {
    let (sid, init) = post(
        port,
        None,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-03-26","capabilities":{},
            "clientInfo":{"name":"siyuan-pomodoro","version":"0.1.0"}}}),
    )?;
    if init.get("error").is_some() {
        return Err(format!("初始化失败：{init}"));
    }
    let sid = sid.ok_or("内核没有返回会话 ID")?;
    let result = (|| {
        // 通知没有响应体，读取时可能报 EOF，忽略即可
        let _ = post(port, Some(&sid), &json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let (_, list) = post(port, Some(&sid), &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))?;
        let needle = format!("__{kind}__");
        let name = list["result"]["tools"]
            .as_array()
            .and_then(|tools| {
                tools.iter().find_map(|t| {
                    let n = t["name"].as_str()?;
                    (n.starts_with("plugin__") && n.contains(&needle)).then(|| n.to_string())
                })
            })
            .ok_or_else(|| {
                format!("内核里没有找到插件工具“{kind}”：请确认工作空间 conf.json 里 bazaar.trust 为 true，且已启用任务笔记管理插件")
            })?;
        let (_, call) = post(
            port,
            Some(&sid),
            &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        )?;
        if let Some(err) = call.get("error") {
            return Err(format!("工具调用失败：{err}"));
        }
        let text = call["result"]["content"][0]["text"]
            .as_str()
            .ok_or("工具没有返回内容")?;
        let v: Value = serde_json::from_str(text).map_err(|e| format!("无法解析工具返回：{e}"))?;
        if v["success"].as_bool() == Some(false) {
            return Err(v["error"].as_str().unwrap_or("工具返回失败").to_string());
        }
        Ok(v)
    })();
    // 尽力关闭会话，失败不影响结果
    let _ = agent()
        .delete(&format!("http://127.0.0.1:{port}/mcp"))
        .set("Mcp-Session-Id", &sid)
        .call();
    result
}

/// 内核里是否已经注册了某个插件工具（内核 HTTP 就绪后，插件工具还要再等几秒才会注册）。
pub fn plugin_tool_ready(port: u16, kind: &str) -> bool {
    let Ok((Some(sid), _)) = post(
        port,
        None,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-03-26","capabilities":{},
            "clientInfo":{"name":"siyuan-pomodoro","version":"0.1.0"}}}),
    ) else {
        return false;
    };
    let _ = post(port, Some(&sid), &json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let ready = post(port, Some(&sid), &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .ok()
        .and_then(|(_, list)| list["result"]["tools"].as_array().cloned())
        .map(|tools| {
            let needle = format!("__{kind}__");
            tools.iter().any(|t| {
                t["name"]
                    .as_str()
                    .map(|n| n.starts_with("plugin__") && n.contains(&needle))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    let _ = agent()
        .delete(&format!("http://127.0.0.1:{port}/mcp"))
        .set("Mcp-Session-Id", &sid)
        .call();
    ready
}

#[derive(Serialize)]
pub struct TaskItem {
    pub id: String,
    pub title: String,
    pub date: String,
    pub completed: bool,
    pub block_id: Option<String>,
    pub doc_id: Option<String>,
    /// 任务所在文档的路径（不含笔记本名），查不到时为 None
    pub doc_path: Option<String>,
}

/// 用内核接口把文档 ID 换成文档路径；同一次搜索里每个文档只查一次，查不到的静默跳过。
fn fill_doc_paths(port: u16, items: &mut [TaskItem]) {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_millis(1500)).build();
    let mut cache: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    for t in items.iter_mut() {
        let Some(id) = t.doc_id.clone() else { continue };
        let path = cache
            .entry(id.clone())
            .or_insert_with(|| {
                let resp = agent
                    .post(&format!("http://127.0.0.1:{port}/api/filetree/getHPathByID"))
                    .set("Content-Type", "application/json")
                    .send_string(&json!({"id": id}).to_string())
                    .ok()?;
                let v: Value = serde_json::from_str(&resp.into_string().ok()?).ok()?;
                v["data"].as_str().map(|p| p.trim_start_matches('/').to_string()).filter(|p| !p.is_empty())
            })
            .clone();
        t.doc_path = path;
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(|x| x.to_string())
}

/// scope：`today` 只看今天；`all` 搜全部任务（含历史）。keyword 为空则不按关键词过滤。
pub fn search_tasks(port: u16, scope: &str, keyword: &str, include_completed: bool) -> Result<Vec<TaskItem>, String> {
    let mut args = json!({"action":"search_task","limit":2000});
    match scope {
        "today" => args["date"] = json!("today"),
        "all" => {}
        other => return Err(format!("未知范围：{other}")),
    }
    let keyword = keyword.trim();
    if !keyword.is_empty() {
        args["keyword"] = json!(keyword);
    }
    if !include_completed {
        args["completed"] = json!(false);
    }
    let v = call_plugin_tool(port, "task", args)?;
    let mut items: Vec<TaskItem> = v["data"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|t| {
            Some(TaskItem {
                id: s(t, "id")?,
                title: s(t, "title").unwrap_or_default(),
                date: s(t, "date").unwrap_or_default(),
                completed: t["completed"].as_bool().unwrap_or(false),
                block_id: s(t, "blockId"),
                doc_id: s(t, "docId"),
                doc_path: None,
            })
        })
        .collect();
    // 今天的视图在用 date 查询时会带出已完成任务，这里再按“含已完成”开关过滤一次
    if !include_completed {
        items.retain(|t| !t.completed);
    }
    // 最近的在前；无日期的排最后
    items.sort_by(|a, b| match (a.date.is_empty(), b.date.is_empty()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => b.date.cmp(&a.date),
    });
    fill_doc_paths(port, &mut items);
    Ok(items)
}

/// 与原版 roundSessionMinutes 一致：四舍五入；大于 0 但不足 1 分钟按 1。
pub fn round_session_minutes(minutes: f64) -> u32 {
    let mut r = minutes.round();
    if r == 0.0 && minutes > 0.0 {
        r = 1.0;
    }
    r.max(0.0) as u32
}

/// 毫秒时间戳 → UTC ISO 字符串（如 2026-11-02T13:10:00.000Z），与插件记录格式一致。
pub fn iso_from_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// 写一条番茄专注记录（stats.create_focus）。返回内核创建的记录。
pub fn record_focus(
    port: u16,
    task_id: &str,
    task_title: &str,
    start_ms: i64,
    end_ms: i64,
    actual_ms: i64,
    planned_min: f64,
    completed: bool,
    kind: &str, // work | shortBreak | longBreak
) -> Result<Value, String> {
    if task_id.trim().is_empty() {
        return Err("缺少任务 ID".to_string());
    }
    if actual_ms <= 0 {
        return Err("实际时长为 0，不写入记录".to_string());
    }
    let duration = round_session_minutes(actual_ms as f64 / 60000.0);
    let planned = round_session_minutes(planned_min).max(1);
    let start_iso = iso_from_ms(start_ms);
    let end_iso = iso_from_ms(end_ms);
    // 防重复：崩溃恢复或重试时，记录可能其实已经写进内核了。先查同一任务、起止时间完全相同的记录，有就直接返回它。
    let from = iso_from_ms(start_ms - 86_400_000)[..10].to_string();
    let to = iso_from_ms(end_ms + 86_400_000)[..10].to_string();
    let existing = call_plugin_tool(
        port,
        "stats",
        json!({"action":"get_focuses_by_time","startDate":from,"endDate":to}),
    )?;
    if let Some(found) = existing["data"].as_array().and_then(|list| {
        list.iter().find(|s| {
            s["eventId"].as_str() == Some(task_id)
                && s["startTime"].as_str() == Some(start_iso.as_str())
                && s["endTime"].as_str() == Some(end_iso.as_str())
        })
    }) {
        return Ok(found.clone());
    }
    let v = call_plugin_tool(
        port,
        "stats",
        json!({
            "action": "create_focus",
            "type": kind,
            "eventId": task_id,
            "eventTitle": task_title,
            "startTime": start_iso,
            "endTime": end_iso,
            "duration": duration,
            "plannedDuration": planned,
            "completed": completed,
        }),
    )?;
    Ok(v["data"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_matches_original() {
        assert_eq!(round_session_minutes(0.0), 0);
        assert_eq!(round_session_minutes(0.2), 1); // 不足 1 分钟按 1
        assert_eq!(round_session_minutes(0.49), 1);
        assert_eq!(round_session_minutes(1.49), 1);
        assert_eq!(round_session_minutes(1.5), 2); // 四舍五入，.5 进位
        assert_eq!(round_session_minutes(24.6), 25);
        assert_eq!(round_session_minutes(25.0), 25);
    }

    #[test]
    fn iso_format() {
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_ms(1793625000000), "2026-11-02T13:10:00.000Z");
        assert_eq!(iso_from_ms(1793627100123), "2026-11-02T13:45:00.123Z");
        assert_eq!(iso_from_ms(951782400000), "2000-02-29T00:00:00.000Z"); // 闰日
    }
}
