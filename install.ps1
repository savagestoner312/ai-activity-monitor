# AI Activity Monitor - install (run in normal PowerShell as your user, from this folder)
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

function Require-Tool {
    param($Name, [scriptblock]$Check, $InstallHint)
    if (-not (& $Check)) {
        Write-Host "Missing: $Name" -ForegroundColor Red
        Write-Host "  Install it yourself, then re-run this script:"
        Write-Host "  $InstallHint"
        exit 1
    }
}

Require-Tool "Rust (cargo)" { [bool](Get-Command cargo -ErrorAction SilentlyContinue) } `
    "winget install --id Rustlang.Rustup -e"
Require-Tool "wasm32-unknown-unknown target" { (rustup target list --installed) -contains "wasm32-unknown-unknown" } `
    "rustup target add wasm32-unknown-unknown"
Require-Tool "trunk" { [bool](Get-Command trunk -ErrorAction SilentlyContinue) } `
    "cargo install trunk --locked  (needs the MSVC linker - if that fails with 'link.exe not found', install the Visual Studio Build Tools C++ workload first)"

# Build the frontend BEFORE the native binaries: aimon-dashboard embeds
# frontend/aimon-ui/dist into itself at compile time (rust-embed), so a
# stale or missing dist/ means a stale or missing UI in the built exe.
Push-Location "$here\frontend\aimon-ui"
trunk build --release
$frontendOk = $LASTEXITCODE -eq 0
Pop-Location
if (-not $frontendOk) { Write-Host "Frontend build failed." -ForegroundColor Red; exit 1 }

cargo build --release --manifest-path "$here\Cargo.toml" -p aimon-collector -p aimon-dashboard -p aimon-report
if ($LASTEXITCODE -ne 0) { Write-Host "Build failed." -ForegroundColor Red; exit 1 }

$bin = "$here\target\release"

# Collector: starts at logon, runs hidden
$a = New-ScheduledTaskAction -Execute "$bin\aimon-collector.exe" -WorkingDirectory $here
$t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName "AIMonitor-Collector" -Action $a -Trigger $t -Settings $s -Force | Out-Null

# Daily report at 9:00 PM
$a2 = New-ScheduledTaskAction -Execute "$bin\aimon-report.exe" -WorkingDirectory $here
$t2 = New-ScheduledTaskTrigger -Daily -At 9pm
Register-ScheduledTask -TaskName "AIMonitor-DailyReport" -Action $a2 -Trigger $t2 -Force | Out-Null

Start-ScheduledTask -TaskName "AIMonitor-Collector"
Write-Host "Installed. Data: $env:LOCALAPPDATA\AIMonitor  |  Report now: & `"$bin\aimon-report.exe`""

# Live dashboard at logon: http://127.0.0.1:8765
$a3 = New-ScheduledTaskAction -Execute "$bin\aimon-dashboard.exe" -WorkingDirectory $here
Register-ScheduledTask -TaskName "AIMonitor-Dashboard" -Action $a3 -Trigger $t -Settings $s -Force | Out-Null
Start-ScheduledTask -TaskName "AIMonitor-Dashboard"
Start-Process "http://127.0.0.1:8765"
