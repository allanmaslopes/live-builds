# Design — live-builds v1: painel da partida no 2º monitor

- Data: 2026-11-06
- Repositório: `D:\Workspace\live-builds` (GitHub `allanmaslopes/live-builds`)
- Projetos relacionados: `D:\Workspace\YourSummoner-API` (Spring Boot 4), `D:\Workspace\YourSummoner-Web` (Go)

## 1. Intenção

O usuário joga League of Legends e quer um app companheiro de partida, no estilo do Blitz e do OP.GG, que leia a partida em andamento e ajude a decidir: que item fazer, quanto ouro falta para voltar à base com proveito, e em que skill pôr o ponto. A motivação central é **desempenho**: o overlay do Blitz deixa o PC lento e consome CPU e memória além do necessário. O `live-builds` tem que ser leve no PC do jogador e funcionar bem sozinho.

A v1 é o primeiro de uma sequência de projetos (seção 9). Ela entrega o painel com dados oficiais e a build recomendada pela Riot, e prova os números de desempenho numa partida de teste. A recomendação inteligente vem depois.

## 2. Objetivos da v1

- App nativo de Windows, numa **janela própria**, pensada para o **2º monitor**, enquanto o jogo roda em **tela cheia** no principal (`WindowMode=0` no `game.cfg` do usuário).
- Detecta sozinho quando uma partida começa e termina.
- **Painel completo (layout B, escolhido pelo usuário):**
  - Seu estado: campeão, nível, barra de XP até o próximo nível, ouro atual, CS, KDA, itens.
  - **Ouro e volta à base:** o que você já pode comprar com o ouro atual, e quanto falta para o próximo componente e para o próximo item completo da build.
  - **Build recomendada da Riot para o campeão,** separada em blocos (início, core, situacional) quando a fonte trouxer blocos, com o próximo item destacado e os já comprados marcados.
  - **Skills:** níveis atuais de Q/W/E/R e aviso de "ponto de skill disponível", com o lembrete do ultimate nos níveis 6/11/16. Só isso na v1 (decisão do usuário).
  - **Os 10 jogadores:** campeão, nível, KDA e **valor em itens** (soma dos preços, porque a API não dá o ouro dos outros), por time, com o total de cada time e a diferença.
  - **Confronto de rota:** você contra o oponente da mesma posição (nível e valor em itens).
- **Desempenho medido** numa partida personalizada do usuário, com números de CPU e RAM registrados no repositório.

## 3. Não-objetivos da v1

- Recomendação adaptativa à partida (motor próprio). Ex.: Thresh com vantagem indo de Mandato Imperial cedo; contra assassinos, Solari e Juramento do Cavaleiro. Isso é o projeto 3.
- Ordem de skills por campeão (precisa dos dados do projeto 2).
- Overlay por cima do jogo.
- Arena, ARAM e outros modos com regra própria (a v1 só precisa não quebrar neles; mostrar o que fizer sentido).
- Fase de seleção de campeão (runas, feitiços).
- Instalador, auto-update, login, conta, telemetria.

## 4. Anti-objetivos (falha mesmo se "funcionar")

- **Injetar código no processo do jogo, ler memória do jogo ou hookar DirectX.** O Vanguard pode punir a conta. Só APIs locais oficiais.
- **Mostrar informação que o jogo esconde do jogador** (ex.: recarga de feitiço do inimigo fora de visão, posição fora de visão). Seguir sempre as políticas da Riot.
- **Usar dados do OP.GG ou de qualquer site de terceiros por raspagem.**
- **Pesar como o Blitz:** redesenhar a tela continuamente, embutir um navegador inteiro, consumir CPU fora de partida.
- Roubar o foco do jogo (a janela não pode se colocar na frente nem pegar o foco sozinha).

## 5. Restrições

- **Fontes de dados permitidas:**
  - **Live Client Data API** do jogo: `https://127.0.0.1:2999/liveclientdata/allgamedata` (e endpoints parciais), disponível só com a partida em andamento. Certificado autoassinado da Riot (`riotgames.pem` público); o app deve aceitar só esse host/porta locais, nunca desligar TLS de forma geral.
  - **Data Dragon** para itens, preços e componentes (`item.json` do patch) e ícones. Versão do patch tirada do jogo (`gameVersion` / `versions.json`).
  - **Build recomendada pela Riot:** a fonte exata é decisão do builder após investigar (candidatas: dados de jogo do cliente via LCU `/lol-game-data/assets/v1/champions/{id}.json`, ou o espelho do Community Dragon `plugins/rcp-be-lol-game-data/global/default/v1/champions/{id}.json`). Preferir a que não exige o cliente aberto em modo especial. Se a fonte não trouxer blocos, mostrar lista simples.
