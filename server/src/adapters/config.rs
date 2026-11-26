#[derive(Debug, PartialEq)]
pub enum Driver {
    T3,
    OpenCode,
    Disabled,
}
pub fn select(driver: Option<&str>, legacy: Option<&str>) -> Result<Driver, &'static str> {
    match driver {
        Some("t3") => Ok(Driver::T3),
        Some("opencode") => Ok(Driver::OpenCode),
        Some(_) => Err("SKYWATCH_DRIVER must be t3 or opencode"),
        None if matches!(legacy, Some("0" | "false")) => Ok(Driver::Disabled),
        None => Ok(Driver::T3),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_matrix() {
        assert_eq!(select(None, None), Ok(Driver::T3));
        assert_eq!(select(None, Some("false")), Ok(Driver::Disabled));
        assert_eq!(select(Some("opencode"), Some("0")), Ok(Driver::OpenCode));
        assert_eq!(select(Some("t3"), Some("0")), Ok(Driver::T3));
        assert!(select(Some(""), None).is_err());
    }
}
