#Requires AutoHotkey v2.0
#SingleInstance Force

clickX := 600
clickY := 815

CoordMode "Mouse", "Screen"

if ProcessExist("msedge.exe") {
    WinActivate "ahk_exe msedge.exe"
    Sleep 200
} else {
    Run "msedge.exe"
    WinWaitActive "ahk_exe msedge.exe", , 10
    Sleep 1000
}

WinActivate "ahk_exe msedge.exe"
WinMaximize "ahk_exe msedge.exe"

Send "^l"
Sleep 100
SendText "edge://extensions/"
Sleep 100
Send "{Enter}"

WinActivate "ahk_exe msedge.exe"
Sleep 300
Click clickX, clickY

ExitApp