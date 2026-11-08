# LCU: API local do cliente do League of Legends

Pesquisa para a issue [#6](https://github.com/allanmaslopes/live-builds/issues/6). Antes da issue #4 (runas, pick e ban).

Data da pesquisa: 2026-11-07.

## Resumo

- A LCU é a API HTTPS local do **cliente** (`LeagueClientUx`), não do jogo.
- A Riot diz que a LCU **não é suportada oficialmente** para apps de terceiros. Não há garantia de documentação, uptime ou aviso de mudança.
- Leitura e escrita de runas, feitiços e item sets são os caminhos mais claros. Ações automáticas (aceitar, pick/ban automático, dodge) são o ponto de risco.
- A política geral da Riot proíbe apps que **removem decisões do jogador**. Por isso, o caminho conservador é: o app só age quando o usuário clica.
- Os caminhos abaixo vieram de comunidade e de bibliotecas. Não há documentação oficial completa. Itens sem confirmação estão marcados como "(não confirmado)".

---

## 1. Conexão

### 1.1 Lockfile

O cliente grava um arquivo `lockfile` na pasta de instalação enquanto está aberto. Ele some ao fechar o cliente.

- Caminho típico no Windows: `C:\Riot Games\League of Legends\lockfile`. O caminho pode mudar com a instalação.
- Formato: uma linha, campos separados por `:`.

| Campo | Posição | Uso |
|---|---|---|
| nome | 1 | nome do processo (`LeagueClient`) |
| PID | 2 | ID do processo |
| porta | 3 | porta HTTPS local |
| senha | 4 | senha da autenticação Basic |
| protocolo | 5 | `https` (ou `wss` para o socket) |

Fonte: [hextechdocs, Getting started](https://hextechdocs.dev/getting-started-with-the-lcu-api/); [league-link (docs.rs)](https://docs.rs/league-link/0.1.0/src/league_link/auth.rs.html).

A porta e a senha mudam a cada reinício do cliente. O app deve reler o lockfile sempre que a conexão falhar.

### 1.2 Autenticação

- Método: HTTP Basic.
- Usuário fixo: `riot`.
- Senha: campo 4 do lockfile.
- Cabeçalho: `Authorization: Basic base64("riot:<senha>")`.
- URL base: `https://127.0.0.1:<porta>`.

Fonte: [hextechdocs, Getting started](https://hextechdocs.dev/getting-started-with-the-lcu-api/); [league-link (docs.rs)](https://docs.rs/league-link/0.1.0/src/league_link/auth.rs.html).

### 1.3 TLS

- O certificado é autoassinado.
- Opção recomendada: confiar só no certificado raiz da Riot (`riotgames.pem`), fixado no projeto.
- Não desligar a verificação TLS em código de produção.

Fonte: [hextechdocs, Getting started](https://hextechdocs.dev/getting-started-with-the-lcu-api/); [lcu-mcp](https://glama.ai/mcp/servers/Triggered0/lcu-mcp) (usa o CA da Riot fixado).

### 1.4 Alternativa: linha de comando do processo

Quando o lockfile não existe, a porta e a senha podem vir dos argumentos do processo `LeagueClientUx.exe`:

- `--app-port=<porta>`
- `--remoting-auth-token=<senha>`

No Windows, a consulta pode ser feita com `wmic`. Em Rust, o crate `irelia` faz essa leitura.

Fonte: [hextechdocs, Getting started](https://hextechdocs.dev/getting-started-with-the-lcu-api/); [irelia (process_info.rs)](https://docs.rs/crate/irelia/latest/source/src/utils/process_info.rs).

O app deve tratar a senha como segredo. Não logar, não salvar em disco e não mostrar em erro.

### 1.5 Eventos em tempo real (WebSocket, WAMP 1.0)

1. Obter a URI do socket. O plugin do cliente expõe o link `riot:plugins:websocket`. Fora do cliente, a URI é `wss://127.0.0.1:<porta>` (não confirmado para Rust: verificar se precisa de cabeçalho `Authorization` no handshake).
2. Conectar com o subprotocolo `wamp`.
3. Ao abrir, enviar `[5, "OnJsonApiEvent"]` para assinar todas as chamadas.
4. Para um caminho específico, trocar `/` por `_` no nome do evento. Exemplo: `OnJsonApiEvent_lol-gameflow_v1_gameflow-phase`.
5. Para cancelar: `[6, "<evento>"]`.

Fonte: [Pengu, LCU request](https://pengu-dev.pages.dev/guide/lcu-request). Não encontrei esse formato na documentação do hextechdocs nem na do riot-api-libraries.

---

## 2. Inventário por grupo

Legenda: **L** = leitura (GET). **E** = escrita. Fonte: "2+" = pelo menos duas fontes. "1" = uma fonte, ver a coluna Fonte.

### 2.1 Seleção de campeões (champ-select)

| Método | Caminho | L/E | Para que serve no live-builds | Fonte |
|---|---|---|---|---|
| GET | `/lol-champ-select/v1/session` | L | estado da seleção: meu time, ações, fase | 1 (gist) + lcu-mcp |
| GET | `/lol-champ-select/v1/session/timer` | L | mostrar o tempo restante | 1 (gist) |
| PATCH | `/lol-champ-select/v1/session/actions/{id}` | E | hover ou pick/ban de uma ação | 2+ (gist, lcu-mcp) |
| POST | `/lol-champ-select/v1/session/actions/{id}/complete` | E | confirmar (travar) a ação | 1 (gist) |
| PATCH | `/lol-champ-select/v1/session/my-selection` | E | definir feitiços e skin | 1 (gist) |
| POST | `/lol-champ-select/v1/session/my-selection/reroll` | E | rolar de novo (modo ARAM) | 1 (gist) |
| GET | `/lol-champ-select/v1/session/trades` | L | listar trocas de campeão | 1 (gist) |
| POST | `/lol-champ-select/v1/session/trades/{id}/request` | E | pedir troca | 1 (gist) |
| POST | `/lol-champ-select/v1/session/trades/{id}/accept` | E | aceitar troca | 1 (gist) |

Corpo de `my-selection`: a referência de terceiros diz que há três inteiros (skin e dois feitiços). O nome do campo de skin seria `selectedSkinId`. Os nomes dos campos dos feitiços: (não confirmado).

Fonte do corpo: [hasagi-types (swagger)](https://raw.githubusercontent.com/dysolix/hasagi-types/main/swagger.json) não confirmou os campos. Validar com o cliente real antes de usar.

Caminhos alternativos citados pelo gist: `/lol-champ-select-legacy/v1` e `/lol-lobby-team-builder/champ-select/v1`. Não usar sem teste.

### 2.2 Runas (perks)

| Método | Caminho | L/E | Para que serve no live-builds | Fonte |
|---|---|---|---|---|
| GET | `/lol-perks/v1/pages` | L | listar páginas do jogador | 2+ (swagger, gist) |
| POST | `/lol-perks/v1/pages` | E | criar página | 2+ (swagger, lcu-mcp) |
| GET | `/lol-perks/v1/pages/{id}` | L | ler uma página | 2+ (swagger, gist) |
| PUT | `/lol-perks/v1/pages/{id}` | E | editar página | 2+ (swagger, lcu-mcp) |
| DELETE | `/lol-perks/v1/pages/{id}` | E | apagar página | 2+ (swagger, gist) |
| GET | `/lol-perks/v1/currentpage` | L | página ativa | 1 (gist) |
| PUT | `/lol-perks/v1/currentpage` | E | ativar página | 1 (gist) |
| GET | `/lol-perks/v1/styles` | L | estilos (árvores de runas) | 1 (gist) |
| GET | `/lol-perks/v1/perks` | L | runas disponíveis | 1 (gist) |
| GET | `/lol-perks/v1/customizationlimits` | L | limites de customização | 1 (gist) |

Limite de páginas de runas: nenhum endpoint de limite foi confirmado. O campo `customizationlimits` pode ter a informação, mas não foi verificado. **(não confirmado)**

Escrita em runas aparece no lcu-mcp como precisando de entrada explícita na allowlist de escrita.

### 2.3 Feitiços de invocador

Os feitiços entram por `my-selection`, na seção 2.1. Não há endpoint separado confirmado.

### 2.4 Conjuntos de itens (item sets)

| Método | Caminho | L/E | Para que serve no live-builds | Fonte |
|---|---|---|---|---|
| GET | `/lol-item-sets/v1/item-sets/{summonerId}/sets` | L | ler item sets do jogador | 1 (gist) |
| POST | `/lol-item-sets/v1/item-sets/{summonerId}/sets` | E | criar item set | 1 (gist) |
| PUT | `/lol-item-sets/v1/item-sets/{summonerId}/sets` | E | substituir item sets | 1 (gist) |

O swagger (hasagi-types) não confirmou esse caminho. Também existe `/lol-loadouts/v1/...` no gist, que parece ser o sistema novo. Validar antes de implementar. **Não confirmado no swagger.**

### 2.5 Fila e partida

| Método | Caminho | L/E | Para que serve no live-builds | Fonte |
|---|---|---|---|---|
| GET | `/lol-gameflow/v1/gameflow-phase` | L | fase atual (Lobby, ChampSelect, InProgress etc.) | 2+ (Pengu, lcu-mcp) |
| GET | `/lol-gameflow/v1/session` | L | sessão de jogo | 1 (gist) |
| GET | `/lol-matchmaking/v1/ready-check` | L | estado do "partida encontrada" | 1 (gist) |
| POST | `/lol-matchmaking/v1/ready-check/accept` | E | aceitar partida | 3 (Pengu, lcu-mcp, gist) |
| POST | `/lol-matchmaking/v1/ready-check/decline` | E | recusar partida | 1 (gist) |
| POST | `/lol-gameflow/v1/session/dodge` | E | sair da seleção (dodge) | 1 (gist) |
| GET | `/lol-lobby/v2/lobby` | L | estado do lobby | 1 (gist) |
| POST | `/lol-lobby/v2/lobby/matchmaking/search` | E | iniciar busca | 2+ (gist, lcu-mcp) |
| DELETE | `/lol-lobby/v2/lobby/matchmaking/search` | E | cancelar busca | 1 (gist) |

Ordem das fases (fonte: referência de skill em search): None, Lobby, Matchmaking, ReadyCheck, ChampSelect, GameStart, InProgress, WaitingForStats, PreEndOfGame, EndOfGame.

### 2.6 Jogador e úteis

| Método | Caminho | L/E | Para que serve no live-builds | Fonte |
|---|---|---|---|---|
| GET | `/lol-summoner/v1/current-summoner` | L | nome, nível e `summonerId` do jogador | 3 (Pengu, gist, lcu-mcp) |
| GET | `/lol-summoner/v1/summoners/{id}` | L | dados de outro invocador | 1 (gist) |
| GET | maestrias de campeão (caminho) | L | maestria por campeão | (não confirmado) |
| GET | histórico local de partidas (caminho) | L | histórico recente | (não confirmado) |

Maestria e histórico: o swagger citado tem `reward-grants` de maestria, mas não o caminho de leitura. **(não confirmado)** para os dois.

### 2.7 Contagem

- Caminhos marcados como fonte única (gist) são menos seguros. Validar no cliente antes de usar.
- Itens "(não confirmado)": 8 (ver lista no fim).

---

## 3. Política da Riot

### 3.1 O que a Riot diz sobre a LCU

Página oficial da LCU ([developer.riotgames.com/league-client-apis.html](https://developer.riotgames.com/league-client-apis.html)):

- A LCU "não é suportada oficialmente para uso com aplicações de terceiros".
- A Riot não dá "garantias de documentação completa, uptime ou comunicação de mudanças".
- Qualquer uso deve ser comunicado: criar um app novo no portal ou deixar nota num app existente.
- Informar quais endpoints são usados e como.
- Apps não podem agir como "data broker" entre a API da Riot e outra empresa terceira.
- Overlays só podem mostrar dados estáticos, antes da partida. Não podem mostrar informação da sessão de jogo que o jogador não tem.
- Não é permitido identificar jogadores que o jogo oculta de propósito.

Não há lista de endpoints aprovados nessa página. A página não traz data de atualização.

Posts da DevRel citados em busca: [changes to the LCU API policy](https://www.riotgames.com/en/DevRel/changes-to-the-lcu-api-policy) e [new League Client API policy](https://riotgames.com/en/DevRel/new-league-client-api-policy). Não abri o conteúdo completo. A busca indicou que a Riot exige contato antes de lançar app com LCU e uma lista de endpoints aprovados. **Não confirmado** pelo texto oficial. O post de 2018 ([league-client-apis-policy-update](https://riotgames.com/en/DevRel/league-client-apis-policy-update)) não traz essas regras.

### 3.2 Regras gerais de produto

Fonte: [developer.riotgames.com/policies/general](https://developer.riotgames.com/policies/general) (atualizada em 11/03/2025 segundo a busca).

- Todo produto deve ser registrado e auditado pela Riot. Mudanças também passam por auditoria.
- Produtos que usam a League Client API devem ler a documentação e registrar o produto.
- É proibido criar vantagem injusta, incluindo "programa de trapaça".
- É proibido **remover decisões do jogador**. O produto pode destacar e oferecer opções, mas não substituir a escolha.
- É proibido criar alternativas a rankings oficiais.
- Usar HTTPS. Não embutir chave de API em binário distribuído.
- Exibir aviso de não endosso.
- Políticas mudam. O desenvolvedor deve se adequar "assim que surgirem".

### 3.3 O que é aceito ou proibido

Esta tabela é uma leitura conservadora das regras acima. Não é parecer jurídico. Não há regra explícita da Riot sobre cada caso.

| Ação | Situação | Leitura |
|---|---|---|
| Importar runas recomendadas | o usuário clica e confirma | aceitável; ação do usuário |
| Importar item set | o usuário clica e confirma | aceitável; ação do usuário |
| Aplicar feitiços | o usuário clica na seleção | aceitável; ação do usuário |
| Mostrar fase e timer | só leitura | baixo risco |
| Pick/ban assistido | o usuário clica, o app executa | risco médio: o app age no lugar do jogador |
| Pick/ban automático | sem clique | risco alto: remove decisão do jogador |
| Aceitar partida automaticamente | sem clique | risco alto: remove decisão do jogador |
| Dodge automático | sem clique | risco alto: remove decisão e pode ser visto como abuso |

Sobre ferramentas citadas na issue (Blitz, Porofessor, Mobalytics): **não verifiquei** o que cada uma faz. Não usar como prova de que a ação é permitida.

### 3.4 Recomendação

- Ficar só com ações iniciadas pelo usuário.
- Não implementar aceitar automático, pick/ban automático nem dodge automático.
- Antes de qualquer release, registrar o app no portal da Riot e listar os endpoints usados, como a página pede.
- Se houver dúvida sobre pick/ban assistido, perguntar à Riot pelo Developer Discord antes de lançar.

---

## 4. Proposta de funcionalidades

Ordem por valor e risco. Esforço: P (pequeno), M (médio), G (grande).

| # | Funcionalidade | Endpoints | Exige clique do usuário? | Risco de política | Esforço |
|---|---|---|---|---|---|
| d | Mostrar fase da seleção e timer | `gameflow-phase`, `champ-select/session`, `session/timer`, evento WebSocket | não (só leitura) | baixo | P |
| a | Importar página de runas recomendada com 1 clique | `perks/pages` (POST/PUT), `perks/currentpage` (PUT) | sim | baixo a médio | M |
| b | Aplicar feitiços de invocador | `champ-select/session/my-selection` (PATCH) | sim | baixo | P |
| c | Importar item set | `item-sets/{summonerId}/sets` (POST/PUT) | sim | baixo a médio | M |
| e | Pick/ban assistido (usuário clica, app executa) | `session/actions/{id}` (PATCH), `.../complete` | sim | médio | M |
| e' | Pick/ban automático | mesmos | não | alto | M |
| f | Aceitar partida com 1 clique | `ready-check/accept` | sim | médio | P |
| f' | Aceitar partida automaticamente | `ready-check/accept` | não | alto | P |

Recomendação para a primeira entrega: **(d)**, **(a)** e **(b)**.

- (d) não escreve nada. Testa a conexão, o lockfile e o WebSocket com risco mínimo.
- (a) é o maior valor para o usuário. Exige validar o limite de páginas e os campos antes.
- (b) é pequeno, mas o corpo do PATCH não foi confirmado. Testar no cliente real antes de liberar.

Itens (c), (e) e (f) ficam para depois. Item set tem caminho de fonte única. Pick/ban e aceitar têm risco de política mais alto.

---

## 5. Riscos técnicos

| Risco | Efeito | Mitigação |
|---|---|---|
| Cliente atualiza e muda caminhos | chamadas quebram | isolar os caminhos em um módulo; teste de fumaça por versão; tratar 404 com aviso |
| Riot removeu o swagger oficial da LCU | não há contrato para validar campos | usar o swagger comunitário só como referência e validar no cliente real (fonte: [lcu-schema](https://github.com/MingweiSamuel/lcu-schema)) |
| Porta e senha mudam a cada reinício | conexão some | reler o lockfile em cada falha; reconectar com backoff |
| Múltiplas instâncias do cliente | lockfile ambíguo | o flag `--allow-multiple-clients` existe ([riot-api-libraries](https://riot-api-libraries.readthedocs.io/en/latest/lcu.html)); comportamento com duas instâncias **(não confirmado)** |
| Vanguard | o Vanguard protege o jogo, não o cliente. A LCU é um processo separado. Não há injeção nem leitura de memória do jogo. | **(não confirmado)** que o Vanguard não reage à API local; nenhuma fonte cobriu o tema |
| Bloqueio de conta por automação | suspensão ou ban | **(não confirmado)** a probabilidade; manter só ações do usuário |
| Senha exposta | acesso à API local por outro processo | não logar; não salvar; não mostrar em erro |
| TLS | erro de certificado | fixar `riotgames.pem`; não desligar verificação |

Sobre o Vanguard: a LCU é o cliente do League, não o jogo. O live-builds não injeta código e não lê memória. Ele só faz chamadas HTTPS ao cliente, como o próprio cliente faz. Isso é a regra de projeto proposta, não uma garantia da Riot.

Overlay e dados da partida: o app já usa a Live Client Data API em `127.0.0.1:2999`. A política de overlay da Riot só aceita dado estático antes da partida. Não exibir na LCU informação de sessão que o jogador não tem.

---

## 6. Fontes

Oficiais (Riot):
- https://developer.riotgames.com/league-client-apis.html
- https://developer.riotgames.com/policies/general
- https://riotgames.com/en/DevRel/league-client-apis-policy-update (2018)
- https://www.riotgames.com/en/DevRel/changes-to-the-lcu-api-policy (só busca)
- https://riotgames.com/en/DevRel/new-league-client-api-policy (só busca)

Comunidade e bibliotecas:
- https://hextechdocs.dev/getting-started-with-the-lcu-api/
- https://hextechdocs.dev/lcu-api-faq/
- https://riot-api-libraries.readthedocs.io/en/latest/lcu.html
- https://docs.rs/league-link/0.1.0/src/league_link/auth.rs.html
- https://docs.rs/crate/irelia/latest/source/src/utils/process_info.rs
- https://pengu-dev.pages.dev/guide/lcu-request
- https://gist.github.com/thgiang/90c90c75187c318baddf11cfe92796e6
- https://github.com/Triggered0/lcu-mcp (via https://glama.ai/mcp/servers/Triggered0/lcu-mcp)
- https://github.com/dysolix/hasagi-types
- https://raw.githubusercontent.com/dysolix/hasagi-types/main/swagger.json (lido parcialmente)
- https://github.com/MingweiSamuel/lcu-schema
- https://github.com/hoocs151/autoaccept (só busca; referência de auto-accept, não usado como prova)
- https://lolnow.gg/riot-cracks-down-on-overlays-in-league-of-legends-a-long-overdue-clean-up/ (artigo de 2025, fonte secundária)

---

## Itens "(não confirmado)"

1. Caminho de leitura de maestrias.
2. Caminho de histórico local de partidas.
3. Limite de páginas de runas.
4. Nomes dos campos dos feitiços no corpo de `my-selection`.
5. Formato do handshake WebSocket fora do cliente (cabeçalho `Authorization` em Rust).
6. Comportamento com múltiplas instâncias do cliente.
7. Comportamento do Vanguard perante a API local.
8. Probabilidade de bloqueio de conta por uso da API local.

## 6. Confirmado pela sonda (2026-11-07, cliente real, patch 16.22)

Sonda: `cargo run --example lcu_probe -- --lcu-lockfile <caminho>` (só GET). Fixtures anonimizadas em `tests/fixtures/lcu/`.

- **Lockfile:** a instalação pode não estar em `C:\Riot Games`. No PC do usuário: `D:\Riot Games\League of Legends\lockfile`. O registro do Windows (`Uninstall\...\League of Legends` → `InstallLocation`) e o caminho do processo `LeagueClientUx.exe` dão a pasta; o app deve procurar por eles antes do padrão.
- **Fase:** `GET /lol-gameflow/v1/gameflow-phase` → `"Lobby"`, `"ChampSelect"`, … (string JSON).
- **Sessão da seleção:** `GET /lol-champ-select/v1/session` → 404 `"No active delegate"` fora da seleção. Na seleção: `localPlayerCellId`; em `myTeam[]` o jogador com esse `cellId` tem `assignedPosition` (minúsculas: `top`, `jungle`, `middle`, `bottom`, `utility`; vazio em escolha às cegas), `championId`, `championPickIntent`, `spell1Id`, `spell2Id`. Em `actions[][]`, a ação `type: "pick"` do `actorCellId` vira `completed: true` com o `championId` ao travar. `timer.phase` = `BAN_PICK` durante bans e picks. Partida personalizada: `isCustomGame: true`.
- **Feitiços:** campos `spell1Id` e `spell2Id` (confirmados na sessão; escrita via `PATCH /lol-champ-select/v1/session/my-selection` com esses nomes — validar a escrita na Task 8).
- **Runas:** `GET /lol-perks/v1/pages` (campos `id`, `name`, `isEditable`, `isDeletable`, `isActive`, `current`, `primaryStyleId`, `subStyleId`, `selectedPerkIds` [6 runas + 3 fragmentos], `isTemporary`, `isRecommendationOverride`, …); `GET /lol-perks/v1/currentpage` igual a uma página. `GET /lol-perks/v1/styles` traz as árvores e slots.
- **Limite de páginas:** `/lol-perks/v1/customizationlimits` não existe (404). Usar `GET /lol-perks/v1/inventory` → `{canAddCustomPage, customPageCount, isCustomPageCreationUnlocked, ownedPageCount}`.
- **Arsenais:** `GET /lol-item-sets/v1/item-sets/{summonerId}/sets` funciona → `{accountId, itemSets[], timestamp}`; cada set: `uid`, `title`, `associatedChampions[]`, `associatedMaps[]`, `blocks[]`, `map`, `mode`, `type` (`custom`), `sortrank`, `startedFrom`, `preferredItemSlots`. `lol-loadouts` também responde, mas não é necessário para item sets.
- **Convivência:** o usuário tem o Blitz importando runas (página "Blitz: …" ativa) e arsenais do U.GG/OP.GG. O app não toca neles; com dois importadores automáticos ligados, um pode desfazer o outro.
