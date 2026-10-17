' sesync.vbs - run the sesync daemon (Rust) with no console window.
' Set SESYNC_EXE to the sesync.exe path to override the default.
' Launched at login from the Startup folder. Single instance: exits if a
' sesync process is already running with --watch.
Option Explicit
Dim sh, fso, home, exe, wmi, procs, p, running
Set sh  = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
home = sh.ExpandEnvironmentStrings("%USERPROFILE%")
exe  = sh.ExpandEnvironmentStrings("%SESYNC_EXE%")
If exe = "%SESYNC_EXE%" Then exe = home & "\bin\sesync.exe"
running = False
Set wmi = GetObject("winmgmts:\\.\root\cimv2")
Set procs = wmi.ExecQuery("SELECT CommandLine FROM Win32_Process WHERE Name='sesync.exe'")
For Each p In procs
  If Not IsNull(p.CommandLine) Then
    If InStr(p.CommandLine, "--watch") > 0 Then running = True
  End If
Next
If Not running Then
  sh.Run """" & exe & """ --watch", 0, False
End If
