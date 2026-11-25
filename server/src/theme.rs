use serde::Serialize;
#[derive(Clone, Serialize)]
pub struct BootConfig {
    pub theme: &'static str,
}
impl BootConfig {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        let theme = match value {
            None | Some("alpine") => "alpine",
            Some("tropical") => "tropical",
            Some("winter") => "winter",
            Some(_) => return Err("SKYWATCH_THEME must be alpine, tropical, or winter".into()),
        };
        Ok(Self { theme })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_public_boot_config() {
        assert_eq!(BootConfig::parse(None).unwrap().theme, "alpine");
        for theme in ["alpine", "tropical", "winter"] {
            assert_eq!(
                serde_json::to_value(BootConfig::parse(Some(theme)).unwrap()).unwrap(),
                serde_json::json!({"theme":theme})
            );
        }
        for invalid in ["", "Winter", "<script>"] {
            assert!(BootConfig::parse(Some(invalid)).is_err());
        }
    }
}
