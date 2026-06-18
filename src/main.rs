use std::env;
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;
use chrono::{DateTime, Local, Utc};
use serde_json::Value;

// ── ANSI Colors ────────────────────────────────────────
const BLUE: &str = "\x1b[38;2;0;153;255m";
const ORANGE: &str = "\x1b[38;2;255;176;85m";
const GREEN: &str = "\x1b[38;2;0;175;80m";
const CYAN: &str = "\x1b[38;2;86;182;194m";
const RED: &str = "\x1b[38;2;255;85;85m";
const YELLOW: &str = "\x1b[38;2;230;200;0m";
const WHITE: &str = "\x1b[38;2;220;220;220m";
const MAGENTA: &str = "\x1b[38;2;180;140;255m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

// ── Helpers ────────────────────────────────────────────

fn format_tokens(num: u64) -> String {
    if num >= 1_000_000 {
        format!("{:.1}m", num as f64 / 1_000_000.0)
    } else if num >= 1_000 {
        format!("{}k", num / 1000)
    } else {
        format!("{num}")
    }
}

fn color_for_pct(pct: u32) -> &'static str {
    if pct >= 90 {
        RED
    } else if pct >= 70 {
        YELLOW
    } else if pct >= 50 {
        ORANGE
    } else {
        GREEN
    }
}

fn build_bar(pct: u32, width: usize) -> String {
    let pct = pct.min(100);
    let filled = (pct as usize * width + 50) / 100; // round, not truncate
    let empty = width - filled;
    let bar_color = color_for_pct(pct);

    let sp = "\u{200A}"; // hair space (thinnest)
    let filled_str = vec!["●"; filled].join(sp);
    let empty_str = vec!["○"; empty].join(sp);

    if filled > 0 && empty > 0 {
        format!("{bar_color}{filled_str}{sp}{DIM}{bar_color}{empty_str}{RESET}")
    } else if filled > 0 {
        format!("{bar_color}{filled_str}{RESET}")
    } else {
        format!("{DIM}{bar_color}{empty_str}{RESET}")
    }
}

// A rate-limit window is expired once its reset time has passed: the
// usage it reports describes a window that no longer exists. The statusLine
// stdin payload gives `resets_at` as Unix epoch SECONDS. Treat a
// missing/invalid value as expired so we never show a window we can't
// anchor in time.
fn reset_is_in_future(resets_at: Option<i64>) -> bool {
    match resets_at {
        Some(epoch) => epoch > Utc::now().timestamp(),
        None => false,
    }
}

// Format a Unix-epoch-seconds reset time for display in the local timezone.
// `style` "time" → "9:00am", "datetime" → "jun 18, 9:00am".
fn format_reset_epoch(epoch: i64, style: &str) -> Option<String> {
    let dt = DateTime::<Utc>::from_timestamp(epoch, 0)?;
    let local: DateTime<Local> = dt.into();

    let formatted = match style {
        "time" => local.format("%-I:%M%p").to_string(),
        "datetime" => local.format("%b %-d, %-I:%M%p").to_string(),
        _ => local.format("%b %-d").to_string(),
    };
    Some(formatted.to_lowercase())
}

