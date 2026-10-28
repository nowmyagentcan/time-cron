use jiff::civil::{Date, Weekday};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    Unix,
    Quartz,
    Aws,
}

impl Dialect {
    /// The name on the command line and in JSON.
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Unix => "unix",
            Dialect::Quartz => "quartz",
            Dialect::Aws => "aws",
        }
    }

    /// The dialect a name such as "quartz" stands for, in any case.
    pub fn from_name(name: &str) -> Option<Dialect> {
        [Dialect::Unix, Dialect::Quartz, Dialect::Aws]
            .into_iter()
            .find(|d| d.name().eq_ignore_ascii_case(name))
    }

    /// The name in messages: "Quartz has 6 or 7 fields".
    pub fn title(self) -> &'static str {
        match self {
            Dialect::Unix => "Unix cron",
            Dialect::Quartz => "Quartz",
            Dialect::Aws => "AWS cron",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Second,
    Minute,
    Hour,
    DayOfMonth,
    Month,
    DayOfWeek,
    Year,
}

impl Kind {
    pub fn words(self) -> &'static str {
        match self {
            Kind::Second => "second",
            Kind::Minute => "minute",
            Kind::Hour => "hour",
            Kind::DayOfMonth => "day of month",
            Kind::Month => "month",
            Kind::DayOfWeek => "day of week",
            Kind::Year => "year",
        }
    }

    /// The name in JSON.
    pub fn key(self) -> &'static str {
        match self {
            Kind::DayOfMonth => "day_of_month",
            Kind::DayOfWeek => "day_of_week",
            _ => self.words(),
        }
    }

    /// The values the model holds (day of week 0-6, Sunday first).
    pub fn range(self) -> (u16, u16) {
        match self {
            Kind::Second | Kind::Minute => (0, 59),
            Kind::Hour => (0, 23),
            Kind::DayOfMonth => (1, 31),
            Kind::Month => (1, 12),
            Kind::DayOfWeek => (0, 6),
            Kind::Year => (1970, 2199),
        }
    }
}

/// One field as written (a nickname contributes its expansion).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub kind: Kind,
    pub text: String,
}

impl Field {
    /// Cron treats a day field that starts with `*` as unrestricted, even `*/2`.
    pub fn star(&self) -> bool {
        self.text.starts_with('*')
    }
}

/// A bit per value. One word covers every field but the year, which needs 2200 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Set<const W: usize = 1>([u64; W]);

pub type Years = Set<35>;

impl<const W: usize> Set<W> {
    pub const EMPTY: Self = Self([0; W]);

    pub fn insert(&mut self, v: u16) {
        self.0[v as usize / 64] |= 1 << (v % 64);
    }

    pub fn contains(&self, v: u16) -> bool {
        self.0
            .get(v as usize / 64)
            .is_some_and(|w| w & 1 << (v % 64) != 0)
    }

    pub fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        (0..W as u16 * 64).filter(|&v| self.contains(v))
    }
}

impl<const W: usize> FromIterator<u16> for Set<W> {
    fn from_iter<I: IntoIterator<Item = u16>>(iter: I) -> Self {
        let mut set = Self::EMPTY;
        iter.into_iter().for_each(|v| set.insert(v));
        set
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomSpecial {
    /// `L`, `L-n`, `LW`, `L-nW`: n days before the last day, or the weekday nearest it.
    Last { offset: u8, weekday: bool },
    /// `nW`: the weekday nearest day n, within the month.
    Nearest(u8),
}

/// Day of month: plain days 1-31 plus Quartz's specials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dom {
    pub days: Set,
    pub specials: Vec<DomSpecial>,
}

impl Dom {
    fn matches(&self, d: Date) -> bool {
        let (day, last) = (d.day(), d.days_in_month());
        self.days.contains(day as u16)
            || self.specials.iter().any(|&special| match special {
                DomSpecial::Last { offset, weekday } => {
                    let target = last - offset as i8;
                    target >= 1
                        && day
                            == if weekday {
                                nearest_weekday(d, target)
                            } else {
                                target
                            }
                }
                DomSpecial::Nearest(n) => n as i8 <= last && day == nearest_weekday(d, n as i8),
            })
    }
}

/// Quartz's `W`: the weekday nearest day `n` of `d`'s month, never leaving the month.
fn nearest_weekday(d: Date, n: i8) -> i8 {
    match d.with().day(n).build().unwrap().weekday() {
        Weekday::Saturday if n == 1 => 3,
        Weekday::Saturday => n - 1,
        Weekday::Sunday if n == d.days_in_month() => n - 2,
        Weekday::Sunday => n + 1,
        _ => n,
    }
}

/// Day of week, 0 = Sunday in every dialect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dow {
    Days(Set),
    /// `nL`: the last such weekday of the month.
    Last(u8),
    /// `n#m`: the m-th such weekday of the month.
    Nth(u8, u8),
}

impl Dow {
    fn matches(&self, d: Date) -> bool {
        let weekday = d.weekday().to_sunday_zero_offset() as u8;
        match *self {
            Dow::Days(set) => set.contains(weekday.into()),
            Dow::Last(w) => weekday == w && d.day() + 7 > d.days_in_month(),
            Dow::Nth(w, n) => weekday == w && (d.day() - 1) / 7 + 1 == n as i8,
        }
    }
}

