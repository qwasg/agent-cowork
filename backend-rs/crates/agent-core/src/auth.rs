//! Account auth: PBKDF2-HMAC-SHA256 password hashing + self-issued HS256 JWT
//! (no external JWT crate / no `ring` C dependency). Port of `auth_service.py`.

use std::sync::Arc;

use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::contracts::models::{new_id, now_ts};
use crate::contracts::{ApiError, ApiResult};
use crate::infra::store::T_USERS;
use crate::infra::Store;

type HmacSha256 = Hmac<Sha256>;

const PBKDF2_ROUNDS: u32 = 200_000;
const JWT_TTL_SECS: i64 = 7 * 24 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredUser {
    id: String,
    email: String,
    display_name: String,
    workspace: String,
    salt_b64: String,
    hash_b64: String,
    created_at: String,
}

pub struct AuthService {
    store: Arc<Store>,
    secret: Vec<u8>,
}

impl AuthService {
    pub fn new(store: Arc<Store>, secret_path: std::path::PathBuf) -> Arc<Self> {
        let secret = load_or_create_secret(&secret_path);
        Arc::new(AuthService { store, secret })
    }

    pub fn register(
        &self,
        email: &str,
        password: &str,
        display_name: &str,
        workspace: &str,
    ) -> ApiResult<Value> {
        let email = email.trim().to_lowercase();
        if email.is_empty() || password.len() < 6 {
            return Err(ApiError::new(
                "AUTH_INVALID_INPUT",
                "email required and password must be >= 6 chars",
            ));
        }
        if self.find_by_email(&email).is_some() {
            return Err(ApiError::new(
                "AUTH_EMAIL_TAKEN",
                "email already registered",
            ));
        }
        let mut salt = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut salt);
        let hash = pbkdf2_hash(password.as_bytes(), &salt);
        let user = StoredUser {
            id: new_id("user"),
            email: email.clone(),
            display_name: display_name.to_string(),
            workspace: workspace.to_string(),
            salt_b64: B64.encode(salt),
            hash_b64: B64.encode(hash),
            created_at: now_ts(),
        };
        let _ = self.store.put(T_USERS, &user.id, &user);
        let _ = self.store.kv_put(&email_key(&email), &user.id);
        Ok(self.auth_payload(&user))
    }

    pub fn login(&self, email: &str, password: &str) -> ApiResult<Value> {
        let email = email.trim().to_lowercase();
        let user = self
            .find_by_email(&email)
            .ok_or_else(|| ApiError::new("AUTH_BAD_CREDENTIALS", "invalid email or password"))?;
        let salt = B64
            .decode(&user.salt_b64)
            .map_err(|_| ApiError::new("AUTH_BAD_CREDENTIALS", "corrupt credential"))?;
        let expected = B64
            .decode(&user.hash_b64)
            .map_err(|_| ApiError::new("AUTH_BAD_CREDENTIALS", "corrupt credential"))?;
        let actual = pbkdf2_hash(password.as_bytes(), &salt);
        if !constant_time_eq(&actual, &expected) {
            return Err(ApiError::new(
                "AUTH_BAD_CREDENTIALS",
                "invalid email or password",
            ));
        }
        Ok(self.auth_payload(&user))
    }

    pub fn user_from_token(&self, token: &str) -> Option<Value> {
        let claims = self.verify_jwt(token)?;
        let uid = claims.get("sub").and_then(|v| v.as_str())?;
        let user = self.store.get::<StoredUser>(T_USERS, uid).ok().flatten()?;
        Some(public_user(&user))
    }

    pub fn update_profile(&self, user_id: &str, patch: &Value) -> ApiResult<Value> {
        let mut user = self
            .store
            .get::<StoredUser>(T_USERS, user_id)
            .ok()
            .flatten()
            .ok_or_else(|| ApiError::new("AUTH_USER_NOT_FOUND", "user not found"))?;
        if let Some(name) = patch.get("displayName").and_then(|v| v.as_str()) {
            user.display_name = name.to_string();
        }
        if let Some(ws) = patch.get("workspace").and_then(|v| v.as_str()) {
            user.workspace = ws.to_string();
        }
        let _ = self.store.put(T_USERS, &user.id, &user);
        Ok(json!({ "user": public_user(&user) }))
    }

    fn find_by_email(&self, email: &str) -> Option<StoredUser> {
        let uid = self.store.kv_get(&email_key(email))?;
        self.store.get::<StoredUser>(T_USERS, &uid).ok().flatten()
    }

    fn auth_payload(&self, user: &StoredUser) -> Value {
        json!({
            "token": self.mint_jwt(&user.id),
            "user": public_user(user),
        })
    }

    fn mint_jwt(&self, sub: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        let header = json!({"alg": "HS256", "typ": "JWT"});
        let payload = json!({"sub": sub, "iat": now, "exp": now + JWT_TTL_SECS});
        let h = B64URL.encode(serde_json::to_vec(&header).unwrap());
        let p = B64URL.encode(serde_json::to_vec(&payload).unwrap());
        let signing_input = format!("{h}.{p}");
        let sig = self.sign(signing_input.as_bytes());
        format!("{signing_input}.{}", B64URL.encode(sig))
    }

    fn verify_jwt(&self, token: &str) -> Option<Value> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let expected = self.sign(signing_input.as_bytes());
        let got = B64URL.decode(parts[2]).ok()?;
        if !constant_time_eq(&expected, &got) {
            return None;
        }
        let claims: Value = serde_json::from_slice(&B64URL.decode(parts[1]).ok()?).ok()?;
        let exp = claims.get("exp").and_then(|v| v.as_i64()).unwrap_or(0);
        if chrono::Utc::now().timestamp() > exp {
            return None;
        }
        Some(claims)
    }

    fn sign(&self, data: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("hmac key");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }
}

fn public_user(user: &StoredUser) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "displayName": user.display_name,
        "workspace": user.workspace,
        "createdAt": user.created_at,
    })
}

fn pbkdf2_hash(password: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, PBKDF2_ROUNDS, &mut out);
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn email_key(email: &str) -> String {
    format!("email:{email}")
}

fn load_or_create_secret(path: &std::path::PathBuf) -> Vec<u8> {
    if let Ok(raw) = std::fs::read(path) {
        if raw.len() >= 32 {
            return raw;
        }
    }
    let mut secret = vec![0u8; 48];
    rand::thread_rng().fill_bytes(&mut secret);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, &secret);
    secret
}
