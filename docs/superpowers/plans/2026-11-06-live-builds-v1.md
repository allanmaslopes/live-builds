# live-builds v1 — Plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** App Rust para o 2º monitor que lê a partida pela Live Client Data API e mostra estado, ouro/volta à base, build recomendada da Riot, aviso de skill e os 10 jogadores, com consumo mínimo de CPU e RAM.

**Architecture:** Um binário com uma biblioteca interna. Uma thread leve faz polling da API local; quando o snapshot muda, uma função pura (`build_view`) calcula o `PanelView`, que vai por canal para a UI `egui`, redesenhada só nesse momento. Catálogo de itens (Data Dragon) e build da Riot (Community Dragon) ficam em cache em disco por patch.

**Tech Stack:** Rust stable (2024 edition), `eframe`/`egui`, `serde`/`serde_json`, `ureq` 3 com `rustls`, `rustls-pemfile` (ou equivalente) para o certificado da Riot.

**Spec:** `docs/superpowers/specs/2026-11-06-live-builds-v1-design.md`

## Global Constraints

- Ambiente Rust: `. D:\Workspace\Languages-Frameworks\rust-env.ps1` antes de qualquer `cargo`.
- Só APIs locais oficiais e arquivos estáticos públicos (Data Dragon, Community Dragon). Nada de injeção, leitura de memória, hooks ou raspagem de sites.
- Nunca mostrar informação que o jogo esconde do jogador.
- Live Client: `https://127.0.0.1:2999/liveclientdata/allgamedata`. TLS validado contra o certificado raiz público da Riot (`riotgames.pem`, embutido no binário). Nunca desligar a verificação de forma geral.
- Polling: 1 s em partida (configurável por `--poll-ms`), 5 s fora de partida.
- Redesenho só quando o `PanelView` muda (`ctx.request_repaint()` disparado pelo poller); nenhum `request_repaint_after` periódico.
- Metas: fora de partida ~0% CPU e < 30 MB RAM; em partida CPU média < 1% de um núcleo e < 60 MB RAM.
- Locale `pt_BR` para nomes de itens.
- Cache em `%LOCALAPPDATA%\live-builds\cache\<patch>\` (`item.json`, `champion-<key>.json`).
- A janela não rouba foco: sem `always_on_top`, sem `focus()` programático; lembra posição e tamanho entre execuções (persistência do `eframe`).
- `cargo clippy -- -D warnings` e `cargo fmt --check` passam ao fim de cada tarefa.
- Commits locais, sem push, terminando com `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Partida termina ou o jogo fecha no meio do polling:** o painel volta a "Aguardando partida…", sem pânico, e o polling cai para 5 s. Teste na Task 2 (`fetch_returns_none_when_connection_refused`) e na Task 5 (`poller_goes_idle_after_game_ends`).
2. **Item que não está no catálogo** (item novo, item de modo especial, upgrade do Ornn): mostrado como `Item <id>`, custo 0, sem pânico. Teste na Task 1 (`unknown_item_is_named_by_id`) e na Task 4 (`view_tolerates_unknown_items`).
3. **Patch do jogo mais novo que o Data Dragon/Community Dragon** (dia de patch): usa o último patch em cache, com aviso visível "catálogo do patch X". Teste na Task 3 (`falls_back_to_cached_patch_when_remote_missing`).
4. **Modos com mais de 2 times ou campos ausentes** (Arena com subtimes, partida personalizada sem `position`): não pânico; jogadores agrupados por `team` e, se houver, por subtime; confronto de rota vazio vira seletor. Teste na Task 4 (`view_handles_arena_snapshot`).
5. **Campeão sem build recomendada** (campeão novo, fonte fora do ar): seção mostra "Sem build recomendada da Riot para este campeão" e o resto do painel funciona. Teste na Task 3 (`missing_build_is_empty_not_error`) e na Task 4.

---

### Task 1: Projeto, catálogo de itens e custo restante

**Delegado:** Codex `gpt-5.6-terra`.

**Files:**
- Create: `Cargo.toml`, `src/main.rs` (só `fn main` chamando `live_builds::run()` no fim da Task 5; por ora imprime a versão), `src/lib.rs`, `src/catalog.rs`, `tests/fixtures/item_sample.json` (recorte real do `item.json` pt_BR com ≥ 6 itens, incluindo um item com `from` de dois níveis).
- Test: `src/catalog.rs` (`#[cfg(test)]`).

