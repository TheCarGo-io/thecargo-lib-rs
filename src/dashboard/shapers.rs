// The dashboard widgets as the Python lib shapes them, text and rounding included: the clients
// render these strings as they come.

use std::collections::HashMap;
use std::hash::BuildHasher;

use chrono::{DateTime, Datelike, NaiveDate, Utc};

use crate::dashboard::period::ResolvedPeriod;
use crate::dashboard::schemas::{
    DashboardChartAverage, DashboardChartFoot, DashboardChartHighLow, DashboardChartPoint, DashboardChartSeries,
    DashboardKpi, DashboardPerformanceResponse, DashboardPipelineResponse, DashboardQueueResponse,
    DashboardTargetsResponse, DashboardTeamResponse, NeedsAttentionPanel, PipelineColumn, PipelineFootItem,
    QueueListItem, ReadyToShipPanel, TargetCard, TeamAverages, TeamLeaderCard, TeamLeaderboard, TeamMember,
    WaitingOnCustomerPanel,
};
use crate::utils::rounding::{py_round, py_round_int};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowMetrics {
    pub leads: i64,
    pub quotes: i64,
    pub orders: i64,
    pub posted: i64,
    pub dispatched: i64,
    pub delivered: i64,
    pub charged_cents: i64,
    pub dispatched_revenue_cents: i64,
    pub collected_cents: i64,
    pub margin_cents: i64,
    pub avg_dispatch_seconds_sum: i64,
    pub dispatch_event_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyMoney {
    pub bucket_date: NaiveDate,
    pub charged_cents: i64,
    pub dispatched_revenue_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWindow {
    pub user_id: String,
    pub current: WindowMetrics,
    pub prior: WindowMetrics,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisplayName {
    pub name: Option<String>,
    pub first_name: Option<String>,
    pub initials: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueItem {
    pub code: Option<String>,
    pub stage: Option<String>,
    pub status: Option<String>,
    pub reason: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub estimated_pickup_at: Option<NaiveDate>,
    pub origin_city: Option<String>,
    pub origin_state: Option<String>,
    // The Python shaper reads `destination_city`, which the queue rows never carry, so a route
    // shows its origin only. Kept as found.
    pub dest_city: Option<String>,
    pub dest_state: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueRaw {
    pub needs_attention: (i64, Option<DateTime<Utc>>, Vec<QueueItem>),
    pub ready_to_ship: (i64, i64, i64, Vec<QueueItem>),
    pub waiting_on_customer: (i64, i64, i64, Vec<QueueItem>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInput {
    pub metric: String,
    pub target_cents: i64,
    pub current_cents: i64,
}

#[expect(clippy::cast_precision_loss, reason = "cents and counts stay far below 2^53")]
fn float(value: i64) -> f64 {
    value as f64
}

// Python's int() on a float: toward zero.
#[expect(clippy::cast_possible_truncation, reason = "int() truncates; the values are cents")]
fn trunc(value: f64) -> i64 {
    value.trunc() as i64
}

fn group_digits(digits: &str) -> String {
    let (sign, digits) = digits.strip_prefix('-').map_or(("", digits), |rest| ("-", rest));
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

// f"{n:,}"
pub fn fmt_count(n: i64) -> String {
    group_digits(&n.to_string())
}

pub fn fmt_money(cents: i64) -> String {
    let dollars = float(cents) / 100.0;
    if dollars >= 1_000_000.0 {
        format!("${:.1}M", dollars / 1_000_000.0)
    } else if dollars >= 10_000.0 {
        format!("${:.1}k", dollars / 1000.0).replace(".0k", "k")
    } else {
        format!("${}", group_digits(&format!("{dollars:.0}")))
    }
}

fn safe_div(num: f64, den: f64) -> Option<f64> {
    (den != 0.0).then(|| num / den)
}

fn delta_pct(now: f64, prior: f64) -> Option<f64> {
    (prior != 0.0).then(|| py_round((now - prior) / prior * 100.0, 1))
}

fn prior_label(period_key: &str) -> &'static str {
    match period_key {
        "today" => "Prior day",
        "7d" => "Prior 7d",
        "30d" => "Prior 30d",
        "mtd" => "Prior month",
        "qtd" => "Prior quarter",
        _ => "Prior period",
    }
}

fn fill_daily_gaps(rows: &[DailyMoney], from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, i64, i64)> {
    let by_date: HashMap<NaiveDate, &DailyMoney> = rows.iter().map(|r| (r.bucket_date, r)).collect();
    from.iter_days()
        .take_while(|day| *day <= to)
        .map(|day| {
            let row = by_date.get(&day);
            (day, row.map_or(0, |r| r.charged_cents), row.map_or(0, |r| r.dispatched_revenue_cents))
        })
        .collect()
}

fn chart_label(day: NaiveDate) -> String {
    day.format("%b %-d").to_string()
}

fn date_long(day: NaiveDate) -> String {
    day.format("%b %-d, %a").to_string()
}

fn short_range_label(start: NaiveDate, end: NaiveDate) -> String {
    if start == end {
        chart_label(start)
    } else if start.month() == end.month() {
        format!("{}–{}", chart_label(start), end.day())
    } else {
        format!("{}–{}", chart_label(start), chart_label(end))
    }
}

fn build_chart(rows: &[DailyMoney], from: NaiveDate, to: NaiveDate) -> DashboardChartSeries {
    let padded = fill_daily_gaps(rows, from, to);
    DashboardChartSeries {
        labels: padded.iter().map(|(day, ..)| chart_label(*day)).collect(),
        charged: padded.iter().map(|(_, charged, _)| py_round(float(*charged) / 100.0, 2)).collect(),
        dispatched: padded.iter().map(|(.., dispatched)| py_round(float(*dispatched) / 100.0, 2)).collect(),
        title_meta: short_range_label(from, to),
    }
}

fn point(cents: i64, day: NaiveDate) -> DashboardChartPoint {
    DashboardChartPoint { amount: fmt_money(cents), date: date_long(day) }
}

fn build_foot(rows: &[DailyMoney], from: NaiveDate, to: NaiveDate) -> DashboardChartFoot {
    let padded = fill_daily_gaps(rows, from, to);
    if padded.is_empty() {
        let zero = DashboardChartPoint { amount: "$0".to_owned(), date: "—".to_owned() };
        return DashboardChartFoot {
            highest: DashboardChartHighLow { charged: zero.clone(), dispatched: zero.clone() },
            lowest: DashboardChartHighLow { charged: zero.clone(), dispatched: zero },
            average: DashboardChartAverage { charged: "$0".to_owned(), dispatched: "$0".to_owned() },
        };
    }
    let mut by_charged = padded.clone();
    by_charged.sort_by_key(|r| r.1);
    let mut by_dispatched = padded.clone();
    by_dispatched.sort_by_key(|r| r.2);
    let n = float(i64::try_from(padded.len()).unwrap_or(i64::MAX));
    let avg_charged = float(padded.iter().map(|r| r.1).sum()) / n;
    let avg_dispatched = float(padded.iter().map(|r| r.2).sum()) / n;
    let (charged_low, charged_high) = (by_charged[0], by_charged[by_charged.len() - 1]);
    let (dispatched_low, dispatched_high) = (by_dispatched[0], by_dispatched[by_dispatched.len() - 1]);
    DashboardChartFoot {
        highest: DashboardChartHighLow {
            charged: point(charged_high.1, charged_high.0),
            dispatched: point(dispatched_high.2, dispatched_high.0),
        },
        lowest: DashboardChartHighLow {
            charged: point(charged_low.1, charged_low.0),
            dispatched: point(dispatched_low.2, dispatched_low.0),
        },
        average: DashboardChartAverage {
            charged: fmt_money(trunc(avg_charged)),
            dispatched: fmt_money(trunc(avg_dispatched)),
        },
    }
}

fn kpi(label: &str, value: String, sub: String, prior: String, delta: Option<f64>) -> DashboardKpi {
    DashboardKpi { label: label.to_owned(), value, sub, prior, delta }
}

fn build_kpis(current: &WindowMetrics, prior: &WindowMetrics, period_key: &str) -> Vec<DashboardKpi> {
    let prior_lbl = prior_label(period_key);
    let quotes_now = current.quotes;
    let leads_now = if current.leads == 0 { quotes_now } else { current.leads };
    let quote_conv = safe_div(float(quotes_now), float(leads_now));
    let quoted_sub = format!(
        "{} of {} leads  ·  {}% con",
        fmt_count(quotes_now),
        fmt_count(leads_now),
        py_round_int(quote_conv.unwrap_or(0.0) * 100.0)
    );
    let dispatched_count = current.dispatched;
    let quote_to_dispatch = safe_div(float(dispatched_count), float(quotes_now));
    let dispatched_sub = format!(
        "{} of {} quotes  ·  {}% con",
        fmt_count(dispatched_count),
        fmt_count(quotes_now),
        py_round_int(quote_to_dispatch.unwrap_or(0.0) * 100.0)
    );
    let orders_now = if current.orders == 0 { 1 } else { current.orders };
    let dispatch_rate_now = float(dispatched_count) / float(orders_now) * 100.0;
    let dispatch_rate_prev =
        if prior.orders == 0 { 0.0 } else { float(prior.dispatched) / float(prior.orders) * 100.0 };
    let rate_sub = format!("{} of {} orders", fmt_count(dispatched_count), fmt_count(current.orders));
    let margin_per_order_now = float(current.margin_cents) / float(orders_now);
    let margin_per_order_prev = if prior.orders == 0 { 0.0 } else { float(prior.margin_cents) / float(prior.orders) };
    let revenue_now = if current.dispatched_revenue_cents == 0 { 1 } else { current.dispatched_revenue_cents };
    let margin_pct_now = float(current.margin_cents) / float(revenue_now) * 100.0;
    let margin_sub = format!("{}% margin  ·  {} orders", py_round_int(margin_pct_now), fmt_count(current.orders));
    let pct_collected = if current.charged_cents == 0 {
        0.0
    } else {
        float(current.collected_cents) / float(current.charged_cents) * 100.0
    };
    let due_cents = (current.charged_cents - current.collected_cents).max(0);
    let payments_sub = format!("{}% of charged  ·  {} due", py_round_int(pct_collected), fmt_money(due_cents));
    vec![
        kpi(
            "Quoted",
            fmt_count(quotes_now),
            quoted_sub,
            format!("{prior_lbl}: {}", fmt_count(prior.quotes)),
            delta_pct(float(quotes_now), float(prior.quotes)),
        ),
        kpi(
            "Dispatched",
            fmt_money(current.dispatched_revenue_cents),
            dispatched_sub,
            format!("{prior_lbl}: {}", fmt_money(prior.dispatched_revenue_cents)),
            delta_pct(float(current.dispatched_revenue_cents), float(prior.dispatched_revenue_cents)),
        ),
        kpi(
            "Dispatch rate",
            format!("{}%", py_round_int(dispatch_rate_now)),
            rate_sub,
            format!("{prior_lbl}: {}%", py_round_int(dispatch_rate_prev)),
            Some(py_round(dispatch_rate_now - dispatch_rate_prev, 1)),
        ),
        kpi(
            "Avg margin / order",
            fmt_money(trunc(margin_per_order_now)),
            margin_sub,
            format!("{prior_lbl}: {}", fmt_money(trunc(margin_per_order_prev))),
            delta_pct(margin_per_order_now, margin_per_order_prev),
        ),
        kpi(
            "Payments collected",
            fmt_money(current.collected_cents),
            payments_sub,
            format!("{prior_lbl}: {}", fmt_money(prior.collected_cents)),
            delta_pct(float(current.collected_cents), float(prior.collected_cents)),
        ),
    ]
}

pub fn shape_performance(
    current: &WindowMetrics,
    prior: &WindowMetrics,
    daily: &[DailyMoney],
    user_full_name: &str,
    resolved: &ResolvedPeriod,
    scope_label: &str,
) -> DashboardPerformanceResponse {
    let user_name = match scope_label {
        "team" => format!("{user_full_name}'s team"),
        "company" => "Company".to_owned(),
        _ => user_full_name.to_owned(),
    };
    DashboardPerformanceResponse {
        user_name,
        period_label: resolved.label.clone(),
        kpis: build_kpis(current, prior, resolved.period.value()),
        chart: build_chart(daily, resolved.current.date_from, resolved.current.date_to),
        foot: build_foot(daily, resolved.current.date_from, resolved.current.date_to),
    }
}

fn team_kpis(cur: &WindowMetrics) -> (i64, i64, f64, i64, i64) {
    let rate = if cur.orders == 0 { 0.0 } else { float(cur.dispatched) / float(cur.orders) * 100.0 };
    let avg_margin_cents = if cur.orders == 0 { 0 } else { trunc(float(cur.margin_cents) / float(cur.orders)) };
    (cur.quotes, cur.dispatched_revenue_cents, rate, avg_margin_cents, cur.collected_cents)
}

fn leader_card<S: BuildHasher>(
    names: &HashMap<String, DisplayName, S>,
    user_id: &str,
    dispatched_cents: i64,
    detail: String,
    delta: Option<f64>,
    trend: &str,
) -> TeamLeaderCard {
    let name = names.get(user_id).cloned().unwrap_or_default();
    TeamLeaderCard {
        user_id: user_id.to_owned(),
        name: name.name,
        initials: name.initials,
        color: name.color,
        dispatched: fmt_money(dispatched_cents),
        detail,
        delta,
        trend: trend.to_owned(),
    }
}

// The first agent wins a tie, as Python's strict comparisons keep it.
fn team_leaderboard<S: BuildHasher>(
    users: &[AgentWindow],
    names: &HashMap<String, DisplayName, S>,
    prior_lbl: &str,
) -> TeamLeaderboard {
    let mut top: Option<(i64, &str, f64)> = None;
    let mut best: Option<(f64, &str, i64)> = None;
    let mut worst: Option<(f64, &str, i64)> = None;
    for u in users {
        let d_now = u.current.dispatched_revenue_cents;
        let rate =
            if u.current.orders == 0 { 0.0 } else { float(u.current.dispatched) / float(u.current.orders) * 100.0 };
        if top.is_none_or(|t| d_now > t.0) {
            top = Some((d_now, &u.user_id, rate));
        }
        let d_prev = u.prior.dispatched_revenue_cents;
        if d_prev > 0 {
            let delta = float(d_now - d_prev) / float(d_prev) * 100.0;
            if best.is_none_or(|b| delta > b.0) {
                best = Some((delta, &u.user_id, d_now));
            }
            if worst.is_none_or(|w| delta < w.0) {
                worst = Some((delta, &u.user_id, d_now));
            }
        }
    }
    TeamLeaderboard {
        top_performer: top
            .filter(|t| t.0 > 0)
            .map(|t| leader_card(names, t.1, t.0, format!("{}% rate", py_round_int(t.2)), None, "")),
        most_improved: best.filter(|b| b.0 > 0.0).map(|b| {
            leader_card(
                names,
                b.1,
                b.2,
                format!("+{}% vs {prior_lbl}", py_round_int(b.0)),
                Some(py_round(b.0, 1)),
                "up",
            )
        }),
        needs_coaching: worst.filter(|w| w.0 < 0.0).map(|w| {
            leader_card(
                names,
                w.1,
                w.2,
                format!("{}% vs {prior_lbl}", py_round_int(w.0)),
                Some(py_round(w.0, 1)),
                "down",
            )
        }),
    }
}

pub fn shape_team<S: BuildHasher>(
    users: &[AgentWindow],
    resolved: &ResolvedPeriod,
    names: &HashMap<String, DisplayName, S>,
) -> DashboardTeamResponse {
    let prior_lbl = prior_label(resolved.period.value()).to_lowercase();
    let mut members = Vec::with_capacity(users.len());
    let (mut quotes_sum, mut dispatched_sum, mut rate_sum, mut margin_sum, mut collected_sum) = (0, 0, 0.0, 0, 0);
    let mut rate_n = 0_i64;
    for u in users {
        let (quotes, dispatched_cents, rate, avg_margin_cents, collected_cents) = team_kpis(&u.current);
        let name = names.get(&u.user_id).cloned().unwrap_or_default();
        members.push(TeamMember {
            user_id: u.user_id.clone(),
            name: name.name,
            first_name: name.first_name,
            initials: name.initials,
            color: name.color,
            quotes,
            dispatch_rate: format!("{}%", py_round_int(rate)),
            dispatched: fmt_money(dispatched_cents),
            dispatched_int: dispatched_cents,
        });
        quotes_sum += quotes;
        dispatched_sum += dispatched_cents;
        margin_sum += avg_margin_cents;
        collected_sum += collected_cents;
        if u.current.orders != 0 {
            rate_sum += rate;
            rate_n += 1;
        }
    }
    members.sort_by_key(|m| std::cmp::Reverse(m.dispatched_int));

    let n = i64::try_from(members.len()).unwrap_or(i64::MAX);
    let averages = if n == 0 {
        TeamAverages::default()
    } else {
        let avg_quoted = py_round_int(float(quotes_sum) / float(n));
        let avg_dispatched = trunc(float(dispatched_sum) / float(n));
        let avg_rate = if rate_n == 0 { 0.0 } else { rate_sum / float(rate_n) };
        let avg_margin = trunc(float(margin_sum) / float(n));
        let avg_collected = trunc(float(collected_sum) / float(n));
        TeamAverages {
            quoted: avg_quoted,
            quoted_label: fmt_count(avg_quoted),
            dispatched_cents: avg_dispatched,
            dispatched_label: fmt_money(avg_dispatched),
            dispatch_rate: py_round(avg_rate, 1),
            dispatch_rate_label: format!("{}%", py_round_int(avg_rate)),
            avg_margin_cents: avg_margin,
            avg_margin_label: fmt_money(avg_margin),
            collected_cents: avg_collected,
            collected_label: fmt_money(avg_collected),
        }
    };
    DashboardTeamResponse {
        period_label: format!("Team · {}", resolved.label),
        team_size: n,
        team: members,
        averages,
        leaderboard: team_leaderboard(users, names, &prior_lbl),
    }
}

pub fn shape_pipeline(
    current: &WindowMetrics,
    prior: &WindowMetrics,
    resolved: &ResolvedPeriod,
) -> DashboardPipelineResponse {
    let col = |label: &str, now: i64, before: i64| PipelineColumn {
        label: label.to_owned(),
        value: now,
        delta: delta_pct(float(now), float(before)),
    };
    let cols = vec![
        col("Leads", current.leads, prior.leads),
        col("Quotes", current.quotes, prior.quotes),
        col("Orders", current.orders, prior.orders),
        col("Posted", current.posted, prior.posted),
        col("Dispatched", current.dispatched, prior.dispatched),
        col("Delivered", current.delivered, prior.delivered),
    ];
    let (orders, quotes) = (current.orders, current.quotes);
    let quote_to_order =
        if quotes == 0 { "0%".to_owned() } else { format!("{}%", py_round_int(float(orders) / float(quotes) * 100.0)) };
    let time_to_dispatch = if current.dispatch_event_count == 0 {
        "—".to_owned()
    } else {
        format!("{:.1} days", float(current.avg_dispatch_seconds_sum) / float(current.dispatch_event_count) / 86400.0)
    };
    let period_label = match resolved.period.value() {
        "today" => "today · vs yesterday".to_owned(),
        "7d" => "this week · vs last week".to_owned(),
        "30d" => "last 30 days · vs prior 30".to_owned(),
        "mtd" => "month to date · vs last month".to_owned(),
        "qtd" => "quarter to date · vs last quarter".to_owned(),
        _ => format!("{} · vs prior period", resolved.label),
    };
    let foot = vec![
        PipelineFootItem { label: "Quote → Order".to_owned(), value: quote_to_order },
        PipelineFootItem { label: "Avg time to dispatch".to_owned(), value: time_to_dispatch },
        PipelineFootItem {
            label: "Avg margin".to_owned(),
            value: fmt_money(if orders == 0 { 0 } else { trunc(float(current.margin_cents) / float(orders)) }),
        },
    ];
    DashboardPipelineResponse { period_label, cols, foot }
}

// html.escape(str(s)) if s else ""
fn escape(text: Option<&str>) -> String {
    let Some(text) = text.filter(|t| !t.is_empty()) else { return String::new() };
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

// str.title(): a letter after a cased letter is lowered, any other letter raised.
fn title(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_cased = false;
    for c in text.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        if cased {
            if after_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
        } else {
            out.push(c);
        }
        after_cased = cased;
    }
    out
}

fn relative_age(moment: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(moment) = moment else { return String::new() };
    let seconds = (now - moment).num_seconds();
    if seconds < 60 {
        return "now".to_owned();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}

fn stage_label(item: &QueueItem) -> String {
    format!("{} <b>#{}</b>", title(item.stage.as_deref().unwrap_or_default()), escape(item.code.as_deref()))
}

fn needs_attention_left(item: &QueueItem) -> String {
    if item.reason.as_deref().is_some_and(|reason| !reason.is_empty()) {
        return format!("{} · {}", stage_label(item), escape(item.reason.as_deref()).replace('_', " "));
    }
    let status = item.status.as_deref().unwrap_or_default().replace('_', " ").to_lowercase();
    format!("{} · {status}", stage_label(item))
}

fn today_pill(day: Option<NaiveDate>, now: DateTime<Utc>) -> String {
    match day {
        None => String::new(),
        Some(day) if day == now.date_naive() => r#"<span class="dash-aq-pill">Today</span>"#.to_owned(),
        Some(day) => escape(Some(&day.format("%Y-%m-%d").to_string())),
    }
}

fn route_label(item: &QueueItem) -> String {
    item.origin_city
        .iter()
        .filter(|city| !city.is_empty())
        .map(|city| escape(Some(city)))
        .collect::<Vec<_>>()
        .join(" → ")
}

pub fn shape_queue(raw: &QueueRaw, now: DateTime<Utc>) -> DashboardQueueResponse {
    let more = |count: i64, shown: usize| (count - i64::try_from(shown).unwrap_or(i64::MAX)).max(0);
    let (na_count, oldest_at, na_items) = &raw.needs_attention;
    let needs_attention = NeedsAttentionPanel {
        count: *na_count,
        oldest: relative_age(*oldest_at, now),
        items: na_items
            .iter()
            .map(|it| QueueListItem { left: needs_attention_left(it), right: relative_age(it.updated_at, now) })
            .collect(),
        more: more(*na_count, na_items.len()),
    };
    let (rts_count, posted, not_posted, rts_items) = &raw.ready_to_ship;
    let ready_to_ship = ReadyToShipPanel {
        count: *rts_count,
        posted: *posted,
        not_posted: *not_posted,
        items: rts_items
            .iter()
            .map(|it| QueueListItem {
                left: format!("{} · {}", escape(it.code.as_deref()), route_label(it))
                    .trim_matches(|c| c == ' ' || c == '·')
                    .to_owned(),
                right: today_pill(it.estimated_pickup_at, now),
            })
            .collect(),
        more: more(*rts_count, rts_items.len()),
    };
    let (woc_count, follow_up, deposit_pending, woc_items) = &raw.waiting_on_customer;
    let waiting_on_customer = WaitingOnCustomerPanel {
        count: *woc_count,
        follow_up: *follow_up,
        deposit_pending: *deposit_pending,
        items: woc_items
            .iter()
            .map(|it| QueueListItem {
                left: format!(
                    "{} · {}",
                    escape(it.code.as_deref()),
                    it.status.as_deref().unwrap_or_default().replace('_', " ")
                ),
                right: relative_age(it.updated_at, now),
            })
            .collect(),
        more: more(*woc_count, woc_items.len()),
    };
    DashboardQueueResponse { needs_attention, ready_to_ship, waiting_on_customer }
}

fn days_in_month(year: i32, month: u32) -> i64 {
    let first = NaiveDate::from_ymd_opt(year, month, 1);
    let next =
        if month == 12 { NaiveDate::from_ymd_opt(year + 1, 1, 1) } else { NaiveDate::from_ymd_opt(year, month + 1, 1) };
    match (first, next) {
        (Some(first), Some(next)) => (next - first).num_days(),
        _ => 30,
    }
}

fn target_status(current_cents: i64, target_cents: i64, pace_expected: f64) -> (String, &'static str) {
    if target_cents == 0 {
        return ("No target set".to_owned(), "ontrack");
    }
    if pace_expected <= 0.0 {
        return ("On track".to_owned(), "ontrack");
    }
    let ratio = float(current_cents) / pace_expected;
    if ratio >= 1.05 {
        (format!("Ahead by {}", fmt_money(trunc(float(current_cents) - pace_expected))), "ahead")
    } else if ratio < 0.95 {
        (format!("Behind by {}", fmt_money(trunc(pace_expected - float(current_cents)))), "behind")
    } else {
        ("On track".to_owned(), "ontrack")
    }
}

fn target_pace(current_cents: i64, target_cents: i64, day_of_month: i64, days_in_month: i64) -> String {
    let pct = if target_cents == 0 { 0 } else { py_round_int(float(current_cents) / float(target_cents) * 100.0) };
    let daily_needed = float((target_cents - current_cents).max(0)) / float((days_in_month - day_of_month).max(1));
    if daily_needed > 0.0 {
        format!("Pace {pct}% (day {day_of_month}/{days_in_month}) · {}/day to catch up", fmt_money(trunc(daily_needed)))
    } else {
        format!("Pace {pct}% (day {day_of_month}/{days_in_month})")
    }
}

pub fn shape_targets(
    (year, month): (i32, u32),
    cards: &[TargetInput],
    daily: &[DailyMoney],
    month_start: NaiveDate,
    month_end: NaiveDate,
) -> DashboardTargetsResponse {
    let days_in_month = days_in_month(year, month);
    let day_of_month = i64::from(month_end.day());
    let cards = cards
        .iter()
        .map(|c| {
            let pace_expected = float(day_of_month) / float(days_in_month.max(1)) * float(c.target_cents);
            let (status, status_class) = target_status(c.current_cents, c.target_cents, pace_expected);
            TargetCard {
                metric: c.metric.clone(),
                label: match c.metric.as_str() {
                    "charged" => "Charged".to_owned(),
                    "dispatched" => "Dispatched".to_owned(),
                    other => title(other),
                },
                target: fmt_money(c.target_cents),
                target_int: c.target_cents.div_euclid(100),
                current: fmt_money(c.current_cents),
                current_int: c.current_cents.div_euclid(100),
                status,
                status_class: status_class.to_owned(),
                pace: target_pace(c.current_cents, c.target_cents, day_of_month, days_in_month),
                remaining: format!("{} days remaining", (days_in_month - day_of_month).max(0)),
            }
        })
        .collect();
    DashboardTargetsResponse {
        meta: format!("{} · day {day_of_month} of {days_in_month}", month_start.format("%B")),
        cards,
        chart: build_chart(daily, month_start, month_end),
        foot: build_foot(daily, month_start, month_end),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    #[test]
    fn money_reads_as_the_python_lib_writes_it() {
        assert_eq!(fmt_money(0), "$0");
        assert_eq!(fmt_money(123_456), "$1,235");
        assert_eq!(fmt_money(999_950), "$10,000");
        assert_eq!(fmt_money(1_000_000), "$10k");
        assert_eq!(fmt_money(1_234_567), "$12.3k");
        assert_eq!(fmt_money(250_000_000), "$2.5M");
        assert_eq!(fmt_money(-123_456), "$-1,235");
        assert_eq!(fmt_count(1_234_567), "1,234,567");
        assert_eq!(fmt_count(-1_000), "-1,000");
    }

    #[test]
    fn text_helpers_follow_python() {
        assert_eq!(title("order"), "Order");
        assert_eq!(title("picked_up 1st"), "Picked_Up 1St");
        assert_eq!(escape(Some("a<b & 'c'")), "a&lt;b &amp; &#x27;c&#x27;");
        assert_eq!(escape(None), "");
    }

    #[test]
    fn ages_round_down_to_the_unit() {
        let now = DateTime::parse_from_rfc3339("2026-10-20T16:00:00Z").unwrap().to_utc();
        assert_eq!(relative_age(Some(now - TimeDelta::seconds(59)), now), "now");
        assert_eq!(relative_age(Some(now - TimeDelta::minutes(61)), now), "1h");
        assert_eq!(relative_age(Some(now - TimeDelta::hours(49)), now), "2d");
        assert_eq!(relative_age(Some(now + TimeDelta::hours(1)), now), "now");
    }
}
