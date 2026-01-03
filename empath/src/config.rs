use std::path::Path;

use anyhow::Context;

use crate::controller::Empath;

/// Parse an Empath configuration from a string.
///
/// # Errors
///
/// Returns an error if the config cannot be parsed.
pub fn parse_config_str(config: &str) -> anyhow::Result<Empath> {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .from_str::<Empath>(config)
        .map_err(Into::into)
}

/// Parse an Empath configuration from a file.
///
/// # Errors
///
/// Returns an error if the file cannot be read or parsed.
pub fn parse_config_file(path: &Path) -> anyhow::Result<Empath> {
    let config_content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config from {}", path.display()))?;
    parse_config_str(&config_content)
        .with_context(|| format!("Failed to parse config {}", path.display()))
}

const fn default_config_template() -> &'static str {
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../empath.config.ron"))
}

/// Emit the default configuration template.
///
/// # Errors
///
/// Returns an error if serialization fails or the config no longer parses.
pub fn emit_default_config() -> anyhow::Result<String> {
    let template = default_config_template();
    let _ = parse_config_str(template)?;
    Ok(template.to_string())
}

#[cfg(test)]
mod tests {
    use super::{default_config_template, parse_config_str};

    #[cfg_attr(miri, ignore = "Miri timeout parsing default config template")]
    #[test]
    fn default_config_template_parses() -> anyhow::Result<()> {
        parse_config_str(default_config_template())?;
        Ok(())
    }
}