**Interfaces:**
- Produces:
  ```rust
  pub struct Item { pub id: u32, pub name: String, pub total_cost: u32, pub base_cost: u32, pub from: Vec<u32>, pub purchasable: bool }
  pub struct ItemCatalog { pub patch: String, items: HashMap<u32, Item> }
  impl ItemCatalog {
      pub fn from_ddragon_json(patch: &str, json: &str) -> Result<ItemCatalog, CatalogError>;
      pub fn get(&self, id: u32) -> Option<&Item>;
      pub fn name(&self, id: u32) -> String;              // "Item <id>" se desconhecido
      pub fn remaining_cost(&self, target: u32, owned: &[u32]) -> u32;
  }
  ```
  `remaining_cost`: custo total do alvo menos o custo total dos componentes (em qualquer nível da árvore `from`) que o jogador já tem, cada item possuído usado no máximo uma vez. Se o alvo já está em `owned`, retorna 0.

- [ ] **Step 1:** Testes: `parses_names_and_costs` (nome pt_BR e `gold.total`); `unknown_item_is_named_by_id` (`name(999999) == "Item 999999"`); `remaining_cost_subtracts_owned_components` (alvo 3000 com componente de 1100 possuído → 1900); `remaining_cost_uses_each_owned_item_once` (dois componentes iguais na receita, um possuído → desconta só um); `remaining_cost_nested_components` (componente de componente possuído é descontado); `remaining_cost_of_owned_item_is_zero`.
- [ ] **Step 2:** `cargo test catalog` → FAIL.
- [ ] **Step 3:** Implementar com `serde` lendo `data.<id>.{name, gold.total, gold.base, gold.purchasable, from}`.
- [ ] **Step 4:** `cargo test catalog` → PASS; `cargo clippy -- -D warnings`; `cargo fmt --check`.
- [ ] **Step 5:** Commit `feat: catálogo de itens do Data Dragon e custo restante`.

### Task 2: Cliente da Live Client Data API

**Delegado:** Codex `gpt-5.6-sol`.

**Files:**
- Create: `src/live/mod.rs`, `src/live/model.rs`, `src/live/tls.rs`, `assets/riotgames.pem` (baixar de `https://static.developer.riotgames.com/docs/lol/riotgames.pem`), `tests/fixtures/allgamedata_classic.json` (exemplo completo de partida normal 5×5; montar a partir da documentação oficial em `https://developer.riotgames.com/docs/lol#game-client-api_live-client-data-api`), `tests/fixtures/allgamedata_arena.json` (variante com `gameMode: "CHERRY"` e mais de 2 times/subtimes).
- Test: `src/live/*.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub struct GameSnapshot { pub game_mode: String, pub game_time: f64, pub active: ActivePlayer, pub players: Vec<PlayerInfo>, pub raw: serde_json::Value }
  pub struct ActivePlayer { pub summoner_name: String, pub level: u32, pub current_gold: f64, pub abilities: AbilityLevels }
  pub struct AbilityLevels { pub q: u32, pub w: u32, pub e: u32, pub r: u32 }
  pub struct PlayerInfo { pub summoner_name: String, pub champion_name: String, pub team: String, pub position: String, pub level: u32, pub kills: u32, pub deaths: u32, pub assists: u32, pub creep_score: u32, pub items: Vec<u32>, pub has_smite: bool, pub subteam: Option<u32> }
  pub fn parse_all_game_data(json: &str) -> Result<GameSnapshot, LiveError>;
  pub struct LiveClient { /* agent ureq configurado */ }
  impl LiveClient { pub fn new() -> LiveClient; pub fn fetch(&self) -> Result<Option<GameSnapshot>, LiveError>; }
  ```
  `fetch` devolve `Ok(None)` quando não há partida (conexão recusada, 404, ou JSON com a partida ainda carregando sem `activePlayer`). O jogador ativo em `players` é identificado por `summonerName`/`riotId` igual ao do `activePlayer`. Campos ausentes viram default (string vazia, 0, `None`), nunca erro.
  TLS: raiz = `assets/riotgames.pem` via `include_bytes!`. Se o certificado do jogo não tiver SAN para `127.0.0.1`, implementar um `ServerCertVerifier` que valida a cadeia contra essa raiz e ignora só o nome do host, e que é usado **somente** pelo `LiveClient` (comentário `// shortcut:` explicando o motivo).

