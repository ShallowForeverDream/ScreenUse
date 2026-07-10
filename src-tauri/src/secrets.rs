use anyhow::Result;

const SERVICE: &str = "ScreenUse";

pub fn save_secret(name: &str, value: &str) -> Result<String> {
    let entry = keyring::Entry::new(SERVICE, name)?;
    entry.set_password(value)?;
    Ok(format!("credential://{}/{}", SERVICE, name))
}

pub fn read_secret(name: &str) -> Result<String> {
    let entry = keyring::Entry::new(SERVICE, name)?;
    Ok(entry.get_password()?)
}

pub fn delete_secret(name: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, name)?;
    let _ = entry.delete_credential();
    Ok(())
}
