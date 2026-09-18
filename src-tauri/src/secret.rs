use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

#[cfg(windows)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len())
            .map_err(|_| "Credential is too large to protect.".to_string())?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!(
            "Cannot protect database credential with Windows DPAPI: {}.",
            std::io::Error::last_os_error()
        ));
    }
    let protected =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(output.pbData.cast());
    }
    Ok(protected)
}

#[cfg(windows)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len())
            .map_err(|_| "Protected credential is too large to read.".to_string())?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!(
            "Cannot decrypt database credential for this Windows user: {}. Restore the matching CoffeePOS configuration or re-provision the runtime.",
            std::io::Error::last_os_error()
        ));
    }
    let plaintext =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(output.pbData.cast());
    }
    Ok(plaintext)
}

#[cfg(not(windows))]
fn protect(_bytes: &[u8]) -> Result<Vec<u8>, String> {
    Err("Protected credential storage is not implemented for this platform in the Windows-first Phase 2 milestone.".into())
}

#[cfg(not(windows))]
fn unprotect(_bytes: &[u8]) -> Result<Vec<u8>, String> {
    Err("Protected credential storage is not implemented for this platform in the Windows-first Phase 2 milestone.".into())
}

fn generate_password() -> Result<String, String> {
    let mut random = [0_u8; 24];
    getrandom::fill(&mut random)
        .map_err(|e| format!("Cannot generate database credential: {e}."))?;
    let mut password = String::with_capacity(random.len() * 2);
    for byte in random {
        use std::fmt::Write as _;
        write!(&mut password, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(password)
}

fn generate_hex_secret(byte_len: usize, label: &str) -> Result<String, String> {
    let mut random = vec![0_u8; byte_len];
    getrandom::fill(&mut random).map_err(|e| format!("Cannot generate {label}: {e}."))?;
    let mut value = String::with_capacity(random.len() * 2);
    for byte in random {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(value)
}

fn store(path: &Path, plaintext: &str, label: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Protected {label} directory is missing."))?;
    fs::create_dir_all(parent).map_err(|e| {
        format!(
            "Cannot create protected {label} directory {}: {e}.",
            parent.display()
        )
    })?;
    let protected = protect(plaintext.as_bytes())?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|e| {
        format!(
            "Cannot create temporary protected {label} in {}: {e}.",
            parent.display()
        )
    })?;
    temporary
        .write_all(&protected)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| format!("Cannot write protected {label}: {e}."))?;
    temporary.persist(path).map_err(|e| {
        format!(
            "Cannot persist protected {label} at {}: {}.",
            path.display(),
            e.error
        )
    })?;
    Ok(())
}

pub fn load(path: &Path) -> Result<String, String> {
    let protected = fs::read(path).map_err(|e| {
        format!(
            "Cannot read protected database credential at {}: {e}.",
            path.display()
        )
    })?;
    let plaintext = unprotect(&protected)?;
    String::from_utf8(plaintext)
        .map_err(|_| "Protected database credential contains invalid UTF-8.".to_string())
}

pub fn create(path: &Path) -> Result<String, String> {
    if path.exists() {
        return load(path);
    }
    let parent = path
        .parent()
        .ok_or("Protected credential directory is missing.")?;
    fs::create_dir_all(parent).map_err(|e| {
        format!(
            "Cannot create protected credential directory {}: {e}.",
            parent.display()
        )
    })?;
    let password = generate_password()?;
    let protected = protect(password.as_bytes())?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|e| {
        format!(
            "Cannot create temporary protected credential in {}: {e}.",
            parent.display()
        )
    })?;
    temporary
        .write_all(&protected)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| format!("Cannot write protected database credential: {e}."))?;
    temporary.persist(path).map_err(|e| {
        format!(
            "Cannot persist protected database credential at {}: {}.",
            path.display(),
            e.error
        )
    })?;
    Ok(password)
}

#[cfg(debug_assertions)]
pub fn store_password(path: &Path, password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("Password cannot be empty.".into());
    }
    store(path, password, "WordPress administrator credential")
}

#[cfg(debug_assertions)]
pub fn promote_staged_password(staged_path: &Path, active_path: &Path) -> Result<(), String> {
    let password = load(staged_path).map_err(|error| {
        format!("Cannot read staged WordPress administrator credential: {error}")
    })?;
    store_password(active_path, &password)?;
    let verified = load(active_path)?;
    if verified != password {
        return Err("Protected WordPress administrator credential verification failed after promotion. The staged credential has been preserved and provisioning is blocked.".into());
    }
    fs::remove_file(staged_path).map_err(|error| {
        format!("Cannot remove staged WordPress administrator credential after promotion: {error}. Provisioning remains blocked until setup is saved again.")
    })?;
    Ok(())
}

pub fn create_machine_token(path: &Path) -> Result<String, String> {
    if path.exists() {
        let token = load(path)?;
        if token.len() == 64
            && token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Ok(token);
        }
        return Err("Protected CoffeePOS machine token has an invalid format.".into());
    }
    let token = generate_hex_secret(32, "CoffeePOS machine token")?;
    store(path, &token, "CoffeePOS machine token")?;
    Ok(token)
}

pub fn store_machine_token(path: &Path, token: &str) -> Result<(), String> {
    if token.len() != 64
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("CoffeePOS machine token has an invalid format.".into());
    }
    store(path, token, "CoffeePOS machine token")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn protected_secret_round_trip_is_stable_for_current_user() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("database.secret");
        let first = create(&path).unwrap();
        assert_eq!(first.len(), 48);
        assert_eq!(load(&path).unwrap(), first);
        assert_eq!(create(&path).unwrap(), first);
        let protected = fs::read(path).unwrap();
        assert!(!protected
            .windows(first.len())
            .any(|w| w == first.as_bytes()));
    }

    #[test]
    fn protected_machine_token_is_32_random_bytes_as_lowercase_hex() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("machine-token.secret");
        let first = create_machine_token(&path).unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(first.to_ascii_lowercase(), first);
        assert_eq!(create_machine_token(&path).unwrap(), first);
        let protected = fs::read(path).unwrap();
        assert!(!protected
            .windows(first.len())
            .any(|window| window == first.as_bytes()));
    }

    #[test]
    fn user_supplied_password_round_trip_is_protected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("wordpress-admin.secret");
        let password = "Phase52SecurePassword2026";
        store_password(&path, password).unwrap();
        assert_eq!(load(&path).unwrap(), password);
        let protected = fs::read(path).unwrap();
        assert!(!protected
            .windows(password.len())
            .any(|window| window == password.as_bytes()));
    }

    #[test]
    fn staged_password_promotion_replaces_active_and_cleans_pending() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("wordpress-admin.secret");
        let pending = temp.path().join("wordpress-admin.pending.secret");
        store_password(&active, "Phase52OldPassword2026").unwrap();
        store_password(&pending, "Phase52NewPassword2026").unwrap();
        promote_staged_password(&pending, &active).unwrap();
        assert_eq!(load(&active).unwrap(), "Phase52NewPassword2026");
        assert!(!pending.exists());
    }

    #[test]
    fn staged_password_replacement_keeps_pending_marker_and_latest_value() {
        let temp = tempfile::tempdir().unwrap();
        let pending = temp.path().join("wordpress-admin.pending.secret");
        store_password(&pending, "FirstPendingPassword2026").unwrap();
        assert!(pending.exists());
        store_password(&pending, "ReplacementPassword2026").unwrap();
        assert!(pending.exists());
        assert_eq!(load(&pending).unwrap(), "ReplacementPassword2026");
    }
}
