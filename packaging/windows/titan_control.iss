; Inno Setup script of the Windows installer. Built by CI (.github/workflows/windows.yml) from the portable package:
;   iscc /DVersion=1.1.0 packaging\windows\titan_control.iss
; The package folder (dist\TitanControl-<version>-windows-x64) is made by scripts/package_windows.sh.
#ifndef Version
  #define Version "1.1.0"
#endif
#define PackageDir "..\..\dist\TitanControl-" + Version + "-windows-x64"

[Setup]
AppId={{B3D2A6C1-4E7F-4A58-9C3B-6F1D8E2A7C45}
AppName=Titan Control
AppVersion={#Version}
AppPublisher=Dreyark
AppPublisherURL=https://github.com/Dreyark/titan-control-windows
DefaultDirName={autopf}\Titan Control
DefaultGroupName=Titan Control
DisableProgramGroupPage=yes
OutputDir=..\..\dist
OutputBaseFilename=TitanControl-{#Version}-setup
SetupIconFile=..\..\assets\windows\titan_control.ico
UninstallDisplayIcon={app}\bin\titan_control.exe
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Per-user by default (no administrator needed), the user may choose all users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; The running app (tray) is closed for an update or removal.
CloseApplications=yes
RestartApplications=no
LicenseFile=..\..\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "polish"; MessagesFile: "compiler:Languages\Polish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#PackageDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{group}\Titan Control"; Filename: "{app}\bin\titan_control.exe"
Name: "{autodesktop}\Titan Control"; Filename: "{app}\bin\titan_control.exe"; Tasks: desktopicon

[Registry]
; The autostart entry the app writes itself is removed with the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "TitanControl"; Flags: uninsdeletevalue dontcreatekey

[Run]
Filename: "{app}\bin\titan_control.exe"; Description: "{cm:LaunchProgram,Titan Control}"; Flags: nowait postinstall skipifsilent
