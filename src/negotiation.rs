pub const ROUNDING: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Counter,
    Escalate,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Counter => "counter",
            Self::Escalate => "escalate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub open: f64,
    pub limit: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub outcome: Outcome,
    pub price: Option<f64>,
}

fn beyond(side: Side, price: f64, mark: f64) -> bool {
    match side {
        Side::Buy => price > mark,
        Side::Sell => price < mark,
    }
}

pub fn ladder(side: Side, band: Band, max_concessions: u32) -> Vec<f64> {
    let gap = match side {
        Side::Buy => band.limit - band.open,
        Side::Sell => band.open - band.limit,
    };
    if gap <= 0.0 || max_concessions == 0 {
        return vec![band.open];
    }
    let weights: Vec<u32> = (2..=max_concessions + 1).rev().collect();
    let total = f64::from(weights.iter().sum::<u32>());
    let mut prices = vec![band.open];
    let mut moved = 0.0;
    for weight in &weights[..weights.len() - 1] {
        moved += gap * f64::from(*weight) / total;
        let step = (moved / ROUNDING).floor() * ROUNDING;
        let price = match side {
            Side::Buy => band.open + step,
            Side::Sell => band.open - step,
        };
        if prices.last().is_some_and(|last| beyond(side, price, *last)) {
            prices.push(price);
        }
    }
    prices.push(band.limit);
    prices
}

fn takes(side: Side, ask: f64, price: f64) -> bool {
    match side {
        Side::Buy => ask <= price,
        Side::Sell => ask >= price,
    }
}

pub fn next_step(side: Side, band: Band, offers: &[f64], ask: Option<f64>, max_concessions: u32) -> Step {
    if let (Some(ask), Some(last)) = (ask, offers.last())
        && takes(side, ask, *last)
    {
        return Step { outcome: Outcome::Accepted, price: Some(ask) };
    }
    let prices = ladder(side, band, max_concessions);
    let Some(candidate) = prices.get(offers.len()) else {
        return Step { outcome: Outcome::Escalate, price: ask };
    };
    if let Some(ask) = ask
        && takes(side, ask, *candidate)
    {
        return Step { outcome: Outcome::Accepted, price: Some(ask) };
    }
    Step { outcome: Outcome::Counter, price: Some(*candidate) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_should_shrink_moves_and_end_on_the_limit() {
        assert_eq!(ladder(Side::Buy, Band { open: 1000.0, limit: 1100.0 }, 2), vec![1000.0, 1060.0, 1100.0]);
    }

    #[test]
    fn ladder_should_round_each_move_down_to_five_dollars() {
        assert_eq!(ladder(Side::Buy, Band { open: 900.0, limit: 1003.0 }, 3), vec![900.0, 945.0, 980.0, 1003.0]);
    }

    #[test]
    fn ladder_should_hold_the_open_price_without_room() {
        assert_eq!(ladder(Side::Buy, Band { open: 1000.0, limit: 990.0 }, 2), vec![1000.0]);
    }

    #[test]
    fn ladder_should_walk_down_for_a_seller() {
        assert_eq!(ladder(Side::Sell, Band { open: 1500.0, limit: 1300.0 }, 2), vec![1500.0, 1380.0, 1300.0]);
    }

    #[test]
    fn next_step_should_take_an_ask_at_or_under_the_last_offer() {
        let step = next_step(Side::Buy, Band { open: 1000.0, limit: 1100.0 }, &[1000.0], Some(990.0), 2);
        assert_eq!(step, Step { outcome: Outcome::Accepted, price: Some(990.0) });
    }

    #[test]
    fn next_step_should_counter_with_the_next_rung() {
        let step = next_step(Side::Buy, Band { open: 1000.0, limit: 1100.0 }, &[1000.0], Some(1200.0), 2);
        assert_eq!(step, Step { outcome: Outcome::Counter, price: Some(1060.0) });
    }

    #[test]
    fn next_step_should_escalate_after_the_limit() {
        let step =
            next_step(Side::Buy, Band { open: 1000.0, limit: 1100.0 }, &[1000.0, 1060.0, 1100.0], Some(1200.0), 2);
        assert_eq!(step, Step { outcome: Outcome::Escalate, price: Some(1200.0) });
    }
}
