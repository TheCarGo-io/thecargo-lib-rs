use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DashboardTodoResponse {
    /// Aggregate count for the action queue chip
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QueueListItem {
    /// HTML for the left column (already escaped where needed)
    pub left: String,
    /// HTML for the right column (relative time or pill)
    pub right: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NeedsAttentionPanel {
    pub count: i64,
    /// Age of the oldest item, '2d' / '5h'
    #[schema(required = false)]
    #[schema(default = "")]
    pub oldest: String,
    pub items: Vec<QueueListItem>,
    #[schema(required = false)]
    #[schema(default = 0)]
    pub more: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReadyToShipPanel {
    pub count: i64,
    pub posted: i64,
    pub not_posted: i64,
    pub items: Vec<QueueListItem>,
    #[schema(required = false)]
    #[schema(default = 0)]
    pub more: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WaitingOnCustomerPanel {
    pub count: i64,
    pub follow_up: i64,
    pub deposit_pending: i64,
    pub items: Vec<QueueListItem>,
    #[schema(required = false)]
    #[schema(default = 0)]
    pub more: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DashboardQueueResponse {
    pub needs_attention: NeedsAttentionPanel,
    pub ready_to_ship: ReadyToShipPanel,
    pub waiting_on_customer: WaitingOnCustomerPanel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PipelineColumn {
    pub label: String,
    pub value: i64,
    /// Signed % change vs prior period; null when prior was zero
    pub delta: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PipelineFootItem {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DashboardPipelineResponse {
    pub period_label: String,
    pub cols: Vec<PipelineColumn>,
    pub foot: Vec<PipelineFootItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TargetCard {
    /// `charged` or `dispatched`
    pub metric: String,
    pub label: String,
    /// Formatted target, e.g. '$80,000'
    pub target: String,
    /// Numeric target in dollars (for client-side math)
    pub target_int: i64,
    /// Formatted MTD actual, e.g. '$52,450'
    pub current: String,
    /// Numeric MTD actual in dollars
    pub current_int: i64,
    /// Human label: 'On track' / 'Behind by $X' / 'Ahead by $X'
    pub status: String,
    /// UI suffix: `ontrack` / `behind` / `ahead`
    pub status_class: String,
    /// Pace line, e.g. 'Pace 73% (day 22/30) · $2,827/day to catch up'
    pub pace: String,
    /// Days remaining label, e.g. '8 days remaining'
    pub remaining: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DashboardChartSeries {
    /// Short labels for each day, e.g. 'Apr 16'
    #[schema(required = false)]
    pub labels: Vec<String>,
    /// Charged amount per day
    #[schema(required = false)]
    pub charged: Vec<f64>,
    /// Dispatched revenue per day
    #[schema(required = false)]
    pub dispatched: Vec<f64>,
    /// Human-readable date range shown beside the chart title, e.g. 'Apr 16–22'
    #[schema(required = false)]
    #[schema(default = "")]
    pub title_meta: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DashboardChartPoint {
    /// Formatted money/number, e.g. '$1,500'
    pub amount: String,
    /// Human-readable date label, e.g. 'Apr 21, Mon'
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DashboardChartHighLow {
    pub charged: DashboardChartPoint,
    pub dispatched: DashboardChartPoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DashboardChartAverage {
    /// Average charged across the window, e.g. '$586'
    pub charged: String,
    /// Average dispatched across the window, e.g. '$314'
    pub dispatched: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DashboardChartFoot {
    pub highest: DashboardChartHighLow,
    pub lowest: DashboardChartHighLow,
    pub average: DashboardChartAverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DashboardTargetsResponse {
    /// Header sub-text, e.g. 'April · day 22 of 30'
    pub meta: String,
    pub cards: Vec<TargetCard>,
    pub chart: DashboardChartSeries,
    pub foot: DashboardChartFoot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DashboardKpi {
    /// Display label shown above the value
    pub label: String,
    /// Formatted main value
    pub value: String,
    /// Secondary line — denominator or context
    #[schema(required = false)]
    #[schema(default = "")]
    pub sub: String,
    /// Prior-period label, e.g. 'Prior 7d: 121'
    #[schema(required = false)]
    #[schema(default = "")]
    pub prior: String,
    /// Signed percentage change vs the prior window. Positive = improvement; for `Dispatch rate` and `Avg margin` higher is better, so the sign is preserved as-is. `null` when there is no comparable prior window.
    pub delta: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DashboardPerformanceResponse {
    /// Display name shown in the header (caller's first+last)
    pub user_name: String,
    /// Period descriptor for the panel header, e.g. 'Last 7 days · Apr 16–22'
    pub period_label: String,
    /// Five KPI tiles in render order: Quoted, Dispatched, Dispatch rate, Avg margin / order, Payments collected.
    pub kpis: Vec<DashboardKpi>,
    pub chart: DashboardChartSeries,
    pub foot: DashboardChartFoot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TeamMember {
    /// Agent's user id — the client resolves name/avatar from its roster
    pub user_id: String,
    /// Full display name (null → resolve client-side)
    pub name: Option<String>,
    /// First name only — used in the compact strip
    pub first_name: Option<String>,
    /// Two-letter avatar initials
    pub initials: Option<String>,
    /// Deterministic avatar colour
    pub color: Option<String>,
    /// Quotes created in the window
    #[schema(required = false)]
    #[schema(default = 0)]
    pub quotes: i64,
    /// Dispatched ÷ orders, formatted
    #[schema(required = false)]
    #[schema(default = "0%")]
    pub dispatch_rate: String,
    /// Dispatched revenue, formatted
    #[schema(required = false)]
    #[schema(default = "$0")]
    pub dispatched: String,
    /// Dispatched revenue in cents — for client-side sorting
    #[schema(required = false)]
    #[schema(default = 0)]
    pub dispatched_int: i64,
}

/// Per-agent means over the active team — feeds the user view's 'Team avg' line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TeamAverages {
    /// Mean quotes per agent
    #[schema(required = false)]
    #[schema(default = 0)]
    pub quoted: i64,
    #[schema(required = false)]
    #[schema(default = "0")]
    pub quoted_label: String,
    /// Mean dispatched revenue per agent, in cents
    #[schema(required = false)]
    #[schema(default = 0)]
    pub dispatched_cents: i64,
    #[schema(required = false)]
    #[schema(default = "$0")]
    pub dispatched_label: String,
    /// Mean dispatch rate per agent, percent
    #[schema(required = false)]
    #[schema(default = 0.0)]
    pub dispatch_rate: f64,
    #[schema(required = false)]
    #[schema(default = "0%")]
    pub dispatch_rate_label: String,
    /// Mean margin per order per agent, in cents
    #[schema(required = false)]
    #[schema(default = 0)]
    pub avg_margin_cents: i64,
    #[schema(required = false)]
    #[schema(default = "$0")]
    pub avg_margin_label: String,
    /// Mean payments collected per agent, in cents
    #[schema(required = false)]
    #[schema(default = 0)]
    pub collected_cents: i64,
    #[schema(required = false)]
    #[schema(default = "$0")]
    pub collected_label: String,
}

impl Default for TeamAverages {
    fn default() -> Self {
        Self {
            quoted: 0,
            quoted_label: "0".to_owned(),
            dispatched_cents: 0,
            dispatched_label: "$0".to_owned(),
            dispatch_rate: 0.0,
            dispatch_rate_label: "0%".to_owned(),
            avg_margin_cents: 0,
            avg_margin_label: "$0".to_owned(),
            collected_cents: 0,
            collected_label: "$0".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TeamLeaderCard {
    /// Agent's user id — the client resolves name/avatar from its roster
    pub user_id: String,
    pub name: Option<String>,
    pub initials: Option<String>,
    pub color: Option<String>,
    /// Dispatched revenue, formatted
    #[schema(required = false)]
    #[schema(default = "$0")]
    pub dispatched: String,
    /// Secondary line: '45% rate' (top) or '+24% vs prior 7d' / '-22% vs prior 7d'.
    #[schema(required = false)]
    #[schema(default = "")]
    pub detail: String,
    /// Signed % change vs prior window (most-improved / needs-coaching); null for top performer.
    pub delta: Option<f64>,
    /// 'up' | 'down' | '' — arrow direction for the delta.
    #[schema(required = false)]
    #[schema(default = "")]
    pub trend: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TeamLeaderboard {
    /// Highest dispatched revenue this window
    pub top_performer: Option<TeamLeaderCard>,
    /// Largest positive dispatched Δ vs prior
    pub most_improved: Option<TeamLeaderCard>,
    /// Largest negative dispatched Δ vs prior
    pub needs_coaching: Option<TeamLeaderCard>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DashboardTeamResponse {
    /// Period descriptor, e.g. 'Team · Last 7 days · Apr 16–22'
    pub period_label: String,
    /// Number of agents with activity in the window
    #[schema(required = false)]
    #[schema(default = 0)]
    pub team_size: i64,
    /// Per-agent strip, sorted by dispatched revenue desc. Client shows the top few + '+N more'.
    #[schema(required = false)]
    pub team: Vec<TeamMember>,
    #[schema(required = false)]
    pub averages: TeamAverages,
    #[schema(required = false)]
    pub leaderboard: TeamLeaderboard,
}
