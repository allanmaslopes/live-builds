# Seleção de campeões pela LCU — design (entrega 1)

Data: 2026-11-07. Repositório: `live-builds`. Issues: #7 (flags), #8 (editor de runas), #9 (sugestão de runas e feitiços), #13 (arsenais). Fica fora: #4 (pick/ban pelo app).

## Por que

O usuário se irrita com três coisas na seleção de campeões: salvar runas manualmente, não saber o que mudar na página para render melhor, e não lembrar de conferir feitiços e runas antes de a partida carregar. O cliente da Riot também costuma travar na tela de runas. Esta entrega resolve isso; a sugestão de pick pelo que o time precisa é a entrega 3.

Ordem acordada: (1) esta entrega — aplicar runas, feitiços e arsenal, editor de runas e de arsenais, lembrete; (2) explicar cada troca de runa (partidas e vitória de elo alto por runa); (3) sugerir campeão pelo que o time precisa (iniciar luta, controle, linha de frente, dano mágico).

## Objetivos

1. **Aplicar** no cliente, para o campeão travado e a posição: a página de runas recomendada, os dois feitiços e o arsenal (item set), vindos do pacote de elo alto já usado no painel (live-builds#1; mesma escolha de entrada e página). Sem dados de elo alto: recomendação oficial da Riot (Community Dragon), como no painel.
2. **Modo misto, decidido pelo usuário:** cada importação (runas, feitiços, arsenal) tem sua chave nas configurações; ligada, aplica sozinha ao travar o campeão; desligada, a tela mostra a sugestão com botão "Aplicar". Padrão: tudo desligado até o usuário ligar (#7). As chaves ficam salvas localmente.
3. **Sempre editável:** depois de aplicar, o usuário altera no editor do app e o app grava de novo no cliente na hora.
4. **Página dedicada:** o app cria e mantém uma página de runas chamada `live-builds` e não toca em nenhuma outra página ao aplicar. Se o cliente estiver no limite de páginas, o app não apaga nada: avisa e pede para o usuário liberar uma.
5. **Editor de runas completo:** lista todas as páginas do jogador; abre, cria, renomeia, edita (árvores, 6 runas, 3 fragmentos) e apaga. Só permite combinações válidas (runa da árvore e do slot certos; secundária de outra árvore; fragmentos por slot). Apagar pede confirmação no app (o cliente não desfaz).
6. **Editor de arsenais completo:** mesma coisa para item sets: listar, abrir, criar, renomear, editar (blocos e itens), apagar com confirmação. O arsenal de elo alto aplicado chama-se `live-builds — <campeão> <posição>` e é atualizado no lugar, sem duplicar.
7. **Lembrete:** antes de a partida carregar (fase final da seleção), se a página ativa ou os feitiços não baterem com o recomendado para o campeão e a posição, o app mostra o aviso "Confira feitiços e runas", com o que difere (ex.: "Incendiar → Exaustão", "falta Golpear" na selva). Vale mesmo com as importações desligadas.
8. **Janela:** durante a seleção, o app mostra a tela de seleção (sugestão, editor, lembrete); quando a partida começa, volta ao painel da partida; fora das duas, mostra o editor (runas e arsenais) se o cliente estiver aberto.

## Não objetivos

- Escolher, banir, travar campeão ou aceitar partida pelo app (#4).
- Explicar o porquê de cada runa com estatística por troca (entrega 2) e sugerir pick pela composição (entrega 3).
- Qualquer coisa dentro do jogo: o app só fala com a LCU (cliente) e a Live Client Data API, nunca com o processo do jogo.

## Anti-objetivos (seria falha mesmo funcionando)

- Apagar ou sobrescrever página de runas ou arsenal do usuário sem ele pedir (só a página `live-builds` e os arsenais `live-builds — …` são do app).
- Gravar página inválida que o cliente rejeite ou corrija silenciosamente.
- Pesar no PC: nada de polling agressivo; a LCU é consultada por eventos (WebSocket) ou em intervalo curto só durante a seleção.
- Mexer no cliente com as importações desligadas (exceto o que o usuário fizer no editor).

## Restrições

- **LCU** conforme `docs/lcu.md`: lockfile (porta e senha mudam a cada abertura), Basic `riot:<senha>`, TLS com o `riotgames.pem` já usado, sem dependências novas além das já aprovadas (`ureq`, `rustls`).
- **8 pontos não confirmados** no `docs/lcu.md` (nomes dos campos de feitiço em `my-selection`, limite de páginas, caminho de item sets vs `lol-loadouts`, WebSocket em Rust, entre outros). A **primeira tarefa** do plano é uma sonda somente leitura com o cliente do usuário aberto (só GET), que confirma cada um e atualiza o `docs/lcu.md`. Nenhuma escrita é implementada sobre campo não confirmado.
- Política da Riot para a LCU: importar runas, feitiços e item sets é prática comum de apps aprovados; automação de pick/ban fica fora. Nada desta entrega decide jogada pelo usuário.
- Mesmo padrão do app: lógica pura testável separada de HTTP; fixtures reais (gravações do cliente, anonimizadas).

## Decidido com o usuário

- Modo misto (automático por chave + sugestão com "Aplicar"), sempre editável.
- Página dedicada `live-builds`; nunca apagar páginas para abrir espaço.
- Editor completo de runas e de arsenais (abrir, criar, renomear, editar, apagar).
- Editor e arsenais entram nesta entrega.

- **Automático mantido** (2026-11-07), mesmo com a leitura conservadora do `docs/lcu.md` ("só agir com clique"): a Riot não proíbe nem autoriza explicitamente importar runas/feitiços/arsenal; o usuário aceitou o risco (baixo a médio). Mitigação: cada importação automática só existe se o usuário ligar a chave (padrão desligado), e a tela informa o que foi aplicado. Nada de pick, ban, aceitar partida ou dodge.

## Decisões do arquiteto

- Sem dados de elo alto: runas pela recomendação oficial da Riot no `.bin.json` do campeão (`ChampionRuneRecommendationsContext`, Community Dragon); feitiços pelo padrão da posição (Flash + Golpear na selva, + Incendiar no meio/topo, + Curar/Barreira no atirador, + Incendiar/Exaustão no suporte — o construtor pode refinar pelo pacote `ANY`).
- Sem posição (escolha às cegas, troca, modo sem rota): entrada `ANY` do pacote. ARAM e outros modos sem dados de elo alto: só a recomendação da Riot para o modo; arsenal e lembrete seguem a mesma fonte.
- Apagar (página ou arsenal) pede confirmação.
- Fonte da recomendação: a mesma do painel (elo alto, depois Riot).
- O lembrete compara com a página **ativa** no cliente, não só a `live-builds`.

## Deixado ao construtor

- Nomes de módulos (`lcu/` sugerido: conexão, sessão de seleção, perks, item sets, feitiços).
- Layout exato da tela de seleção e dos editores em egui, seguindo o estilo atual.
- Eventos por WebSocket ou polling de 1 s só durante a seleção, conforme a sonda confirmar.
