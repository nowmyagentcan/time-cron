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
