use crate::catalog::ItemCatalog;
use crate::knowledge::KnowledgeNames;
use crate::lcu::item_sets::{ItemBlock, ItemSet};
use crate::lcu::perks::ClientPage;
use crate::lcu::runes::{RunePageDraft, RuneRules};
use crate::settings::Settings;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeRow {
    Primary(usize),
    Secondary(usize),
    Shard(usize),
}

pub fn icon_cache_path(cache_dir: &std::path::Path, icon: &str) -> std::path::PathBuf {
    let hash = icon
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    cache_dir.join(format!("{hash:016x}.png"))
}

pub fn icon_label(name: &str, _icon: Option<&egui::TextureHandle>) -> String {
    name.to_owned()
}

#[derive(Debug, Eq, PartialEq)]
pub struct DecodedIcon {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

pub fn decode_icon(png: &[u8]) -> Result<DecodedIcon, String> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?
        .to_rgba8();
    Ok(DecodedIcon {
        size: [image.width() as usize, image.height() as usize],
        rgba: image.into_raw(),
    })
}

pub fn tree_row_options(rules: &RuneRules, draft: &RunePageDraft, row: TreeRow) -> Vec<u32> {
    match row {
        TreeRow::Primary(slot) => rules.slot_perks(draft.primary_style, slot),
        TreeRow::Secondary(index) => rune_options(rules, draft, index + 4),
        TreeRow::Shard(index) => rules.slot_perks(draft.primary_style, index + 4),
    }
}

pub fn select_primary_style(rules: &RuneRules, draft: &mut RunePageDraft, style: u32) -> bool {
    if style == draft.primary_style || !rules.style_ids().contains(&style) {
        return false;
    }
    let previous_primary = draft.primary_style;
    let previous_sub = draft.sub_style;
    let allowed = rules.allowed_sub_styles(style);
    let Some(sub_style) = (if previous_sub != style && allowed.contains(&previous_sub) {
        Some(previous_sub)
    } else if allowed.contains(&previous_primary) {
        Some(previous_primary)
    } else {
        allowed.first().copied()
    }) else {
        return false;
    };
    let Some(perks) = draft_for_styles(rules, style, sub_style) else {
        return false;
    };

    draft.primary_style = style;
    draft.sub_style = sub_style;
    draft.perks = perks;
    true
}

pub fn select_secondary_style(rules: &RuneRules, draft: &mut RunePageDraft, style: u32) -> bool {
    if style == draft.sub_style
        || !rules
            .allowed_sub_styles(draft.primary_style)
            .contains(&style)
    {
        return false;
    }
    let Some(first) = rules.slot_perks(style, 1).first().copied() else {
        return false;
    };
    let Some(second) = rules.slot_perks(style, 2).first().copied() else {
        return false;
    };
    draft.sub_style = style;
    draft.perks[4] = first;
    draft.perks[5] = second;
    true
}

pub fn select_tree_perk(
    rules: &RuneRules,
    draft: &mut RunePageDraft,
    row: TreeRow,
    perk: u32,
) -> bool {
    if !tree_row_options(rules, draft, row).contains(&perk) {
        return false;
    }
    let index = match row {
        TreeRow::Primary(slot) => slot,
        TreeRow::Secondary(index) => index + 4,
        TreeRow::Shard(index) => index + 6,
    };
    let Some(selected) = draft.perks.get_mut(index) else {
        return false;
    };
    if *selected == perk {
        return false;
    }
    *selected = perk;
    true
}

fn draft_for_styles(rules: &RuneRules, primary: u32, secondary: u32) -> Option<[u32; 9]> {
    Some([
        *rules.slot_perks(primary, 0).first()?,
        *rules.slot_perks(primary, 1).first()?,
        *rules.slot_perks(primary, 2).first()?,
        *rules.slot_perks(primary, 3).first()?,
        *rules.slot_perks(secondary, 1).first()?,
        *rules.slot_perks(secondary, 2).first()?,
        *rules.slot_perks(primary, 4).first()?,
        *rules.slot_perks(primary, 5).first()?,
        *rules.slot_perks(primary, 6).first()?,
    ])
}

