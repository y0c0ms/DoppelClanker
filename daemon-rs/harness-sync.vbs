' harness-sync.vbs - run the harness-sync daemon (Rust) with no console window.
' Set HARNESS_SYNC_EXE to the harness-sync.exe path to override the default.
' Launched at login from the Startup folder. Single instance: exits if a
' harness-sync process is already running with --watch.
Option Explicit
Dim sh, fso, home, exe, wmi, procs, p, running
Set sh  = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
home = sh.ExpandEnvironmentStrings("%USERPROFILE%")
exe  = sh.ExpandEnvironmentStrings("%HARNESS_SYNC_EXE%")
If exe = "%HARNESS_SYNC_EXE%" Then exe = home & "\bin\harness-sync.exe"
running = False
Set wmi = GetObject("winmgmts:\\.\root\cimv2")
Set procs = wmi.ExecQuery("SELECT CommandLine FROM Win32_Process WHERE Name='harness-sync.exe'")
For Each p In procs
  If Not IsNull(p.CommandLine) Then
    If InStr(p.CommandLine, "--watch") > 0 Then running = True
  End If
Next
If Not running Then
  sh.Run """" & exe & """ --watch", 0, False
End If
