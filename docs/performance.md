# Performance do live-builds

Metas de consumo do painel, medidas com `tools/measure.ps1`.

## Metas

| Cenário | CPU média | RAM |
|---|---|---|
| Fora de partida | ~0% de CPU | < 30 MB |
| Em partida | < 1% de um núcleo | < 60 MB |

**Conversão de unidades.** O script mostra CPU como % do total de núcleos lógicos, não de um núcleo. Para converter:

`% de 1 núcleo = % do script × nº de núcleos`

Exemplo: com 8 núcleos, a meta de 1% de um núcleo aparece no script como 0,125%. O script imprime o nº de núcleos no início da saída.

## Como medir

Rode com o painel aberto (e a partida em andamento, no cenário de partida):

```
powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Process live-builds -Seconds 120 -Out perf.csv
```

Para comparar com outro processo (por exemplo, o cliente do LoL), use `-Compare`:

```
powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Process live-builds -Seconds 120 -Out perf.csv -Compare Blitz
```

- `-Process`: nome do processo sem `.exe`. Todas as instâncias com esse nome são somadas.
- `-Seconds`: duração da medição, em segundos (padrão 120).
- `-Out`: grava uma linha por segundo em CSV (`timestamp,processo,cpu_pct,ws_mb,private_mb`).
- `-Compare`: mede um segundo processo na mesma janela de tempo.

No fim, o script imprime CPU média, p95 e máxima, RAM de trabalho (WS) média e máxima, memória privada máxima e o nº de amostras.

## Resultados

| Data | Versão | Cenário | CPU média (% de 1 núcleo) | CPU p95 | RAM média (WS MB) | RAM máx | Observações |
|---|---|---|---|---|---|---|---|
| 2026-11-06 | 9b9de8d (egui + glow) | Fora de partida, janela visível, 60 s | 0,48 (0,03% de 16 núcleos) | 3,0 | 76,7 | 79,3 | Private máx 156 MB. Quase toda a RAM vem do driver OpenGL da AMD RX 6750 XT. Acima da meta de 30 MB. |
| 2026-11-06 | spike (egui + wgpu DX12) | Fora de partida, janela visível, 60 s | 8,2 (0,51% de 16 núcleos) | 94,6 | 210,5 | 210,9 | Private máx 508 MB. Descartado: muito pior que o glow. |
