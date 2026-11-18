//! Log level names the ESL `log` command accepts.

wire_enum! {
    /// Argument of the ESL `log` command, which sets the level of `LOG` events
    /// the connection receives.
    ///
    /// Names and numbers are `LEVELS[]` / `switch_log_level_t`, read by
    /// `switch_log_str2level`. The switch compares them case-insensitively and
    /// also accepts a bare number (a negative one reads as 0, one past 64 as 63,
    /// and 64 itself is refused); this enum speaks the canonical lowercase form.
    /// A number past 7 receives what `Debug` does: no message is delivered above
    /// 7, since `switch_log_meta_vprintf` hands the `DEBUG1`..`DEBUG10` levels on
    /// at level 1 once the core debug level admits them.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[repr(i8)]
    pub enum LogLevel {
        Disable = -1 => "disable",
        Console = 0 => "console",
        Alert = 1 => "alert",
        Crit = 2 => "crit",
        Error = 3 => "err",
        Warning = 4 => "warning",
        Notice = 5 => "notice",
        Info = 6 => "info",
        Debug = 7 => "debug",
    }
    error ParseLogLevelError("log level");
    numeric: from_number(i8);
    tests: log_level_wire_tests;
}