/// How the two day fields combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayRule {
    Or,
    And,
    DomOnly,
    DowOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub dialect: Dialect,
    /// The dialect's fields in order; empty for `@reboot`.
    pub fields: Vec<Field>,
    pub second: Set,
    pub minute: Set,
    pub hour: Set,
    pub dom: Dom,
    pub month: Set,
    pub dow: Dow,
    pub year: Years,
    pub day_rule: DayRule,
    /// Neither minute nor hour starts with `*`: cronie's rule for replaying runs lost to DST.
    pub fixed_time: bool,
    pub reboot: bool,
}

impl Schedule {
    /// The field as written, if the dialect has it.
    pub fn field(&self, kind: Kind) -> Option<&Field> {
        self.fields.iter().find(|f| f.kind == kind)
    }

    /// The values a field holds; day of week only when it is a plain set of days.
    pub fn values(&self, kind: Kind) -> Vec<u16> {
        match kind {
            Kind::Second => self.second.iter().collect(),
            Kind::Minute => self.minute.iter().collect(),
            Kind::Hour => self.hour.iter().collect(),
            Kind::DayOfMonth => self.dom.days.iter().collect(),
            Kind::Month => self.month.iter().collect(),
            Kind::DayOfWeek => match self.dow {
                Dow::Days(set) => set.iter().collect(),
                _ => vec![],
            },
            Kind::Year => self.year.iter().collect(),
        }
    }

    /// Whether the schedule runs at some time on this date.
    pub fn runs_on(&self, d: Date) -> bool {
        let (dom, dow) = (|| self.dom.matches(d), || self.dow.matches(d));
        self.year.contains(d.year() as u16)
            && self.month.contains(d.month() as u16)
            && match self.day_rule {
                DayRule::Or => dom() || dow(),
                DayRule::And => dom() && dow(),
                DayRule::DomOnly => dom(),
                DayRule::DowOnly => dow(),
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_hold_values_up_to_their_width() {
        let s: Set = [0, 5, 63].into_iter().collect();
        assert!(s.contains(5) && !s.contains(6) && !s.contains(64));
        assert_eq!(s.iter().collect::<Vec<_>>(), [0, 5, 63]);
        let y: Years = [1970, 2199].into_iter().collect();
        assert_eq!(y.iter().collect::<Vec<_>>(), [1970, 2199]);
    }

    /// The days of a month the expression runs on.
    fn days(text: &str, year: i16, month: i8) -> Vec<i8> {
        let s = crate::expr::parse(text, None).unwrap();
        let first = jiff::civil::date(year, month, 1);
        (1..=first.days_in_month())
            .filter(|&d| s.runs_on(first.with().day(d).build().unwrap()))
            .collect()
    }

    #[test]
    fn matches_quartz_day_specials() {
        let none: [i8; 0] = [];
        assert_eq!(days("0 0 12 L * ?", 2028, 2), [29]);
        assert_eq!(days("0 0 12 L-3 * ?", 2026, 11), [27]);
        assert_eq!(days("0 0 12 L-30 * ?", 2026, 2), none);
        assert_eq!(days("0 0 12 L-30 * ?", 2026, 1), [1]);
        assert_eq!(days("0 0 9 LW * ?", 2026, 10), [30]); // the 31st is a Saturday
        assert_eq!(days("0 0 9 L-1W * ?", 2026, 5), [29]); // the 30th is a Saturday
        assert_eq!(days("0 0 9 15W * ?", 2026, 11), [16]); // Sunday the 15th
        assert_eq!(days("0 0 9 14W * ?", 2026, 11), [13]); // Saturday the 14th
        assert_eq!(days("0 0 9 1W * ?", 2026, 8), [3]); // Saturday the 1st stays in August
        assert_eq!(days("0 0 9 31W * ?", 2026, 5), [29]); // Sunday the 31st stays in May
        assert_eq!(days("0 0 9 31W * ?", 2026, 6), none);
        assert_eq!(days("0 0 9 L,15 * ?", 2026, 11), [15, 30]);
        assert_eq!(days("0 15 10 ? * 6L", 2026, 10), [30]);
        assert_eq!(days("0 0 12 ? * MON#2", 2026, 11), [9]);
        assert_eq!(days("0 0 12 ? * 2#5", 2026, 11), [30]);
        assert_eq!(days("0 0 12 ? * 2#5", 2026, 10), none);
        assert_eq!(days("0 0 12 ? * L", 2026, 10), [3, 10, 17, 24, 31]);
    }

    #[test]
    fn combines_day_fields_by_the_day_rule() {
        let none: [i8; 0] = [];
        assert_eq!(days("0 9 1 * MON", 2026, 11), [1, 2, 9, 16, 23, 30]);
        assert_eq!(days("0 9 */2 * MON", 2026, 11), [9, 23]);
        assert_eq!(days("0 0 * * 7", 2026, 11), [1, 8, 15, 22, 29]);
        assert_eq!(days("0 0 29 2 *", 2027, 2), none);
        assert_eq!(days("0 0 29 2 *", 2028, 2), [29]);
        assert_eq!(days("0 0 1 1 ? 2030", 2030, 1), [1]);
        assert_eq!(days("0 0 1 1 ? 2030", 2031, 1), none);
    }
}
