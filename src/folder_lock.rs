use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = ".swift-explorer-folder-lock-v1";
const ITERATIONS: u32 = 600_000;

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
struct Verifier {
    salt: [u8; 16],
    hash: [u8; 32],
}

#[derive(Clone, PartialEq, Eq)]
pub struct FolderLock {
    pub folder: PathBuf,
    verifier: Verifier,
}

impl FolderLock {
    pub fn verify(&self, password: &str) -> io::Result<bool> {
        let hash = derive(password.as_bytes(), &self.verifier.salt, ITERATIONS)?;
        let difference = hash
            .iter()
            .zip(self.verifier.hash)
            .fold(0, |diff, (a, b)| diff | (a ^ b));
        Ok(difference == 0)
    }

    pub fn remove(&self) -> io::Result<()> {
        let path = self.folder.join(FILE_NAME);
        if read_verifier(&self.folder)?.as_ref() != Some(&self.verifier) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "folder lock changed",
            ));
        }
        fs::remove_file(path)
    }
}

pub fn is_marker(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(prefix) = name.get(..FILE_NAME.len()) else {
        return false;
    };
    if prefix != FILE_NAME && !(cfg!(windows) && prefix.eq_ignore_ascii_case(FILE_NAME)) {
        return false;
    }
    let suffix = &name[FILE_NAME.len()..];
    suffix.is_empty()
        || suffix
            .strip_prefix('.')
            .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn has_marker(folder: &Path) -> bool {
    !matches!(fs::symlink_metadata(folder.join(FILE_NAME)), Err(e) if e.kind() == io::ErrorKind::NotFound)
}

fn read_verifier(folder: &Path) -> io::Result<Option<Verifier>> {
    let path = folder.join(FILE_NAME);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || metadata.len() > 1024 => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid folder lock file",
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(1024).read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn own_lock(folder: &Path) -> io::Result<Option<FolderLock>> {
    let folder = fs::canonicalize(folder)?;
    Ok(read_verifier(&folder)?.map(|verifier| FolderLock { folder, verifier }))
}

pub fn required(path: &Path) -> io::Result<Vec<FolderLock>> {
    if crate::tab::is_virtual(path) {
        return Ok(Vec::new());
    }
    let path = crate::tab::normalize(path);
    let absolute = std::path::absolute(&path)?;
    let canonical = fs::canonicalize(&absolute)?;
    let directory = canonical.is_dir();
    let mut locks: Vec<FolderLock> = Vec::new();
    // Bağlantının bulunduğu klasör de gerçek hedefin üst klasörleri de kilitli olabilir.
    let paths = if absolute == canonical {
        vec![&canonical]
    } else {
        vec![&absolute, &canonical]
    };
    for path in paths {
        let ancestors: Vec<_> = path.ancestors().skip(usize::from(!directory)).collect();
        for folder in ancestors.into_iter().rev().filter(|p| p.has_root()) {
            if let Some(verifier) = read_verifier(folder)? {
                let folder = fs::canonicalize(folder)?;
                if !locks.iter().any(|lock| lock.folder == folder) {
                    locks.push(FolderLock { folder, verifier });
                }
            }
        }
    }
    Ok(locks)
}

pub fn create(folder: &Path, password: &str) -> io::Result<()> {
    let folder = fs::canonicalize(folder)?;
    let salt = random_salt()?;
    let verifier = Verifier {
        salt,
        hash: derive(password.as_bytes(), &salt, ITERATIONS)?,
    };
    let target = folder.join(FILE_NAME);
    let stage = folder.join(format!("{FILE_NAME}.{:032x}", u128::from_ne_bytes(salt)));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&stage)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(&verifier)?)?;
        file.sync_all()?;
        drop(file);
        install_marker(&stage, &target)
    })();
    let _ = fs::remove_file(&stage);
    result
}

#[cfg(not(windows))]
fn install_marker(stage: &Path, target: &Path) -> io::Result<()> {
    fs::hard_link(stage, target)
}

#[cfg(windows)]
fn install_marker(stage: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN,
    };

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let (stage, target) = (wide(stage), wide(target));
    if unsafe { SetFileAttributesW(stage.as_ptr(), FILE_ATTRIBUTE_HIDDEN) } == 0
        || unsafe { MoveFileW(stage.as_ptr(), target.as_ptr()) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn random_salt() -> io::Result<[u8; 16]> {
    let mut salt = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut salt)?;
    Ok(salt)
}

