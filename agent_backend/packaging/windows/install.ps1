[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'JIsjtu'),
    [switch]$NoPath,
    [switch]$NoShortcuts,
    [switch]$NoOpenConfig
)

$ErrorActionPreference = 'Stop'

function Add-CommandPath([string]$BinDir) {
    # Read the unexpanded value to retain entries such as %USERPROFILE%\bin.
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        $path = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
        foreach ($entry in $path.Split(';')) {
            if ([Environment]::ExpandEnvironmentVariables($entry.Trim('"')).TrimEnd('\') -eq $BinDir) { return }
        }
        $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        if ($key.GetValueNames() -contains 'Path') { $kind = $key.GetValueKind('Path') }
        $newPath = ($path.TrimEnd(';') + ';' + $BinDir).TrimStart(';')
        $key.SetValue('Path', $newPath, $kind)
    } finally {
        $key.Dispose()
    }
    # Notify Explorer so newly opened terminals inherit the new PATH.
    if (-not ('JIsjtu.EnvironmentChange' -as [type])) {
        Add-Type -Namespace JIsjtu -Name EnvironmentChange -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)]
public static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint msg, UIntPtr wp, string lp,
    uint flags, uint timeout, out UIntPtr result);
'@
    }
    $result = [UIntPtr]::Zero
    [void][JIsjtu.EnvironmentChange]::SendMessageTimeout([IntPtr]65535, 0x1a, [UIntPtr]::Zero,
        'Environment', 2, 2000, [ref]$result)
}

function Add-Shortcut($Shell, $Directory, $Name, $Target, $Arguments) {
    $link = $Shell.CreateShortcut((Join-Path $Directory "$Name.lnk"))
    $link.TargetPath = $Target
    $link.Arguments = $Arguments
    $link.WorkingDirectory = $InstallDir
    $link.Save()
}

$arch = $env:PROCESSOR_ARCHITEW6432
if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
if ($arch -ne 'AMD64') { throw "本安装包适用于 Intel / AMD x64 版 Windows。检测到的架构：$arch" }
if (-not [IO.Path]::IsPathRooted($InstallDir)) { throw '安装目录必须使用绝对路径。' }
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
if (-not $NoPath -and $InstallDir.Contains(';')) { throw '安装目录含有分号，无法加入 PATH。请更换目录，或加 -NoPath 安装。' }

# Only shipped assets can be updated. User files such as .env and sessions stay in place.
$files = [ordered]@{}
foreach ($name in 'agent.exe', '.env.example', 'requirements-sso.txt', 'Setup-Campus.cmd', 'README.md', 'LICENSE',
    'agent_frontend/index.html', 'agent_frontend/style.css', 'agent_frontend/app.js') {
    $files[$name] = Join-Path $PSScriptRoot $name
}
$files['bin/JIsjtu.cmd'] = Join-Path $PSScriptRoot 'JIsjtu.cmd'
if (-not [IO.File]::Exists((Join-Path $PSScriptRoot 'pdf-tools/pdfium.dll'))) {
    throw '安装包缺少 PDF 渲染库，请先完整解压 ZIP。'
}
foreach ($file in Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'pdf-tools') -File -Recurse) {
    $relative = $file.FullName.Substring($PSScriptRoot.Length + 1)
    $files[$relative] = $file.FullName
}
foreach ($dll in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.dll' -File) {
    $files[$dll.Name] = $dll.FullName
}
$skills = @(Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'skills') -Directory)
if (-not $skills) { throw '安装包缺少内置技能，请先完整解压 ZIP。' }
foreach ($skill in $skills) {
    foreach ($file in Get-ChildItem -LiteralPath $skill.FullName -File -Recurse) {
        $relative = $file.FullName.Substring($PSScriptRoot.Length + 1)
        $files[$relative] = $file.FullName
    }
    if (-not (Test-Path -LiteralPath (Join-Path $skill.FullName 'SKILL.md') -PathType Leaf)) {
        throw "安装包内的 $($skill.Name) 缺少 SKILL.md。"
    }
}
foreach ($source in $files.Values) {
    if (-not [IO.File]::Exists($source)) { throw "安装包缺少 $source，请先完整解压 ZIP。" }
}
if (-not $NoShortcuts -and -not [IO.File]::Exists((Join-Path $PSScriptRoot 'Set-ShortcutId.ps1'))) {
    throw '安装包缺少 Set-ShortcutId.ps1，请先完整解压 ZIP。'
}