- [ ] **Step 1:** Testes: `parses_classic_snapshot` (nível, ouro, Q/W/E/R, 10 jogadores, itens por jogador); `missing_fields_default` (JSON sem `position` e sem `subteam`); `parses_arena_snapshot` (subtimes preenchidos); `loading_screen_is_none` (JSON sem `activePlayer` → `parse` sinaliza "sem partida", e `fetch` → `None`); `fetch_returns_none_when_connection_refused` (porta local sem servidor → `Ok(None)`, sem esperar mais que 500 ms).
- [ ] **Step 2:** `cargo test live` → FAIL.
- [ ] **Step 3:** Implementar. Timeout de 500 ms por requisição.
- [ ] **Step 4:** `cargo test live` → PASS; clippy; fmt.
- [ ] **Step 5:** Commit `feat: cliente da Live Client Data API com TLS fixado na raiz da Riot`.

### Task 3: Fontes estáticas por patch (catálogo e build da Riot) com cache

**Delegado:** Codex `gpt-5.6-sol`.

**Files:**
- Create: `src/static_data.rs`, `src/riot_build.rs`, `tests/fixtures/cdragon_champion_sample.json` (recorte real do JSON do campeão no Community Dragon com o campo de itens recomendados).
- Test: nos próprios módulos.

**Interfaces:**
- Consumes: `ItemCatalog::from_ddragon_json` (Task 1).
- Produces:
  ```rust
  pub struct BuildBlock { pub name: String, pub items: Vec<u32> }
  pub struct RecommendedBuild { pub champion_key: u32, pub blocks: Vec<BuildBlock> } // vazio = sem build
  pub fn parse_cdragon_recommended(json: &str, map_id: u32, mode: &str) -> RecommendedBuild;
  pub struct StaticData { pub catalog: ItemCatalog, pub catalog_patch: String, pub stale: bool }
  pub struct StaticSource { cache_dir: PathBuf, fetch: Box<dyn Fn(&str) -> Result<String, String> + Send + Sync> }
  impl StaticSource {
      pub fn new(cache_dir: PathBuf) -> StaticSource;                       // fetch real via ureq
      pub fn with_fetcher(cache_dir: PathBuf, f: impl Fn(&str)->Result<String,String>+Send+Sync+'static) -> StaticSource;
      pub fn load(&self, game_version: &str) -> Result<StaticData, String>;   // patch do Data Dragon a partir da versão do jogo
      pub fn recommended_build(&self, patch: &str, champion_key: u32, map_id: u32, mode: &str) -> RecommendedBuild;
  }
  ```
  Versão do jogo `16.22.712.1234` → patch Data Dragon `16.22.1` (major.minor + `.1`; confirmar contra `versions.json` e usar a maior versão `16.22.*` listada). Mapa clássico = 11. Blocos: se a fonte trouxer blocos nomeados, usar os nomes; senão um bloco "Recomendados".

- [ ] **Step 1:** Testes (todos com `with_fetcher` e diretório temporário): `maps_game_version_to_ddragon_patch`; `caches_catalog_on_disk` (segunda `load` não chama o fetcher); `falls_back_to_cached_patch_when_remote_missing` (remoto 404 para o patch novo, cache do anterior existe → `stale = true`, `catalog_patch` = anterior); `parses_recommended_blocks`; `missing_build_is_empty_not_error` (404 do Community Dragon → `blocks` vazio).
- [ ] **Step 2:** `cargo test static_data riot_build` → FAIL.
- [ ] **Step 3:** Implementar. Escrita no cache atômica (grava em `.tmp` e renomeia). URLs:
  - `https://ddragon.leagueoflegends.com/api/versions.json`
  - `https://ddragon.leagueoflegends.com/cdn/<patch>/data/pt_BR/item.json`
  - Community Dragon: confirmar o caminho do JSON do campeão com itens recomendados (ex.: `https://raw.communitydragon.org/<patch-curto>/plugins/rcp-be-lol-game-data/global/default/v1/champions/<key>.json`, com fallback para `latest`), e registrar o caminho escolhido em comentário.
- [ ] **Step 4:** PASS; clippy; fmt.
- [ ] **Step 5:** Commit `feat: catálogo e build recomendada da Riot por patch com cache em disco`.

