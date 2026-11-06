pub mod catalog;
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

/// Flags: `--poll-ms <n>` (default 1000) and `--record <dir>`.
fn parse_args() -> (Duration, Option<PathBuf>) {
    let mut poll = Duration::from_millis(1000);
    let mut record = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--poll-ms" => {
                if let Some(ms) = args.next().and_then(|v| v.parse::<u64>().ok()) {
                    poll = Duration::from_millis(ms.max(100));
                }
            }
            "--record" => record = args.next().map(PathBuf::from),
            _ => {}
        }
    }
    (poll, record)
}

pub fn run() -> eframe::Result {
    let (poll, record) = parse_args();
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
            Ok(Box::new(ui::App::new(
                cc,
                cache_dir(),
                PollConfig::new(poll, record),
            )))
        }),
    )
}
