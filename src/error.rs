/// A failure the user can act on: a stable code for scripts, a sentence, and a fix when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// 3 when the machine failed us, 1 when the input is at fault.
    pub fn exit_code(&self) -> u8 {
        match self.code {
            "port_in_use" | "self_check" | "io" => 3,
            _ => 1,
        }
    }
}

/// Something worth knowing that doesn't stop the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

impl Warning {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn carries_code_message_and_hint() {
        let e = Error::new("empty", "No cron expression given.").hint("Example: x");
        assert_eq!(e.code, "empty");
        assert_eq!(e.message, "No cron expression given.");
        assert_eq!(e.hint.as_deref(), Some("Example: x"));
        assert_eq!(Error::new("too_long", "m").hint, None);
    }

    #[test]
    fn runtime_failures_exit_3() {
        for code in ["port_in_use", "self_check", "io"] {
            assert_eq!(Error::new(code, "").exit_code(), 3);
        }
        assert_eq!(Error::new("never_runs", "").exit_code(), 1);
    }
}
