mod model;
mod tls;

use std::fmt;
use std::io::ErrorKind;
use std::time::Duration;

pub use model::{AbilityLevels, ActivePlayer, GameSnapshot, PlayerInfo, parse_all_game_data};

#[derive(Debug)]
pub enum LiveError {
    NoActivePlayer,
    Json(serde_json::Error),
    Http(ureq::Error),
    Tls(String),
}

impl fmt::Display for LiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoActivePlayer => formatter.write_str("Partida sem jogador ativo"),
            Self::Json(error) => write!(formatter, "JSON da partida inválido: {error}"),
            Self::Http(error) => write!(formatter, "Falha na Live Client Data API: {error}"),
            Self::Tls(error) => write!(formatter, "Falha ao configurar TLS da Riot: {error}"),
        }
    }
}

impl std::error::Error for LiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Http(error) => Some(error),
            Self::NoActivePlayer | Self::Tls(_) => None,
        }
    }
}

pub struct LiveClient {
    endpoint: String,
    agent: Result<ureq::Agent, String>,
}

impl Default for LiveClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveClient {
    pub fn new() -> LiveClient {
        LiveClient {
            endpoint: "https://127.0.0.1:2999/liveclientdata/allgamedata".to_owned(),
            agent: tls::agent(),
        }
    }

    pub fn fetch(&self) -> Result<Option<GameSnapshot>, LiveError> {
        let agent = self
            .agent
            .as_ref()
            .map_err(|error| LiveError::Tls(error.clone()))?;
        let mut response = match agent
            .get(&self.endpoint)
            .config()
            .timeout_connect(Some(Duration::from_millis(400)))
            .build()
            .call()
        {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(404)) => return Ok(None),
            Err(ureq::Error::Timeout(ureq::Timeout::Connect)) => return Ok(None),
            Err(ureq::Error::Io(error)) if error.kind() == ErrorKind::ConnectionRefused => {
                return Ok(None);
            }
            Err(error) => return Err(LiveError::Http(error)),
        };
        if !response.status().is_success() {
            return Err(LiveError::Http(ureq::Error::StatusCode(
                response.status().as_u16(),
            )));
        }
        let json = response
            .body_mut()
            .read_to_string()
            .map_err(LiveError::Http)?;
        match parse_all_game_data(&json) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(LiveError::NoActivePlayer) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    fn fetch_http_response(status: &str, body: &str) -> Result<Option<GameSnapshot>, LiveError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!(
            "http://{}/liveclientdata/allgamedata",
            listener.local_addr().unwrap()
        );
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "client did not connect");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("test server failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut request = [0; 4096];
            let bytes_read = stream.read(&mut request).unwrap();
            assert!(bytes_read > 0, "client did not send an HTTP request");
            stream.write_all(response.as_bytes()).unwrap();
        });

        let result = LiveClient {
            endpoint,
            ..LiveClient::new()
        }
        .fetch();
        if result.is_ok() {
            server.join().unwrap();
        }
        result
    }

    #[test]
    fn fetch_parses_successful_snapshot() {
        let snapshot = fetch_http_response(
            "200 OK",
            include_str!("../../tests/fixtures/allgamedata_classic.json"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(snapshot.game_mode, "CLASSIC");
        assert_eq!(snapshot.active.summoner_name, "FixturePlayer");
        assert_eq!(snapshot.players.len(), 10);
    }

    #[test]
    fn loading_screen_is_none() {
        let result = fetch_http_response("200 OK", r#"{"allPlayers":[],"gameData":{}}"#);

        assert!(
            matches!(result, Ok(None)),
            "expected no game while loading: {result:?}"
        );
    }

    #[test]
    fn fetch_returns_none_when_connection_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/liveclientdata/allgamedata",
            listener.local_addr().unwrap()
        );
        drop(listener);
        let client = LiveClient {
            endpoint,
            ..LiveClient::new()
        };

        let started = Instant::now();
        let result = client.fetch();

        assert!(
            matches!(result, Ok(None)),
            "expected no game after connection refusal: {result:?}"
        );
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn fetch_returns_none_when_not_found() {
        let result = fetch_http_response("404 Not Found", r#"{"error":"No game"}"#);

        assert!(
            matches!(result, Ok(None)),
            "expected no game on HTTP 404: {result:?}"
        );
    }
}
