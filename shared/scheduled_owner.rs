//! The task action accepts one strictly parsed parameter, never a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledOwner {
    pub pid: u32,
    pub birth: u64,
    pub nonce: uuid::Uuid,
}

impl ScheduledOwner {
    pub fn parse(value: &str) -> Result<Self, String> {
        let parts: Vec<_> = value.split(',').collect();
        if parts.len() != 3
            || !parts[..2]
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("Invalid scheduled owner".into());
        }
        let pid = parts[0].parse().map_err(|_| "Invalid desktop PID")?;
        let birth = parts[1]
            .parse()
            .map_err(|_| "Invalid desktop creation time")?;
        let nonce = uuid::Uuid::parse_str(parts[2]).map_err(|_| "Invalid stop event ID")?;
        if pid == 0 || birth == 0 || parts[2] != nonce.to_string() {
            return Err("Invalid scheduled owner".into());
        }
        Ok(Self { pid, birth, nonce })
    }

    pub fn parameter(&self) -> String {
        format!("{},{},{}", self.pid, self.birth, self.nonce)
    }

    pub fn stop_event(&self) -> String {
        format!("Local\\ConvenientWindow.HelperStop.{}", self.nonce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_parameter_cannot_supply_extra_arguments() {
        let owner = ScheduledOwner {
            pid: 10,
            birth: 99,
            nonce: uuid::Uuid::new_v4(),
        };
        assert_eq!(ScheduledOwner::parse(&owner.parameter()).unwrap(), owner);
        for value in [
            "",
            "0,99,xxx",
            "10,0,xxx",
            "10,99,$(Arg0)",
            "10,99,\" --data-dir C:\\evil",
            "10 11,99,xxx",
        ] {
            assert!(ScheduledOwner::parse(value).is_err());
        }
        assert!(ScheduledOwner::parse(&format!("{} --help", owner.parameter())).is_err());
    }
}
