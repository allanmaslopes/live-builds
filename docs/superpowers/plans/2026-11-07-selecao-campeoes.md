# Seleção de campeões pela LCU (entrega 1) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Na seleção de campeões, aplicar (automático por chave ou com "Aplicar") runas, feitiços e arsenal de elo alto; editar todas as páginas de runas e arsenais pelo app; avisar antes da partida se runas/feitiços não batem.

**Architecture:** Novo módulo `src/lcu/` (conexão pelo lockfile, cliente HTTP, modelos), lógica pura separada (validação de página, conversões, lembrete, orquestração da aplicação) e um segundo "poller" leve da LCU que só consulta durante a seleção. A recomendação vem de `knowledge.rs` (elo alto) com fallback da Riot, reaproveitando `select_entry`/`select_page`/`to_recommendations`.

**Tech Stack:** Rust stable (gnu), eframe/egui 0.33 (glow), `ureq` 3 + `rustls` (ring), `serde_json`. Sem dependências novas.

**Spec:** `docs/superpowers/specs/2026-11-07-selecao-campeoes-design.md` (ler inteiro antes de cada task) e `docs/lcu.md`.

## Global Constraints

- Sem dependências novas. TLS da LCU validado com o `riotgames.pem` já embutido; Basic `riot:<senha do lockfile>`.
- Lockfile: `C:\Riot Games\League of Legends\lockfile` por padrão, campos `nome:pid:porta:senha:protocolo`; flag `--lcu-lockfile <caminho>` para sobrescrever. Reler a cada conexão (porta/senha mudam).
- Página do app chama-se exatamente `live-builds`; arsenais do app: `live-builds — <campeão> <posição>`. Nunca alterar/apagar outra página ou arsenal ao aplicar.
- Limite de páginas cheio: não apagar nada; mostrar "Limite de páginas de runas atingido: libere uma para o live-builds".
- Chaves (todas padrão `false`), salvas em `%LOCALAPPDATA%\live-builds\settings.json`: `auto_runes`, `auto_spells`, `auto_item_set`.
- Apagar página/arsenal pelo editor exige confirmação na UI.
- Nenhuma escrita sobre campo "(não confirmado)" do `docs/lcu.md` antes da Task 1 confirmá-lo.
- Nada de pick, ban, travar campeão, aceitar partida ou dodge.
- LCU só é consultada com o cliente aberto; durante a seleção, intervalo de 1 s; fora dela, 5 s; com cliente fechado, 10 s. Sem rede, sem erro na tela além de "Cliente do LoL fechado".
- Fixtures do cliente real em `tests/fixtures/lcu/`, anonimizadas (sem `summonerId`, `puuid`, nomes; troque por valores fixos).
- Agente delegado não roda git; o arquiteto commita.

## Review Focus

1. Usuário troca de campeão depois de travar (troca com aliado) ou a seleção é cancelada (dodge de outro jogador): reaplicar para o campeão novo / limpar estado, sem aplicar ao campeão antigo. → `champion_swap_reapplies_and_dodge_resets` (Task 6).
2. Página `live-builds` apagada pelo usuário no cliente entre duas aplicações: o app recria, não falha. → `missing_app_page_is_recreated` (Task 4).
3. Cliente reiniciado durante a seleção (nova porta/senha): reconectar relendo o lockfile. → `reconnects_after_lockfile_changes` (Task 2).
4. Página do pacote com runa que o cliente não tem mais (patch novo): não gravar página inválida; cair na recomendação da Riot e avisar. → `invalid_package_page_falls_back` (Task 3).
5. Chave automática ligada e usuário editou a página no editor: o automático não sobrescreve a edição na mesma seleção. → `auto_apply_does_not_override_user_edit` (Task 6).

---

### Task 1: Sonda somente leitura da LCU (com o cliente do usuário aberto)

**Files:**
- Create: `examples/lcu_probe.rs`
- Modify: `docs/lcu.md` (marcar cada item confirmado/refutado, com o formato real)
- Create: `tests/fixtures/lcu/*.json` (respostas anonimizadas)

**Interfaces:**
- Produces: fixtures `gameflow-phase.json`, `champ-select-session.json` (se houver seleção aberta), `perks-pages.json`, `perks-currentpage.json`, `perks-styles.json`, `perks-customizationlimits.json` ou equivalente, `item-sets.json` (ou `loadouts.json`), `current-summoner.json` (anonimizado), mais um resumo `tests/fixtures/lcu/README.md` dizendo o caminho e os campos confirmados de cada um.

