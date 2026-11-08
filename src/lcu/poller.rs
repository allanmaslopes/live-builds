use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::lcu::champ_select::{Phase, SelectState, parse_session, phase_from_json};
use crate::lcu::{LcuClient, LcuError, LcuHttp, find_lockfile};

const SELECT_INTERVAL: Duration = Duration::from_secs(1);
const OPEN_INTERVAL: Duration = Duration::from_secs(5);
const CLOSED_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LcuPollState {
    ClientClosed,
    Error(String),
    Open(Phase),
    Selecting(SelectState),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollResult {
    pub state: LcuPollState,
    pub next: Duration,
}

pub fn poll_client(http: &dyn LcuHttp) -> Result<PollResult, LcuError> {
    let phase = phase_from_json(&http.get("/lol-gameflow/v1/gameflow-phase")?)?;
    if phase == Phase::ChampSelect {
        let session = http.get("/lol-champ-select/v1/session")?;
        let my_cell = session["localPlayerCellId"]
            .as_i64()
            .ok_or_else(|| LcuError::Other("sessão sem localPlayerCellId".to_owned()))?;
        return Ok(PollResult {
            state: LcuPollState::Selecting(parse_session(&session, my_cell)),
            next: SELECT_INTERVAL,
        });
    }
    Ok(PollResult {
        state: LcuPollState::Open(phase),
        next: OPEN_INTERVAL,
    })
}

pub struct LcuPoller {
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl LcuPoller {
    pub fn spawn(
        explicit_lockfile: Option<PathBuf>,
        on_change: impl Fn(LcuPollState) + Send + 'static,
    ) -> Self {
        let (stop, receiver) = mpsc::channel();
        let thread = thread::spawn(move || run(explicit_lockfile, receiver, on_change));
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for LcuPoller {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct PollWorker<C> {
    path: Option<PathBuf>,
    lockfile: Option<String>,
    client: Option<Box<dyn LcuHttp>>,
    connect: C,
}

impl<C> PollWorker<C>
where
    C: FnMut(&Path) -> Result<Box<dyn LcuHttp>, LcuError>,
{
    fn new(explicit_lockfile: Option<PathBuf>, connect: C) -> Self {
        Self {
            path: explicit_lockfile,
            lockfile: None,
            client: None,
            connect,
        }
    }

    fn tick(&mut self) -> PollResult {
        let result = self.try_tick();
        if result.is_err() {
            self.client = None;
        }
        match result {
            Ok(result) => result,
            Err(LcuError::ClientClosed) => PollResult {
                state: LcuPollState::ClientClosed,
                next: CLOSED_INTERVAL,
            },
            Err(error) => PollResult {
                state: LcuPollState::Error(error.to_string()),
                next: OPEN_INTERVAL,
            },
        }
    }

    fn try_tick(&mut self) -> Result<PollResult, LcuError> {
        if self.path.is_none() {
            self.path = find_lockfile(None);
        }
        let path = self.path.as_deref().ok_or(LcuError::ClientClosed)?;
        let contents = fs::read_to_string(path).map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                LcuError::ClientClosed
            } else {
                LcuError::Other(format!("falha ao ler o lockfile da LCU: {error}"))
            }
        })?;
        if self.lockfile.as_deref() != Some(contents.as_str()) {
            self.lockfile = Some(contents);
            self.client = None;
        }
        if self.client.is_none() {
            self.client = Some((self.connect)(path)?);
        }
        let result = poll_client(self.client.as_deref().expect("cliente conectado"));
        if result.is_err() {
            self.client = None;
        }
        result
    }
}

fn run(explicit_lockfile: Option<PathBuf>, stop: Receiver<()>, on_change: impl Fn(LcuPollState)) {
    let mut previous = None;
    let mut worker = PollWorker::new(explicit_lockfile, |path| {
        LcuClient::connect(path).map(|client| Box::new(client) as Box<dyn LcuHttp>)
    });
    loop {
        let poll = worker.tick();
        let state = poll.state;
        let interval = poll.next;
        if previous.as_ref() != Some(&state) {
            previous = Some(state.clone());
            on_change(state);
        }
        if stop.recv_timeout(interval).is_ok() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::rc::Rc;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::*;

    struct FakeClient(Result<Value, LcuError>);

    impl LcuHttp for FakeClient {
        fn get(&self, _path: &str) -> Result<Value, LcuError> {
            match &self.0 {
                Ok(value) => Ok(value.clone()),
                Err(LcuError::ClientClosed) => Err(LcuError::ClientClosed),
                Err(LcuError::Http(status, message)) => {
                    Err(LcuError::Http(*status, message.clone()))
                }
                Err(LcuError::Other(message)) => Err(LcuError::Other(message.clone())),
            }
        }

        fn send(&self, _method: &str, _path: &str, _body: &Value) -> Result<Value, LcuError> {
            unreachable!()
        }
    }

    #[test]
    fn keeps_connection_until_lockfile_changes() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("lockfile");
        fs::write(&path, "LeagueClient:1:1111:first:https").unwrap();
        let connections = Rc::new(Cell::new(0));
        let count = Rc::clone(&connections);
        let mut worker = PollWorker::new(Some(path.clone()), move |_| {
            count.set(count.get() + 1);
            Ok(Box::new(FakeClient(Ok(json!("Lobby")))) as Box<dyn LcuHttp>)
        });

        assert!(matches!(worker.tick().state, LcuPollState::Open(_)));
        assert!(matches!(worker.tick().state, LcuPollState::Open(_)));
        assert_eq!(connections.get(), 1);

        fs::write(&path, "LeagueClient:1:2222:second:https").unwrap();
        assert!(matches!(worker.tick().state, LcuPollState::Open(_)));
        assert_eq!(connections.get(), 2);
    }

    #[test]
    fn operational_error_is_reported_and_retries_after_five_seconds() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("lockfile");
        fs::write(&path, "LeagueClient:1:1111:first:https").unwrap();
        let mut worker = PollWorker::new(Some(path), |_| {
            Ok(Box::new(FakeClient(Err(LcuError::Http(401, "auth".into())))) as Box<dyn LcuHttp>)
        });

        let result = worker.tick();

        assert_eq!(result.next, Duration::from_secs(5));
        assert!(matches!(result.state, LcuPollState::Error(message) if message.contains("401")));
    }
}
