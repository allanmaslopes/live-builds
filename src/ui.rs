use crate::live::LiveClient;
use crate::poller::{PanelState, PollConfig, Poller};
use crate::static_data::StaticSource;
use crate::view::{BuildView, LaneView, MeView, PanelView, SkillView, TeamView};
use egui::{Color32, RichText};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const MUTED: Color32 = Color32::from_gray(150);
const GOOD: Color32 = Color32::from_rgb(110, 200, 120);
const WARN: Color32 = Color32::from_rgb(240, 190, 80);

pub struct App {
    state: Arc<Mutex<PanelState>>,
    poller: Poller,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cache_dir: PathBuf, cfg: PollConfig) -> App {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        cc.egui_ctx.style_mut(|style| {
            for (text_style, size) in [
                (egui::TextStyle::Body, 16.0),
                (egui::TextStyle::Button, 16.0),
                (egui::TextStyle::Heading, 22.0),
            ] {
                if let Some(font) = style.text_styles.get_mut(&text_style) {
                    font.size = size;
                }
            }
        });
        let state = Arc::new(Mutex::new(PanelState::Waiting));
        let shared = state.clone();
        let ctx = cc.egui_ctx.clone();
        // Repaint only when the poller reports a new state; egui is idle otherwise.
        let poller = Poller::spawn(
            Box::new(LiveClient::new()),
            StaticSource::new(cache_dir),
            move |new| {
                if let Ok(mut guard) = shared.lock() {
                    *guard = new;
                }
                ctx.request_repaint();
            },
            cfg,
        );
        App { state, poller }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let state = self
            .state
            .lock()
            .map_or(PanelState::Waiting, |guard| guard.clone());
        match state {
            PanelState::Waiting => centered(ctx, RichText::new("Aguardando partida…").heading()),
            PanelState::Error(message) => {
                centered(ctx, RichText::new(message).color(Color32::LIGHT_RED));
            }
            PanelState::InGame(view) => {
                if let Some(pick) = self.panel(ctx, &view) {
                    self.poller.set_lane_pick(Some(pick));
                }
            }
        }
    }
}

fn centered(ctx: &egui::Context, text: RichText) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.centered_and_justified(|ui| ui.label(text));
    });
}

impl App {
    /// Draws layout B; returns the enemy champion the user clicked, if any.
    fn panel(&self, ctx: &egui::Context, view: &PanelView) -> Option<String> {
        egui::TopBottomPanel::bottom("lane").show(ctx, |ui| lane(ui, view.lane.as_ref()));
        egui::SidePanel::left("me")
            .exact_width(330.0)
            .resizable(false)
            .show(ctx, |ui| me(ui, &view.me, &view.skills));
        let picked = egui::SidePanel::right("teams")
            .exact_width(420.0)
            .resizable(false)
            .show(ctx, |ui| teams(ui, &view.teams, view.lane.as_ref()))
            .inner;
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(note) = &view.catalog_note {
                ui.label(RichText::new(note).color(WARN));
            }
            build(ui, &view.build, view.me.gold);
        });
        picked
    }
}

fn me(ui: &mut egui::Ui, me: &MeView, skills: &SkillView) {
    ui.heading(format!("{} · nível {}", me.champion, me.level));
    if let Some(fraction) = me.xp_fraction {
        ui.add(egui::ProgressBar::new(fraction));
    }
    ui.label(format!("Ouro: {}   CS: {}", me.gold, me.cs));
    ui.label(format!("KDA: {}/{}/{}", me.kda.0, me.kda.1, me.kda.2));
    ui.separator();
    ui.label(RichText::new("Itens").color(MUTED));
    if me.items.is_empty() {
        ui.label("—");
    }
    for item in &me.items {
        ui.label(item);
    }
    ui.separator();
    ui.label(RichText::new("Skills").color(MUTED));
    ui.label(format!(
        "Q {}   W {}   E {}   R {}",
        skills.q, skills.w, skills.e, skills.r
    ));
    if skills.points_available > 0 {
        let next = skills
            .next_skill
            .map_or(String::new(), |c| format!(" → {c}"));
        ui.label(
            RichText::new(format!(
                "{} ponto(s) de skill{next}",
                skills.points_available
            ))
            .color(WARN),
        );
    }
    if skills.ult_hint {
        ui.label(RichText::new("Ultimate disponível para evoluir").color(WARN));
    }
    if let Some(label) = &skills.priority_label {
        ui.label(format!("Prioridade: {label}"));
    }
}

fn build(ui: &mut egui::Ui, build: &BuildView, gold: u32) {
    ui.heading("Build recomendada");
    ui.label(RichText::new(format!("Ouro agora: {gold}")).color(GOOD));
    if let Some(next) = &build.next_item {
        ui.label(format!(
            "Próximo item: {} (faltam {} de ouro)",
            next.name, next.missing_gold
        ));
        if let Some((name, cost)) = &next.next_component {
            ui.label(format!("Próximo componente: {name} ({cost})"));
        }
    }
    if let Some(line) = &build.free_boots_line {
        ui.label(line);
    }
    if !build.affordable_now.is_empty() {
        ui.label(format!(
            "Dá para comprar já: {}",
            build.affordable_now.join(", ")
        ));
    }
    ui.separator();
    if let Some(message) = &build.empty_message {
        ui.label(message);
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for block in &build.blocks {
            ui.label(RichText::new(&block.name).strong());
            for (name, owned) in &block.items {
                if *owned {
                    ui.label(RichText::new(format!("✓ {name}")).color(GOOD));
                } else {
                    ui.label(format!("• {name}"));
                }
            }
            ui.add_space(6.0);
        }
    });
}

/// Returns the champion of the enemy row clicked this frame.
fn teams(ui: &mut egui::Ui, teams: &[TeamView], lane: Option<&LaneView>) -> Option<String> {
    let mut picked = None;
    for (index, team) in teams.iter().enumerate() {
        ui.label(RichText::new(format!("{} · itens {}", team.label, team.items_value)).strong());
        for player in &team.players {
            let text = format!(
                "{}  nv {}  {}/{}/{}  {}",
                player.champion,
                player.level,
                player.kda.0,
                player.kda.1,
                player.kda.2,
                player.items_value
            );
            if index == 0 {
                let text = RichText::new(text);
                ui.label(if player.is_me { text.color(GOOD) } else { text });
            } else {
                let selected = lane.is_some_and(|l| l.opponent == player.champion);
                if ui.selectable_label(selected, text).clicked() {
                    picked = Some(player.champion.clone());
                }
            }
        }
        ui.add_space(8.0);
    }
    if let [first, second] = teams {
        let diff = i64::from(first.items_value) - i64::from(second.items_value);
        ui.label(RichText::new(format!("Diferença de itens: {diff:+}")).color(MUTED));
    }
    picked
}

fn lane(ui: &mut egui::Ui, lane: Option<&LaneView>) {
    match lane {
        Some(l) => ui.label(format!(
            "Rota vs {}: nível {:+}, itens {:+}",
            l.opponent, l.level_diff, l.items_value_diff
        )),
        None => ui.label(RichText::new("Rota: clique num inimigo para comparar").color(MUTED)),
    };
}
