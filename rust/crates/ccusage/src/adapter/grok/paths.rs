use std::{env, path::PathBuf};

use crate::Result;

pub(super) const GROK_HOME_ENV: &str = "GROK_HOME";

pub(super) fn grok_home(custom: Option<&str>) -> Result<PathBuf> {
    if let Some(custom) = custom.filter(|path| !path.trim().is_empty()) {
        return Ok(PathBuf::from(custom));
    }
    if let Ok(env_home) = env::var(GROK_HOME_ENV) {
        let trimmed = env_home.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home =
        crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
    Ok(home.join(".grok"))
}

pub(super) fn unified_log_path(home: &std::path::Path) -> PathBuf {
    home.join("logs").join("unified.jsonl")
}

pub(super) fn sessions_root(home: &std::path::Path) -> PathBuf {
    home.join("sessions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn custom_grok_home_overrides_env_and_default() {
        let fixture = fs_fixture!({});
        let _env = EnvVarGuard::set(GROK_HOME_ENV, "/env/grok");
        let home = grok_home(Some(fixture.root().to_str().unwrap())).unwrap();
        assert_eq!(home, fixture.root());
    }

    #[test]
    fn env_grok_home_is_used_when_custom_is_unset() {
        let fixture = fs_fixture!({});
        let _env = EnvVarGuard::set(GROK_HOME_ENV, fixture.root().to_str().unwrap());
        let home = grok_home(None).unwrap();
        assert_eq!(home, fixture.root());
    }
}