### Task 4: Modelo do painel (`build_view`)

**Delegado:** Codex `gpt-5.6-sol`.

**Files:**
- Create: `src/view.rs`.
- Test: `src/view.rs` usando os fixtures das Tasks 1–3.

**Interfaces:**
- Consumes: `GameSnapshot` (Task 2), `ItemCatalog` (Task 1), `RecommendedBuild` (Task 3).
- Produces:
  ```rust
  #[derive(Clone, PartialEq)] pub struct PanelView {
      pub me: MeView, pub build: BuildView, pub skills: SkillView,
      pub teams: Vec<TeamView>, pub lane: Option<LaneView>, pub catalog_note: Option<String>,
  }
  pub struct MeView { pub champion: String, pub level: u32, pub xp_fraction: Option<f32>, pub gold: u32, pub cs: u32, pub kda: (u32,u32,u32), pub items: Vec<String> }
  pub struct BuildView { pub blocks: Vec<BlockView>, pub next_item: Option<NextItem>, pub affordable_now: Vec<String>, pub empty_message: Option<String> }
  pub struct BlockView { pub name: String, pub items: Vec<(String, bool /*já tem*/)> }
  pub struct NextItem { pub name: String, pub missing_gold: u32, pub next_component: Option<(String, u32 /*falta*/)> }
  pub struct SkillView { pub q: u32, pub w: u32, pub e: u32, pub r: u32, pub points_available: u32, pub ult_hint: bool }
  pub struct TeamView { pub label: String, pub items_value: u32, pub players: Vec<PlayerRow> }
  pub struct PlayerRow { pub champion: String, pub level: u32, pub kda: (u32,u32,u32), pub items_value: u32, pub is_me: bool }
  pub struct LaneView { pub opponent: String, pub level_diff: i32, pub items_value_diff: i32 }
  pub fn build_view(s: &GameSnapshot, c: &ItemCatalog, b: &RecommendedBuild, lane_pick: Option<&str>, catalog_note: Option<String>) -> PanelView;
  ```
  Regras:
  - `next_item` = primeiro item dos blocos (na ordem, pulando o bloco de início se você já tem algum item dele) que você ainda não tem; `missing_gold = remaining_cost − ouro atual` (mín. 0); `next_component` = componente mais barato ainda não possuído desse item, com o quanto falta para ele.
  - `affordable_now` = itens da build e componentes do `next_item` com `remaining_cost ≤ ouro` e `purchasable`.
  - `points_available = level − (q+w+e+r)` (mín. 0); `ult_hint = points_available > 0 && level ∈ {6, 11, 16}`.
  - `items_value` = soma de `total_cost` dos itens; times ordenados com o seu time primeiro.
  - `xp_fraction`: `None` se a API não der XP (não inventar).
  - `lane`: pela `position` igual; se vazia, `lane_pick`; se o jogador tiver Smite, o inimigo com Smite; senão `None`.

- [ ] **Step 1:** Testes: `next_item_skips_owned_and_starter`; `missing_gold_and_next_component`; `affordable_now_respects_gold`; `skill_points_and_ult_hint` (nível 6 com Q2 W1 E2 R0 → 1 ponto, `ult_hint`); `team_values_and_my_team_first`; `lane_by_position_then_pick_then_smite`; `view_tolerates_unknown_items`; `view_handles_arena_snapshot` (mais de 2 times, sem pânico, `lane = None`); `empty_build_shows_message`; `same_snapshot_gives_equal_view` (duas chamadas → `==`, base da detecção de mudança).
- [ ] **Step 2:** `cargo test view` → FAIL.
- [ ] **Step 3:** Implementar como funções puras, sem I/O.
- [ ] **Step 4:** PASS; clippy; fmt.
- [ ] **Step 5:** Commit `feat: modelo do painel com build, ouro, skills, times e rota`.

### Task 5: Poller, UI egui e gravação de snapshots

**Delegado:** Codex `gpt-5.6-sol`.

**Files:**
- Create: `src/poller.rs`, `src/ui.rs`, `src/record.rs`; Modify: `src/lib.rs` (`pub fn run() -> eframe::Result`), `src/main.rs`.
- Test: `src/poller.rs`, `src/record.rs`.

