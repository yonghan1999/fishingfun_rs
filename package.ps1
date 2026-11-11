[CmdletBinding()]
param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"

# 构建 GUI 并把可执行文件复制为产品名：进程名、任务管理器和文件资源管理器
# 中显示的都是 "Google Chrome"，与窗口标题一致。
Push-Location "$PSScriptRoot"
try {
    if ($Profile -eq "release") {
        & cargo build -p fishing-gui --release --locked
    } else {
        & cargo build -p fishing-gui --locked
    }
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败。" }

    $metadataText = & cargo metadata --no-deps --format-version 1 --locked
    if ($LASTEXITCODE -ne 0) { throw "无法获取 Cargo 构建目录。" }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $source = Join-Path "$($metadata.target_directory)" "$Profile/fishing-gui.exe"
    $target = Join-Path "$($metadata.target_directory)" "$Profile/Google Chrome.exe"
    Copy-Item -LiteralPath $source -Destination $target -Force
    Write-Output "BUILT=$target"
} finally {
    Pop-Location
}
