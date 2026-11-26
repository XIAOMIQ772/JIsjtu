[CmdletBinding()]
param([string]$Archive = (Join-Path $PSScriptRoot '../../JIsjtu-Windows-x64.zip'))

# Run on Windows x64. All writes, including the test shortcut, stay under TEMP.
$ErrorActionPreference = 'Stop'
$work = Join-Path ([IO.Path]::GetTempPath()) ('JIsjtu-test-' + [guid]::NewGuid().ToString('N'))
$package = Join-Path $work 'unpacked'
$app = Join-Path $work '校园 assistant'
$backend = $null
try {
    Expand-Archive -LiteralPath $Archive -DestinationPath $package
    $setup = Join-Path $package 'Install.cmd'
    $options = @('-InstallDir', $app, '-NoPath', '-NoShortcuts', '-NoOpenConfig')
    & $setup @options
    if ($LASTEXITCODE -ne 0) { throw 'First install failed' }

    $config = Join-Path $app '.env'
    if ((Get-FileHash -LiteralPath $config).Hash -ne (Get-FileHash -LiteralPath (Join-Path $app '.env.example')).Hash) {
        throw 'First install failed to create the personal config from the template'
    }
    [IO.File]::AppendAllText($config, "`r`n# keep-on-upgrade`r`n")
    $original = (Get-FileHash -LiteralPath $config).Hash
    & $setup @options
    if ($LASTEXITCODE -ne 0 -or (Get-FileHash -LiteralPath $config).Hash -ne $original) {
        throw 'Reinstall failed to preserve the personal config'
    }

    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $work 'test.lnk'))
    $shortcut.TargetPath = Join-Path $app 'agent.exe'
    $shortcut.WorkingDirectory = $app
    $shortcut.Save()
    & (Join-Path $package 'Set-ShortcutId.ps1') -Path (Join-Path $work 'test.lnk')
    $shell = New-Object -ComObject Shell.Application
    $identity = $shell.NameSpace($work).ParseName('test.lnk').ExtendedProperty('System.AppUserModel.ID')
    if ($identity -ne 'JIsjtu.Desktop') { throw "Unexpected shortcut app ID: $identity" }
    Rename-Item -LiteralPath $package -NewName 'unpacked-moved'

    $launcher = Join-Path $app 'bin/JIsjtu.cmd'
    $output = & $launcher --tool get_time_stamp
    if ($LASTEXITCODE -ne 0 -or -not ($output -match '\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}')) {
        throw 'The installed command failed to return a timestamp'
    }

    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = Join-Path $app 'agent.exe'
    $info.Arguments = '--web'
    $info.WorkingDirectory = $app
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.EnvironmentVariables['AGENT_SSO_AUTO_LOGIN'] = '0'
    $info.EnvironmentVariables['AGENT_REMINDERS'] = '0'
    $info.EnvironmentVariables['AGENT_NO_BROWSER'] = '1'
    $info.EnvironmentVariables.Remove('AGENT_FRONTEND_DIR')
    $info.EnvironmentVariables['AGENT_SESSIONS_DIR'] = (Join-Path $app 'sessions')
    $info.EnvironmentVariables['AGENT_BIND_ADDR'] = '127.0.0.1'
    $info.EnvironmentVariables['AGENT_HTTP_PORT'] = '0'
    $info.EnvironmentVariables['AGENT_TCP_PORT'] = '0'
    $backend = [Diagnostics.Process]::Start($info)
    $url = $null
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    while ($deadline.ElapsedMilliseconds -lt 15000) {
        $line = $backend.StandardOutput.ReadLineAsync()
        if (-not $line.Wait([Math]::Max(0, 15000 - [int]$deadline.ElapsedMilliseconds))) { break }
        if ($null -eq $line.Result) { break }
        if ($line.Result -match 'http://127\.0\.0\.1:\d+') {
            $url = $Matches[0]
            break
        }
    }
    if (-not $url) { throw 'Web server failed to report its listening address' }
    $page = Invoke-WebRequest "$url/" -UseBasicParsing -TimeoutSec 10
    if ($page.StatusCode -ne 200 -or $page.Content -notmatch '/app\.js') { throw 'Web UI did not load' }
    $sessions = Invoke-WebRequest "$url/api/sessions" -UseBasicParsing -TimeoutSec 10
    if ($sessions.StatusCode -ne 200 -or $sessions.Content.Trim() -ne '[]') { throw 'Session API failed' }
    Write-Host 'PASS: Windows install, upgrade, config preservation, shortcut ID, CLI tool, Web UI and session API.'
} finally {
    if ($backend) {
        if (-not $backend.HasExited) { $backend.Kill() }
        $backend.WaitForExit()
        $backend.Dispose()
    }
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
