//! The unit's history over time, laid out the same way by the command line and the TUI.

use jiff::tz::TimeZone;
use jiff::{RoundMode, SignedDuration, Timestamp, Unit, Zoned, ZonedRound};
use melcloud::OperationMode;

use crate::cache::Sample;

/// Samples further apart are not joined: the unit was not read in between.
pub const GAP: SignedDuration = SignedDuration::from_mins(45);

/// Minutes between time labels: the shortest step that leaves about a dozen columns per label.
const LABEL_MINUTES: [i64; 14] = [
    5, 10, 15, 30, 60, 120, 180, 360, 720, 1440, 2880, 10_080, 20_160, 43_200,
];
const COLUMNS_PER_LABEL: usize = 12;
const DAY_MINUTES: i64 = 1440;

/// What the unit did during one column of a chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    /// Mean room temperature of the samples in the column.
    pub room: f64,
    /// The mode the unit ran in for most of the column; `None` when it was mostly off.
    pub running: Option<OperationMode>,
}

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

    /// What the unit did in each column, from the samples read in it. A column without samples
    /// takes the state from the sample before it, unless the unit went unread for longer than
    /// [`GAP`]; then it is `None`. `samples` are oldest first.
    pub fn columns(&self, samples: &[Sample]) -> Vec<Option<Column>> {
        (0..self.width)
            .map(|index| {
                let start = self.column_start(index);
                let end = self.column_start(index + 1);
                let first = samples.partition_point(|s| s.at < start);
                let last = samples.partition_point(|s| s.at < end).max(first);
                let inside = &samples[first..last];
                let before = first
                    .checked_sub(1)
                    .map(|i| &samples[i..=i])
                    .filter(|before| start.duration_since(before[0].at) <= GAP);
                summarize(if inside.is_empty() {
                    before.unwrap_or_default()
                } else {
                    inside
                })
            })
            .collect()
    }

    /// Times under the columns, `width` characters: clock times, or days at midnight for long
    /// spans, as many as fit, with "now" at the right edge.
    pub fn axis(&self, tz: &TimeZone) -> String {
        let fit = i64::try_from(self.width / COLUMNS_PER_LABEL)
            .unwrap_or(i64::MAX)
            .max(1);
        let minutes = self.span().as_mins();
        let step = LABEL_MINUTES
            .into_iter()
            .find(|step| minutes / step <= fit)
            .unwrap_or(LABEL_MINUTES[LABEL_MINUTES.len() - 1]);
        let last = self.width.saturating_sub(1);
        let mut marks = vec![(last, "now".to_owned())];
        let mut mark = |at: &Zoned, format: &str| {
            let column = cell(self.share(at.timestamp()), last);
            marks.push((column, at.strftime(format).to_string()));
        };
        let start = self.since.to_zoned(tz.clone());
        let before_now = |at: &Zoned| at.timestamp() < self.now;
        if step < DAY_MINUTES {
            let (unit, increment) = if step < 60 {
                (Unit::Minute, step)
            } else {
                (Unit::Hour, step / 60)
            };
            // Rounding after every step keeps the labels on round clock times across a shift
            // of the clock that is not a whole step, like half an hour.
            let align = |at: Zoned| {
                let round = ZonedRound::new()
                    .smallest(unit)
                    .increment(increment)
                    .mode(RoundMode::Ceil);
                at.round(round).ok()
            };
            let mut next = align(start);
            while let Some(at) = next.take().filter(before_now) {
                mark(&at, "%H:%M");
                next = at
                    .checked_add(SignedDuration::from_mins(step))
                    .ok()
                    .and_then(align);
            }
        } else {
            // Weekdays within a week, dates beyond.
            let format = if self.span() <= SignedDuration::from_hours(24 * 7) {
                "%a %d"
            } else {
                "%b %d"
            };
            let every = step / DAY_MINUTES;
            let mut next = start
                .round(ZonedRound::new().smallest(Unit::Day).mode(RoundMode::Ceil))
                .ok();
            let mut count = 0;
            while let Some(at) = next.take().filter(before_now) {
                if count % every == 0 {
                    mark(&at, format);
                }
                count += 1;
                // From the start of each day, which is not always midnight.
                next = at.tomorrow().and_then(|day| day.start_of_day()).ok();
            }
        }
        place(&mut marks, self.width)
    }
}

