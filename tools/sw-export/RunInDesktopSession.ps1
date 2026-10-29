param(
    [Parameter(Mandatory=$true)][string]$InputList,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [switch]$Step,
    [switch]$Dxf,
    [switch]$NoSelections,
    [int]$TimeoutMinutes = 60
)
# Run on the SolidWorks host, for example over SSH. SolidWorks cannot be started from an SSH session, so the
# exporter runs as a one-off scheduled task in this account's logged-in desktop. The account must have a
# desktop session open.
$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot 'peet-sw-export.exe'
$InputList = [IO.Path]::GetFullPath($InputList)
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$arguments = '"' + $InputList + '" "' + $OutputDirectory + '"'
if ($Step) { $arguments += ' --step' }
if ($Dxf) { $arguments += ' --dxf' }
if ($NoSelections) { $arguments += ' --no-selections' }

$action = New-ScheduledTaskAction -Execute $exe -Argument $arguments -WorkingDirectory $PSScriptRoot
$principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes $TimeoutMinutes)
$taskName = 'PeetSwExport-' + [guid]::NewGuid().ToString('N')
Register-ScheduledTask -TaskName $taskName -Action $action -Principal $principal -Settings $settings | Out-Null
try {
    Start-ScheduledTask -TaskName $taskName
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        Start-Sleep -Seconds 2
        $task = Get-ScheduledTask -TaskName $taskName
        if ($timer.Elapsed.TotalMinutes -gt $TimeoutMinutes) { throw "The exporter timed out. See $OutputDirectory\export.log and the desktop session." }
    } while ($task.State -eq 'Running' -or $task.State -eq 'Queued')
    $result = (Get-ScheduledTaskInfo -TaskName $taskName).LastTaskResult
    # 1 means at least one document was not fully exported: its file says why.
    if ($result -ne 0 -and $result -ne 1) { throw "The exporter returned $result. See $OutputDirectory\export.log" }
    Get-Content -LiteralPath (Join-Path $OutputDirectory 'summary.json') -Raw
} finally {
    # A task that is still running is left in place so it can be diagnosed.
    if ((Get-ScheduledTask -TaskName $taskName).State -ne 'Running') {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
    }
}