fn get_git_info(cwd: &str) -> (String, String) {
    let branch = Command::new("git")
        .args(["-C", cwd, "symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if branch.is_empty() {
        return (String::new(), String::new());
    }

    let dirty = Command::new("git")
        .args(["-C", cwd, "status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| !o.stdout.is_empty())
        .map(|_| "*".to_string())
        .unwrap_or_default();

    (branch, dirty)
}

// ── Main ───────────────────────────────────────────────

fn main() {
    let mut input = String::new();
    if io::stdin().read_to_string(&mut input).is_err() || input.trim().is_empty() {
        print!("Claude");
        return;
    }

    let json: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            print!("Claude");
            return;
        }
    };

    let sep = format!(" {DIM}│{RESET} ");

    // ── Extract all JSON fields (single parse) ────────
    let model_name = json["model"]["display_name"]
        .as_str()
        .unwrap_or("Claude");

    let size = {
        let s = json["context_window"]["context_window_size"]
            .as_u64()
            .unwrap_or(200000);
        if s == 0 { 200000 } else { s }
    };

    let input_tokens = json["context_window"]["current_usage"]["input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cache_create = json["context_window"]["current_usage"]["cache_creation_input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cache_read = json["context_window"]["current_usage"]["cache_read_input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let current = input_tokens + cache_create + cache_read;

    let _used_tokens = format_tokens(current);
    let _total_tokens = format_tokens(size);
    let pct_used = if size > 0 {
        (current * 100 / size) as u32
    } else {
        0
    };

    // ── Thinking status ───────────────────────────────
    // Read the live session state from stdin (`thinking.enabled`) rather than
    // the global `~/.claude/settings.json` `alwaysThinkingEnabled` flag — the
    // stdin field reflects whether extended thinking is actually on for THIS
    // session. Absent (older models / not applicable) is treated as off.
    let thinking_on = json["thinking"]["enabled"].as_bool().unwrap_or(false);

    // ── Directory + Git ───────────────────────────────
    let cwd = json["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        });

    let dirname = Path::new(&cwd)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let (git_branch, git_dirty) = get_git_info(&cwd);

    // ── Session duration ──────────────────────────────
    // From stdin `cost.total_duration_ms` (wall-clock ms for the session).
    // The old code read `session.start_time`, which the statusLine payload
    // does not contain — so the ⏱ segment never rendered. This uses the real
    // field. Absent/zero → no segment.
    let session_duration = json["cost"]["total_duration_ms"]
        .as_f64()
        .map(|ms| (ms / 1000.0).max(0.0) as u64)
        .filter(|&secs| secs > 0)
        .map(|secs| {
            if secs >= 3600 {
                format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
            } else if secs >= 60 {
                format!("{}m", secs / 60)
            } else {
                format!("{secs}s")
            }
        });

    // ── LINE 1 ────────────────────────────────────────
    let pct_color = color_for_pct(pct_used);

    let mut out = String::with_capacity(512);
    out.push_str(&format!("{BLUE}{model_name}{RESET}"));
    // Writing-hand WITHOUT the VS16 (U+FE0F) emoji-presentation selector.
    // The real cause of the tmux corruption was the VS16, not the glyph: it
    // forces emoji presentation (font draws 2 cells) while tmux measures the
    // base U+270D as 1 cell — so the cells after it get clobbered. Dropping
    // VS16 keeps the icon but makes width agree (1 cell), exactly like the
    // ⏱ session glyph that never broke. A trailing space mirrors ⏱ 's padding.
    out.push_str(&format!("{sep}{DIM}✍ {RESET}{pct_color}{pct_used}%{RESET}"));
    out.push_str(&format!("{sep}{CYAN}{dirname}{RESET}"));

    if !git_branch.is_empty() {
        out.push_str(&format!(
            " {GREEN}({git_branch}{RED}{git_dirty}{GREEN}){RESET}"
        ));
    }

    // Reasoning effort level (stdin `effort.level`) — only for models that
    // support it; absent otherwise.
    if let Some(level) = json["effort"]["level"].as_str() {
        out.push_str(&format!("{sep}{DIM}effort{RESET} {WHITE}{level}{RESET}"));
    }

    // Estimated session cost in USD (stdin `cost.total_cost_usd`). Client-side
    // estimate, may differ from the actual bill. Skip when absent or zero.
    if let Some(cost) = json["cost"]["total_cost_usd"].as_f64() {
        if cost > 0.0 {
            out.push_str(&format!("{sep}{DIM}${RESET}{WHITE}{cost:.2}{RESET}"));
        }
    }

    // Claude Code app version (stdin `version`).
    if let Some(ver) = json["version"].as_str() {
        if !ver.is_empty() {
            out.push_str(&format!("{sep}{DIM}v{ver}{RESET}"));
        }
    }

    if let Some(ref dur) = session_duration {
        out.push_str(&format!("{sep}{DIM}⏱ {RESET}{WHITE}{dur}{RESET}"));
    }

    out.push_str(&sep);
    if thinking_on {
        out.push_str(&format!("{MAGENTA}◐ thinking{RESET}"));
    } else {
        out.push_str(&format!("{DIM}◑ thinking{RESET}"));
    }

    // ── RATE LIMITS ───────────────────────────────────
    // Claude Code passes subscription usage into the statusLine command via
    // the stdin JSON `rate_limits` object — no OAuth/Keychain/API call needed.
    // The object is present only for Pro/Max subscribers after the session's
    // first API response, and each window may be independently absent.
    //   rate_limits.five_hour.used_percentage : number 0-100   ("current")
    //   rate_limits.five_hour.resets_at       : Unix epoch seconds
    //   rate_limits.seven_day.{used_percentage,resets_at}       ("weekly")
    {
        let bar_width = 10;
        let rl = &json["rate_limits"];

        // 5-hour ("current") — only when present and its window is in the future.
        let five_reset = rl["five_hour"]["resets_at"].as_i64();
        if reset_is_in_future(five_reset) {
            if let Some(five_pct) = rl["five_hour"]["used_percentage"].as_f64() {
                let five_pct = five_pct.round() as u32;
                let five_reset_str = five_reset
                    .and_then(|e| format_reset_epoch(e, "time"))
                    .unwrap_or_default();
                let five_bar = build_bar(five_pct, bar_width);
                let five_color = color_for_pct(five_pct);

                out.push_str(&format!(
                    "\n\n{WHITE}current{RESET} {five_bar} {five_color}{five_pct:3}%{RESET} {DIM}⟳{RESET} {WHITE}{five_reset_str}{RESET}"
                ));
            }
        }

        // 7-day ("weekly") — same presence + expiry guard.
        let seven_reset = rl["seven_day"]["resets_at"].as_i64();
        if reset_is_in_future(seven_reset) {
            if let Some(seven_pct) = rl["seven_day"]["used_percentage"].as_f64() {
                let seven_pct = seven_pct.round() as u32;
                let seven_reset_str = seven_reset
                    .and_then(|e| format_reset_epoch(e, "datetime"))
                    .unwrap_or_default();
                let seven_bar = build_bar(seven_pct, bar_width);
                let seven_color = color_for_pct(seven_pct);

                out.push_str(&format!(
                    "\n{WHITE}weekly{RESET}  {seven_bar} {seven_color}{seven_pct:3}%{RESET} {DIM}⟳{RESET} {WHITE}{seven_reset_str}{RESET}"
                ));
            }
        }
    }

    print!("{out}");
}
