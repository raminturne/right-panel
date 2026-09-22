; Right Panel — Windows installer (Inno Setup 6). Per-user install, no admin needed.
#define AppName "Right Panel"
#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#define AppExe "RightPanel.exe"

[Setup]
AppId={{6C0B4F53-8E0B-4E0F-9D5B-2E5A1B7C3D11}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=raminturne
AppPublisherURL=https://github.com/raminturne/right-panel
AppSupportURL=https://github.com/raminturne/right-panel/issues
DefaultDirName={localappdata}\Programs\Right Panel
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=RightPanel-Setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=force
RestartApplications=no

[Tasks]
Name: "startup"; Description: "Start Right Panel when Windows starts"; GroupDescription: "Options:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Options:"; Flags: unchecked
Name: "contextmenu"; Description: "Add ""Add to Right Panel"" to the right-click menu"; GroupDescription: "Options:"

[Files]
Source: "..\target\release\right-panel.exe"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "RightPanel"; ValueData: """{app}\{#AppExe}"""; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\{#AppExe}"; Parameters: "--context-menu on"; Flags: runhidden waituntilterminated; Tasks: contextmenu
Filename: "{app}\{#AppExe}"; Description: "Launch Right Panel now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\{#AppExe}"; Parameters: "--context-menu off"; Flags: runhidden waituntilterminated; RunOnceId: "CtxMenu"
Filename: "taskkill.exe"; Parameters: "/F /IM {#AppExe}"; Flags: runhidden; RunOnceId: "KillApp"

[UninstallDelete]
Type: filesandordirs; Name: "{userappdata}\RightPanel\webview"
