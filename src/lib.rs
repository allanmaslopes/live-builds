pub mod catalog;
pub mod knowledge;
pub mod lcu;
pub mod live;
pub mod poller;
pub mod riot_build;
pub mod static_data;
mod ui;
pub mod view;

use poller::PollConfig;
use std::path::PathBuf;
use std::time::Duration;

fn cache_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("live-builds")
        .join("cache")
}

/// Flags: `--poll-ms <n>`, `--record <dir>`, `--knowledge-url <url>` and
/// `--lcu-lockfile <path>`.
pub struct AppConfig {
    pub poll: Duration,
    pub record: Option<PathBuf>,
    pub knowledge_url: String,
    pub lcu_lockfile: Option<PathBuf>,
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> AppConfig {
    let mut poll = Duration::from_millis(1000);
    let mut record = None;
    let mut knowledge_url = "http://localhost:8082".to_owned();
    let mut lcu_lockfile = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--poll-ms" => {
                if let Some(ms) = args.next().and_then(|v| v.parse::<u64>().ok()) {
                    poll = Duration::from_millis(ms.max(100));
                }
            }
            "--record" => record = args.next().map(PathBuf::from),
            "--knowledge-url" => {
                if let Some(url) = args.next() {
                    knowledge_url = url;
                }
            }
            "--lcu-lockfile" => {
                if let Some(path) = args.next() {
                    lcu_lockfile = Some(PathBuf::from(path));
                }
            }
            _ => {}
        }
    }
    AppConfig {
        poll,
        record,
        knowledge_url,
        lcu_lockfile,
    }
}

fn parse_args() -> AppConfig {
    parse_args_from(std::env::args().skip(1))
}

pub fn run() -> eframe::Result {
    let config = parse_args();
    let options = eframe::NativeOptions {
        // Not focused on start: the window must never steal focus from the game.
        viewport: egui::ViewportBuilder::default()
            .with_title("live-builds")
            .with_inner_size([1280.0, 720.0])
            .with_active(false),
        ..Default::default()
    };
    eframe::run_native(
        "live-builds",
        options,
        Box::new(move |cc| {
            let mut cfg = PollConfig::new(config.poll, config.record.clone());
            cfg.knowledge_url = config.knowledge_url.clone();
            Ok(Box::new(ui::App::new(cc, cache_dir(), cfg)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_knowledge_url_flag() {
        let config = parse_args_from([
            "--poll-ms".to_owned(),
            "250".to_owned(),
            "--knowledge-url".to_owned(),
            "http://processor:9000/".to_owned(),
        ]);

        assert_eq!(config.poll, Duration::from_millis(250));
        assert!(config.record.is_none());
        assert_eq!(config.knowledge_url, "http://processor:9000/");
    }

    #[test]
    fn parses_lcu_lockfile_flag() {
        let config = parse_args_from([
            "--lcu-lockfile".to_owned(),
            r"D:\League\lockfile".to_owned(),
        ]);

        assert_eq!(
            config.lcu_lockfile,
            Some(PathBuf::from(r"D:\League\lockfile"))
        );
    }
}
