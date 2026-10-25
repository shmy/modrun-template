use humantime::parse_duration;
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ReadableDuration(Duration);

impl ReadableDuration {
    pub fn duration(&self) -> Duration {
        self.0
    }
}

impl FromStr for ReadableDuration {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_duration(s)
            .map(ReadableDuration)
            .map_err(|e| format!("invalid readable duration '{}': {}", s, e))
    }
}
