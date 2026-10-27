//! The unit's history over time, laid out the same way by the command line and the TUI.

use jiff::tz::TimeZone;
use jiff::{RoundMode, SignedDuration, Timestamp, Unit, ZonedRound};

use crate::cache::Sample;

/// Samples further apart are not joined: the unit was not read in between.
pub const GAP: SignedDuration = SignedDuration::from_mins(45);

/// Hours between time labels, the shortest that leaves about a dozen columns per label.
const LABEL_HOURS: [i64; 10] = [1, 2, 3, 6, 12, 24, 48, 168, 336, 720];
const COLUMNS_PER_LABEL: usize = 12;

/// A span of time up to now, drawn `width` columns wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeline {
    pub since: Timestamp,
    pub now: Timestamp,
    pub width: usize,
}

impl Timeline {
    pub fn span(&self) -> SignedDuration {
        self.now.duration_since(self.since)
    }

    /// Where `at` lies between the start (0) and now (1).
    pub fn share(&self, at: Timestamp) -> f64 {
        at.duration_since(self.since).as_secs_f64() / self.span().as_secs_f64().max(1.0)
    }

    /// When column `index` starts; `width` gives now.
    fn column_start(&self, index: usize) -> Timestamp {
        let width = i128::try_from(self.width.max(1)).unwrap_or(1);
        let index = i128::try_from(index).unwrap_or(0);
        let offset = self.span().as_nanos() * index / width;
        self.since + SignedDuration::from_nanos(i64::try_from(offset).unwrap_or(i64::MAX))
    }

    /// The state of the unit during each column: the latest sample read before the column ends,
    /// or nothing when the unit went unread for longer than [`GAP`] before it. `samples` are
    /// oldest first.
    pub fn columns<'a>(&self, samples: &'a [Sample]) -> Vec<Option<&'a Sample>> {
        (0..self.width)
            .map(|index| {
                let start = self.column_start(index);
                let end = self.column_start(index + 1);
                let read = samples.partition_point(|s| s.at < end);
                read.checked_sub(1)
                    .map(|last| &samples[last])
                    .filter(|s| start.duration_since(s.at) <= GAP)
            })
            .collect()
    }

    /// Times under the columns, `width` characters: clock hours, or days at midnight for long
    /// spans, as many as fit, with "now" at the right edge.
    pub fn axis(&self, tz: &TimeZone) -> String {
        let fit = i64::try_from(self.width / COLUMNS_PER_LABEL)
            .unwrap_or(i64::MAX)
            .max(1);
        let hours = self.span().as_hours();
        let step = LABEL_HOURS
            .into_iter()
            .find(|step| hours / step <= fit)
            .unwrap_or(LABEL_HOURS[LABEL_HOURS.len() - 1]);
        let last = self.width.saturating_sub(1);
        let mut marks = vec![(last, "now".to_owned())];
        let (unit, every, format) = if step < 24 {
            (Unit::Hour, step, "%H:%M")
        } else {
            (Unit::Day, step / 24, "%a %d")
        };
        let mut next = self
            .since
            .to_zoned(tz.clone())
            .round(ZonedRound::new().smallest(unit).mode(RoundMode::Ceil))
            .ok();
        let mut count = 0;
        while let Some(at) = next.take().filter(|at| at.timestamp() < self.now) {
            let due = if unit == Unit::Hour {
                i64::from(at.hour()) % every == 0
            } else {
                count % every == 0
            };
            if due {
                let column = cell(self.share(at.timestamp()), last);
                marks.push((column, at.strftime(format).to_string()));
            }
            count += 1;
            next = if unit == Unit::Hour {
                at.checked_add(SignedDuration::from_hours(1)).ok()
            } else {
                at.tomorrow().ok()
            };
        }
        place(&mut marks, self.width)
    }
}

/// The cell for `share` (0 to 1) of a scale whose last cell is `last`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the share is clamped to 0..=1 and a scale is a few hundred cells at most"
)]
pub fn cell(share: f64, last: usize) -> usize {
    (share.clamp(0.0, 1.0) * last as f64).round() as usize
}

/// `marks` (column, label) centered on their columns in a line `width` wide. The first mark
/// always shows; from the right, a label that would touch the one after it is left out.
fn place(marks: &mut [(usize, String)], width: usize) -> String {
    if let [_, rest @ ..] = marks {
        rest.sort_by_key(|&(center, _)| std::cmp::Reverse(center));
    }
    let mut text = vec![' '; width];
    let mut limit = width;
    for (center, label) in marks.iter() {
        let length = label.chars().count();
        let start = center
            .saturating_sub(length / 2)
            .min(width.saturating_sub(length));
        if start + length > limit {
            continue;
        }
        for (slot, glyph) in text[start..].iter_mut().zip(label.chars()) {
            *slot = glyph;
        }
        limit = start.saturating_sub(1);
    }
    text.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use melcloud::OperationMode;

    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn sample(time: &str, power: bool) -> Sample {
        Sample {
            at: at(time),
            power,
            mode: OperationMode::Heat,
            target: 22.0,
            room: 21.0,
            outside: None,
            fan: None,
        }
    }

    fn day(width: usize) -> Timeline {
        let now = at("2026-01-15T10:05:30Z");
        Timeline {
            since: now - SignedDuration::from_hours(24),
            now,
            width,
        }
    }

    #[test]
    fn the_axis_shows_clock_hours_up_to_now() {
        let axis = |width| day(width).axis(&TimeZone::UTC);
        assert_eq!(
            axis(50),
            "  12:00       18:00       00:00        06:00   now"
        );
        assert_eq!(axis(30), "12:00          00:00       now");
        assert_eq!(
            axis(12),
            format!("{}now", " ".repeat(9)),
            "a day label too close to now is left out"
        );
        let zone = TimeZone::fixed(jiff::tz::offset(1));
        assert_eq!(
            day(50).axis(&zone),
            format!(
                "12:00{}18:00{}00:00{}06:00{}now",
                " ".repeat(7),
                " ".repeat(7),
                " ".repeat(8),
                " ".repeat(5)
            ),
            "hours are local"
        );
    }

    #[test]
    fn a_week_is_labelled_by_days() {
        let now = at("2026-01-15T10:05:30Z");
        let week = Timeline {
            since: now - SignedDuration::from_hours(24 * 7),
            now,
            width: 60,
        };
        assert_eq!(
            week.axis(&TimeZone::UTC),
            format!(
                "{}Fri 09{}Sun 11{}Tue 13{}now",
                " ".repeat(2),
                " ".repeat(11),
                " ".repeat(11),
                " ".repeat(15)
            ),
            "every other day, at midnight"
        );
    }

    #[test]
    fn columns_follow_the_latest_sample_until_a_gap() {
        let samples = [
            sample("2026-01-15T06:00:00Z", false),
            sample("2026-01-15T07:00:00Z", true),
        ];
        let now = at("2026-01-15T10:00:00Z");
        let timeline = Timeline {
            since: now - SignedDuration::from_hours(5),
            now,
            width: 10,
        };
        let shown: String = timeline
            .columns(&samples)
            .iter()
            .map(|column| match column {
                None => ' ',
                Some(s) if s.power => '▀',
                Some(_) => '·',
            })
            .collect();
        // Columns of 30 min from 05:00: off from 06:00, on from 07:00 until 45 min after it.
        assert_eq!(shown, "  ··▀▀    ");
    }
}