**Interfaces:**
- Consumes: `LiveClient`, `StaticSource`, `build_view`.
- Produces:
  ```rust
  pub enum PanelState { Waiting, InGame(PanelView), Error(String) }
  pub trait SnapshotSource: Send { fn fetch(&self) -> Result<Option<GameSnapshot>, LiveError>; }  // implementado por LiveClient
  pub struct Poller { /* … */ }
  impl Poller { pub fn spawn(src: Box<dyn SnapshotSource>, statics: StaticSource, on_change: impl Fn(PanelState) + Send + 'static, cfg: PollConfig) -> Poller; }
  pub struct PollConfig { pub in_game: Duration, pub idle: Duration, pub record_dir: Option<PathBuf> }
  ```
  - `on_change` só é chamado quando o `PanelState` novo difere do anterior; na UI ele guarda o estado e chama `ctx.request_repaint()`.
  - Estáticos carregados uma vez por partida (e por campeão), nunca a cada tique.
  - `--record <dir>`: grava o `raw` do snapshot a cada 10 s de jogo em `<dir>/<gameStart>/<gameTime>.json`.
  - UI: layout B do spec (seu estado à esquerda, build e ouro no centro, times à direita, rota embaixo; clique num inimigo fixa o `lane_pick`). Tema escuro simples, sem animações. Título "live-builds". Tamanho inicial 1280×720; posição/tamanho persistidos pelo `eframe`.
  - CLI: `--poll-ms <n>` (padrão 1000), `--record <dir>`.

- [ ] **Step 1:** Testes do poller com `SnapshotSource` falso e `StaticSource::with_fetcher`: `emits_only_on_change` (3 snapshots iguais → 1 chamada); `poller_goes_idle_after_game_ends` (snapshot → `None` → estado `Waiting` e próximo intervalo = `idle`); `statics_loaded_once_per_game`; `record_writes_every_10s_of_game_time`.
- [ ] **Step 2:** `cargo test poller record` → FAIL.
- [ ] **Step 3:** Implementar. O poller roda numa `std::thread` e dorme entre tiques (`thread::sleep`), sem runtime async.
- [ ] **Step 4:** PASS; `cargo build --release`; abrir o app sem o jogo e ver "Aguardando partida…"; clippy; fmt.
- [ ] **Step 5:** Commit `feat: poller com redesenho sob demanda, UI egui e gravação de snapshots`.

### Task 6: Medição de desempenho e documentação

**Delegado:** Gemini (README) + Codex `gpt-5.6-luna` (script).

**Files:**
- Create: `tools/measure.ps1`, `docs/performance.md`; Modify: `README.md`.

- [ ] **Step 1:** `tools/measure.ps1 -Process live-builds -Seconds 120 -Out <csv>`: amostra a cada 1 s `CPU` (delta de `TotalProcessorTime` ÷ núcleos), `WorkingSet64` e `PrivateMemorySize64`, e imprime média, p95 e máximo. Opcional `-Compare <nome>` para medir outro processo (ex.: Blitz) na mesma janela.
- [ ] **Step 2:** Rodar com o app aberto fora de partida por 60 s e registrar em `docs/performance.md` (tabela: cenário, CPU média/p95, RAM média/máx, data, versão).
- [ ] **Step 3:** `README.md`: o que é, o que lê (só APIs locais oficiais), o que nunca faz (injeção, info escondida), como compilar (`rust-env.ps1`, `cargo build --release`), como rodar no 2º monitor com o jogo em tela cheia, flags, onde fica o cache.
- [ ] **Step 4:** Commit `docs: medição de desempenho e README`.

### Task 7: Partida de teste com o usuário

**Delegado:** Opus coordena; o usuário inicia a partida personalizada **quando o arquiteto pedir**.

- [ ] **Step 1:** Com o app em `--record`, pedir ao usuário uma partida personalizada (Summoner's Rift, contra bots), 10–15 min.
- [ ] **Step 2:** Rodar `tools/measure.ps1` durante a partida; registrar os números em `docs/performance.md` e comparar com as metas.
- [ ] **Step 3:** Conferir no painel: ouro, próximo item, comprável agora, aviso de skill, times e rota (com o seletor, já que a partida personalizada tende a vir sem `position`).
- [ ] **Step 4:** Copiar 3–5 snapshots gravados para `tests/fixtures/recorded/` e acrescentar um teste `build_view` que roda sobre todos sem pânico.
- [ ] **Step 5:** Commit `test: snapshots reais da partida de teste e números de desempenho`.
