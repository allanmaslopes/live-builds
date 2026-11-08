use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

#[derive(Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub port: u16,
    pub password: String,
}

pub fn parse_lockfile(text: &str) -> Result<Lockfile, String> {
    let fields: Vec<_> = text.trim().split(':').collect();
    if fields.len() != 5 {
        return Err("lockfile da LCU deve ter cinco campos".to_owned());
    }
    let port = fields[2]
        .parse()
        .map_err(|_| "porta inválida no lockfile da LCU".to_owned())?;
    if fields[3].is_empty() {
        return Err("senha ausente no lockfile da LCU".to_owned());
    }

    Ok(Lockfile {
        port,
        password: fields[3].to_owned(),
    })
}

pub fn parses_install_location_from_reg_output(output: &str) -> Option<PathBuf> {
    output.lines().find_map(|line| {
        let (_, value) = line.split_once("REG_SZ")?;
        line.trim_start()
            .starts_with("InstallLocation")
            .then(|| PathBuf::from(value.trim()))
    })
}

pub fn find_lockfile(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return path.is_file().then(|| path.to_path_buf());
    }

    for key in [
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ] {
        let Ok(output) = Command::new("reg")
            .args(["query", key, "/s", "/f", "League of Legends"])
            .output()
        else {
            continue;
        };
        if let Some(path) =
            parses_install_location_from_reg_output(&String::from_utf8_lossy(&output.stdout))
                .map(|directory| directory.join("lockfile"))
                .filter(|path| path.is_file())
        {
            return Some(path);
        }
    }

    let fallback = PathBuf::from(r"C:\Riot Games\League of Legends\lockfile");
    fallback.is_file().then_some(fallback)
}

#[derive(Debug)]
pub enum LcuError {
    ClientClosed,
    Http(u16, String),
    Other(String),
}

impl fmt::Display for LcuError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientClosed => formatter.write_str("Cliente do LoL fechado"),
            Self::Http(status, message) => {
                write!(formatter, "LCU respondeu HTTP {status}: {message}")
            }
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for LcuError {}

pub trait LcuHttp {
    fn get(&self, path: &str) -> Result<Value, LcuError>;
    fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, LcuError>;
}

pub struct LcuClient {
    agent: ureq::Agent,
    endpoint: String,
    authorization: String,
}

impl LcuClient {
    pub fn connect(lockfile_path: &Path) -> Result<Self, LcuError> {
        let text = fs::read_to_string(lockfile_path).map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                LcuError::ClientClosed
            } else {
                LcuError::Other(format!("falha ao ler o lockfile da LCU: {error}"))
            }
        })?;
        let lockfile = parse_lockfile(&text).map_err(LcuError::Other)?;
        let agent = crate::live::tls::agent().map_err(LcuError::Other)?;

        Ok(Self {
            agent,
            endpoint: format!("https://127.0.0.1:{}", lockfile.port),
            authorization: format!(
                "Basic {}",
                base64_encode(format!("riot:{}", lockfile.password).as_bytes())
            ),
        })
    }

    fn url(&self, path: &str) -> Result<String, LcuError> {
        if !path.starts_with('/') {
            return Err(LcuError::Other(
                "caminho da LCU deve começar com /".to_owned(),
            ));
        }
        Ok(format!("{}{path}", self.endpoint))
    }

    fn parse_response(
        &self,
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Value, LcuError> {
        let mut response = response
            .map_err(|error| LcuError::Other(format!("falha ao acessar a LCU: {error}")))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|error| LcuError::Other(format!("falha ao ler resposta da LCU: {error}")))?;
        if let Some(value) = validate_response(status, &text)? {
            return Ok(value);
        }
        parse_json(&text)
            .map_err(|error| LcuError::Other(format!("JSON inválido recebido da LCU: {error}")))
    }
}

