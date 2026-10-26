use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::HashMap,
    fs, io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

type HmacSha256 = Hmac<Sha256>;
const PAIRING_LIFETIME: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub enum AuthError {
    Invalid,
    Expired,
    Revoked,
    Io(io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => write!(f, "invalid authentication"),
            Self::Expired => write!(f, "pairing secret expired"),
            Self::Revoked => write!(f, "device revoked"),
            Self::Io(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for AuthError {}
impl From<io::Error> for AuthError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for AuthError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Debug, Clone)]
pub struct PairingState {
    secret: Vec<u8>,
    expires_at: u64,
    consumed: Arc<AtomicBool>,
}

impl PairingState {
    pub fn new() -> Self {
        let mut secret = vec![0; 32];
        OsRng.fill_bytes(&mut secret);
        Self {
            secret,
            expires_at: unix_now() + PAIRING_LIFETIME.as_secs(),
            consumed: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn secret(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.secret)
    }
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
    #[cfg(test)]
    pub fn is_valid(&self, presented: &str) -> bool {
        self.expires_at > unix_now()
            && !self.consumed.load(Ordering::Acquire)
            && decode_secret(presented)
                .map(|v| constant_eq(&v, &self.secret))
                .unwrap_or(false)
    }
    #[cfg(test)]
    pub fn consume(&self, presented: &str) -> bool {
        let Ok(reservation) = self.reserve(presented) else {
            return false;
        };
        reservation.commit();
        true
    }

    fn reserve(&self, presented: &str) -> Result<PairingReservation, AuthError> {
        if self.expires_at <= unix_now() {
            return Err(AuthError::Expired);
        }
        let valid = decode_secret(presented)
            .map(|value| constant_eq(&value, &self.secret))
            .unwrap_or(false);
        if !valid
            || self
                .consumed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(AuthError::Invalid);
        }
        Ok(PairingReservation {
            consumed: Arc::clone(&self.consumed),
            committed: false,
        })
    }
}

struct PairingReservation {
    consumed: Arc<AtomicBool>,
    committed: bool,
}

impl PairingReservation {
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for PairingReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.consumed.store(false, Ordering::Release);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredDevice {
    secret_hash: String,
    revoked: bool,
    created_at: u64,
    last_seen: u64,
}

#[derive(Debug)]
pub struct DeviceStore {
    path: PathBuf,
    devices: Mutex<HashMap<String, StoredDevice>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    pub device_id: String,
    pub revoked: bool,
    pub created_at: u64,
    pub last_seen: u64,
}

impl DeviceStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, AuthError> {
        let path = path.into();
        let devices = match fs::read_to_string(&path) {
            Ok(value) => serde_json::from_str(&value)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            devices: Mutex::new(devices),
        })
    }

    pub fn default_path() -> PathBuf {
        std::env::var_os("HYUSK_LINK_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .map(|home| PathBuf::from(home).join(".local").join("state"))
                    })
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("hyusk")
                    .join("link-devices.json")
            })
    }

    pub async fn authenticate(
        &self,
        device_id: &str,
        device_secret: &[u8],
        pairing_secret: Option<&str>,
        pairing: &PairingState,
    ) -> Result<(), AuthError> {
        let mut devices = self.devices.lock().await;
        let original = devices.clone();
        let mut pairing_reservation = None;
        if let Some(device) = devices.get_mut(device_id) {
            if device.revoked {
                return Err(AuthError::Revoked);
            }
            if !constant_time_str_eq(&hash_secret(device_secret), &device.secret_hash) {
                return Err(AuthError::Invalid);
            }
            device.last_seen = unix_now();
        } else {
            let provided = pairing_secret.ok_or(AuthError::Invalid)?;
            pairing_reservation = Some(pairing.reserve(provided)?);
            devices.insert(
                device_id.to_string(),
                StoredDevice {
                    secret_hash: hash_secret(device_secret),
                    revoked: false,
                    created_at: unix_now(),
                    last_seen: unix_now(),
                },
            );
        }
        if let Err(error) = self.persist_locked(&devices) {
            *devices = original;
            return Err(error);
        }
        if let Some(reservation) = pairing_reservation {
            reservation.commit();
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn revoke(&self, device_id: &str) -> Result<bool, AuthError> {
        let mut devices = self.devices.lock().await;
        let changed = devices
            .get_mut(device_id)
            .map(|device| {
                let changed = !device.revoked;
                device.revoked = true;
                changed
            })
            .unwrap_or(false);
        if changed {
            self.persist_locked(&devices)?;
        }
        Ok(changed)
    }

    #[cfg(test)]
    pub async fn is_revoked(&self, device_id: &str) -> bool {
        self.devices
            .lock()
            .await
            .get(device_id)
            .map(|device| device.revoked)
            .unwrap_or(false)
    }

    pub async fn list(&self) -> Vec<DeviceInfo> {
        self.devices
            .lock()
            .await
            .iter()
            .map(|(device_id, device)| DeviceInfo {
                device_id: device_id.clone(),
                revoked: device.revoked,
                created_at: device.created_at,
                last_seen: device.last_seen,
            })
            .collect()
    }

    fn persist_locked(&self, devices: &HashMap<String, StoredDevice>) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("tmp");
        fs::write(&temp, serde_json::to_vec_pretty(devices)?)?;
        fs::rename(temp, &self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

pub fn random_secret() -> Vec<u8> {
    let mut value = vec![0; 32];
    OsRng.fill_bytes(&mut value);
    value
}
pub fn encode_secret(value: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(value)
}
pub fn decode_secret(value: &str) -> Result<Vec<u8>, AuthError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AuthError::Invalid)
}
pub fn proof(secret: &[u8], challenge: &[u8], device_id: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts every key length");
    mac.update(challenge);
    mac.update(device_id.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}
pub(crate) fn request_mac(secret: &[u8], request_bytes: &[u8]) -> String {
    proof(secret, request_bytes, "request")
}
pub fn verify_proof(secret: &[u8], challenge: &[u8], device_id: &str, presented: &str) -> bool {
    decode_secret(presented)
        .map(|expected| {
            constant_eq(
                &expected,
                &decode_secret(&proof(secret, challenge, device_id)).unwrap_or_default(),
            )
        })
        .unwrap_or(false)
}
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn hash_secret(secret: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(b"hyusk-link-device-v1").unwrap();
    mac.update(secret);
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}
fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
pub(crate) fn constant_time_str_eq(a: &str, b: &str) -> bool {
    constant_eq(a.as_bytes(), b.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_binds_challenge_and_device() {
        let secret = random_secret();
        let proof_value = proof(&secret, b"challenge", "phone");
        assert!(verify_proof(&secret, b"challenge", "phone", &proof_value));
        assert!(!verify_proof(&secret, b"other", "phone", &proof_value));
        assert!(!verify_proof(&secret, b"challenge", "other", &proof_value));
    }

    #[tokio::test]
    async fn revoked_devices_survive_reload() {
        let path = std::env::temp_dir().join(format!(
            "hyusk-link-test-{}.json",
            encode_secret(&random_secret())
        ));
        let pairing = PairingState::new();
        let store = DeviceStore::load(&path).unwrap();
        let device_secret = random_secret();
        store
            .authenticate("phone", &device_secret, Some(&pairing.secret()), &pairing)
            .await
            .unwrap();
        assert!(store.revoke("phone").await.unwrap());
        let reloaded = DeviceStore::load(&path).unwrap();
        assert!(reloaded.is_revoked("phone").await);
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn failed_auth_persistence_rolls_back_in_memory_update() {
        let directory = std::env::temp_dir().join(format!(
            "hyusk-link-auth-fail-{}.json",
            encode_secret(&random_secret())
        ));
        let store = DeviceStore::load(&directory).unwrap();
        let pairing = PairingState::new();
        let secret = random_secret();
        store
            .authenticate("phone", &secret, Some(&pairing.secret()), &pairing)
            .await
            .unwrap();
        let previous = store.list().await[0].last_seen;

        fs::remove_file(&directory).unwrap();
        fs::create_dir(&directory).unwrap();
        assert!(store
            .authenticate("phone", &secret, None, &pairing)
            .await
            .is_err());
        assert_eq!(store.list().await[0].last_seen, previous);
        let _ = fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn failed_first_pairing_write_does_not_consume_pairing_secret() {
        let path = std::env::temp_dir().join(format!(
            "hyusk-link-pair-fail-{}.json",
            encode_secret(&random_secret())
        ));
        let store = DeviceStore::load(&path).unwrap();
        let pairing = PairingState::new();
        let pairing_secret = pairing.secret();
        let device_secret = random_secret();
        fs::create_dir(&path).unwrap();

        assert!(store
            .authenticate("phone", &device_secret, Some(&pairing_secret), &pairing)
            .await
            .is_err());
        assert!(pairing.is_valid(&pairing_secret));
        assert!(store.list().await.is_empty());

        fs::remove_dir(&path).unwrap();
        store
            .authenticate("phone", &device_secret, Some(&pairing_secret), &pairing)
            .await
            .unwrap();
        assert!(!pairing.is_valid(&pairing_secret));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pairing_secret_is_single_use() {
        let pairing = PairingState::new();
        let secret = pairing.secret();
        assert!(pairing.consume(&secret));
        assert!(!pairing.consume(&secret));
    }
}
