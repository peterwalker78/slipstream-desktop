//! Month grids for the notification centre's calendar. Pure date arithmetic on the proleptic
//! Gregorian calendar, with weeks starting on Monday.

pub const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// A calendar date. `month` runs 1–12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The day of the week, 0 for Monday to 6 for Sunday (Sakamoto's method).
pub fn weekday(date: Date) -> u32 {
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let year = if date.month < 3 {
        date.year - 1
    } else {
        date.year
    };
    let sunday_first = (year + year.div_euclid(4) - year.div_euclid(100)
        + year.div_euclid(400)
        + OFFSETS[date.month as usize - 1]
        + date.day as i32)
        .rem_euclid(7);
    ((sunday_first + 6) % 7) as u32
}

/// The month `by` months after `year`-`month`.
pub fn shift(year: i32, month: u32, by: i32) -> (i32, u32) {
    let index = year * 12 + month as i32 - 1 + by;
    (index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
}

/// One cell of a month grid: the day of the month, and whether it belongs to the month shown
/// rather than the one before or after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub day: u32,
    pub in_month: bool,
}

/// Whole weeks covering the month, Monday first: five rows, or six when the month needs them.
pub fn grid(year: i32, month: u32) -> Vec<Cell> {
    let lead = weekday(Date {
        year,
        month,
        day: 1,
    });
    let days = days_in_month(year, month);
    let (before_year, before_month) = shift(year, month, -1);
    let days_before = days_in_month(before_year, before_month);
    let rows = (lead + days).div_ceil(7).max(5);
    (0..rows * 7)
        .map(|i| {
            if i < lead {
                Cell {
                    day: days_before - lead + i + 1,
                    in_month: false,
                }
            } else if i < lead + days {
                Cell {
                    day: i - lead + 1,
                    in_month: true,
                }
            } else {
                Cell {
                    day: i - lead - days + 1,
                    in_month: false,
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn weekdays_match_known_dates() {
        assert_eq!(weekday(date(2026, 9, 11)), 4, "a Friday");
        assert_eq!(weekday(date(2000, 1, 1)), 5, "a Saturday");
        assert_eq!(weekday(date(2024, 2, 29)), 3, "a Thursday");
        assert_eq!(weekday(date(1970, 1, 1)), 3, "a Thursday");
    }

    #[test]
    fn leap_years_follow_the_gregorian_rules() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2026, 9), 30);
    }

    #[test]
    fn shifting_months_crosses_years() {
        assert_eq!(shift(2026, 1, -1), (2025, 12));
        assert_eq!(shift(2026, 12, 1), (2027, 1));
        assert_eq!(shift(2026, 9, -21), (2024, 12));
    }

    #[test]
    fn september_2026_starts_on_a_tuesday_after_august_31() {
        let cells = grid(2026, 9);
        assert_eq!(cells.len(), 35);
        assert_eq!(
            cells[0],
            Cell {
                day: 31,
                in_month: false
            }
        );
        assert_eq!(
            cells[1],
            Cell {
                day: 1,
                in_month: true
            }
        );
        assert_eq!(
            cells[30],
            Cell {
                day: 30,
                in_month: true
            }
        );
        assert_eq!(
            cells[31],
            Cell {
                day: 1,
                in_month: false
            }
        );
    }

    #[test]
    fn long_months_that_start_late_take_six_rows() {
        // August 2026 starts on a Saturday: 5 leading days + 31 needs six weeks.
        assert_eq!(grid(2026, 8).len(), 42);
        // February 2027 starts on a Monday and fills exactly four weeks; the grid keeps five.
        assert_eq!(grid(2027, 2).len(), 35);
    }
}