- [ ] **Step 1:** `examples/lcu_probe.rs`: lê o lockfile, faz **só GET** em: `/lol-gameflow/v1/gameflow-phase`, `/lol-summoner/v1/current-summoner`, `/lol-champ-select/v1/session`, `/lol-perks/v1/pages`, `/lol-perks/v1/currentpage`, `/lol-perks/v1/styles`, `/lol-perks/v1/customizationlimits`, `/lol-perks/v1/inventory`, `/lol-item-sets/v1/item-sets/{summonerId}/sets`, `/lol-loadouts/v4/loadouts/scope/account`; anonimiza (`summonerId`, `accountId`, `puuid`, `gameName`, `tagLine`, `displayName`, `internalName` → valores fixos) e grava em `tests/fixtures/lcu/`.
- [ ] **Step 2 (arquiteto, com o usuário):** rodar `cargo run --example lcu_probe` com o cliente aberto no lobby e depois numa seleção (partida personalizada). Conferir que nenhum dado pessoal ficou nas fixtures.
- [ ] **Step 3:** atualizar `docs/lcu.md`: para os 8 itens "(não confirmado)", escrever o resultado (campos de feitiço em `my-selection`, limite de páginas, item sets vs loadouts, campos de posição e de campeão travado na sessão).
- [ ] **Step 4: Commit** (arquiteto) — `docs: sonda da LCU confirma campos de runas, feitiços e item sets`.

### Task 2: Conexão e cliente HTTP da LCU

**Files:**
- Create: `src/lcu/mod.rs`, `src/lcu/connection.rs`
- Modify: `src/lib.rs` (registrar módulo e a flag `--lcu-lockfile`)

**Interfaces:**
- Produces: `pub struct Lockfile { pub port: u16, pub password: String }`; `pub fn parse_lockfile(text: &str) -> Result<Lockfile, String>`; `pub trait LcuHttp { fn get(&self, path: &str) -> Result<serde_json::Value, LcuError>; fn send(&self, method: &str, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, LcuError>; }`; `pub struct LcuClient` (implementa `LcuHttp`, `LcuClient::connect(lockfile_path: &Path) -> Result<LcuClient, LcuError>`); `pub enum LcuError { ClientClosed, Http(u16, String), Other(String) }`.

