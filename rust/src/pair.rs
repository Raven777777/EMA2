//! 配对：token 生成与持久化、欢迎消息的 HMAC 挑战应答。
//!
//! 设计要点：
//! - token 只在首次授权时生成，写入用户级状态文件（`%LOCALAPPDATA%\EMA2\state.json`）。
//! - 扩展每次连接都带随机 challenge，中继用 token 做 HMAC-SHA256 证明自己是
//!   “持有 token 的那个中继”，扩展验签通过前不执行任何工具调用。
//! - 日志里永远不打印 token。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

/// 欢迎消息证明的域前缀，避免同一 token 在其它协议里被复用。
pub const WELCOME_PREFIX: &str = "ema2-welcome|";
/// challenge 上限，防止对端用超长字符串制造无意义的 HMAC 计算。
pub const MAX_CHALLENGE_CHARS: usize = 128;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PersistedState {
    /// 配对后生成的 MCP / 扩展共享 token。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// 授权时间（Unix 秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired_at: Option<u64>,
    /// 授权时的扩展版本，仅用于诊断。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_version: Option<String>,
    /// 授权页勾选要写入的 AI 终端 id。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clients: Vec<String>,
    /// 是否明确提交过选择（区分“没选过”和“一个都不选”）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub clients_chosen: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// 32 字节随机 token 的十六进制表示。
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex(&bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 原始 HMAC-SHA256（测试用固定向量验证）。
pub fn hmac_hex(key: &str, message: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(message.as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// `welcome` 里的 proof 值。
pub fn welcome_proof(token: &str, challenge: &str) -> String {
    hmac_hex(token, &format!("{WELCOME_PREFIX}{challenge}"))
}

/// challenge 必须是长度受控的 ASCII（扩展发送的是 16 字节随机数的 hex）。
pub fn is_valid_challenge(challenge: &str) -> bool {
    !challenge.is_empty()
        && challenge.len() <= MAX_CHALLENGE_CHARS
        && challenge
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// token 的短指纹，用于界面辨认“是不是同一把 token”，不泄露 token 本身。
pub fn fingerprint(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token.as_bytes());
    hex(&digest)[..8].to_owned()
}

pub fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join("state.json")
}

pub fn load(path: &Path) -> PersistedState {
    let Ok(raw) = fs::read_to_string(path) else {
        return PersistedState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// 原子写：先写临时文件再替换，避免进程中途退出留下半个 JSON。
pub fn save(path: &Path, state: &PersistedState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    // Windows 上 rename 不能覆盖已存在的目标。
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&tmp, path)
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_32_random_bytes_of_hex() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(token, generate_token());
    }

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        // RFC 4231 §4.3：key="Jefe", data="what do ya want for nothing?"
        assert_eq!(
            hmac_hex("Jefe", "what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn welcome_proof_is_domain_separated() {
        let token = "s3cret";
        let challenge = "0123456789abcdef";
        assert_eq!(
            welcome_proof(token, challenge),
            hmac_hex(token, "ema2-welcome|0123456789abcdef")
        );
        assert_ne!(welcome_proof(token, challenge), hmac_hex(token, challenge));
        assert_ne!(
            welcome_proof("other", challenge),
            welcome_proof(token, challenge)
        );
    }

    #[test]
    fn challenge_validation_rejects_noise() {
        assert!(is_valid_challenge("0123456789abcdef"));
        assert!(is_valid_challenge("abc-DEF_123"));
        assert!(!is_valid_challenge(""));
        assert!(!is_valid_challenge("has space"));
        assert!(!is_valid_challenge(&"a".repeat(MAX_CHALLENGE_CHARS + 1)));
    }

    #[test]
    fn state_round_trips_and_overwrites() {
        let dir = std::env::temp_dir().join(format!(
            "ema2-pair-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = state_path(&dir);
        assert!(load(&path).token.is_none(), "missing state must be empty");

        let first = PersistedState {
            token: Some("abc".into()),
            paired_at: Some(1),
            extension_version: Some("1.8.1".into()),
            clients: vec!["opencode".into()],
            clients_chosen: true,
        };
        save(&path, &first).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.token.as_deref(), Some("abc"));
        assert_eq!(loaded.paired_at, Some(1));

        let second = PersistedState {
            token: Some("def".into()),
            paired_at: Some(2),
            extension_version: None,
            clients: Vec::new(),
            clients_chosen: false,
        };
        save(&path, &second).unwrap();
        assert_eq!(load(&path).token.as_deref(), Some("def"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
