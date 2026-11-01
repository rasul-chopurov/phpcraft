; Inno Setup script for the Windows installer.
; Built in CI: iscc /DAppVersion=0.1.0 installer\windows\phpcraft.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

[Setup]
AppId={{6F2C0E9B-4F1E-4C1B-9D2A-5A7E3C1B9F10}
AppName=PhpCraft
AppVersion={#AppVersion}
AppPublisher=PhpCraft
AppPublisherURL=https://github.com/rasul-chopurov/phpcraft
DefaultDirName={autopf}\PhpCraft
DefaultGroupName=PhpCraft
UninstallDisplayIcon={app}\phpcraft.exe
SetupIconFile=..\..\assets\icon\phpcraft.ico
OutputDir=..\..\target\release
OutputBaseFilename=PhpCraft-{#AppVersion}-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\..\target\release\phpcraft.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\PhpCraft"; Filename: "{app}\phpcraft.exe"
Name: "{group}\{cm:UninstallProgram,PhpCraft}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\PhpCraft"; Filename: "{app}\phpcraft.exe"; Tasks: desktopicon

[Registry]
; "Open Folder as PhpCraft Project" in Explorer.
Root: HKA; Subkey: "Software\Classes\Directory\shell\PhpCraft"; ValueType: string; ValueName: ""; ValueData: "Open as PhpCraft Project"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\PhpCraft"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\phpcraft.exe"
Root: HKA; Subkey: "Software\Classes\Directory\shell\PhpCraft\command"; ValueType: string; ValueName: ""; ValueData: """{app}\phpcraft.exe"" ""%1"""

[Run]
Filename: "{app}\phpcraft.exe"; Description: "{cm:LaunchProgram,PhpCraft}"; Flags: nowait postinstall skipifsilent
