#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Unix,
    Quartz,
    Aws,
}

impl Dialect {
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

/// Day of week, 0 = Sunday in every dialect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dow {
    Days(Set),
    /// `nL`: the last such weekday of the month.
    Last(u8),
    /// `n#m`: the m-th such weekday of the month.
    Nth(u8, u8),
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
}