pub fn new_item_set(sets: &[ItemSet], timestamp_ms: u128) -> ItemSet {
    let base = format!("live-builds-user-{timestamp_ms}");
    let uid = (0..)
        .map(|suffix| {
            if suffix == 0 {
                base.clone()
            } else {
                format!("{base}-{suffix}")
            }
        })
        .find(|candidate| sets.iter().all(|set| set.uid != *candidate))
        .expect("there is always another numeric suffix");
    ItemSet {
        uid,
        title: "Novo arsenal".to_owned(),
        champion_ids: vec![],
        blocks: vec![ItemBlock {
            name: "Itens".to_owned(),
            items: vec![],
            original: None,
        }],
    }
}

pub fn rune_options(rules: &RuneRules, draft: &RunePageDraft, index: usize) -> Vec<u32> {
    match index {
        0..=3 => rules.slot_perks(draft.primary_style, index),
        4..=5 => {
            let other = if index == 4 {
                draft.perks[5]
            } else {
                draft.perks[4]
            };
            let occupied = rules.regular_slot_of(draft.sub_style, other);
            (1..=3)
                .filter(|slot| Some(*slot) != occupied)
                .flat_map(|slot| rules.slot_perks(draft.sub_style, slot))
                .collect()
        }
        6..=8 => rules.slot_perks(draft.primary_style, index - 2),
        _ => Vec::new(),
    }
}

/// Árvore compartilhada pelos dois editores de runas. As opções sempre vêm das regras da LCU.
pub fn draw_rune_tree(
    ui: &mut egui::Ui,
    rules: &RuneRules,
    draft: &mut RunePageDraft,
    names: &KnowledgeNames,
    icons: &HashMap<String, egui::TextureHandle>,
) -> bool {
    let before = draft.clone();
    ui.set_min_width(ui.available_width());
    ui.columns(2, |columns| {
        draw_primary_tree(&mut columns[0], rules, draft, names, icons);
        draw_secondary_tree(&mut columns[1], rules, draft, names, icons);
    });
    *draft != before
}

fn draw_primary_tree(
    ui: &mut egui::Ui,
    rules: &RuneRules,
    draft: &mut RunePageDraft,
    names: &KnowledgeNames,
    icons: &HashMap<String, egui::TextureHandle>,
) {
    ui.heading("Árvore primária");
    ui.horizontal_wrapped(|ui| {
        for style in rules.style_ids() {
            let name = names.style(style);
            if rune_icon_button(
                ui,
                draft.primary_style == style,
                &name,
                names.style_icon(style),
                icons,
                32.0,
                style_color(style),
            ) {
                select_primary_style(rules, draft, style);
            }
        }
    });
    for slot in 0..4 {
        ui.horizontal_wrapped(|ui| {
            for perk in tree_row_options(rules, draft, TreeRow::Primary(slot)) {
                let name = names.rune(perk);
                if rune_icon_button(
                    ui,
                    draft.perks[slot] == perk,
                    &name,
                    names.rune_icon(perk),
                    icons,
                    if slot == 0 { 64.0 } else { 48.0 },
                    style_color(draft.primary_style),
                ) {
                    select_tree_perk(rules, draft, TreeRow::Primary(slot), perk);
                }
            }
        });
    }
}

fn draw_secondary_tree(
    ui: &mut egui::Ui,
    rules: &RuneRules,
    draft: &mut RunePageDraft,
    names: &KnowledgeNames,
    icons: &HashMap<String, egui::TextureHandle>,
) {
    ui.label("Árvore secundária");
    ui.horizontal_wrapped(|ui| {
        for style in rules.allowed_sub_styles(draft.primary_style) {
            let name = names.style(style);
            if rune_icon_button(
                ui,
                draft.sub_style == style,
                &name,
                names.style_icon(style),
                icons,
                28.0,
                style_color(style),
            ) {
                select_secondary_style(rules, draft, style);
            }
        }
    });
    for slot in 1..=3 {
        ui.horizontal_wrapped(|ui| {
            for perk in rules.slot_perks(draft.sub_style, slot) {
                let name = names.rune(perk);
                if rune_icon_button(
                    ui,
                    draft.perks[4..6].contains(&perk),
                    &name,
                    names.rune_icon(perk),
                    icons,
                    48.0,
                    style_color(draft.sub_style),
                ) {
                    select_secondary_perk(rules, draft, slot, perk);
                }
            }
        });
    }
    ui.label("Fragmentos");
    for index in 0..3 {
        ui.label(match index {
            0 => "Fragmento 1 (ofensivo)",
            1 => "Fragmento 2 (flexível)",
            _ => "Fragmento 3 (defensivo)",
        });
        ui.horizontal_wrapped(|ui| {
            for perk in tree_row_options(rules, draft, TreeRow::Shard(index)) {
                let name = names.rune(perk);
                if rune_icon_button(
                    ui,
                    draft.perks[index + 6] == perk,
                    &name,
                    names.rune_icon(perk),
                    icons,
                    32.0,
                    style_color(draft.primary_style),
                ) {
                    select_tree_perk(rules, draft, TreeRow::Shard(index), perk);
                }
            }
        });
    }
}

