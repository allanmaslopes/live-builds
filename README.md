# live-builds

Painel de partida de League of Legends para o 2º monitor. Leve, em Rust.

## O que lê

- Só a Live Client Data API local oficial do jogo (`https://127.0.0.1:2999`). O TLS é validado com o certificado raiz da Riot (`riotgames.pem`).
- Data Dragon e Community Dragon: dados públicos de itens e recomendações oficiais.

## O que nunca faz

- Não injeta código no jogo.
- Não lê memória do processo.
- Não usa hooks nem overlay dentro do jogo.
- Não mostra informação que o jogo esconde.
- Não raspa sites de terceiros.

Como não toca no processo do jogo, não interage com o Vanguard (anti-cheat da Riot).

## O que mostra na v1

- Seu estado e seu ouro.
- O que comprar ao voltar à base.
- Build recomendada da Riot, por fases.
- Próxima skill, pela ordem oficial da Riot.
- Os 10 jogadores, com valor em itens.
- Confronto de rota.

## Como compilar

Toolchain `stable-x86_64-pc-windows-gnu` com MinGW-w64.

```
. D:\Workspace\Languages-Frameworks\rust-env.ps1
cargo build --release
```

Se a política do PowerShell bloquear o script, use `powershell -ExecutionPolicy Bypass`.

## Como usar

1. Deixe o LoL em tela cheia no monitor principal.
2. Abra `target\release\live-builds.exe`.
3. Arraste a janela para o 2º monitor.

Não clique na janela durante a luta: isso tira o foco do jogo.

## Flags

- `--poll-ms <n>`: intervalo de leitura da API, em ms (padrão 1000).
- `--record <dir>`: grava as respostas da API na pasta indicada.

## Cache

`%LOCALAPPDATA%\live-builds\cache`

## Documentação

- Design: [docs/superpowers/specs/2026-11-06-live-builds-v1-design.md](docs/superpowers/specs/2026-11-06-live-builds-v1-design.md)
- Plano: [docs/superpowers/plans/2026-11-06-live-builds-v1.md](docs/superpowers/plans/2026-11-06-live-builds-v1.md)
- Performance: [docs/performance.md](docs/performance.md)

## Licença

MIT. Veja [LICENSE](LICENSE).
