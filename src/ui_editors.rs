use crate::catalog::ItemCatalog;
use crate::knowledge::KnowledgeNames;
use crate::lcu::item_sets::{ItemBlock, ItemSet};
use crate::lcu::perks::ClientPage;
use crate::lcu::runes::{RunePageDraft, RuneRules};
use crate::settings::Settings;
use std::time::{SystemTime, UNIX_EPOCH};

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
        perks: [
            *rules.slot_perks(primary, 0).first()?,
            *rules.slot_perks(primary, 1).first()?,
            *rules.slot_perks(primary, 2).first()?,
            *rules.slot_perks(primary, 3).first()?,
            *rules.slot_perks(secondary, 1).first()?,
            *rules.slot_perks(secondary, 2).first()?,
            *rules.slot_perks(primary, 4).first()?,
            *rules.slot_perks(primary, 5).first()?,
            *rules.slot_perks(primary, 6).first()?,
        ],
    })
}

pub fn draw(
    ctx: &egui::Context,
    state: &mut EditorsState,
    settings: &mut Settings,
    names: &KnowledgeNames,
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
            EditorTab::Runes => draw_runes(ui, state, names, &mut actions),
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
    actions
}

fn draw_runes(
    ui: &mut egui::Ui,
    state: &mut EditorsState,
    names: &KnowledgeNames,
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
            combo(
                ui,
                "Árvore primária",
                &mut page.draft.primary_style,
                rules.style_ids(),
                |id| names.style(id),
            );
            combo(
                ui,
                "Árvore secundária",
                &mut page.draft.sub_style,
                rules.allowed_sub_styles(page.draft.primary_style),
                |id| names.style(id),
            );
            for rune_index in 0..9 {
                let label = match rune_index {
                    0..=5 => format!("Runa {}", rune_index + 1),
                    6 => "Fragmento 1 (ofensivo)".to_owned(),
                    7 => "Fragmento 2 (flexível)".to_owned(),
                    8 => "Fragmento 3 (defensivo)".to_owned(),
                    _ => unreachable!(),
                };
                let options = rune_options(&rules, &page.draft, rune_index);
                combo(
                    ui,
                    &label,
                    &mut page.draft.perks[rune_index],
                    options,
                    |id| names.rune(id),
                );
            }
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

fn combo(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut u32,
    options: Vec<u32>,
    name: impl Fn(u32) -> String,
) -> bool {
    let before = *selected;
    egui::ComboBox::from_label(label)
        .selected_text(name(*selected))
        .show_ui(ui, |ui| {
            for option in options {
                ui.selectable_value(selected, option, name(option));
            }
        });
    *selected != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::KnowledgePackage;
    use crate::lcu::runes::from_knowledge;
    use std::collections::HashMap;

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
}
