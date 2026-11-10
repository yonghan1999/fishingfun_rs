Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32b {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdcBlt, uint nFlags);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$p = Start-Process 'F:\claude\FishingFun_Rust\fishing-fun\target\debug\fishing-gui.exe' -PassThru
Start-Sleep 7
$p.Refresh()
Write-Output ("MainWindowTitle: " + $p.MainWindowTitle)
$hwnd = $p.MainWindowHandle
$rc = New-Object Win32b+RECT
[Win32b]::GetWindowRect($hwnd, [ref]$rc) | Out-Null
$w = $rc.Right - $rc.Left; $h = $rc.Bottom - $rc.Top
Write-Output ("rect: ${w}x${h}")
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# PW_RENDERFULLCONTENT = 2，可正确截取 WebView2 内容
[Win32b]::PrintWindow($hwnd, $hdc, 2) | Out-Null
$g.ReleaseHdc($hdc)
$out = 'F:\claude\FishingFun_Rust\fishing-fun\gui-smoke.png'
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
$p.Kill()
Write-Output $out
