#requires -Version 5.1
<#
.SYNOPSIS
    Analyzes Waz BYOP prompt cache hit rate (based on the `[byop-cache]` log lines
    printed at the end of each stream in chat_stream.rs::generate_byop_output).

.DESCRIPTION
    1. Automatically locates the Waz log file: `%LOCALAPPDATA%\waz\Waz\data\logs\waz.log`
    2. Greps lines matching the following format:
       [byop-cache] prompt_tokens=N cache_read=R (X.X%) cache_create=W (Y.Y%) model=M compaction=L
       Where compaction= is an optional field added in P2-16 (none / inactive / active(hidden=N))
    3. Group and aggregate by model, outputting the following metrics for each model:
       - Request count
       - Average cache_read ratio (primary hit metric)
       - Average cache_create ratio (write metric, high for the first request, should be low afterwards)
       - Total prompt tokens / Total cache_read tokens / Total cache_create tokens
       - Compaction-related request statistics (P2-16)
    4. Provides a comparison mode (-Tail N) to only view the latest N records, suitable for A/B testing.

.PARAMETER LogPath
    Custom log path. Defaults to searching in standard Waz locations.

.PARAMETER Tail
    Analyze only the last N [byop-cache] lines (defaults to all).

.PARAMETER Watch
    Continuously tail the log file, printing new cache hit rate lines in real time (Ctrl+C to exit).

.EXAMPLE
    .\analyze-prompt-cache.ps1
.EXAMPLE
    .\analyze-prompt-cache.ps1 -Tail 20
.EXAMPLE
    .\analyze-prompt-cache.ps1 -Watch
.EXAMPLE
    .\analyze-prompt-cache.ps1 -LogPath "D:\backup\waz.log"

.NOTES
    Requires Waz to have INFO level logging enabled (`[byop-cache]` is logged at log::info!).
    If no `[byop-cache]` lines are found:
      - The upstream provider did not return cache fields (DeepSeek/Ollama implicit caching might be 0)
      - Or RUST_LOG filtered out INFO level logs
#>
[CmdletBinding()]
param(
    [string]$LogPath,
    [int]$Tail = 0,
    [switch]$Watch
)

$ErrorActionPreference = 'Stop'

# ---------- 1. Locate Log File ----------
function Resolve-WazLog {
    param([string]$Override)
    if ($Override) {
        if (-not (Test-Path -LiteralPath $Override)) {
            throw "Specified log path does not exist: $Override"
        }
        return (Resolve-Path -LiteralPath $Override).Path
    }
    $candidates = @()
    if ($env:LOCALAPPDATA) {
        # Current version path (Windows branch in `crates/simple_logger/src/manager.rs::log_directory_path`)
        $candidates += (Join-Path -Path $env:LOCALAPPDATA -ChildPath 'waz\Waz\data\logs\waz.log')
        # Fallbacks (older version paths)
        $candidates += (Join-Path -Path $env:LOCALAPPDATA -ChildPath 'waz\Waz\data\waz.log')
        $candidates += (Join-Path -Path $env:LOCALAPPDATA -ChildPath 'waz\Waz\waz.log')
    }
    if ($env:APPDATA) {
        $candidates += (Join-Path -Path $env:APPDATA -ChildPath 'waz\Waz\data\logs\waz.log')
        $candidates += (Join-Path -Path $env:APPDATA -ChildPath 'waz\Waz\data\waz.log')
    }
    foreach ($c in $candidates) {
        if ($c -and (Test-Path -LiteralPath $c)) { return (Resolve-Path -LiteralPath $c).Path }
    }
    throw @"
Waz log file not found. Please check the following locations or use -LogPath to specify:
  $($candidates -join "`n  ")
If Waz has not run yet, start it once and run this script again.
"@
}