fn validate_response(status: u16, text: &str) -> Result<Option<Value>, LcuError> {
    if !(200..300).contains(&status) {
        return Err(LcuError::Http(status, text.to_owned()));
    }
    Ok(text.trim().is_empty().then_some(Value::Null))
}

fn parse_json(text: &str) -> Result<Value, LcuError> {
    serde_json::from_str(text)
        .map_err(|error| LcuError::Other(format!("JSON inv\u{e1}lido recebido da LCU: {error}")))
}

impl LcuHttp for LcuClient {
    fn get(&self, path: &str) -> Result<Value, LcuError> {
        let response = self
            .agent
            .get(self.url(path)?)
            .header("Authorization", &self.authorization)
            .config()
            .http_status_as_error(false)
            .build()
            .call();
        self.parse_response(response)
    }

    fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, LcuError> {
        let url = self.url(path)?;
        let response = match method {
            "POST" => self
                .agent
                .post(url)
                .header("Authorization", &self.authorization)
                .config()
                .http_status_as_error(false)
                .build()
                .send_json(body),
            "PUT" => self
                .agent
                .put(url)
                .header("Authorization", &self.authorization)
                .config()
                .http_status_as_error(false)
                .build()
                .send_json(body),
            "PATCH" => self
                .agent
                .patch(url)
                .header("Authorization", &self.authorization)
                .config()
                .http_status_as_error(false)
                .build()
                .send_json(body),
            "DELETE" => self
                .agent
                .delete(url)
                .header("Authorization", &self.authorization)
                .config()
                .http_status_as_error(false)
                .build()
                .call(),
            _ => {
                return Err(LcuError::Other(format!(
                    "método HTTP da LCU não suportado: {method}"
                )));
            }
        };
        self.parse_response(response)
    }
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(ALPHABET[((value >> 18) & 0x3f) as usize] as char);
        encoded.push(ALPHABET[((value >> 12) & 0x3f) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(value & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parses_lockfile_fields() {
        let lockfile = parse_lockfile("LeagueClient:1234:54321:abc:https").unwrap();

        assert_eq!(lockfile.port, 54321);
        assert_eq!(lockfile.password, "abc");
    }

    #[test]
    fn parses_install_location_from_registry_output() {
        let output = "InstallLocation    REG_SZ    D:/Riot Games/League of Legends";

        assert_eq!(
            super::parses_install_location_from_reg_output(output),
            Some(PathBuf::from("D:/Riot Games/League of Legends"))
        );
    }

    #[test]
    fn basic_auth_matches_known_vector() {
        assert_eq!(base64_encode(b"riot:abc"), "cmlvdDphYmM=");
    }

    #[test]
    fn empty_2xx_is_null() {
        assert_eq!(validate_response(204, " ").unwrap(), Some(Value::Null));
    }

    #[test]
    fn non_2xx_is_http_error() {
        assert!(matches!(
            validate_response(404, "missing"),
            Err(LcuError::Http(404, message)) if message == "missing"
        ));
    }

    #[test]
    fn invalid_json_is_error() {
        assert!(matches!(parse_json("not json"), Err(LcuError::Other(_))));
    }

    #[test]
    fn missing_lockfile_is_client_closed() {
        let directory = tempfile::tempdir().unwrap();
        let result = LcuClient::connect(&directory.path().join("lockfile"));

        assert!(matches!(result, Err(LcuError::ClientClosed)));
    }

    #[test]
    fn reconnects_after_lockfile_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("lockfile");
        fs::write(&path, "LeagueClient:1234:54321:first:https").unwrap();
        let first = LcuClient::connect(&path).unwrap();

        fs::write(&path, "LeagueClient:1234:54322:second:https").unwrap();
        let second = LcuClient::connect(&path).unwrap();

        assert_eq!(first.endpoint, "https://127.0.0.1:54321");
        assert_eq!(second.endpoint, "https://127.0.0.1:54322");
        assert_ne!(first.authorization, second.authorization);
    }
}
