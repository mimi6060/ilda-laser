; Windows installer for Laser Studio (Inno Setup 6), built by the Release
; workflow:
;   iscc /DAppVersion=0.1.0 /DBinary=..\..\..\target\release\laser-studio.exe studio\packaging\windows\laser-studio.iss
; One click: no question is asked. It installs for the current user only
; (no administrator rights), adds Start menu and desktop shortcuts and an
; uninstaller, then starts the studio. Run again, it updates in place.
; The console window stays: closing it stops the studio.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Binary
  #define Binary "..\..\..\target\release\laser-studio.exe"
#endif

[Setup]
AppId={{6F0B8C59-3E0B-4C51-9D0E-6A2B7C1D5E42}
AppName=Laser Studio
AppVersion={#AppVersion}
AppPublisher=Laser Studio
AppPublisherURL=https://mimi6060.github.io/ilda-laser/
DefaultDirName={autopf}\Laser Studio
DefaultGroupName=Laser Studio
DisableProgramGroupPage=yes
DisableWelcomePage=yes
DisableDirPage=yes
DisableReadyPage=yes
DisableFinishedPage=yes
CloseApplications=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=icon.ico
UninstallDisplayIcon={app}\icon.ico
OutputDir=..\..\..\out
OutputBaseFilename=LaserStudio-Windows-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[Files]
Source: "{#Binary}"; DestDir: "{app}"; DestName: "Laser Studio.exe"; Flags: ignoreversion
Source: "icon.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Laser Studio"; Filename: "{app}\Laser Studio.exe"; IconFilename: "{app}\icon.ico"
Name: "{autodesktop}\Laser Studio"; Filename: "{app}\Laser Studio.exe"; IconFilename: "{app}\icon.ico"

[Run]
Filename: "{app}\Laser Studio.exe"; Description: "{cm:LaunchProgram,Laser Studio}"; Flags: nowait skipifsilent
