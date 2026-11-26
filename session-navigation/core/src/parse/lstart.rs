//! `ps -o lstart` in UTC, the format of the Claude registry's `procStart`
//! (e.g. `Wed Sep 30 10:00:00 2026`, VERIFIED Claude facts C). Whole seconds.
use super::ParseError;

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub(crate) fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn number(token: &str, min: i64, max: i64) -> Result<i64, ParseError> {
    if token.is_empty() || token.len() > 4 || !token.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::InvalidValue("lstart number"));
    }
    let value: i64 = token
        .parse()
        .map_err(|_| ParseError::InvalidValue("lstart number"))?;
    if value < min || value > max {
        return Err(ParseError::InvalidValue("lstart range"));
    }
    Ok(value)
}

/// Parse to seconds since the Unix epoch. The weekday must agree with the date.
pub fn parse(text: &str) -> Result<u64, ParseError> {
    if text.len() > 64 {
        return Err(ParseError::TooLarge);
    }
    let tokens: Vec<&str> = text.split_ascii_whitespace().collect();
    let [weekday, month, day, time, year] = tokens.as_slice() else {
        return Err(ParseError::InvalidValue("lstart shape"));
    };
    let weekday_index = WEEKDAYS
        .iter()
        .position(|w| w == weekday)
        .ok_or(ParseError::InvalidValue("lstart weekday"))?;
    let month_index = MONTHS
        .iter()
        .position(|m| m == month)
        .ok_or(ParseError::InvalidValue("lstart month"))?;
    let month = i64::try_from(month_index).map_err(|_| ParseError::InvalidValue("month"))? + 1;
    let year = number(year, 1970, 9999)?;
    let day = number(day, 1, days_in_month(year, month))?;
    let parts: Vec<&str> = time.split(':').collect();
    let [h, m, s] = parts.as_slice() else {
        return Err(ParseError::InvalidValue("lstart time"));
    };
    let (h, m, s) = (number(h, 0, 23)?, number(m, 0, 59)?, number(s, 0, 59)?);
    let days = days_from_civil(year, month, day);
    let expected_weekday = usize::try_from((days + 4).rem_euclid(7))
        .map_err(|_| ParseError::InvalidValue("weekday"))?;
    if expected_weekday != weekday_index {
        return Err(ParseError::InvalidValue("lstart weekday mismatch"));
    }
    let seconds = days * 86_400 + h * 3600 + m * 60 + s;
    u64::try_from(seconds).map_err(|_| ParseError::InvalidValue("lstart before epoch"))
}

/// Format seconds since the epoch the way `TZ=UTC ps -o lstart=` does.
pub fn format(seconds: u64) -> String {
    let secs = i64::try_from(seconds.min(253_402_300_799)).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let weekday = usize::try_from((days + 4).rem_euclid(7)).unwrap_or(0);
    let month_name = usize::try_from(month - 1)
        .ok()
        .and_then(|i| MONTHS.get(i))
        .copied()
        .unwrap_or("Jan");
    let weekday_name = WEEKDAYS.get(weekday).copied().unwrap_or("Sun");
    format!(
        "{weekday_name} {month_name} {day:>2} {:02}:{:02}:{:02} {year}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}