- [ ] **Step 1: Testes que falham** — `parses_lockfile_fields` (`"LeagueClient:1234:54321:abc:https"` → porta 54321, senha `abc`); `missing_lockfile_is_client_closed`; `reconnects_after_lockfile_changes` (fake de arquivo: porta muda → nova conexão usa a porta nova).
- [ ] **Step 2:** rodar `cargo test lcu::` e ver falhar.
- [ ] **Step 3:** implementar com `ureq` e o `riotgames.pem` (mesmo agente TLS do Live Client); `DELETE` via `send("DELETE", …, &Value::Null)`.
- [ ] **Step 4:** `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- [ ] **Step 5: Commit** (arquiteto) — `feat: conexão com a LCU pelo lockfile`.

### Task 3: Modelo e validação de página de runas

**Files:**
- Create: `src/lcu/runes.rs`
- Test fixtures: `tests/fixtures/lcu/perks-styles.json` (Task 1), `tests/fixtures/runesReforged-16.22.1.json` (já existe)

**Interfaces:**
- Consumes: `knowledge::RunePage`, `knowledge::StatPerks`.
- Produces: `pub struct RunePageDraft { pub name: String, pub primary_style: u32, pub sub_style: u32, pub perks: [u32; 9] }` (6 runas + 3 fragmentos na ordem do cliente); `pub struct RuneRules` (de `/lol-perks/v1/styles`) com `pub fn validate(&self, d: &RunePageDraft) -> Result<(), String>`; `pub fn from_knowledge(page: &RunePage, name: &str) -> Option<RunePageDraft>` (`None` se faltar `statPerks`: usar fragmentos padrão 5008/5008/5011 — `shortcut: fragmentos padrão quando o pacote não tem, revisar com dados`); `pub fn from_riot_bin(bin_json: &str, position: &str, name: &str) -> Option<RunePageDraft>` (lê `ChampionRuneRecommendationsContext` do `.bin.json` do Community Dragon).

- [ ] **Step 1: Testes que falham** — `validates_real_package_page` (página real da Kai'Sa da fixture de conhecimento → `Ok`); `rejects_rune_outside_slot`; `rejects_secondary_same_as_primary`; `rejects_bad_shard_slot`; `invalid_package_page_falls_back` (página com runa inexistente → `validate` erra e o orquestrador da Task 6 usará `from_riot_bin`; aqui só o `Err`); `parses_riot_rune_recommendation` (fixture recortada do `.bin.json` real de um campeão com `ChampionRuneRecommendationsContext`).
- [ ] **Steps 2-5:** ver falhar, implementar, rodar os 3 comandos, commit `feat: página de runas validada pelas regras do cliente`.

### Task 4: Páginas de runas no cliente (listar, criar, editar, apagar, página do app)

**Files:**
- Create: `src/lcu/perks.rs`

**Interfaces:**
- Consumes: `LcuHttp`, `RunePageDraft`.
- Produces: `pub struct ClientPage { pub id: u64, pub name: String, pub editable: bool, pub current: bool, pub draft: RunePageDraft }`; `pub fn list_pages(h: &dyn LcuHttp) -> Result<Vec<ClientPage>, LcuError>`; `pub fn create_page(h, d) -> Result<ClientPage, LcuError>`; `pub fn update_page(h, id, d) -> Result<(), LcuError>`; `pub fn delete_page(h, id) -> Result<(), LcuError>`; `pub fn set_current(h, id) -> Result<(), LcuError>`; `pub fn upsert_app_page(h, d) -> Result<ApplyOutcome, LcuError>` com `pub enum ApplyOutcome { Updated, Created, LimitReached }` (procura `live-builds`; atualiza; senão cria; erro de limite → `LimitReached`, nunca apaga) e depois ativa a página.

- [ ] **Step 1: Testes que falham** (fake `LcuHttp` que grava chamadas): `upsert_updates_existing_app_page`, `upsert_creates_when_missing`, `missing_app_page_is_recreated`, `limit_reached_never_deletes` (fake devolve o erro de limite confirmado na Task 1 → `LimitReached` e zero `DELETE`), `never_touches_other_pages` (só a página `live-builds` recebe PUT).
- [ ] **Steps 2-5:** como acima; commit `feat: páginas de runas no cliente com página dedicada live-builds`.

### Task 5: Arsenais (item sets) no cliente

**Files:**
- Create: `src/lcu/item_sets.rs`

**Interfaces:**
- Consumes: `LcuHttp`, `riot_build::RecommendedBuild`, o caminho confirmado na Task 1 (`lol-item-sets` ou `lol-loadouts`).
- Produces: `pub struct ItemSet { pub uid: String, pub title: String, pub champion_ids: Vec<u32>, pub blocks: Vec<ItemBlock> }`, `pub struct ItemBlock { pub name: String, pub items: Vec<u32> }`; `pub fn from_build(champion: &str, champion_id: u32, position: &str, b: &RecommendedBuild) -> ItemSet` (blocos "Início", "Botas", "Itens", título `live-builds — <campeão> <posição>`); `pub fn list_sets(h) -> Result<Vec<ItemSet>, LcuError>`; `pub fn save_sets(h, sets: &[ItemSet]) -> Result<(), LcuError>` (o PUT substitui a lista inteira: sempre ler, alterar só o necessário, gravar a lista completa); `pub fn upsert_app_set(h, s) -> Result<(), LcuError>`.

- [ ] **Step 1: Testes que falham:** `builds_item_set_from_recommendation`, `upsert_keeps_user_sets_untouched` (lista com 2 sets do usuário + o do app → só o do app muda, os 2 seguem iguais byte a byte), `upsert_does_not_duplicate`.
- [ ] **Steps 2-5:** commit `feat: arsenal de elo alto no cliente`.

### Task 6: Seleção: sessão, feitiços, aplicação automática e lembrete

**Files:**
- Create: `src/lcu/champ_select.rs`, `src/settings.rs`
- Create: `src/lcu/poller.rs` (thread leve da LCU, mesmo padrão do `poller.rs` atual)

**Interfaces:**
- Consumes: Tasks 2–5; `knowledge::{select_entry, select_page, to_recommendations}`; `StaticSource::recommendations` (fallback Riot).
- Produces: `pub struct Settings { pub auto_runes: bool, pub auto_spells: bool, pub auto_item_set: bool }` com `load(path) -> Settings` / `save(&self, path)`; `pub struct SelectState { pub phase: Phase, pub champion_id: Option<u32>, pub locked: bool, pub position: String, pub spells: (u32, u32), pub map_mode: (u32, String) }` com `pub fn parse_session(json: &Value, my_cell: i64) -> SelectState`; `pub struct Recommendation { pub page: RunePageDraft, pub spells: (u32, u32), pub item_set: ItemSet, pub source: Source }` com `pub enum Source { HighElo, Riot }`; `pub fn recommend(...) -> Option<Recommendation>` (regras do spec: elo alto → Riot; sem posição → `ANY`; ARAM → Riot do modo; feitiços padrão por posição quando a Riot não traz: Flash + Golpear (selva), Incendiar (meio/topo), Curar (atirador), Incendiar (suporte)); `pub fn set_spells(h, a: u32, b: u32)` (PATCH `my-selection` com os campos confirmados na Task 1); `pub struct Applier` com `pub fn on_state(&mut self, s: &SelectState, settings: &Settings, user_edited: bool) -> Vec<Action>` (puro: decide o que aplicar; aplica uma vez por campeão travado); `pub fn reminder(active: &RunePageDraft, spells: (u32, u32), rec: &Recommendation, names: &KnowledgeNames) -> Vec<String>` (ex.: `"Incendiar → Exaustão"`, `"falta Golpear"`).

- [ ] **Step 1: Testes que falham:** `parses_locked_champion_and_position` (fixture da Task 1), `auto_off_applies_nothing`, `auto_on_applies_once_per_lock`, `champion_swap_reapplies_and_dodge_resets`, `auto_apply_does_not_override_user_edit`, `aram_uses_riot_recommendation`, `reminder_lists_spell_and_rune_differences`, `reminder_flags_missing_smite_in_jungle`, `settings_default_all_off_and_roundtrip`.
- [ ] **Steps 2-5:** commit `feat: aplicação automática na seleção e lembrete de runas e feitiços`.

### Task 7: Telas: seleção, editor de runas, editor de arsenais, configurações

**Files:**
- Create: `src/ui_select.rs`, `src/ui_editors.rs`
- Modify: `src/ui.rs` (troca de tela: seleção → partida → editor), `src/lib.rs`

**Interfaces:**
- Consumes: Tasks 3–6.
- Produces: tela de seleção (recomendação com origem "elo alto"/"Riot", o que foi aplicado, botões "Aplicar runas", "Aplicar feitiços", "Aplicar arsenal", lembrete em destaque); editor de runas (lista de páginas; abrir, criar, renomear, editar só com opções válidas de `RuneRules`, apagar com diálogo de confirmação, salvar grava no cliente na hora); editor de arsenais (listar, abrir, criar, renomear, editar blocos/itens com busca de item por nome, apagar com confirmação); painel de configurações com as 3 chaves.

- [ ] **Step 1: Testes que falham** (lógica de view pura, como `view.rs`): `select_screen_shows_source_and_reminder`, `editor_offers_only_valid_runes_per_slot`, `delete_requires_confirmation`, `screen_switches_select_game_editor` (fase da LCU + estado do Live Client decidem a tela).
- [ ] **Steps 2-5:** commit `feat: telas de seleção e editores de runas e arsenais`.

### Task 8: Verificação com o cliente real (arquiteto e usuário)

- [ ] **Step 1:** partida personalizada: com chaves desligadas, nada é gravado; "Aplicar" grava runas, feitiços e arsenal; a página `live-builds` aparece no cliente e as demais ficam iguais (comparar `GET /lol-perks/v1/pages` antes/depois).
- [ ] **Step 2:** ligar as 3 chaves; travar campeão → aplica sozinho uma vez; trocar campeão → reaplica; lembrete aparece com feitiço errado.
- [ ] **Step 3:** editor: criar, renomear, editar e apagar (com confirmação) uma página de teste e um arsenal de teste.
- [ ] **Step 4:** `measure.ps1` durante a seleção: CPU e RAM continuam no padrão do `docs/performance.md`.
- [ ] **Step 5:** revisão linha a linha de cada task antes do commit (`review.ps1`); fechar #7, #8, #9, #13 no quadro.
