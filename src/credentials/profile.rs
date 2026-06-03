use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{auth::Credentials, error::Error};

pub(crate) fn profile_from_env() -> Result<String, Error> {
    if let Some((name, profile)) =
        crate::util::env::optional_first_var(&["AWS_PROFILE", "AWS_DEFAULT_PROFILE"])?
    {
        return validate_profile_name(name, profile);
    }

    Ok("default".to_string())
}

pub(crate) fn load_profile_credentials(profile: &str) -> Result<Credentials, Error> {
    let profile = validate_profile_name("profile", profile.to_string())?;
    let creds_ini = read_ini_file(&credentials_path()?)?;
    let config_ini = match config_path() {
        Ok(path) if path.exists() => read_ini_file(&path)?,
        _ => HashMap::new(),
    };

    let profile_section_credentials = profile.clone();
    let profile_section_config = if profile == "default" {
        "default".to_string()
    } else {
        format!("profile {profile}")
    };

    let access_key_id = lookup_access_key(&creds_ini, &profile_section_credentials)
        .or_else(|| lookup_access_key(&config_ini, &profile_section_config))
        .ok_or_else(|| Error::invalid_config("missing aws_access_key_id in profile"))?;
    let secret_access_key = lookup_secret_key(&creds_ini, &profile_section_credentials)
        .or_else(|| lookup_secret_key(&config_ini, &profile_section_config))
        .ok_or_else(|| Error::invalid_config("missing aws_secret_access_key in profile"))?;

    let session_token = lookup(
        &creds_ini,
        &profile_section_credentials,
        "aws_session_token",
    )
    .or_else(|| lookup(&config_ini, &profile_section_config, "aws_session_token"));

    let mut creds = Credentials::new(access_key_id, secret_access_key)?;
    if let Some(token) = session_token {
        creds = creds.with_session_token(token)?;
    }

    Ok(creds)
}

fn validate_profile_name(name: &'static str, value: String) -> Result<String, Error> {
    if value.is_empty() {
        return Err(Error::invalid_config(format!("{name} must not be empty")));
    }
    if value.trim() != value {
        return Err(Error::invalid_config(format!(
            "{name} must not include leading or trailing whitespace"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(Error::invalid_config(format!(
            "{name} must not contain control characters"
        )));
    }
    Ok(value)
}

fn env_path(name: &'static str) -> Result<Option<PathBuf>, Error> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(Error::invalid_config(format!("{name} must not be empty")));
    }
    Ok(Some(PathBuf::from(value)))
}

fn home_dir() -> Result<Option<PathBuf>, Error> {
    if let Some(path) = env_path("HOME")? {
        return Ok(Some(path));
    }
    env_path("USERPROFILE")
}

fn default_aws_dir() -> Result<PathBuf, Error> {
    let home = home_dir()?.ok_or_else(|| Error::invalid_config("cannot determine home dir"))?;
    Ok(home.join(".aws"))
}

fn credentials_path() -> Result<PathBuf, Error> {
    if let Some(path) = env_path("AWS_SHARED_CREDENTIALS_FILE")? {
        return Ok(path);
    }
    Ok(default_aws_dir()?.join("credentials"))
}

fn config_path() -> Result<PathBuf, Error> {
    if let Some(path) = env_path("AWS_CONFIG_FILE")? {
        return Ok(path);
    }
    Ok(default_aws_dir()?.join("config"))
}

fn read_ini_file(path: &Path) -> Result<HashMap<String, HashMap<String, String>>, Error> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        Error::invalid_config(format!(
            "failed to read AWS profile file {}: {e}",
            path.display()
        ))
    })?;
    Ok(parse_ini(&contents))
}

fn parse_ini(contents: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current: Option<String> = None;

    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
            let name = line[1..line.len() - 1].trim();
            if name.is_empty() {
                current = None;
            } else {
                current = Some(name.to_string());
            }
            continue;
        }

        let Some(section) = current.as_ref() else {
            continue;
        };

        let Some((k, v)) = line.split_once('=').or_else(|| line.split_once(':')) else {
            continue;
        };

        let key = k.trim().to_ascii_lowercase();
        let value = v.trim().to_string();
        if key.is_empty() {
            continue;
        }

        sections
            .entry(section.to_string())
            .or_default()
            .insert(key, value);
    }

    sections
}

fn lookup(
    map: &HashMap<String, HashMap<String, String>>,
    section: &str,
    key: &str,
) -> Option<String> {
    map.get(section)
        .and_then(|s| s.get(key))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn lookup_access_key(
    map: &HashMap<String, HashMap<String, String>>,
    section: &str,
) -> Option<String> {
    lookup(map, section, "aws_access_key_id").or_else(|| lookup(map, section, "aws_access_key"))
}

fn lookup_secret_key(
    map: &HashMap<String, HashMap<String, String>>,
    section: &str,
) -> Option<String> {
    lookup(map, section, "aws_secret_access_key").or_else(|| lookup(map, section, "aws_secret_key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ini_sections_and_keys() {
        let ini = r#"
; comment
[default]
aws_access_key_id = AKID
aws_secret_access_key= SECRET
aws_session_token: TOKEN

[profile dev]
aws_access_key = AKID2
aws_secret_key : SECRET2

ignored = outside
"#;

        let parsed = parse_ini(ini);

        assert_eq!(
            lookup_access_key(&parsed, "default").as_deref(),
            Some("AKID")
        );
        assert_eq!(
            lookup_secret_key(&parsed, "default").as_deref(),
            Some("SECRET")
        );
        assert_eq!(
            lookup(&parsed, "default", "aws_session_token").as_deref(),
            Some("TOKEN")
        );

        assert_eq!(
            lookup_access_key(&parsed, "profile dev").as_deref(),
            Some("AKID2")
        );
        assert_eq!(
            lookup_secret_key(&parsed, "profile dev").as_deref(),
            Some("SECRET2")
        );

        assert!(lookup(&parsed, "default", "ignored").is_none());
    }

    #[test]
    fn profile_name_rejects_ambiguous_values() {
        assert_eq!(
            validate_profile_name("AWS_PROFILE", "dev profile".to_string()).unwrap(),
            "dev profile"
        );
        assert!(validate_profile_name("AWS_PROFILE", String::new()).is_err());
        assert!(validate_profile_name("AWS_PROFILE", " dev".to_string()).is_err());
        assert!(validate_profile_name("AWS_PROFILE", "dev ".to_string()).is_err());
        assert!(validate_profile_name("AWS_PROFILE", "dev\nprofile".to_string()).is_err());
    }

    #[test]
    fn load_profile_credentials_rejects_invalid_profile_before_io() {
        match load_profile_credentials(" dev") {
            Err(Error::InvalidConfig { message }) => assert!(message.contains("profile")),
            Err(other) => panic!("expected invalid config, got {other:?}"),
            Ok(_) => panic!("expected invalid profile to fail"),
        }
    }
}
