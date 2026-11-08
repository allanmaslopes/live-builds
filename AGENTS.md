# live-builds

App Rust (eframe/egui 0.33, glow) para o 2º monitor: lê a Live Client Data API local do LoL (`https://127.0.0.1:2999`, TLS validado com `riotgames.pem`) e mostra build, compras, skills e os 10 jogadores. Leve: meta de CPU e RAM baixas (ver `docs/performance.md`).

## Regras

- Nunca injeta código, lê memória do jogo, usa overlay ou raspa sites. Só Live Client Data API, LCU (quando entrar, ver `docs/lcu.md`), Data Dragon e Community Dragon.
- Sem dependências novas sem pedido. `ureq` 3 + `rustls` (ring) para HTTP.
- Fixtures de itens vêm recortadas do `item.json` real (`D:\Workspace\live-builds-data\item.json`), nunca inventadas.
- Posse de item: só ID exato ou evolução gratuita (`from == [T]` e `base_cost == 0`); custo e componentes usam o inventário real. Ver issues #5, #14, #15.
- NÃO rode git quando for agente delegado: o arquiteto commita.

## Rodar (PowerShell)

```powershell
$env:RUSTUP_HOME='D:\Workspace\Languages-Frameworks\rust\rustup'
$env:CARGO_HOME='D:\Workspace\Languages-Frameworks\rust\cargo'
$env:Path="D:\Workspace\Languages-Frameworks\mingw64-16.2.0\bin;$env:CARGO_HOME\bin;$env:Path"
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Toolchain `stable-x86_64-pc-windows-gnu`. Gravações reais em `tests/fixtures/recorded` (anônimas).

## Ferramentas do workspace

Scripts de rotina em `D:\Workspace\Scripts\yoursummoner` (ver `README.md` lá): `verify.ps1 -Repo live-builds`, `measure.ps1` (CPU/RAM), `board.ps1` (quadro), `stack.ps1` (serviços locais).

## Invariantes (nunca quebrar)

1. **Dados do usuário:** só tocar a página de runas `live-builds` e os arsenais `live-builds-*`. Ao gravar no cliente, partir do JSON original e alterar só o necessário; nunca reserializar com um modelo incompleto. Nada é apagado sem confirmação do usuário. Exceções decididas pelo usuário (não são violação): o **arsenal exclusivo** (`exclusive_item_set`, padrão **ligado**) apaga os arsenais de outros apps do mesmo campeão ao gravar o nosso; os **arquivos globais** de outros apps (`Config\Global\Recommended\*.json`, ex. Blitz) são apagados só com confirmação explícita da lista exata.
2. **Assíncrono:** todo comando e resultado do worker carrega a chave da seleção e uma geração; resultado de chave/geração antiga é descartado. Nada de I/O na thread da UI.
3. **Dados externos** (LCU, Live Client, Data Dragon, Community Dragon, processor): nunca `unwrap`/`expect`; erro vira fallback ou mensagem na tela.
4. **Produto:** sempre há recomendação (elo alto → Riot → aviso "sem recomendação"). Aplicação automática só em Summoner's Rift (ranqueada, normal, personalizada).
5. **Desempenho:** redesenhar só quando o estado muda; nada pesado no loop de 1 s.
6. **Testes:** fixtures reais (nunca inventar formato de dado); teste vermelho antes da implementação; proibido teste que passa vazio (`is_none_or` com `None`, `.all()` sem checar tamanho).
7. **Limites numéricos** em constante nomeada.