fn select_secondary_perk(
    rules: &RuneRules,
    draft: &mut RunePageDraft,
    slot: usize,
    perk: u32,
) -> bool {
    if !rules.slot_perks(draft.sub_style, slot).contains(&perk) || draft.perks[4..6].contains(&perk)
    {
        return false;
    }
    let index = draft.perks[4..6]
        .iter()
        .position(|selected| rules.regular_slot_of(draft.sub_style, *selected) == Some(slot))
        .map_or(4, |index| index + 4);
    draft.perks[index] = perk;
    true
}

fn rune_icon_button(
    ui: &mut egui::Ui,
    selected: bool,
    name: &str,
    icon_path: Option<&str>,
    icons: &HashMap<String, egui::TextureHandle>,
    size: f32,
    color: egui::Color32,
) -> bool {
    let tint = if selected {
        color
    } else {
        egui::Color32::from_gray(105)
    };
    let stroke = if selected {
        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(218, 165, 32))
    } else {
        egui::Stroke::NONE
    };
    if let Some(texture) = icon_path.and_then(|path| icons.get(path)) {
        ui.add(
            egui::Button::image(
                egui::Image::new(texture)
                    .fit_to_exact_size(egui::vec2(size, size))
                    .tint(tint),
            )
            .stroke(stroke),
        )
        .on_hover_text(name)
        .clicked()
    } else {
        ui.add(
            egui::Button::new(egui::RichText::new(icon_label(name, None)).color(tint))
                .stroke(stroke),
        )
        .on_hover_text(name)
        .clicked()
    }
}

fn style_color(style: u32) -> egui::Color32 {
    match style {
        8000 => egui::Color32::from_rgb(240, 190, 80),
        8100 => egui::Color32::from_rgb(210, 80, 80),
        8200 => egui::Color32::from_rgb(95, 155, 240),
        8300 => egui::Color32::from_rgb(100, 210, 215),
        8400 => egui::Color32::from_rgb(100, 185, 110),
        _ => egui::Color32::WHITE,
    }
}

#[derive(Clone, Debug)]
pub struct DeleteConfirmation<T> {
    pending: Option<(T, String)>,
}

impl<T> Default for DeleteConfirmation<T> {
    fn default() -> Self {
        Self { pending: None }
    }
}