- **Desempenho (metas a medir, não chutes a defender):**
  - Fora de partida: ~0% de CPU, < 30 MB de RAM.
  - Em partida: leitura da API a cada 1 s (configurável); CPU média < 1% de um núcleo; RAM < 60 MB.
  - Redesenho **só quando um dado exibido muda**; nenhum loop de animação.
  - Sem processo extra (nada de navegador embutido na v1).
- Idioma da interface e dos nomes de itens: `pt_BR` (Data Dragon no locale `pt_BR`).
- Funciona offline depois de baixar o catálogo do patch uma vez (cache em disco por patch).
- Plataforma: Windows 11 x64. Monitor do usuário: 1920×1080.

## 6. Decisões tomadas com o usuário

- App novo, repositório `live-builds`.
- Linguagem com foco em desempenho: **Rust** (sem coletor de lixo, binário único).
- Janela própria no 2º monitor, não overlay, não navegador.
- Layout **B** (painel completo), com a regra de redesenhar só quando muda.
- Começar pelo básico: Riot recomendada + estado + ouro/volta à base + aviso de skill.
- Medir desempenho numa partida personalizada **quando o arquiteto pedir** (o usuário não quer o jogo rodando à toa).
- Arquitetura de longo prazo: o servidor (YourSummoner) aprende com partidas de elo alto; o PC do jogador decide localmente (seção 9).

## 7. Decisões do arquiteto (revise)

1. **UI com `egui`/`eframe`,** com repaint sob demanda (`request_repaint` só quando o estado muda). Motivo: menor consumo entre as opções nativas, e o `egui` também compila para WebAssembly, o que mantém aberta a ideia do usuário de o YourSummoner-Web usar a mesma arquitetura de front no futuro (projeto 4). Se a medição mostrar consumo acima das metas, o plano B é Slint.
2. **Itens e preços direto do Data Dragon na v1,** com cache em disco por patch. A YourSummoner-API ainda não tem catálogo de itens (F4 do roadmap). Quando o projeto 2 publicar o pacote de conhecimento por patch, o `live-builds` passa a consumir de lá.
3. **"Comprável agora"** = itens e componentes da build recomendada (e o próximo componente do item em andamento) cujo custo restante ≤ ouro atual, usando a árvore `from`/`into` e o preço `gold.total`/`gold.base` do `item.json`, descontando componentes que você já tem.
4. **Oponente de rota** pela `position` que a API traz. Quando vier vazia (comum em partida personalizada), usar Smite para casar os junglers e, para os demais, um seletor no próprio painel (clicar no inimigo para fixá-lo como oponente), em vez de chutar.
5. **Gravação de "fotos" da partida** (o JSON do `allgamedata` a cada N segundos) atrás de uma flag, salva em disco local, para virar dado de teste e, mais tarde, do motor (projeto 3). Desligada por padrão.
6. **Fonte da build da Riot:** espelho estático do Community Dragon (`raw.communitydragon.org/<patch>/plugins/rcp-be-lol-game-data/global/default/v1/champions/{id}.json` ou o caminho equivalente que o builder confirmar), sem autenticação na LCU. A LCU é só plano B.
7. **Aviso de skill:** usa `activePlayer.abilities.{Q,W,E,R}.abilityLevel` e `activePlayer.level` da Live Client Data; pontos disponíveis = nível − soma dos níveis das skills. Leitura a cada 1 s é suficiente para um aviso.
8. **Instalação do Rust** portátil em `D:\Workspace\Languages-Frameworks` (padrão do workspace), pelo agente `ops-runner`.
9. **Delegação:** Codex implementa; Gemini lê e rascunha docs; Ollama gera boilerplate; Opus planeja e revisa.

## 8. O que fica com o builder

Estrutura de crates e módulos, cliente HTTP, parser JSON, formato do cache, visual exato (cores, fontes, ícones), como medir CPU/RAM (ex.: script PowerShell com `Get-Process` amostrando a cada segundo), nomes de arquivos, e a investigação da fonte da build recomendada pela Riot (seção 5).

## 9. Sequência combinada (fora da v1)

1. **live-builds v1** (este design).
2. **YourSummoner: coleta de partidas de elo alto** pela `match-v5` com **timeline**, guardando o conteúdo do jogo (campeões, itens e ordem, rota, resultado, patch, e a situação de ouro/nível/placar a cada compra) **sem PUUID nem Riot ID**; partidas de elo baixo são descartadas. Publica um pacote de conhecimento por patch. Segue as políticas da Riot.
3. **Motor de recomendação próprio, em repositório próprio,** que o `live-builds` usa localmente: compara a situação ao vivo com as situações em que jogadores de elo alto compraram cada item (ameaça inimiga, tipo de dano, cura, vantagem de rota). Inclui ordem de skills por campeão.
4. **Front compartilhado** entre `live-builds` e YourSummoner-Web.
5. **YourSummoner em microsserviços** Spring Boot, cada um consumindo uma parte da Riot.
6. **Arena** (o usuário joga muito; antes, checar a restrição da Riot sobre expor taxa de vitória do Arena).
