use super::*;

#[test]
fn civil_days_round_trip() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    for z in [-800_000, -1, 0, 59, 11_016, 19_723, 2_000_000] {
        let (y, m, d) = civil_from_days(z);
        assert_eq!(days_from_civil(y, m, d), z);
    }
    assert_eq!(fields(days_from_civil(2024, 1, 7) as f64 * DAY).wd, 0, "2024-01-07 was a Sunday");
}

#[test]
fn reads_dayjs_formats_and_iso() {
    let jan5 = days_from_civil(2024, 1, 5) as f64 * DAY;
    assert_eq!(parse("2024-01-05", "YYYY-MM-DD"), Some(jan5));
    assert_eq!(parse("05/01/2024", "DD/MM/YYYY"), Some(jan5));
    assert_eq!(parse("2024-01-05 13:30", "YYYY-MM-DD HH:mm"), Some(jan5 + 13.5 * HOUR));
    assert_eq!(parse("Jan 5 2024 1:30 pm", "MMM D YYYY h:mm a"), Some(jan5 + 13.5 * HOUR));
    assert_eq!(parse("2024-01-05", "DD.MM.YY"), Some(jan5), "falls back to ISO");
    assert_eq!(parse("1704412800", "X"), Some(jan5));
    assert_eq!(parse("2024-02-30", "YYYY-MM-DD"), None);
    assert_eq!(parse("soon", "YYYY-MM-DD"), None);
    assert_eq!(parse("99999999999999999999", "X"), None);
}

#[test]
fn spans_and_months() {
    let jan31 = days_from_civil(2024, 1, 31) as f64 * DAY;
    assert_eq!(span("3d"), Some(Span::Ms(3.0 * DAY)));
    assert_eq!(span("1.5h"), Some(Span::Ms(1.5 * HOUR)));
    assert_eq!(span("2w"), Some(Span::Ms(14.0 * DAY)));
    assert_eq!(span("3x"), None);
    assert_eq!(Span::Months(1.0).after(jan31), days_from_civil(2024, 2, 29) as f64 * DAY);
}

#[test]
fn prints_d3_formats() {
    let t = days_from_civil(2024, 3, 7) as f64 * DAY + 15.0 * HOUR + 4.0 * MIN;
    assert_eq!(format(t, "%Y-%m-%d"), "2024-03-07");
    assert_eq!(format(t, "%b %-d"), "Mar 7");
    assert_eq!(format(t, "%a %e %I:%M %p"), "Thu  7 03:04 PM");
    assert_eq!(format(t, "%B %j %%"), "March 067 %");
}

#[test]
fn ticks_land_on_the_calendar() {
    let lo = days_from_civil(2024, 1, 3) as f64 * DAY + HOUR;
    let hi = days_from_civil(2024, 4, 20) as f64 * DAY;
    let day = |t: f64| civil_from_days((t / DAY) as i64);
    let weeks = Step::Weeks(1).ticks(lo, hi, 100);
    assert_eq!(day(weeks[0]), (2024, 1, 7));
    assert!(weeks.iter().all(|&t| fields(t).wd == 0));
    let months = Step::Months(1).ticks(lo, hi, 100);
    assert_eq!(months.iter().map(|&t| day(t)).collect::<Vec<_>>(), [(2024, 2, 1), (2024, 3, 1), (2024, 4, 1)]);
    assert_eq!(Step::Days(1).ticks(lo, hi, 5).len(), 5, "capped");
    assert_eq!(Step::parse("2week"), Some(Step::Weeks(2)));
    assert_eq!(Step::parse("6hour"), Some(Step::Ms(6.0 * HOUR)));
}
