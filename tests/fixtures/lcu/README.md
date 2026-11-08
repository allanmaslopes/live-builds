`gameflow-phase.json`: GET `/lol-gameflow/v1/gameflow-phase`, 200, lobby; fase usada para decidir o estado da LCU.
`current-summoner.json`: GET `/lol-summoner/v1/current-summoner`, 200, lobby; `summonerId` usado apenas para consultar item sets.
`champ-select-session.json`: GET `/lol-champ-select/v1/session`, 200, campeao travado; time, acoes e feitiços usados para contexto da selecao.
`champ-select-session-ban.json`: GET `/lol-champ-select/v1/session`, 200, ban; `actions` e `timer` usados para estado da selecao.
`champ-select-session-locked.json`: GET `/lol-champ-select/v1/session`, 200, campeao travado; pick concluido e posicao usados para contexto.
`perks-pages.json`: GET `/lol-perks/v1/pages`, 200, lobby; paginas e runas usadas para leitura.
`perks-currentpage.json`: GET `/lol-perks/v1/currentpage`, 200, lobby; pagina ativa e runas usadas para leitura.
`perks-styles.json`: GET `/lol-perks/v1/styles`, 200, lobby; arvores e slots de runas usados para leitura.
`perks-customizationlimits.json`: GET `/lol-perks/v1/customizationlimits`, 404, lobby; resposta de indisponibilidade registrada.
`perks-inventory.json`: GET `/lol-perks/v1/inventory`, 200, lobby; limite e contagem de paginas usados para leitura.
`item-sets.json`: GET `/lol-item-sets/v1/item-sets/{summonerId}/sets`, 200, lobby; blocos e itens dos arsenais usados para leitura.