# ---------- 2. Parse Single Line ----------
# Line format (single line, may be wrapped due to terminal width, but log crate newlines only occur at the end):
# [byop-cache] prompt_tokens=12345 cache_read=10000 (81.0%) cache_create=200 (1.6%) model=claude-opus-4-7 compaction=none
# compaction= field is added in P2-16, values: none / inactive / active(hidden=N).
# The compaction field is optional for backward compatibility with older logs.
$cacheLineRegex = [regex]'\[byop-cache\]\s+prompt_tokens=(?<prompt>\d+)\s+cache_read=(?<read>\d+)\s+\(\s*(?<read_pct>[\d\.]+)%\)\s+cache_create=(?<create>\d+)\s+\(\s*(?<create_pct>[\d\.]+)%\)\s+model=(?<model>\S+?)(?:\s+compaction=(?<compaction>\S+))?$'

function Parse-CacheLine {
    param([string]$Line)
    $m = $cacheLineRegex.Match($Line)
    if (-not $m.Success) { return $null }
    $compactionRaw = if ($m.Groups['compaction'].Success) { $m.Groups['compaction'].Value } else { '' }
    [pscustomobject]@{
        Timestamp    = $null
        PromptTokens = [int]$m.Groups['prompt'].Value
        CacheRead    = [int]$m.Groups['read'].Value
        CacheCreate  = [int]$m.Groups['create'].Value
        ReadPct      = [double]$m.Groups['read_pct'].Value
        CreatePct    = [double]$m.Groups['create_pct'].Value
        Model        = $m.Groups['model'].Value
        # P2-16: Compaction status. Values: ''(old logs) / 'none' / 'inactive' / 'active(hidden=N)'
        Compaction   = $compactionRaw
        Raw          = $Line
    }
}

# ---------- 3. Aggregate & Format Output ----------
function Format-Summary {
    param([System.Collections.IList]$Records)
    if ($Records.Count -eq 0) {
        Write-Host 'No matching [byop-cache] lines found.' -ForegroundColor Yellow
        Write-Host @'

Possible reasons:
  1. No requests have been made using the BYOP path yet (no AI chat after starting Waz)
  2. The upstream provider did not return cache fields (e.g., DeepSeek/Ollama server-side implicit cache)
  3. RUST_LOG filtered out INFO level logs - check startup environment variables

Troubleshooting steps:
  $env:RUST_LOG = 'info'   # Set this before starting Waz
  Send at least 2 messages to the AI in Waz (same conversation) to trigger BYOP
  Then re-run this script
'@ -ForegroundColor Yellow
        return
    }

    Write-Host ''
    Write-Host '========== Waz BYOP Prompt Cache Hit Rate Analysis ==========' -ForegroundColor Cyan
    Write-Host ("Total matched lines: {0}" -f $Records.Count)

    # P2-16: Compaction related summary
    $compactionActive = @($Records | Where-Object { $_.Compaction -like 'active*' })
    if ($compactionActive.Count -gt 0) {
        Write-Host ("  └─ Of which active compaction: {0} lines" -f $compactionActive.Count) -ForegroundColor DarkYellow
    }
    Write-Host ''

    # Group by model
    $byModel = $Records | Group-Object Model

    $byModel | ForEach-Object {
        $model = $_.Name
        $rs    = $_.Group
        $n     = $rs.Count
        $sumPrompt = ($rs | Measure-Object PromptTokens -Sum).Sum
        $sumRead   = ($rs | Measure-Object CacheRead    -Sum).Sum
        $sumCreate = ($rs | Measure-Object CacheCreate  -Sum).Sum
        $avgReadPct   = ($rs | Measure-Object ReadPct   -Average).Average
        $avgCreatePct = ($rs | Measure-Object CreatePct -Average).Average

        $globalReadPct = if ($sumPrompt -gt 0) { 100.0 * $sumRead / $sumPrompt } else { 0.0 }
        $globalCreatePct = if ($sumPrompt -gt 0) { 100.0 * $sumCreate / $sumPrompt } else { 0.0 }

        Write-Host ("Model: {0}" -f $model) -ForegroundColor Green
        Write-Host ("  Request count:    {0}" -f $n)
        Write-Host ("  Total prompt tokens: {0:N0}" -f $sumPrompt)
        Write-Host ("  Total cache_read:    {0:N0}  ({1:F1}% of total)" -f $sumRead,   $globalReadPct)
        Write-Host ("  Total cache_create:  {0:N0}  ({1:F1}% of total)" -f $sumCreate, $globalCreatePct)
        Write-Host ("  Average read ratio:  {0:F1}%   <- Primary hit rate indicator (>=20% normal, >=50% excellent)" -f $avgReadPct)
        Write-Host ("  Average create ratio:{0:F1}%   <- Should decrease with conversation turns" -f $avgCreatePct)

        # Trend analysis (turn vs read ratio): check if hit rate rises as conversation progresses
        if ($n -ge 3) {
            $trend = $rs | ForEach-Object -Begin { $i = 0 } -Process {
                $i++
                $marker = if ($_.Compaction -like 'active*') { '*' } else { '' }
                "{0}{1}:{2:F0}%" -f $i, $marker, $_.ReadPct
            }
            Write-Host ("  Read ratio trend:  {0}" -f ($trend -join ' -> '))
            if ($rs | Where-Object { $_.Compaction -like 'active*' }) {
                Write-Host ("  (* = compaction active, cache miss is expected for this turn)") -ForegroundColor DarkGray
            }
        }
        Write-Host ''
    }

    # Global health check
    $allReadPct = ($Records | Measure-Object ReadPct -Average).Average
    Write-Host '----------- Global Health -----------' -ForegroundColor Cyan
    if ($allReadPct -ge 50) {
        Write-Host ("✅ Global average hit rate {0:F1}% - Excellent" -f $allReadPct) -ForegroundColor Green
    } elseif ($allReadPct -ge 20) {
        Write-Host ("⚠️  Global average hit rate {0:F1}% - Normal, but has room for improvement" -f $allReadPct) -ForegroundColor Yellow
    } else {
        Write-Host ("❌ Global average hit rate {0:F1}% - Low, potential prefix instability" -f $allReadPct) -ForegroundColor Red
        Write-Host '   Troubleshooting: Check if system prompt contains fields changing per request, or if MCP tools order is unstable'
    }

    if ($compactionActive.Count -gt 0) {
        $nonCompactionRecords = @($Records | Where-Object { $_.Compaction -notlike 'active*' })
        if ($nonCompactionRecords.Count -gt 0) {
            $nonCompactionAvg = ($nonCompactionRecords | Measure-Object ReadPct -Average).Average
            Write-Host ("ℹ️  Average hit rate excluding compaction turns {0:F1}% (n={1})" -f $nonCompactionAvg, $nonCompactionRecords.Count) -ForegroundColor DarkCyan
        }
    }
}

