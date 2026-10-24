use anyhow::Result;

/// Load a value the way a crate that broke the rule would.
pub fn load() -> Result<String> {
    Ok(String::new())
}
