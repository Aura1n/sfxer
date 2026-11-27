<#
.SYNOPSIS
  sfxer 一键脚本：测试、构建、同步版本号、提交、打 tag、发布。
.EXAMPLE
  scripts\sfx.cmd test            # 生成样例音频 + cargo test + clippy
  scripts\sfx.cmd build           # 构建安装包，并复制到 release\
  scripts\sfx.cmd version 0.9.7   # 同步所有文件里的版本号
  scripts\sfx.cmd release         # 检查 -> 测试 -> 构建 -> 提交 -> tag -> 推送 -> GitHub Release
  scripts\sfx.cmd ffmpeg          # 下载 ffmpeg 到 %APPDATA%\dev.zara.sfxer\ffmpeg（增强解析能力）
#>
param(
    [Parameter(Position = 0)][string]$Command = "help",
    [Parameter(Position = 1, ValueFromRemainingArguments = $true)][string[]]$Rest
)

$ErrorActionPreference = "Stop"
if ($PSVersionTable.PSVersion.Major -ge 7) { $PSNativeCommandUseErrorActionPreference = $false }
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root
$Utf8 = New-Object System.Text.UTF8Encoding $false

function Step($text) { Write-Host "`n==> $text" -ForegroundColor Cyan }
function Ok($text) { Write-Host "    $text" -ForegroundColor Green }
function Warn($text) { Write-Host "    $text" -ForegroundColor Yellow }

function Run([string]$exe, [string[]]$arguments) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = "Continue"   # cargo 把进度写到 stderr，Windows PowerShell 5 会把它当错误
    try { & $exe @arguments } finally { $ErrorActionPreference = $old }
    if ($LASTEXITCODE -ne 0) { throw "命令失败（退出码 $LASTEXITCODE）：$exe $($arguments -join ' ')" }
}

# 直接调用本地安装的 tauri，绕开 npx 的包解析（node_modules 的缓存过期时 npx 会报
# "could not determine executable to run"）。
function Invoke-Tauri([string[]]$arguments) {
    $local = Join-Path $Root "node_modules\.bin\tauri.cmd"
    if (Test-Path $local) { Run $local $arguments; return }
    Warn "node_modules 里没有 tauri，先执行 npm install"
    Run npm @("install")
    if (Test-Path $local) { Run $local $arguments; return }
    Run npx (@("tauri") + $arguments)
}

function Has([string]$name) { return [bool](Get-Command $name -ErrorAction SilentlyContinue) }

function Read-Text([string]$path) { return [IO.File]::ReadAllText((Join-Path $Root $path), $Utf8) }
function Write-Text([string]$path, [string]$text) { [IO.File]::WriteAllText((Join-Path $Root $path), $text, $Utf8) }

# ---------------------------------------------------------------- 版本号
function Get-Versions {
    $lock = Read-Text "src-tauri\Cargo.lock"
    $cargoLock = ([regex]::Match($lock, 'name = "sfxer"\r?\nversion = "([^"]+)"')).Groups[1].Value
    return [ordered]@{
        "package.json"              = ([regex]::Match((Read-Text "package.json"), '"version":\s*"([^"]+)"')).Groups[1].Value
        "package-lock.json"         = ([regex]::Match((Read-Text "package-lock.json"), '"version":\s*"([^"]+)"')).Groups[1].Value
        "src-tauri\Cargo.toml"      = ([regex]::Match((Read-Text "src-tauri\Cargo.toml"), '(?m)^version\s*=\s*"([^"]+)"')).Groups[1].Value
        "src-tauri\Cargo.lock"      = $cargoLock
        "src-tauri\tauri.conf.json" = ([regex]::Match((Read-Text "src-tauri\tauri.conf.json"), '"version":\s*"([^"]+)"')).Groups[1].Value
    }
}

function Get-Text-Version {
    return ([regex]::Match((Read-Text "src-tauri\tauri.conf.json"), '"version":\s*"([^"]+)"')).Groups[1].Value
}