# ---------- 4. Main Flow ----------
$logFile = Resolve-WazLog -Override $LogPath
Write-Host "Log path: $logFile" -ForegroundColor DarkGray

if ($Watch) {
    Write-Host 'Entering watch mode, Ctrl+C to exit. Newly added [byop-cache] lines will be printed in real time:' -ForegroundColor Cyan
    Get-Content -LiteralPath $logFile -Wait -Tail 0 | ForEach-Object {
        $rec = Parse-CacheLine $_
        if ($rec) {
            $color = if ($rec.ReadPct -ge 50) { 'Green' }
                     elseif ($rec.ReadPct -ge 20) { 'Yellow' }
                     else { 'Red' }
            $compactionTag = if ($rec.Compaction) { " [$($rec.Compaction)]" } else { '' }
            $msg = '[{0}] read={1:F1}% create={2:F1}% prompt={3} model={4}{5}' -f `
                (Get-Date -Format 'HH:mm:ss'), $rec.ReadPct, $rec.CreatePct, $rec.PromptTokens, $rec.Model, $compactionTag
            Write-Host $msg -ForegroundColor $color
        }
    }
    return
}

# Static analysis (one-time scan)
$records = New-Object System.Collections.ArrayList
Get-Content -LiteralPath $logFile -ReadCount 1000 | ForEach-Object {
    foreach ($line in $_) {
        $rec = Parse-CacheLine $line
        if ($rec) { [void]$records.Add($rec) }
    }
}

if ($Tail -gt 0 -and $records.Count -gt $Tail) {
    $records = [System.Collections.ArrayList]::new(
        $records.GetRange($records.Count - $Tail, $Tail)
    )
    Write-Host "(Analyzing only the last $Tail entries)" -ForegroundColor DarkGray
}

Format-Summary -Records $records
