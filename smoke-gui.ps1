[CmdletBinding()]
param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "debug",
    [string]$ExePath,
    [ValidateRange(1, 120)]
    [int]$TimeoutSeconds = 15,
    [string]$ScreenshotPath
)

$ErrorActionPreference = "Stop"
if ($env:OS -ne "Windows_NT") {
    throw "GUI 冒烟测试仅支持 Windows 桌面会话。"
}

if (-not $ExePath) {
    # cargo metadata 同时兼容 CARGO_TARGET_DIR 和 .cargo/config.toml。
    Push-Location "$PSScriptRoot"
    try {
        $metadataText = & cargo metadata --no-deps --format-version 1 --locked
        if ($LASTEXITCODE -ne 0) { throw "无法获取 Cargo 构建目录。" }
        $metadata = ($metadataText -join "`n") | ConvertFrom-Json
        $ExePath = Join-Path "$($metadata.target_directory)" "$Profile/fishing-gui.exe"
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath "$ExePath" -PathType Leaf)) {
    throw "找不到程序：$ExePath。请先运行 cargo build -p fishing-gui（release 模式需加 --release）。"
}
$ExePath = (Resolve-Path -LiteralPath "$ExePath").Path
$process = $null
$bitmap = $null
$graphics = $null
$hdc = [IntPtr]::Zero

try {
    $process = Start-Process -FilePath "$ExePath" -WorkingDirectory "$PSScriptRoot" -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $process.Refresh()
        if ($process.HasExited) { throw "GUI 提前退出，退出码：$($process.ExitCode)" }
        if ($process.MainWindowHandle -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)

    if ($process.MainWindowHandle -eq [IntPtr]::Zero) {
        throw "等待 GUI 主窗口超时（${TimeoutSeconds}s）。请在交互式桌面会话运行。"
    }

    # 留出 WebView2 加载本地页面的时间；存活和窗口检查不代表前端功能已通过。
    Start-Sleep -Seconds 2
    $process.Refresh()
    if ($process.HasExited) { throw "GUI 加载期间退出，退出码：$($process.ExitCode)" }

    if ($ScreenshotPath) {
        Add-Type -AssemblyName System.Drawing
        if (-not ("FishingFun.SmokeNative" -as [type])) {
            Add-Type @"
using System;
using System.Runtime.InteropServices;
namespace FishingFun {
    public static class SmokeNative {
        [DllImport("user32.dll", SetLastError = true)]
        public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
        [DllImport("user32.dll", SetLastError = true)]
        public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
        public struct RECT { public int Left, Top, Right, Bottom; }
    }
}
"@
        }
        $rect = New-Object FishingFun.SmokeNative+RECT
        if (-not [FishingFun.SmokeNative]::GetWindowRect($process.MainWindowHandle, [ref]$rect)) {
            throw "读取窗口尺寸失败。"
        }
        $width = $rect.Right - $rect.Left
        $height = $rect.Bottom - $rect.Top
        if ($width -le 0 -or $height -le 0) { throw "窗口尺寸无效：${width}x${height}" }

        $bitmap = New-Object System.Drawing.Bitmap $width, $height
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $hdc = $graphics.GetHdc()
        # PW_RENDERFULLCONTENT = 2，尝试包含 WebView2 内容。
        if (-not [FishingFun.SmokeNative]::PrintWindow($process.MainWindowHandle, $hdc, 2)) {
            throw "窗口截图失败。"
        }
        $graphics.ReleaseHdc($hdc)
        $hdc = [IntPtr]::Zero
        $outputPath = [System.IO.Path]::GetFullPath($ScreenshotPath)
        $bitmap.Save($outputPath, [System.Drawing.Imaging.ImageFormat]::Png)
        Write-Output "SCREENSHOT=$outputPath"
    }

    Write-Output "RUNNING_OK title=$($process.MainWindowTitle)"
} finally {
    if ($graphics) {
        if ($hdc -ne [IntPtr]::Zero) { $graphics.ReleaseHdc($hdc) }
        $graphics.Dispose()
    }
    if ($bitmap) { $bitmap.Dispose() }
    # 仅回收本脚本启动的进程，不按名称终止用户的其他实例。
    if ($process) {
        try {
            if (-not $process.HasExited) {
                $process.Kill()
                $process.WaitForExit()
            }
        } finally {
            $process.Dispose()
        }
    }
}
