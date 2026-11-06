<#
.SYNOPSIS
  Mede CPU e RAM de um processo (somando todas as instâncias) a cada segundo.
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Process live-builds -Seconds 120 -Out perf.csv -Compare Blitz
#>
param(
    [Parameter(Mandatory = $true)] [string] $Process,
    [ValidateRange(1, 86400)] [int] $Seconds = 120,
    [string] $Out,
    [string] $Compare
)

$ErrorActionPreference = 'Stop'
# Cultura invariante: decimal com ponto no CSV, mesmo em Windows pt-BR
$inv = [Globalization.CultureInfo]::InvariantCulture
$cores = [Environment]::ProcessorCount

$targets = @($Process)
if ($Compare) { $targets += $Compare }

# CPU é guardada por PID: o delta é por processo, então processo que nasce ou morre no meio não distorce a conta
$prevCpu = @{}
$present = @{}
foreach ($t in $targets) {
    $prevCpu[$t] = @{}
    $present[$t] = New-Object System.Collections.Generic.List[object]
}

function Read-Sample {
    param([string] $Name, [hashtable] $Prev, [double] $ElapsedSec, [int] $Cores)

    $procs = @(Get-Process -Name $Name -ErrorAction SilentlyContinue)
    if ($procs.Count -eq 0) { $Prev.Clear(); return $null }

    $cpuSec = 0.0; $ws = 0.0; $priv = 0.0; $alive = @{}
    foreach ($p in $procs) {
        try {
            $now = $p.TotalProcessorTime.TotalSeconds
            $wsNow = $p.WorkingSet64
            $privNow = $p.PrivateMemorySize64
        } catch { continue }   # processo saiu entre a listagem e a leitura
        $alive[$p.Id] = $true
        if ($Prev.ContainsKey($p.Id)) { $cpuSec += $now - $Prev[$p.Id] }
        $Prev[$p.Id] = $now
        $ws += $wsNow
        $priv += $privNow
    }
    foreach ($id in @($Prev.Keys)) { if (-not $alive.ContainsKey($id)) { $Prev.Remove($id) } }
    if ($alive.Count -eq 0) { return $null }

    [pscustomobject]@{
        cpu_pct    = $cpuSec / ($ElapsedSec * $Cores) * 100
        ws_mb      = $ws / 1MB
        private_mb = $priv / 1MB
    }
}

function Get-Summary {
    param([string] $Name, $List)

    if ($List.Count -eq 0) {
        return [pscustomobject]@{
            Processo = $Name; 'CPU média %' = '-'; 'CPU p95 %' = '-'; 'CPU máx %' = '-'
            'WS média MB' = '-'; 'WS máx MB' = '-'; 'Private máx MB' = '-'; Amostras = 0
        }
    }
    $cpu = @($List | ForEach-Object { $_.cpu_pct } | Sort-Object)
    $ws = @($List | ForEach-Object { $_.ws_mb })
    $priv = @($List | ForEach-Object { $_.private_mb })
    # p95 por posição (nearest rank)
    $idx = [Math]::Max(0, [int][Math]::Ceiling(0.95 * $cpu.Count) - 1)

    [pscustomobject]@{
        Processo         = $Name
        'CPU média %'    = [Math]::Round(($cpu | Measure-Object -Average).Average, 2)
        'CPU p95 %'      = [Math]::Round($cpu[$idx], 2)
        'CPU máx %'      = [Math]::Round(($cpu | Measure-Object -Maximum).Maximum, 2)
        'WS média MB'    = [Math]::Round(($ws | Measure-Object -Average).Average, 1)
        'WS máx MB'      = [Math]::Round(($ws | Measure-Object -Maximum).Maximum, 1)
        'Private máx MB' = [Math]::Round(($priv | Measure-Object -Maximum).Maximum, 1)
        Amostras         = $List.Count
    }
}

if ($Out) { Set-Content -LiteralPath $Out -Value 'timestamp,processo,cpu_pct,ws_mb,private_mb' -Encoding UTF8 }

# Leitura de base: fixa o CPU inicial, os deltas começam no primeiro tick
foreach ($t in $targets) { [void](Read-Sample -Name $t -Prev $prevCpu[$t] -ElapsedSec 1 -Cores $cores) }

$sw = [System.Diagnostics.Stopwatch]::StartNew()
for ($i = 0; $i -lt $Seconds; $i++) {
    Start-Sleep -Milliseconds 1000
    # Intervalo real medido, não o nominal: Start-Sleep e as leituras atrasam o tick
    $elapsed = $sw.Elapsed.TotalSeconds
    $sw.Restart()
    $ts = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss', $inv)

    foreach ($t in $targets) {
        $s = Read-Sample -Name $t -Prev $prevCpu[$t] -ElapsedSec $elapsed -Cores $cores
        if ($s) { $present[$t].Add($s) }
        if ($Out) {
            if ($s) {
                $line = '{0},{1},{2},{3},{4}' -f $ts, $t, $s.cpu_pct.ToString('F2', $inv),
                    $s.ws_mb.ToString('F1', $inv), $s.private_mb.ToString('F1', $inv)
            } else {
                # processo ausente neste tick: linha com campos vazios, sem parar
                $line = '{0},{1},,,' -f $ts, $t
            }
            Add-Content -LiteralPath $Out -Value $line -Encoding UTF8
        }
    }
}

Write-Host ("Núcleos lógicos: {0} (CPU % = % do total; % de 1 núcleo = % x {0})" -f $cores)
$summary = foreach ($t in $targets) { Get-Summary -Name $t -List $present[$t] }
$summary | Format-Table -AutoSize
if ($Out) { Write-Host "CSV: $Out" }
