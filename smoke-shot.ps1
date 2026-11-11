[CmdletBinding()]
param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "debug",
    [string]$ExePath,
    [ValidateRange(1, 120)]
    [int]$TimeoutSeconds = 15,
    [string]$OutputPath = (Join-Path "$PSScriptRoot" "gui-smoke.png")
)

$ErrorActionPreference = "Stop"
& "$PSScriptRoot/smoke-gui.ps1" -Profile $Profile -ExePath $ExePath -TimeoutSeconds $TimeoutSeconds -ScreenshotPath $OutputPath
