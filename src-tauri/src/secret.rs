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
    let protected = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };
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
    let plaintext = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };
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
        assert!(!protected.windows(first.len()).any(|w| w == first.as_bytes()));
    }
}
