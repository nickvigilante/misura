//! Percentiles over repeated runs.

use std::time::Duration;

use serde::Serialize;

use crate::measure::round_ms;

/// The spread of one phase across runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub min: Duration,
    pub p50: Duration,
    pub p90: Duration,
    pub p99: Duration,
    pub max: Duration,
}

impl Serialize for Summary {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Summary", 5)?;
        st.serialize_field("min", &round_ms(self.min))?;
        st.serialize_field("p50", &round_ms(self.p50))?;
        st.serialize_field("p90", &round_ms(self.p90))?;
        st.serialize_field("p99", &round_ms(self.p99))?;
        st.serialize_field("max", &round_ms(self.max))?;
        st.end()
    }
}

/// Nearest-rank percentile: the smallest value with at least `p` percent of
/// the values at or below it. It's always one of the measured values, never
/// an interpolation between two.
fn nearest_rank(sorted: &[Duration], p: u32) -> Duration {
    let rank = (sorted.len() * p as usize).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// `None` when there are no values, such as DNS for an IP literal.
pub fn summarize(mut values: Vec<Duration>) -> Option<Summary> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(Summary {
        min: values[0],
        p50: nearest_rank(&values, 50),
        p90: nearest_rank(&values, 90),
        p99: nearest_rank(&values, 99),
        max: values[values.len() - 1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn nearest_rank_percentiles() {
        let s = summarize((1..=100).rev().map(ms).collect()).unwrap();
        assert_eq!(
            (s.min, s.p50, s.p90, s.p99, s.max),
            (ms(1), ms(50), ms(90), ms(99), ms(100))
        );
        let s = summarize(vec![ms(3), ms(1), ms(2)]).unwrap();
        assert_eq!(
            (s.min, s.p50, s.p90, s.p99, s.max),
            (ms(1), ms(2), ms(3), ms(3), ms(3))
        );
        let s = summarize(vec![ms(7)]).unwrap();
        assert_eq!((s.min, s.p50, s.max), (ms(7), ms(7), ms(7)));
        assert_eq!(summarize(Vec::new()), None);
    }
}
