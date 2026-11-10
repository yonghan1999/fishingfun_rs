$p = Start-Process 'F:\claude\FishingFun_Rust\fishing-fun\target\debug\fishing-gui.exe' -PassThru
Start-Sleep 8
if ($p.HasExited) {
    Write-Output ("EXITED code=" + $p.ExitCode)
} else {
    Write-Output "RUNNING_OK"
    $p.Kill()
}