impl<T> DeleteConfirmation<T> {
    pub fn request_page(&mut self, target: T, name: &str) {
        self.pending = Some((
            target,
            format!("Apagar a página {name}? Não dá para desfazer."),
        ));
    }
    pub fn request_item_set(&mut self, target: T, name: &str) {
        self.pending = Some((
            target,
            format!("Apagar o arsenal {name}? Não dá para desfazer."),
        ));
    }
    pub fn message(&self) -> Option<&str> {
        self.pending.as_ref().map(|(_, text)| text.as_str())
    }
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    pub fn confirm(&mut self) -> Option<T> {
        self.pending.take().map(|(target, _)| target)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EditorTab {
    #[default]
    Runes,
    ItemSets,
    Settings,
}

#[derive(Clone, Debug)]
pub struct PageEditorDraft {
    pub id: Option<u64>,
    pub editable: bool,
    pub draft: RunePageDraft,
    /// Estado ao abrir o rascunho; serve para saber se há alterações sujas.
    pub baseline: RunePageDraft,
}

#[derive(Clone, Debug)]
pub struct SetEditorDraft {
    pub is_new: bool,
    pub set: ItemSet,
    pub baseline: ItemSet,
}

/// Troca de rascunho pendente de confirmação "Descartar alterações?".
#[derive(Clone, Debug)]
pub enum DiscardTarget {
    EditPage(usize),
    NewPage(RunePageDraft),
    EditSet(usize),
    NewSet(ItemSet),
}

#[derive(Default)]
pub struct EditorsState {
    pub tab: EditorTab,
    pub pages: Vec<ClientPage>,
    pub selected_page: Option<usize>,
    pub page_draft: Option<PageEditorDraft>,
    pub rules: Option<RuneRules>,
    pub sets: Vec<ItemSet>,
    pub selected_set: Option<usize>,
    pub set_draft: Option<SetEditorDraft>,
    pub item_query: String,
    pub page_delete: DeleteConfirmation<u64>,
    pub set_delete: DeleteConfirmation<String>,
    pub loaded: bool,
    pub status: Option<String>,
    pub discard: Option<DiscardTarget>,
}

impl EditorsState {
    pub fn edit_page(&mut self, index: usize) {
        let Some(page) = self.pages.get(index) else {
            return;
        };
        self.selected_page = Some(index);
        self.page_delete.cancel();
        self.page_draft = Some(PageEditorDraft {
            id: Some(page.id),
            editable: page.editable,
            draft: page.draft.clone(),
            baseline: page.draft.clone(),
        });
    }

    pub fn page_dirty(&self) -> bool {
        self.page_draft
            .as_ref()
            .is_some_and(|page| page.draft != page.baseline)
    }

    pub fn set_dirty(&self) -> bool {
        self.set_draft
            .as_ref()
            .is_some_and(|editor| editor.set != editor.baseline)
    }

    fn start_new_page(&mut self, draft: RunePageDraft) {
        self.selected_page = None;
        self.page_delete.cancel();
        self.page_draft = Some(PageEditorDraft {
            id: None,
            editable: true,
            baseline: draft.clone(),
            draft,
        });
    }

    fn start_new_set(&mut self, set: ItemSet) {
        self.selected_set = None;
        self.set_delete.cancel();
        self.set_draft = Some(SetEditorDraft {
            is_new: true,
            baseline: set.clone(),
            set,
        });
    }

    /// Clique numa página da lista: com rascunho sujo, pede confirmação antes de trocar.
    pub fn open_page(&mut self, index: usize) {
        if self.selected_page == Some(index) {
            return;
        }
        if self.page_dirty() {
            self.discard = Some(DiscardTarget::EditPage(index));
        } else {
            self.edit_page(index);
        }
    }

    pub fn open_set(&mut self, index: usize) {
        if self.selected_set == Some(index) {
            return;
        }
        if self.set_dirty() {
            self.discard = Some(DiscardTarget::EditSet(index));
        } else {
            self.edit_set(index);
        }
    }

    pub fn request_new_page(&mut self, draft: RunePageDraft) {
        if self.page_dirty() {
            self.discard = Some(DiscardTarget::NewPage(draft));
        } else {
            self.start_new_page(draft);
        }
    }

    pub fn request_new_set(&mut self, set: ItemSet) {
        if self.set_dirty() {
            self.discard = Some(DiscardTarget::NewSet(set));
        } else {
            self.start_new_set(set);
        }
    }

    pub fn confirm_discard(&mut self) {
        match self.discard.take() {
            Some(DiscardTarget::EditPage(index)) => self.edit_page(index),
            Some(DiscardTarget::NewPage(draft)) => self.start_new_page(draft),
            Some(DiscardTarget::EditSet(index)) => self.edit_set(index),
            Some(DiscardTarget::NewSet(set)) => self.start_new_set(set),
            None => {}
        }
    }

    /// Confirmação de apagar não sobrevive ao rascunho que ela mirava.
    pub fn sync_confirmations(&mut self) {
        if self.page_draft.is_none() {
            self.page_delete.cancel();
        }
        if self.set_draft.is_none() {
            self.set_delete.cancel();
        }
    }

    /// Troca as listas do cliente. Só o rascunho do item salvo (`reset_*`) ou limpo é recarregado;
    /// um rascunho sujo de outro item sobrevive (ex.: "Atualizar").
    pub fn replace_lists(
        &mut self,
        pages: Vec<ClientPage>,
        sets: Vec<ItemSet>,
        reset_page: bool,
        reset_set: bool,
    ) {
        self.pages = pages;
        self.sets = sets;
        if reset_page || !self.page_dirty() {
            let index = self
                .selected_page
                .filter(|index| *index < self.pages.len())
                .or_else(|| (!self.pages.is_empty()).then_some(0));
            self.page_draft = None;
            self.selected_page = None;
            if let Some(index) = index {
                self.edit_page(index);
            }
        } else {
            let id = self.page_draft.as_ref().and_then(|page| page.id);
            self.selected_page = id.and_then(|id| self.pages.iter().position(|p| p.id == id));
        }
        if reset_set || !self.set_dirty() {
            let index = self
                .selected_set
                .filter(|index| *index < self.sets.len())
                .or_else(|| (!self.sets.is_empty()).then_some(0));
            self.set_draft = None;
            self.selected_set = None;
            if let Some(index) = index {
                self.edit_set(index);
            }
        } else {
            let uid = self.set_draft.as_ref().map(|editor| editor.set.uid.clone());
            self.selected_set = uid.and_then(|uid| self.sets.iter().position(|set| set.uid == uid));
        }
        self.sync_confirmations();
    }

    pub fn edit_set(&mut self, index: usize) {
        let Some(set) = self.sets.get(index) else {
            return;
        };
        self.selected_set = Some(index);
        self.set_delete.cancel();
        self.set_draft = Some(SetEditorDraft {
            is_new: false,
            set: set.clone(),
            baseline: set.clone(),
        });
    }
}

pub enum EditorAction {
    Refresh,
    CreatePage(RunePageDraft),
    SavePage(u64, RunePageDraft),
    DeletePage(u64),
    CreateSet(ItemSet),
    SaveSet(ItemSet),
    DeleteSet(String),
    SaveSettings,
}

pub fn new_page(rules: &RuneRules) -> Option<RunePageDraft> {
    let primary = rules.style_ids().into_iter().next()?;
    let secondary = rules.allowed_sub_styles(primary).into_iter().next()?;
    Some(RunePageDraft {
        name: "Nova página".to_owned(),
        primary_style: primary,
        sub_style: secondary,
        perks: draft_for_styles(rules, primary, secondary)?,
    })
}

pub fn draw(
    ctx: &egui::Context,
    state: &mut EditorsState,
    settings: &mut Settings,
    names: &KnowledgeNames,
    icons: &HashMap<String, egui::TextureHandle>,
    catalog: Option<&ItemCatalog>,
    enabled: bool,
) -> Vec<EditorAction> {
    let mut actions = Vec::new();
    egui::TopBottomPanel::top("editor_tabs").show(ctx, |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.tab, EditorTab::Runes, "Runas");
                ui.selectable_value(&mut state.tab, EditorTab::ItemSets, "Arsenais");
                ui.selectable_value(&mut state.tab, EditorTab::Settings, "Configurações");
                if ui.button("Atualizar").clicked() {
                    actions.push(EditorAction::Refresh);
                }
                if let Some(status) = &state.status {
                    ui.label(status);
                }
            })
        });
    });
    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            if state.discard.is_some() {
                ui.colored_label(egui::Color32::LIGHT_RED, "Descartar alterações?");
                ui.horizontal(|ui| {
                    if ui.button("Descartar").clicked() {
                        state.confirm_discard();
                    }
                    if ui.button("Continuar editando").clicked() {
                        state.discard = None;
                    }
                });
                ui.separator();
            }
            state.sync_confirmations();
            ui.add_enabled_ui(enabled, |ui| match state.tab {
                EditorTab::Runes => draw_runes(ui, state, names, icons, &mut actions),
                EditorTab::ItemSets => draw_sets(ui, state, catalog, &mut actions),
                EditorTab::Settings => {
                    ui.heading("Configurações");
                    let changed = ui
                        .checkbox(&mut settings.auto_runes, "Aplicar runas automaticamente")
                        .changed()
                        | ui.checkbox(
                            &mut settings.auto_spells,
                            "Aplicar feitiços automaticamente",
                        )
                        .changed()
                        | ui.checkbox(
                            &mut settings.auto_item_set,
                            "Aplicar arsenal automaticamente",
                        )
                        .changed()
                        | ui.checkbox(
                            &mut settings.exclusive_item_set,
                            "Arsenal exclusivo: apagar os outros arsenais do campeão ao aplicar",
                        )
                        .changed();
                    if changed {
                        actions.push(EditorAction::SaveSettings);
                    }
                    if settings.corrupted {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            "As configurações estavam corrompidas; revise e salve novamente.",
                        );
                    }
                }
            });
        });
    });
    actions
}

