use crate::knowledge::KnowledgeNames;
use crate::lcu::champ_select::{Recommendation, Source};
use egui::{Color32, RichText};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AppliedItems {
    pub runes: bool,
    pub spells: bool,
    pub item_set: bool,
}

pub struct SelectScreenInput<'a> {
    pub champion: &'a str,
    pub position: &'a str,
    pub recommendation: Option<&'a Recommendation>,
    pub source_patch: Option<&'a str>,
    pub source_games: Option<u32>,
    pub names: &'a KnowledgeNames,
    pub item_names: &'a dyn Fn(u32) -> String,
    pub differences: &'a [String],
    pub applied: AppliedItems,
    pub chosen: Option<usize>,
    pub limit_reached: bool,
    pub settings_corrupted: bool,
    pub loading: bool,
    pub error: Option<&'a str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuneOptionView {
    pub title: String,
    pub detail: String,
    pub low_sample: bool,
    pub chosen: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectScreenView {
    pub champion_position: String,
    pub source: String,
    pub runes: Vec<String>,
    pub spells: String,
    pub item_set: Vec<String>,
    pub options: Vec<RuneOptionView>,
    pub can_apply_runes: bool,
    pub can_apply_spells: bool,
    pub can_apply_item_set: bool,
    pub applied: Vec<String>,
    pub reminder_title: Option<String>,
    pub differences: Vec<String>,
    pub limit_message: Option<String>,
    pub settings_warning: Option<String>,
    pub error: Option<String>,
}

pub fn select_screen(input: SelectScreenInput<'_>) -> SelectScreenView {
    let mut view = SelectScreenView {
        champion_position: format!("{} · {}", input.champion, input.position),
        differences: input.differences.to_vec(),
        ..SelectScreenView::default()
    };
    let Some(recommendation) = input.recommendation else {
        if input.loading {
            view.source = "Carregando…".to_owned();
            view.runes.push("Carregando recomendação…".to_owned());
        } else {
            view.source = "Sem recomendação".to_owned();
            view.runes.push("sem recomendação de runas".to_owned());
        }
        return add_notices(view, &input);
    };
    view.source = match recommendation.source {
        Source::HighElo => match (input.source_patch, input.source_games) {
            (Some(patch), Some(games)) => format!("elo alto {patch} ({games} partidas)"),
            _ => "elo alto".to_owned(),
        },
        Source::Riot => "Riot".to_owned(),
    };
    view.options = recommendation
        .pages
        .iter()
        .enumerate()
        .map(|(index, option)| RuneOptionView {
            title: option.keystone_name.clone(),
            detail: if option.games == 0 {
                String::new()
            } else {
                format!(
                    "{} partidas · {:.1}% vitória",
                    option.games,
                    option.win_rate * 100.0
                )
            },
            low_sample: option.low_sample,
            chosen: input.chosen == Some(index),
        })
        .collect();
    if let Some(page) = &recommendation.page {
        view.runes.push(format!(
            "{} · {}",
            input.names.style(page.primary_style),
            input.names.style(page.sub_style)
        ));
        view.runes
            .extend(page.perks.map(|perk| input.names.rune(perk)));
        view.can_apply_runes = true;
    } else {
        view.runes.push("sem recomendação de runas".to_owned());
    }
    view.spells = format!(
        "{} · {}",
        input.names.summoner_spell(recommendation.spells.0),
        input.names.summoner_spell(recommendation.spells.1)
    );
    view.can_apply_spells = recommendation.spells != (0, 0);
    if let Some(item_set) = &recommendation.item_set {
        for block in &item_set.blocks {
            let items = block
                .items
                .iter()
                .map(|id| (input.item_names)(*id))
                .collect::<Vec<_>>()
                .join(" · ");
            view.item_set.push(format!("{}: {items}", block.name));
        }
        view.can_apply_item_set = true;
    }
    if input.applied.runes {
        view.applied.push("✓ Runas".to_owned());
    }
    if input.applied.spells {
        view.applied.push("✓ Feitiços".to_owned());
    }
    if input.applied.item_set {
        view.applied.push("✓ Arsenal".to_owned());
    }
    add_notices(view, &input)
}

fn add_notices(mut view: SelectScreenView, input: &SelectScreenInput<'_>) -> SelectScreenView {
    view.error = input.error.map(str::to_owned);
    if !view.differences.is_empty() {
        view.reminder_title = Some("Confira feitiços e runas".to_owned());
    }
    if input.limit_reached {
        view.limit_message =
            Some("Limite de páginas de runas atingido: libere uma para o live-builds".to_owned());
    }
    if input.settings_corrupted {
        view.settings_warning = Some(
            "As configurações estavam corrompidas; os valores padrão foram restaurados.".to_owned(),
        );
    }
    view
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectAction {
    ApplyRunes,
    ApplySpells,
    ApplyItemSet,
    ChooseOption(usize),
}

pub fn draw(ctx: &egui::Context, view: &SelectScreenView) -> Option<SelectAction> {
    let mut action = None;
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading(&view.champion_position);
        ui.label(RichText::new(&view.source).color(Color32::from_gray(170)));
        ui.separator();
        ui.columns(3, |columns| {
            columns[0].heading("Runas");
            for line in &view.runes {
                columns[0].label(line);
            }
            if columns[0]
                .add_enabled(view.can_apply_runes, egui::Button::new("Aplicar runas"))
                .clicked()
            {
                action = Some(SelectAction::ApplyRunes);
            }
            for (index, option) in view.options.iter().enumerate() {
                columns[0].separator();
                columns[0].label(RichText::new(&option.title).strong());
                if !option.detail.is_empty() {
                    columns[0].label(&option.detail);
                }
                if option.low_sample {
                    columns[0].label(
                        RichText::new("⚠ poucas partidas").color(Color32::from_rgb(240, 190, 80)),
                    );
                }
                if option.chosen {
                    columns[0].label(RichText::new("✓ escolhida").color(Color32::LIGHT_GREEN));
                } else if columns[0].button("Usar esta").clicked() {
                    action = Some(SelectAction::ChooseOption(index));
                }
            }
            columns[1].heading("Feitiços");
            columns[1].label(&view.spells);
            if columns[1]
                .add_enabled(view.can_apply_spells, egui::Button::new("Aplicar feitiços"))
                .clicked()
            {
                action = Some(SelectAction::ApplySpells);
            }
            columns[2].heading("Arsenal");
            for line in &view.item_set {
                columns[2].label(line);
            }
            if columns[2]
                .add_enabled(
                    view.can_apply_item_set,
                    egui::Button::new("Aplicar arsenal"),
                )
                .clicked()
            {
                action = Some(SelectAction::ApplyItemSet);
            }
        });
        if !view.applied.is_empty() {
            ui.separator();
            ui.label(view.applied.join("   "));
        }
        if let Some(title) = &view.reminder_title {
            ui.separator();
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(Color32::from_rgb(240, 190, 80)),
            );
            for difference in &view.differences {
                ui.label(difference);
            }
        }
        if let Some(message) = &view.limit_message {
            ui.label(RichText::new(message).color(Color32::LIGHT_RED));
        }
        if let Some(message) = &view.settings_warning {
            ui.label(RichText::new(message).color(Color32::LIGHT_RED));
        }
        if let Some(message) = &view.error {
            ui.label(RichText::new(message).color(Color32::LIGHT_RED));
        }
    });
    action
}
