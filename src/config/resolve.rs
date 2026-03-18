use crate::error::AppError;
use std::str::FromStr;

pub fn resolve_required<T>(param: Option<T>, env_var: &str, field: &str) -> Result<T, AppError>
where
    T: FromStr,
    T::Err: std::fmt::Debug,
{
    if let Some(value) = param {
        return Ok(value);
    }

    match std::env::var(env_var) {
        Ok(value) => value.parse::<T>().map_err(|e| {
            AppError::internal_error(
                format!("Config error [{field}/{env_var}]: Failed to parse value: {e:?}"),
                None,
            )
        }),
        Err(_) => Err(AppError::internal_error(
            format!(
                "Config error [{field}/{env_var}]: Value not provided and environment variable not set"
            ),
            None,
        )),
    }
}

pub fn resolve_with_default<T>(param: Option<T>, env_var: &str, default: T) -> T
where
    T: FromStr,
{
    if let Some(value) = param {
        return value;
    }

    std::env::var(env_var)
        .ok()
        .and_then(|v| v.parse::<T>().ok())
        .unwrap_or(default)
}

pub fn resolve_optional<T>(param: Option<T>, env_var: &str) -> Option<T>
where
    T: FromStr,
{
    if param.is_some() {
        return param;
    }

    std::env::var(env_var)
        .ok()
        .and_then(|v| v.parse::<T>().ok())
}

pub fn resolve_required_string(
    param: Option<String>,
    env_var: &str,
    field: &str,
) -> Result<String, AppError> {
    resolve_required(param, env_var, field)
}

pub fn resolve_optional_string(param: Option<String>, env_var: &str) -> Option<String> {
    resolve_optional(param, env_var)
}