function Assert-VersionsAgree {
    $all = Get-Versions
    $all.GetEnumerator() | ForEach-Object { Write-Host ("    {0,-28} {1}" -f $_.Key, $_.Value) }
    $distinct = @($all.Values | Select-Object -Unique)
    if ($distinct.Count -ne 1) { throw "版本号不一致。先运行：scripts\sfx.cmd version <x.y.z>" }
}

function Set-Version([string]$v) {
    if ($v -notmatch '^\d+\.\d+\.\d+$') { throw "版本号格式应为 1.2.3，收到：$v" }
    $edits = @(
        @("package.json", '"version":\s*"[^"]+"', "`"version`": `"$v`"", 1),
        @("package-lock.json", '"version":\s*"[^"]+"', "`"version`": `"$v`"", 2),
        @("src-tauri\Cargo.toml", '(?m)^version\s*=\s*"[^"]+"', "version = `"$v`"", 1),
        @("src-tauri\tauri.conf.json", '"version":\s*"[^"]+"', "`"version`": `"$v`"", 1),
        @("src-tauri\Cargo.lock", '(name = "sfxer"\r?\nversion = )"[^"]+"', "`${1}`"$v`"", 1)
    )
    foreach ($e in $edits) {
        $re = New-Object System.Text.RegularExpressions.Regex($e[1])
        Write-Text $e[0] ($re.Replace((Read-Text $e[0]), $e[2], $e[3]))
    }
    Ok "版本号已同步为 $v"
}

# ---------------------------------------------------------------- 测试与构建
function Invoke-Tests {
    Step "生成样例音频（test-library）"
    if (-not (Has node)) { throw "找不到 node，请先安装 Node.js" }
    Run node @("tools/make-test-audio.mjs")
    Step "cargo test --lib"
    Push-Location (Join-Path $Root "src-tauri")
    try {
        Run cargo @("test", "--lib")
        Step "cargo clippy（有警告不会中断）"
        $old = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try { & cargo clippy --all-targets } finally { $ErrorActionPreference = $old }
        if ($LASTEXITCODE -ne 0) { Warn "clippy 报告了问题（见上方），不影响测试结果" }
    }
    finally { Pop-Location }
    Ok "测试通过"
}

function Invoke-Build {
    $v = Get-Text-Version
    Step "构建 $v（npx tauri build）"
    Invoke-Tauri @("build")
    $releaseDir = Join-Path $Root "release"
    New-Item -ItemType Directory -Force $releaseDir | Out-Null
    $setup = Join-Path $Root "src-tauri\target\release\bundle\nsis\sfxer_${v}_x64-setup.exe"
    $exe = Join-Path $Root "src-tauri\target\release\sfxer.exe"
    if (-not (Test-Path $setup)) { throw "没有找到安装包：$setup" }
    Copy-Item $setup (Join-Path $releaseDir "sfxer_${v}_x64-setup.exe") -Force
    Copy-Item $exe (Join-Path $releaseDir "sfxer_${v}_x64-portable.exe") -Force
    Copy-Item $exe (Join-Path $releaseDir "sfxer.exe") -Force
    Ok "已写入 release\sfxer_${v}_x64-setup.exe 与 -portable.exe"
}

# ---------------------------------------------------------------- Git 与发布
function Get-Notes([string]$v) {
    $log = Read-Text "CHANGELOG.md"
    $m = [regex]::Match($log, "(?ms)^## $([regex]::Escape($v))\b[^\r\n]*\r?\n(.*?)(?=^## |\z)")
    if (-not $m.Success) { return "sfxer $v" }
    return $m.Groups[1].Value.Trim()
}

function Confirm([string]$question) {
    $a = Read-Host "$question [y/N]"
    return ($a -match '^(y|yes|是|好)$')
}

function Invoke-Commit([string]$message) {
    if (-not (Has git)) { throw "找不到 git" }
    $pending = (& git status --porcelain)
    if (-not $pending) { Ok "没有需要提交的改动"; return }
    Run git @("add", "-A")
    Run git @("commit", "-m", $message)
}

function Invoke-Release {
    if (-not (Has git)) { throw "找不到 git" }
    $v = Get-Text-Version
    $tag = "v$v"
    Step "检查版本号"
    Assert-VersionsAgree
    if ((& git tag --list $tag)) {
        $onRemote = (& git ls-remote --tags origin $tag)
        if ($onRemote) { throw "tag $tag 已经在 GitHub 上，请先 scripts\sfx.cmd version <新版本号>" }
    }
    Invoke-Tests
    Invoke-Build
    Step "提交"
    Invoke-Commit "Release $v"
    # A tag that only exists locally may point at an older commit: move it to this release.
    if (& git tag --list $tag) { Run git @("tag", "-d", $tag) }
    Run git @("tag", $tag)

    if (-not (Confirm "推送 main 和 $tag 到 GitHub 吗？")) { Warn "已停止，未推送。"; return }
    Run git @("push", "origin", "HEAD")
    Run git @("push", "origin", $tag)

    Step "发布 GitHub Release"
    $notes = Join-Path $Root "release\notes-$v.md"
    [IO.File]::WriteAllText($notes, (Get-Notes $v), $Utf8)
    $files = @("release\sfxer_${v}_x64-setup.exe", "release\sfxer_${v}_x64-portable.exe")
    if (Has gh) {
        Run gh (@("release", "create", $tag) + $files + @("--title", "sfxer $v", "--notes-file", $notes))
        Ok "已发布 $tag"
    }
    else {
        Warn "没有安装 GitHub CLI（gh）。请打开 https://github.com/Aura1n/sfxer/releases/new"
        Warn "选择 tag $tag，正文粘贴 $notes，并上传："
        $files | ForEach-Object { Warn "  $_" }
    }
}

# ---------------------------------------------------------------- ffmpeg
function Install-Ffmpeg {
    $dest = Join-Path $env:APPDATA "dev.zara.sfxer\ffmpeg"
    $tmp = Join-Path $env:TEMP "sfxer-ffmpeg"
    $source = if ($Rest -and $Rest[0]) { $Rest[0] } else { "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip" }
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $tmp | Out-Null
    $zip = Join-Path $tmp "ffmpeg.zip"
    if (Test-Path $source) {
        Step "使用本地压缩包 $source"
        Copy-Item $source $zip
    }
    else {
        Step "下载 $source"
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $source -OutFile $zip -UseBasicParsing
    }
    Step "解压"
    Expand-Archive $zip (Join-Path $tmp "x") -Force
    $bin = Get-ChildItem (Join-Path $tmp "x") -Recurse -Filter ffmpeg.exe | Select-Object -First 1
    if (-not $bin) { throw "压缩包里没有找到 ffmpeg.exe" }
    New-Item -ItemType Directory -Force $dest | Out-Null
    Copy-Item $bin.FullName $dest -Force
    $probe = Join-Path $bin.DirectoryName "ffprobe.exe"
    if (Test-Path $probe) { Copy-Item $probe $dest -Force } else { Warn "压缩包里没有 ffprobe.exe，属性读取会受限" }
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    Ok "已安装到 $dest"
    Ok "重启 sfxer，在 设置 -> 导入与扫描 点「重试失败项」即可重新解析"
}

function Show-Help {
    Get-Help $PSCommandPath | Out-Host
}

switch ($Command.ToLower()) {
    "test" { Invoke-Tests }
    "build" { Invoke-Build }
    "dev" { Invoke-Tauri @("dev") }
    "version" {
        if ($Rest -and $Rest[0]) { Set-Version $Rest[0] }
        Assert-VersionsAgree
    }
    "check" { Assert-VersionsAgree }
    "commit" { Invoke-Commit $(if ($Rest) { $Rest -join " " } else { "Update" }) }
    "release" { Invoke-Release }
    "ffmpeg" { Install-Ffmpeg }
    default { Show-Help }
}