$marker = Join-Path $InstallDir '.jisjtu-install'
if ([IO.Directory]::Exists($InstallDir)) {
    if (-not [IO.File]::Exists($marker) -or [IO.File]::ReadAllText($marker).Trim() -ne 'JIsjtu') {
        throw "$InstallDir 已存在且不属于安装器。请使用其他安装目录。"
    }
} else {
    [void][IO.Directory]::CreateDirectory($InstallDir)
    [IO.File]::WriteAllText($marker, "JIsjtu")
}

$config = Join-Path $InstallDir '.env'
$lockPath = Join-Path $InstallDir '.install.lock'
try {
    $lock = [IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
} catch {
    throw "无法锁定安装目录，请关闭其他安装窗口后重试。$($_.Exception.Message)"
}
try {
    $newConfig = -not [IO.File]::Exists($config)
    if ($newConfig) { $files['.env'] = $files['.env.example'] }
    $stage = Join-Path $InstallDir ('.install-' + [guid]::NewGuid().ToString('N'))
    [void][IO.Directory]::CreateDirectory($stage)
    $applied = New-Object System.Collections.ArrayList
    $keepBackup = $false
    try {
        foreach ($name in $files.Keys) {
            $target = Join-Path $InstallDir $name
            [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target))
            $next = Join-Path $stage 'next'
            [IO.File]::Copy($files[$name], $next)
            $backup = $null
            if ([IO.File]::Exists($target)) {
                $backup = Join-Path $stage ($applied.Count.ToString() + '.bak')
                [IO.File]::Replace($next, $target, $backup)
            } else {
                [IO.File]::Move($next, $target)
            }
            [void]$applied.Add(@{ Target = $target; Backup = $backup })
        }
    } catch {
        $reason = $_.Exception.Message
        for ($i = $applied.Count - 1; $i -ge 0; $i--) {
            $entry = $applied[$i]
            try {
                if ($entry.Backup) {
                    [IO.File]::Copy($entry.Backup, $entry.Target, $true)
                } else {
                    [IO.File]::Delete($entry.Target)
                }
            } catch {
                $keepBackup = $true
                Write-Warning "无法恢复 $($entry.Target)，备份目录：$stage"
            }
        }
        throw "安装文件更新失败。请退出正在运行的 JIsjtu 后重试。$reason"
    } finally {
        if (-not $keepBackup) { [IO.Directory]::Delete($stage, $true) }
    }

    if (-not $NoPath) { Add-CommandPath (Join-Path $InstallDir 'bin') }
    if (-not $NoShortcuts) {
        $menu = Join-Path ([Environment]::GetFolderPath('Programs')) 'JIsjtu'
        [void][IO.Directory]::CreateDirectory($menu)
        $shell = New-Object -ComObject WScript.Shell
        $agent = Join-Path $InstallDir 'agent.exe'
        Add-Shortcut $shell $menu 'JIsjtu' $agent ''
        & (Join-Path $PSScriptRoot 'Set-ShortcutId.ps1') -Path (Join-Path $menu 'JIsjtu.lnk')
        Add-Shortcut $shell $menu 'JIsjtu 网页版' $agent '--web'
        Add-Shortcut $shell $menu 'JIsjtu 配置' (Join-Path $env:SystemRoot 'System32/notepad.exe') ('"' + $config + '"')
        Add-Shortcut $shell $menu 'JIsjtu 校园依赖' (Join-Path $InstallDir 'Setup-Campus.cmd') ''
    }
} finally {
    $lock.Dispose()
}

Write-Host "安装完成：$InstallDir"
Write-Host "个人配置：$config"
Write-Host '对话需要填写 OPENAI_API_KEY、OPENAI_BASE_URL 和 MODEL。'
if (-not $NoShortcuts) { Write-Host '打开开始菜单中的 JIsjtu 或 JIsjtu 网页版即可使用。' }
if (-not $NoPath) { Write-Host '也可新开终端，运行 JIsjtu、JIsjtu --web 或 JIsjtu --doctor。' }
if ($newConfig -and -not $NoOpenConfig) {
    Start-Process -FilePath (Join-Path $env:SystemRoot 'System32/notepad.exe') -ArgumentList ('"' + $config + '"')
}
