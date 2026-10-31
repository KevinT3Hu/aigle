use crate::{
    AppState,
    library::{now, secret},
};
use anyhow::Result;
use axum::http::HeaderMap;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

#[derive(Clone, Debug)]
pub struct Session {
    pub csrf: String,
    pub hash: String,
}
#[derive(Default)]
pub struct LoginLimiter {
    attempts: VecDeque<Instant>,
}
impl LoginLimiter {
    pub fn allow(&mut self) -> bool {
        let now = Instant::now();
        while self
            .attempts
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
        {
            self.attempts.pop_front();
        }
        if self.attempts.len() >= 20 {
            return false;
        }
        self.attempts.push_back(now);
        true
    }
}
#[derive(Deserialize)]
pub struct Login {
    pub token: String,
}
impl AppState {
    pub fn bearer(&self, headers: &HeaderMap) -> bool {
        let Some(value) = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        else {
            return false;
        };
        self.check_token(value)
    }
    pub fn check_token(&self, value: &str) -> bool {
        bool::from(
            blake3::hash(value.as_bytes())
                .as_bytes()
                .ct_eq(&self.credential_hash),
        )
    }
    pub fn cookie_name(&self) -> &'static str {
        if self.config.secure_cookies() {
            "__Host-thesauros"
        } else {
            "thesauros_session"
        }
    }
    fn cookie_value(&self, headers: &HeaderMap) -> Option<String> {
        headers
            .get("cookie")?
            .to_str()
            .ok()?
            .split(';')
            .filter_map(|p| p.trim().split_once('='))
            .find(|(key, _)| *key == self.cookie_name())
            .map(|(_, value)| value.to_owned())
            .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
    }
    pub async fn session(&self, headers: &HeaderMap) -> Result<Option<Session>> {
        let Some(value) = self.cookie_value(headers) else {
            return Ok(None);
        };
        let hash = blake3::hash(value.as_bytes()).to_hex().to_string();
        let credential = blake3::Hash::from(self.credential_hash)
            .to_hex()
            .to_string();
        self.call(move |lib| {
            Ok(lib
                .db()?
                .query_row(
                    "SELECT csrf FROM sessions WHERE hash=? AND credential_hash=? AND expires>?",
                    params![hash, credential, now()],
                    |r| {
                        Ok(Session {
                            csrf: r.get(0)?,
                            hash: hash.clone(),
                        })
                    },
                )
                .optional()?)
        })
        .await
    }
    pub async fn create_session(&self) -> Result<String> {
        let token = secret();
        let hash = blake3::hash(token.as_bytes()).to_hex().to_string();
        let csrf = secret();
        let credential = blake3::Hash::from(self.credential_hash)
            .to_hex()
            .to_string();
        let ttl = self.config.session_ttl_seconds;
        self.call(move |lib| {
            lib.db()?.execute(
                "INSERT INTO sessions VALUES (?,?,?,?)",
                params![hash, csrf, credential, now() + ttl as i64],
            )?;
            Ok(())
        })
        .await?;
        let secure = if self.config.secure_cookies() {
            "; Secure"
        } else {
            ""
        };
        Ok(format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ttl}{secure}",
            self.cookie_name()
        ))
    }
    pub fn clear_cookie(&self) -> String {
        format!(
            "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
            self.cookie_name(),
            if self.config.secure_cookies() {
                "; Secure"
            } else {
                ""
            }
        )
    }
    pub fn csrf(&self, session: &Session, value: &str) -> bool {
        bool::from(session.csrf.as_bytes().ct_eq(value.as_bytes()))
    }
    pub fn origin_allowed(&self, headers: &HeaderMap) -> bool {
        let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
            return false;
        };
        if let Some(expected) = &self.config.public_url {
            return origin == expected;
        }
        let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) else {
            return false;
        };
        let Ok(url) = url::Url::parse(&format!("http://{host}")) else {
            return false;
        };
        if !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")) {
            return false;
        }
        origin == format!("http://{host}")
    }
}
