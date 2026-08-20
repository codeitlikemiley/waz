# Waz — Command prediction for PowerShell
# Add to your profile:
#   Invoke-Expression (& waz init powershell | Out-String)
#
# Typical profile paths:
#   pwsh:   $HOME/.config/powershell/Microsoft.PowerShell_profile.ps1
#   WinPS:  $HOME/Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1

if ($global:WAZ_SHELL_LOADED) { return }
$global:WAZ_SHELL_LOADED = $true

$wazCmd = Get-Command waz -ErrorAction SilentlyContinue
if (-not $wazCmd) {
    $wazCmd = Get-Command waz.exe -ErrorAction SilentlyContinue
}
$global:WAZ_EXE = if ($wazCmd) { $wazCmd.Source } else { 'waz' }

if (-not $env:WAZ_SESSION_ID) {
    try {
        $env:WAZ_SESSION_ID = (& $global:WAZ_EXE session-id 2>$null | Out-String).Trim()
    } catch {
        $env:WAZ_SESSION_ID = [guid]::NewGuid().ToString()
    }
}

$global:_WazPendingCommand = $null

function global:_waz_cwd {
    try {
        return (Get-Location).ProviderPath
    } catch {
        return (Get-Location).Path
    }
}

function global:_waz_record {
    param(
        [bool]$CommandSucceeded,
        $LastExitCode
    )

    $cmd = $global:_WazPendingCommand
    $global:_WazPendingCommand = $null
    if ([string]::IsNullOrWhiteSpace($cmd)) { return }
    if ($cmd -match '^\s*waz(\.exe)?(\s|$)') { return }

    $code = 0
    if (-not $CommandSucceeded) {
        if ($null -ne $LastExitCode -and $LastExitCode -ne 0) {
            $code = [int]$LastExitCode
        } else {
            $code = 1
        }
    }

    try {
        & $global:WAZ_EXE record --cwd (_waz_cwd) --session $env:WAZ_SESSION_ID --exit-code $code -- $cmd 1>$null 2>$null
    } catch {}
}

if (Test-Path Function:\prompt) {
    Copy-Item Function:\prompt Function:\_waz_original_prompt -Force
}

function global:prompt {
    $ok = $?
    $native = $global:LASTEXITCODE
    _waz_record -CommandSucceeded $ok -LastExitCode $native

    if (Test-Path Function:\_waz_original_prompt) {
        return & (Get-Item Function:\_waz_original_prompt)
    }
    return "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
}

$hasPSReadLine = $false
if (Get-Module PSReadLine -ErrorAction SilentlyContinue) {
    $hasPSReadLine = $true
} else {
    try {
        Import-Module PSReadLine -ErrorAction Stop
        $hasPSReadLine = $true
    } catch {
        $hasPSReadLine = $false
    }
}

if ($hasPSReadLine) {
    Set-PSReadLineKeyHandler -Key Enter -BriefDescription WazAcceptLine -Description 'Record the line for waz, then accept' -ScriptBlock {
        $line = $null
        $cursor = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$cursor)
        $global:_WazPendingCommand = $line
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
    }

    Set-PSReadLineKeyHandler -Chord Ctrl+Spacebar -BriefDescription WazPredict -Description 'Fill the buffer with a waz prediction' -ScriptBlock {
        $line = $null
        $cursor = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$cursor)
        $predArgs = @('predict', '--cwd', (_waz_cwd), '--session', $env:WAZ_SESSION_ID, '--fast')
        if (-not [string]::IsNullOrWhiteSpace($line)) {
            $predArgs += @('--prefix', $line)
        }
        try {
            $prediction = (& $global:WAZ_EXE @predArgs 2>$null | Out-String).Trim()
        } catch {
            $prediction = ''
        }
        if ($prediction -and $prediction -ne $line) {
            [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert($prediction)
        }
    }

    Set-PSReadLineKeyHandler -Chord Ctrl+t -BriefDescription WazTui -Description 'Launch the waz command palette' -ScriptBlock {
        $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('.waz_result.' + $PID)
        try {
            & $global:WAZ_EXE tui --cwd (_waz_cwd) --result-file $tmp
        } catch {}
        [Microsoft.PowerShell.PSConsoleReadLine]::InvokePrompt()
        if (Test-Path -LiteralPath $tmp) {
            $result = $null
            try {
                $result = (Get-Content -LiteralPath $tmp -Raw -ErrorAction SilentlyContinue)
            } catch {}
            Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
            if ($result) {
                $result = $result.Trim()
                if ($result) {
                    [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
                    [Microsoft.PowerShell.PSConsoleReadLine]::Insert($result)
                    [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
                }
            }
        }
    }
}

$global:_WazPrevCommandNotFound = $ExecutionContext.InvokeCommand.CommandNotFoundAction
$ExecutionContext.InvokeCommand.CommandNotFoundAction = {
    param($Name, $EventArgs)

    $full = $global:_WazPendingCommand
    if ([string]::IsNullOrWhiteSpace($full)) { $full = $Name }
    if ([string]::IsNullOrWhiteSpace($full) -or $full -match '^\s*waz(\.exe)?(\s|$)') {
        if ($global:_WazPrevCommandNotFound) {
            & $global:_WazPrevCommandNotFound $Name $EventArgs
        }
        return
    }

    $handled = $false
    try {
        & $global:WAZ_EXE check-nl -- $full 1>$null 2>$null
        if ($LASTEXITCODE -eq 0) {
            $waz = $global:WAZ_EXE
            $cwd = _waz_cwd
            $query = $full
            $EventArgs.StopSearch = $true
            $EventArgs.CommandScriptBlock = {
                $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('.waz_nl.' + $PID)
                try {
                    & $waz tui --cwd $cwd --query $query --result-file $tmp
                } catch {}
                if (Test-Path -LiteralPath $tmp) {
                    $result = $null
                    try {
                        $result = (Get-Content -LiteralPath $tmp -Raw -ErrorAction SilentlyContinue)
                    } catch {}
                    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
                    if ($result) {
                        $result = $result.Trim()
                        if ($result) { Invoke-Expression $result }
                    }
                }
            }.GetNewClosure()
            $handled = $true
        }
    } catch {}

    if (-not $handled -and $global:_WazPrevCommandNotFound) {
        & $global:_WazPrevCommandNotFound $Name $EventArgs
    }
}