fn draw_runes(
    ui: &mut egui::Ui,
    state: &mut EditorsState,
    names: &KnowledgeNames,
    icons: &HashMap<String, egui::TextureHandle>,
    actions: &mut Vec<EditorAction>,
) {
    let Some(rules) = state.rules.clone() else {
        ui.label("Carregando páginas de runas…");
        return;
    };
    ui.columns(2, |columns| {
        columns[0].heading("Páginas");
        let mut selected = None;
        for (index, page) in state.pages.iter().enumerate() {
            if columns[0]
                .selectable_label(state.selected_page == Some(index), &page.name)
                .clicked()
            {
                selected = Some(index);
            }
        }
        if let Some(index) = selected {
            state.open_page(index);
        }
        if columns[0].button("Criar página").clicked()
            && let Some(draft) = new_page(&rules)
        {
            state.request_new_page(draft);
        }
        let Some(page) = state.page_draft.as_mut() else {
            return;
        };
        columns[1].heading("Editar página");
        if !page.editable {
            columns[1].colored_label(egui::Color32::YELLOW, "Página do cliente: não editável");
        }
        let page_id = page.id;
        let page_name = page.draft.name.clone();
        columns[1].add_enabled_ui(page.editable, |ui| {
            ui.text_edit_singleline(&mut page.draft.name);
            draw_rune_tree(ui, &rules, &mut page.draft, names, icons);
            if ui
                .add_enabled(
                    rules.validate(&page.draft).is_ok(),
                    egui::Button::new("Salvar"),
                )
                .clicked()
            {
                actions.push(match page.id {
                    Some(id) => EditorAction::SavePage(id, page.draft.clone()),
                    None => EditorAction::CreatePage(page.draft.clone()),
                });
            }
            if let Some(id) = page_id
                && ui.button("Apagar página").clicked()
            {
                state.page_delete.request_page(id, &page_name);
            }
        });
        if let Some(message) = state.page_delete.message().map(str::to_owned) {
            columns[1].separator();
            columns[1].colored_label(egui::Color32::LIGHT_RED, message);
            columns[1].horizontal(|ui| {
                if ui.button("Confirmar").clicked()
                    && let Some(id) = state.page_delete.confirm()
                {
                    actions.push(EditorAction::DeletePage(id));
                }
                if ui.button("Cancelar").clicked() {
                    state.page_delete.cancel();
                }
            });
        }
    });
}