/// The mean room temperature of `samples` and the state they were in most often; `None` when
/// there are none.
fn summarize(samples: &[Sample]) -> Option<Column> {
    let count = u32::try_from(samples.len()).ok().filter(|&n| n > 0)?;
    let room = samples.iter().map(|s| s.room).sum::<f64>() / f64::from(count);
    let state = |s: &Sample| s.power.then_some(s.mode);
    // The latest of the most frequent states wins a tie.
    let running = samples
        .iter()
        .map(state)
        .max_by_key(|candidate| samples.iter().filter(|s| state(s) == *candidate).count())?;
    Some(Column { room, running })
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
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn sample(time: &str, power: bool, mode: OperationMode, room: f64) -> Sample {
        Sample {
            at: at(time),
            power,
            mode,
            target: 22.0,
            room,
            outside: None,
            fan: None,
        }
    }

    fn timeline(hours: i64, width: usize) -> Timeline {
        let now = at("2026-01-15T10:05:30Z");
        Timeline {
            since: now - SignedDuration::from_hours(hours),
            now,
            width,
        }
    }

    fn spaced(parts: &[(usize, &str)]) -> String {
        parts
            .iter()
            .map(|&(spaces, label)| " ".repeat(spaces) + label)
            .collect()
    }

    #[test]
    fn the_axis_shows_clock_hours_up_to_now() {
        let axis = |width| timeline(24, width).axis(&TimeZone::UTC);
        assert_eq!(
            axis(50),
            spaced(&[
                (2, "12:00"),
                (7, "18:00"),
                (7, "00:00"),
                (8, "06:00"),
                (3, "now")
            ])
        );
        assert_eq!(axis(30), spaced(&[(0, "12:00"), (10, "00:00"), (7, "now")]));
        assert_eq!(
            axis(12),
            spaced(&[(9, "now")]),
            "a label too close to now is left out"
        );
        let plus_one = TimeZone::fixed(jiff::tz::offset(1));
        assert_eq!(
            timeline(24, 50).axis(&plus_one),
            spaced(&[
                (0, "12:00"),
                (7, "18:00"),
                (7, "00:00"),
                (8, "06:00"),
                (5, "now")
            ]),
            "hours are local"
        );
    }

    #[test]
    fn short_spans_are_labelled_in_minutes() {
        assert_eq!(
            timeline(1, 60).axis(&TimeZone::UTC),
            spaced(&[(7, "09:15"), (10, "09:30"), (10, "09:45"), (15, "now")])
        );
    }

    #[test]
    fn long_spans_are_labelled_by_days_then_dates() {
        assert_eq!(
            timeline(24 * 7, 60).axis(&TimeZone::UTC),
            spaced(&[(2, "Fri 09"), (11, "Sun 11"), (11, "Tue 13"), (15, "now")]),
            "every other day within a week"
        );
        let month = timeline(24 * 30, 60).axis(&TimeZone::UTC);
        assert!(month.starts_with("Dec 17"), "weekly: {month}");
        assert!(
            month.contains("Jan 07") && month.ends_with("now"),
            "{month}"
        );
    }

    #[test]
    fn labels_stay_on_round_times_across_a_half_hour_clock_change() {
        // Lord Howe Island moves its clocks by 30 minutes, here on 2026-10-04.
        let zone = TimeZone::posix("<+1030>-10:30<+11>-11,M10.1.0,M4.1.0").unwrap();
        let now = at("2026-10-04T12:00:00Z");
        let day = Timeline {
            since: now - SignedDuration::from_hours(24),
            now,
            width: 100,
        };
        let axis = day.axis(&zone);
        let labels: Vec<&str> = axis.split_whitespace().collect();
        assert!(labels.len() > 4, "{axis}");
        for label in &labels[..labels.len() - 1] {
            let (hour, minute) = label.split_once(':').unwrap();
            assert_eq!(minute, "00", "{axis}");
            assert_eq!(hour.parse::<u32>().unwrap() % 3, 0, "{axis}");
        }
    }

    #[test]
    fn columns_summarize_their_samples_and_stop_at_a_gap() {
        let heat = OperationMode::Heat;
        let cool = OperationMode::Cool;
        let samples = [
            sample("2026-01-15T06:00:00Z", false, heat, 20.0),
            sample("2026-01-15T07:00:00Z", true, heat, 21.0),
            sample("2026-01-15T07:10:00Z", true, cool, 22.0),
            sample("2026-01-15T07:20:00Z", true, cool, 23.0),
        ];
        let now = at("2026-01-15T10:00:00Z");
        let timeline = Timeline {
            since: now - SignedDuration::from_hours(5),
            now,
            width: 10,
        };
        let columns = timeline.columns(&samples);
        let shown: String = columns
            .iter()
            .map(|column| match column {
                None => ' ',
                Some(Column { running: None, .. }) => '·',
                Some(Column {
                    running: Some(OperationMode::Cool),
                    ..
                }) => 'c',
                Some(_) => 'h',
            })
            .collect();
        // 30 min columns from 05:00: off from 06:00, mostly cool from 07:00, and on until 45 min
        // after the last sample.
        assert_eq!(shown, "  ··ccc   ");
        assert_eq!(columns[4].unwrap().room, 22.0, "the mean of 21, 22 and 23");
        assert_eq!(columns[5].unwrap().room, 23.0, "the last sample carries on");
    }
}