#[cfg(windows)]
fn random_salt() -> io::Result<[u8; 16]> {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };

    let mut salt = [0; 16];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            salt.as_mut_ptr(),
            salt.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    bcrypt_result(status)?;
    Ok(salt)
}

#[cfg(windows)]
fn derive(password: &[u8], salt: &[u8], iterations: u32) -> io::Result<[u8; 32]> {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDeriveKeyPBKDF2, BCryptOpenAlgorithmProvider,
        BCRYPT_ALG_HANDLE, BCRYPT_ALG_HANDLE_HMAC_FLAG, BCRYPT_SHA256_ALGORITHM,
    };

    let mut alg: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
    bcrypt_result(unsafe {
        BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_SHA256_ALGORITHM,
            std::ptr::null(),
            BCRYPT_ALG_HANDLE_HMAC_FLAG,
        )
    })?;
    let mut hash = [0; 32];
    let status = unsafe {
        BCryptDeriveKeyPBKDF2(
            alg,
            password.as_ptr(),
            password.len() as u32,
            salt.as_ptr(),
            salt.len() as u32,
            iterations.into(),
            hash.as_mut_ptr(),
            hash.len() as u32,
            0,
        )
    };
    unsafe { BCryptCloseAlgorithmProvider(alg, 0) };
    bcrypt_result(status)?;
    Ok(hash)
}

#[cfg(windows)]
fn bcrypt_result(status: i32) -> io::Result<()> {
    if status < 0 {
        return Err(io::Error::other(format!("BCrypt NTSTATUS {status:#010x}")));
    }
    Ok(())
}

#[cfg(not(windows))]
fn derive(password: &[u8], salt: &[u8], iterations: u32) -> io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};

    let long_key;
    let key = if password.len() > 64 {
        long_key = Sha256::digest(password);
        long_key.as_slice()
    } else {
        password
    };
    let mut inner_pad = [0x36; 64];
    let mut outer_pad = [0x5c; 64];
    for (i, byte) in key.iter().enumerate() {
        inner_pad[i] ^= byte;
        outer_pad[i] ^= byte;
    }
    let inner = Sha256::new_with_prefix(inner_pad);
    let outer = Sha256::new_with_prefix(outer_pad);
    let hmac = |data: &[u8]| {
        outer
            .clone()
            .chain_update(inner.clone().chain_update(data).finalize())
            .finalize()
    };
    // PBKDF2'nin ilk bloğu, SHA-256 uzunluğundaki doğrulayıcı için yeterlidir.
    let mut first = salt.to_vec();
    first.extend(1u32.to_be_bytes());
    let mut previous = hmac(&first);
    let mut hash = previous;
    for _ in 1..iterations {
        previous = hmac(&previous);
        for (sum, byte) in hash.iter_mut().zip(previous) {
            *sum ^= byte;
        }
    }
    Ok(hash.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_sha256_matches_known_vector() {
        assert_eq!(
            derive(b"password", b"salt", 2).unwrap(),
            [
                0xae, 0x4d, 0x0c, 0x95, 0xaf, 0x6b, 0x46, 0xd3, 0x2d, 0x0a, 0xdf, 0xf9, 0x28, 0xf0,
                0x6d, 0xd0, 0x2a, 0x30, 0x3f, 0x8e, 0xf3, 0xc2, 0x51, 0xdf, 0xd6, 0xe2, 0xd8, 0x5a,
                0x95, 0x47, 0x4c, 0x43,
            ]
        );
    }

    #[test]
    fn lock_protects_descendants_and_survives_renaming() {
        let root = std::env::temp_dir().join(format!(
            "swift-explorer-folder-password-{}",
            std::process::id()
        ));
        let locked = root.join("şifreli");
        let child = locked.join("alt klasör");
        fs::create_dir_all(&child).unwrap();
        let secret = child.join("secret.txt");
        fs::write(&secret, "example").unwrap();

        assert!(required(&secret).unwrap().is_empty());
        create(&locked, "şifre denemesi").unwrap();
        assert!(required(&root).unwrap().is_empty());
        let locks = required(&secret).unwrap();
        assert_eq!(locks.len(), 1);
        assert!(!locks[0].verify("wrong password").unwrap());
        assert!(locks[0].verify("şifre denemesi").unwrap());

        let renamed = root.join("renamed");
        fs::rename(&locked, &renamed).unwrap();
        let renamed_child = renamed.join("alt klasör");
        assert_eq!(required(&renamed_child).unwrap().len(), 1);
        own_lock(&renamed).unwrap().unwrap().remove().unwrap();
        assert!(required(&renamed_child).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(renamed_child.join("secret.txt")).unwrap(),
            "example"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
