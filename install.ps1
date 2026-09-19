# AI Activity Monitor - install (run in normal PowerShell as your user, from this folder)
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
python -m pip install --upgrade psutil
$pyw = (Get-Command pythonw.exe).Source
$py  = (Get-Command python.exe).Source

# Collector: starts at logon, runs hidden
$a = New-ScheduledTaskAction -Execute $pyw -Argument "`"$here\src\collector.py`"" -WorkingDirectory $here
$t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName "AIMonitor-Collector" -Action $a -Trigger $t -Settings $s -Force | Out-Null

# Daily report at 9:00 PM
$a2 = New-ScheduledTaskAction -Execute $py -Argument "`"$here\src\report.py`"" -WorkingDirectory $here
$t2 = New-ScheduledTaskTrigger -Daily -At 9pm
Register-ScheduledTask -TaskName "AIMonitor-DailyReport" -Action $a2 -Trigger $t2 -Force | Out-Null

Start-ScheduledTask -TaskName "AIMonitor-Collector"
Write-Host "Installed. Data: $env:LOCALAPPDATA\AIMonitor  |  Report now: python `"$here\src\report.py`""

# Live dashboard at logon: http://127.0.0.1:8765
$a3 = New-ScheduledTaskAction -Execute $pyw -Argument "`"$here\src\dashboard.py`"" -WorkingDirectory $here
Register-ScheduledTask -TaskName "AIMonitor-Dashboard" -Action $a3 -Trigger $t -Settings $s -Force | Out-Null
Start-ScheduledTask -TaskName "AIMonitor-Dashboard"
Start-Process "http://127.0.0.1:8765"