fn draw_sets(
    ui: &mut egui::Ui,
    state: &mut EditorsState,
    catalog: Option<&ItemCatalog>,
    actions: &mut Vec<EditorAction>,
) {
    ui.columns(2, |columns| {
        columns[0].heading("Arsenais");
        let mut selected = None;
        for (index, set) in state.sets.iter().enumerate() {
            if columns[0]
                .selectable_label(state.selected_set == Some(index), &set.title)
                .clicked()
            {
                selected = Some(index);
            }
        }
        if let Some(index) = selected {
            state.open_set(index);
        }
        if columns[0].button("Criar arsenal").clicked() {
            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            state.request_new_set(new_item_set(&state.sets, timestamp_ms));
        }
        let Some(editor) = state.set_draft.as_mut() else {
            return;
        };
        let set = &mut editor.set;
        columns[1].heading("Editar arsenal");
        columns[1].text_edit_singleline(&mut set.title);
        let mut remove_block = None;
        for (block_index, block) in set.blocks.iter_mut().enumerate() {
            columns[1].horizontal(|ui| {
                ui.text_edit_singleline(&mut block.name);
                if ui.button("Remover bloco").clicked() {
                    remove_block = Some(block_index);
                }
            });
            let mut remove_item = None;
            for (item_index, item) in block.items.iter().enumerate() {
                columns[1].horizontal(|ui| {
                    ui.label(
                        catalog.map_or_else(|| item.to_string(), |catalog| catalog.name(*item)),
                    );
                    if ui.small_button("×").clicked() {
                        remove_item = Some(item_index);
                    }
                });
            }
            if let Some(item_index) = remove_item {
                block.items.remove(item_index);
            }
        }
        if let Some(block_index) = remove_block {
            set.blocks.remove(block_index);
        }
        if columns[1].button("Adicionar bloco").clicked() {
            set.blocks.push(ItemBlock {
                name: "Novo bloco".to_owned(),
                items: vec![],
                original: None,
            });
        }
        columns[1].separator();
        columns[1].label("Buscar item por nome");
        columns[1].text_edit_singleline(&mut state.item_query);
        if let Some(catalog) = catalog {
            for (id, name) in catalog.search(&state.item_query) {
                if columns[1].button(format!("+ {name}")).clicked()
                    && let Some(block) = set.blocks.last_mut()
                {
                    block.items.push(id);
                }
            }
        }
        if columns[1].button("Salvar").clicked() {
            actions.push(if editor.is_new {
                EditorAction::CreateSet(set.clone())
            } else {
                EditorAction::SaveSet(set.clone())
            });
        }
        if !editor.is_new && columns[1].button("Apagar arsenal").clicked() {
            state
                .set_delete
                .request_item_set(set.uid.clone(), &set.title);
        }
        if let Some(message) = state.set_delete.message().map(str::to_owned) {
            columns[1].separator();
            columns[1].colored_label(egui::Color32::LIGHT_RED, message);
            columns[1].horizontal(|ui| {
                if ui.button("Confirmar").clicked()
                    && let Some(uid) = state.set_delete.confirm()
                {
                    actions.push(EditorAction::DeleteSet(uid));
                }
                if ui.button("Cancelar").clicked() {
                    state.set_delete.cancel();
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::KnowledgePackage;
    use crate::lcu::runes::from_knowledge;
    use std::collections::HashMap;

    #[test]
    fn icon_cache_path_never_uses_remote_path_parts() {
        let cache = std::path::Path::new("cache/icons");
        for icon in ["..\\x", "C:\\a", "../../a"] {
            let path = icon_cache_path(cache, icon);
            assert_eq!(path.parent(), Some(cache));
            assert_eq!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("png")
            );
            assert!(path.file_stem().is_some_and(|name| name != icon));
        }
    }

    #[test]
    fn missing_icon_falls_back_to_text() {
        assert_eq!(icon_label("Ritmo Fatal", None), "Ritmo Fatal");
    }

    #[test]
    fn decodes_embedded_png() {
        const PNG: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15,
            0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];

        let decoded = decode_icon(PNG).unwrap();

        assert_eq!(decoded.size, [1, 1]);
        assert_eq!(decoded.rgba.len(), 4);
    }

    #[test]
    fn tree_offers_only_valid_runes_per_row() {
        let rules =
            RuneRules::from_json(include_str!("../tests/fixtures/lcu/perks-styles.json")).unwrap();
        let draft = new_page(&rules).unwrap();

        assert_eq!(
            tree_row_options(&rules, &draft, TreeRow::Primary(1)),
            rules.slot_perks(draft.primary_style, 1)
        );
        assert!(
            tree_row_options(&rules, &draft, TreeRow::Secondary(0))
                .iter()
                .all(|perk| rules.regular_slot_of(draft.sub_style, *perk)
                    != rules.regular_slot_of(draft.sub_style, draft.perks[5]))
        );
    }

    #[test]
    fn precision_keystones_are_all_selectable() {
        let rules =
            RuneRules::from_json(include_str!("../tests/fixtures/lcu/perks-styles.json")).unwrap();
        let mut draft = new_page(&rules).unwrap();
        select_primary_style(&rules, &mut draft, 8100);
        select_secondary_style(&rules, &mut draft, 8000);

        assert!(select_primary_style(&rules, &mut draft, 8000));
        for perk in [8005, 8008, 8021, 8010] {
            draft.perks[0] = if perk == 8005 { 8008 } else { 8005 };
            assert!(select_tree_perk(
                &rules,
                &mut draft,
                TreeRow::Primary(0),
                perk
            ));
            assert_eq!(draft.perks[0], perk);
            assert!(rules.validate(&draft).is_ok());
        }
    }

    fn assert_every_primary_tree_rune_is_selectable(style: u32) {
        let rules =
            RuneRules::from_json(include_str!("../tests/fixtures/lcu/perks-styles.json")).unwrap();
        let mut draft = new_page(&rules).unwrap();
        if draft.primary_style == style {
            assert!(select_primary_style(&rules, &mut draft, 8100));
        }
        assert!(select_primary_style(&rules, &mut draft, style));

        for slot in 0..7 {
            let row = if slot < 4 {
                TreeRow::Primary(slot)
            } else {
                TreeRow::Shard(slot - 4)
            };
            for perk in rules.slot_perks(style, slot) {
                let mut selected = draft.clone();
                let index = if slot < 4 { slot } else { slot + 2 };
                selected.perks[index] = rules
                    .slot_perks(style, slot)
                    .into_iter()
                    .find(|candidate| *candidate != perk)
                    .unwrap();
                assert!(select_tree_perk(&rules, &mut selected, row, perk));
                assert!(
                    rules.validate(&selected).is_ok(),
                    "style {style}, perk {perk}"
                );
            }
        }
    }

    #[test]
    fn precision_tree_runes_are_selectable_and_valid() {
        assert_every_primary_tree_rune_is_selectable(8000);
    }

    #[test]
    fn domination_tree_runes_are_selectable_and_valid() {
        assert_every_primary_tree_rune_is_selectable(8100);
    }

    #[test]
    fn sorcery_tree_runes_are_selectable_and_valid() {
        assert_every_primary_tree_rune_is_selectable(8200);
    }

    #[test]
    fn inspiration_tree_runes_are_selectable_and_valid() {
        assert_every_primary_tree_rune_is_selectable(8300);
    }

    #[test]
    fn resolve_tree_runes_are_selectable_and_valid() {
        assert_every_primary_tree_rune_is_selectable(8400);
    }

    fn painted_text(shape: &egui::epaint::Shape, text: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(shape) => text.push(shape.galley.text().to_owned()),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    painted_text(shape, text);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn editor_shows_shard_names() {
        let package: KnowledgePackage =
            serde_json::from_str(include_str!("../tests/fixtures/knowledge-16.22.json")).unwrap();
        let page = package
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap()
            .rune_pages
            .first()
            .unwrap();
        let draft = from_knowledge(page, "Kai'Sa").unwrap();
        let mut state = EditorsState {
            rules: Some(
                RuneRules::from_json(include_str!("../tests/fixtures/lcu/perks-styles.json"))
                    .unwrap(),
            ),
            page_draft: Some(PageEditorDraft {
                id: None,
                editable: true,
                baseline: draft.clone(),
                draft,
            }),
            ..Default::default()
        };
        let names = KnowledgeNames {
            runes: HashMap::from([(8008, "Ritmo Fatal".to_owned())]),
            ..Default::default()
        };
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |ctx| {
            draw(
                ctx,
                &mut state,
                &mut Settings::default(),
                &names,
                &HashMap::new(),
                None,
                true,
            );
        });
        let mut text = Vec::new();
        for shape in &output.shapes {
            painted_text(&shape.shape, &mut text);
        }

        assert!(text.iter().any(|value| value == "Fragmento 1 (ofensivo)"));
        assert!(text.iter().any(|value| value == "Fragmento 2 (flexível)"));
        assert!(text.iter().any(|value| value == "Fragmento 3 (defensivo)"));
        assert!(text.iter().any(|value| value == "Velocidade de ataque"));
        assert!(text.iter().any(|value| value == "Força adaptativa"));
        assert!(text.iter().any(|value| value == "Vida"));
        assert!(text.iter().any(|value| value == "Ritmo Fatal"));
    }

    #[test]
    fn layout_uses_available_width() {
        let rules =
            RuneRules::from_json(include_str!("../tests/fixtures/lcu/perks-styles.json")).unwrap();
        let names = KnowledgeNames::default();
        let render_width = |width| {
            let context = egui::Context::default();
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 900.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut draft = new_page(&rules).unwrap();
                        let mut tree_width = 0.0;
                        ui.scope(|ui| {
                            draw_rune_tree(ui, &rules, &mut draft, &names, &HashMap::new());
                            tree_width = ui.min_rect().width();
                        });
                        ui.data_mut(|data| {
                            data.insert_temp(egui::Id::new("tree width"), tree_width)
                        });
                    });
                },
            );
            context.data_mut(|data| data.get_temp::<f32>(egui::Id::new("tree width")).unwrap())
        };

        assert!(render_width(1_000.0) > 900.0);
        assert!(render_width(1_000.0) > render_width(400.0));
    }
}
