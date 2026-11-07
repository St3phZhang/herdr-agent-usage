//! One-line quota summary for Herdr's tab bar.
//!
//! Herdr runs `ui.tab_bar_right` command entries on the server and renders the
//! last line of their stdout. The sidebar owns session management in this
//! mode, so the bar is the quota surface. Only harnesses with a live pane are
//! shown; a harness with no quota tokens is omitted rather than rendering a
//! placeholder.

use crate::herdr::{list_agent_panes_for_icons, AgentPane};
use crate::model::Harness;
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};

/// Print the line Herdr should put in the tab bar. Empty stdout clears it.
pub fn run() -> Result<()> {
    let panes = list_agent_panes_for_icons()?;
    let line = format_line(&panes);
    if !line.is_empty() {
        println!("{line}");
    }
    Ok(())
}

/// Format the quota line. Pure so the grouping rules stay testable without a
/// running Herdr server.
pub fn format_line(panes: &[AgentPane]) -> String {
    let mut groups: Vec<Group> = Vec::new();
    for pane in panes {
        let provider = pane
            .tokens
            .get("quota_provider")
            .map(String::as_str)
            .unwrap_or("")
            .to_string();
        let windows = Windows {
            five_hour: window_percent(&pane.tokens, "quota_5h"),
            week: window_percent(&pane.tokens, "quota_week"),
            month: window_percent(&pane.tokens, "quota_month"),
        };
        if windows.is_empty() {
            continue;
        }
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.harness == pane.harness && group.provider == provider)
        {
            group.windows.merge(windows);
        } else {
            groups.push(Group {
                harness: pane.harness,
                provider,
                windows,
            });
        }
    }

    // A pane without a provider token and a sibling with one are the same
    // harness. Keep the provider-named group and fold the anonymous windows
    // into it instead of printing `OC` and `OC:Go` twice.
    let mut index = 0;
    while index < groups.len() {
        if groups[index].provider.is_empty() {
            if let Some(target) = groups.iter().position(|group| {
                group.harness == groups[index].harness && !group.provider.is_empty()
            }) {
                let anonymous = groups.remove(index);
                let target = if target > index { target - 1 } else { target };
                groups[target].windows.merge(anonymous.windows);
                continue;
            }
        }
        index += 1;
    }

    let mut counts = HashMap::new();
    for group in &groups {
        *counts.entry(group.harness).or_insert(0usize) += 1;
    }
    groups
        .into_iter()
        .map(|group| {
            let show_provider = counts.get(&group.harness).copied().unwrap_or(0) > 1;
            group.render(show_provider)
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// OpenCode Go publishes the usage allowance as 20% of the monthly limit for
/// the 5-hour window and 50% for the weekly window. Comparing raw remaining
/// percentages would treat 1% of the weekly allowance as equal to 1% of the
/// much smaller 5-hour allowance. Normalize to monthly-allowance points and
/// show the window with the least absolute headroom left.
const FIVE_HOUR_ALLOWANCE: f64 = 0.20;
const WEEK_ALLOWANCE: f64 = 0.50;
const MONTH_ALLOWANCE: f64 = 1.00;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Windows {
    five_hour: Option<u8>,
    week: Option<u8>,
    month: Option<u8>,
}

impl Windows {
    fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.week.is_none() && self.month.is_none()
    }

    fn merge(&mut self, other: Self) {
        if self.five_hour.is_none() {
            self.five_hour = other.five_hour;
        }
        if self.week.is_none() {
            self.week = other.week;
        }
        if self.month.is_none() {
            self.month = other.month;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    harness: Harness,
    provider: String,
    windows: Windows,
}

impl Group {
    fn render(self, show_provider: bool) -> String {
        let mut label = short_harness(self.harness).to_string();
        if show_provider {
            if let Some(suffix) = provider_suffix(self.harness, &self.provider) {
                label.push(':');
                label.push_str(&suffix);
            }
        }
        let mut best: Option<(&'static str, u8, f64)> = None;
        for (window, percent, allowance) in [
            ("5h", self.windows.five_hour, FIVE_HOUR_ALLOWANCE),
            ("7d", self.windows.week, WEEK_ALLOWANCE),
            ("30d", self.windows.month, MONTH_ALLOWANCE),
        ] {
            let Some(percent) = percent else {
                continue;
            };
            let score = f64::from(percent) * allowance;
            if best.is_none_or(|(_, _, best_score)| score < best_score) {
                best = Some((window, percent, score));
            }
        }
        if let Some((window, percent, _)) = best {
            label.push(' ');
            label.push_str(&format!("{window}{percent}%"));
        }
        label
    }
}

fn short_harness(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude => "CL",
        Harness::Codex => "CX",
        Harness::Grok => "GR",
        Harness::Agy => "AG",
        Harness::OpenCode => "OC",
        Harness::Pi => "PI",
        Harness::Omp => "OMP",
        Harness::Devin => "DV",
        Harness::Muse => "MU",
        Harness::Cursor => "CU",
        Harness::Kilo => "KI",
        Harness::Mimo => "MI",
    }
}

fn provider_suffix(harness: Harness, provider: &str) -> Option<String> {
    let provider = provider.trim();
    if provider.is_empty() {
        return None;
    }
    let remainder = provider
        .strip_prefix(harness.display_name())
        .map(str::trim)
        .unwrap_or(provider);
    if remainder.is_empty() {
        return None;
    }
    Some(remainder.chars().take(8).collect())
}

/// Pick the first non-empty `normal`/`warning`/`danger` token for a window
/// family and return its percentage.
///
/// `unknown` values are deliberately ignored: they read as `5h --`/`7d --`
/// and would only add noise to the bar.
fn window_percent(tokens: &BTreeMap<String, String>, base: &str) -> Option<u8> {
    // Nested vendor heads in the Gauges layout publish `quota_share_*`
    // instead of `quota_*`. `quota_week_inline_*` is the pack when 5h is
    // hidden. Accept every spelling the publisher can produce.
    let rest = base.strip_prefix("quota_").unwrap_or(base);
    let mut keys = Vec::new();
    for prefix in ["quota_", "quota_share_"] {
        for suffix in ["normal", "warning", "danger"] {
            keys.push(format!("{prefix}{rest}_{suffix}"));
            if rest == "week" {
                keys.push(format!("{prefix}{rest}_inline_{suffix}"));
            }
        }
    }
    for key in keys {
        let Some(value) = tokens.get(&key) else {
            continue;
        };
        if let Some(percent) = value
            .split_whitespace()
            .find_map(|part| part.strip_suffix('%'))
        {
            if let Ok(percent) = percent.parse::<u8>() {
                return Some(percent);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{AgentSession, AgentStatus};
    use std::collections::BTreeMap;

    fn pane(harness: Harness, tokens: &[(&str, &str)]) -> AgentPane {
        AgentPane {
            pane_id: format!("w1:p{}", tokens.len()),
            workspace_id: "w1".to_string(),
            cwd: String::new(),
            title: String::new(),
            harness,
            session: None::<AgentSession>,
            session_summary: String::new(),
            topic: String::new(),
            tokens: tokens
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<BTreeMap<_, _>>(),
            status: AgentStatus::Idle,
            focused: false,
            state_change_seq: 0,
        }
    }

    #[test]
    fn shows_only_harnesses_with_live_quota_and_compacts_the_windows() {
        let panes = vec![
            pane(
                Harness::OpenCode,
                &[
                    ("quota_provider", "OpenCode Go"),
                    ("quota_5h_normal", "5h 88% 54m"),
                    ("quota_week_normal", "7d 95% 2d14h"),
                ],
            ),
            pane(
                Harness::Claude,
                &[
                    ("quota_provider", "Claude"),
                    ("quota_5h_normal", "5h 61% 2h"),
                ],
            ),
            pane(Harness::Cursor, &[("quota_model", "cursor")]),
        ];
        assert_eq!(format_line(&panes), "OC 5h88% · CL 5h61%");
    }

    #[test]
    fn separate_providers_for_one_harness_stay_separate() {
        let panes = vec![
            pane(
                Harness::OpenCode,
                &[
                    ("quota_provider", "OpenCode Go"),
                    ("quota_5h_normal", "5h 88% 54m"),
                ],
            ),
            pane(
                Harness::OpenCode,
                &[
                    ("quota_provider", "OpenCode Zen"),
                    ("quota_5h_normal", "5h 40% 54m"),
                ],
            ),
        ];
        assert_eq!(format_line(&panes), "OC:Go 5h88% · OC:Zen 5h40%");
    }

    #[test]
    fn a_tight_week_outranks_an_ample_five_hour_window() {
        let panes = vec![pane(
            Harness::OpenCode,
            &[
                ("quota_provider", "OpenCode Go"),
                ("quota_5h_normal", "5h 90% 4h"),
                ("quota_week_normal", "7d 1% 6d"),
            ],
        )];
        assert_eq!(format_line(&panes), "OC 7d1%");
    }

    #[test]
    fn a_provider_with_only_a_weekly_window_shows_that_window() {
        let panes = vec![pane(
            Harness::Codex,
            &[("quota_week_normal", "7d 100% 6d23h")],
        )];
        assert_eq!(format_line(&panes), "CX 7d100%");
    }

    #[test]
    fn merges_anonymous_and_provider_groups_for_one_harness() {
        let panes = vec![
            pane(Harness::Codex, &[("quota_5h_normal", "5h 42% 1h")]),
            pane(
                Harness::Codex,
                &[
                    ("quota_provider", "Codex"),
                    ("quota_week_normal", "7d 70% 3d"),
                ],
            ),
        ];
        assert_eq!(format_line(&panes), "CX 5h42%");
    }

    #[test]
    fn reads_share_and_inline_window_spellings() {
        let panes = vec![pane(
            Harness::OpenCode,
            &[
                ("quota_provider", "OpenCode Go"),
                ("quota_share_5h_normal", "5h 88% 54m"),
                ("quota_share_week_inline_normal", "7d 95% 2d14h"),
            ],
        )];
        assert_eq!(format_line(&panes), "OC 5h88%");
    }

    #[test]
    fn falls_back_to_the_monthly_window() {
        let panes = vec![pane(
            Harness::Grok,
            &[("quota_month_normal", "30d 12% 20d")],
        )];
        assert_eq!(format_line(&panes), "GR 30d12%");
    }

    #[test]
    fn an_empty_inventory_produces_no_line() {
        assert_eq!(format_line(&[]), "");
    }
}
